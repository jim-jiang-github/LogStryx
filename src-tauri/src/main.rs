#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::{
    cmp::Ordering,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::{Arc, RwLock},
    time::Instant,
};
use tauri::State;

const DEMO_RECORDS_PER_FILE: usize = 500_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogRow {
    timestamp: String,
    timestamp_highlight_ranges: Vec<[usize; 2]>,
    level: String,
    source: String,
    message: String,
    highlight_ranges: Vec<[usize; 2]>,
}

struct LogRecord {
    sort_key: u64,
    file_index: usize,
    sequence: u64,
    timestamp: String,
    level: String,
    message: String,
}

#[derive(Default)]
struct LogStore {
    records: Vec<LogRecord>,
    matches: Vec<usize>,
    sources: Vec<String>,
    search_regex: Option<Regex>,
    dataset_version: u64,
    filter_revision: u64,
}

#[derive(Default)]
struct AppState {
    store: Arc<RwLock<LogStore>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadStats {
    total_count: usize,
    skipped_lines: usize,
    load_ms: u128,
    algorithm: String,
    sources: Vec<String>,
    dataset_version: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilterStats {
    matched_count: usize,
    filter_ms: u128,
}

#[tauri::command]
async fn pick_log_files() -> Result<Option<Vec<String>>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择两个 UTF-8 日志文件")
            .add_filter("日志文件", &["log", "txt"])
            .pick_files()
            .map(|paths| {
                paths
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            })
    })
    .await
    .map_err(|error| format!("文件选择窗口失败：{error}"))
}

#[tauri::command]
async fn load_log_files(
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<LoadStats, String> {
    if paths.len() != 2 {
        return Err("请选择且仅选择两个日志文件".to_owned());
    }

    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| read_log_file(&paths[0], 0));
            let right = scope.spawn(|| read_log_file(&paths[1], 1));
            (left.join(), right.join())
        });
        let (mut left, skipped_left) =
            left.map_err(|_| "读取第一个文件时发生线程错误".to_owned())??;
        let (mut right, skipped_right) =
            right.map_err(|_| "读取第二个文件时发生线程错误".to_owned())??;
        let sources = vec![file_name(&paths[0]), file_name(&paths[1])];
        let (records, algorithm) = merge_files(&mut left, &mut right);
        let total_count = records.len();
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        current.dataset_version = current.dataset_version.wrapping_add(1);
        current.filter_revision = 0;
        current.search_regex = None;
        let dataset_version = current.dataset_version;
        current.matches = (0..total_count).collect();
        current.records = records;
        current.sources = sources.clone();
        Ok(LoadStats {
            total_count,
            skipped_lines: skipped_left + skipped_right,
            load_ms: started.elapsed().as_millis(),
            algorithm: algorithm.to_owned(),
            sources,
            dataset_version,
        })
    })
    .await
    .map_err(|error| format!("日志加载任务失败：{error}"))?
}

#[tauri::command]
async fn generate_demo_logs(state: State<'_, AppState>) -> Result<LoadStats, String> {
    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let mut left = generate_demo_file(0, DEMO_RECORDS_PER_FILE);
        let mut right = generate_demo_file(1, DEMO_RECORDS_PER_FILE);
        let (records, algorithm) = merge_files(&mut left, &mut right);
        let total_count = records.len();
        let sources = vec!["模拟文件 A.log".to_owned(), "模拟文件 B.log".to_owned()];
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        current.dataset_version = current.dataset_version.wrapping_add(1);
        current.filter_revision = 0;
        current.search_regex = None;
        let dataset_version = current.dataset_version;
        current.matches = (0..total_count).collect();
        current.records = records;
        current.sources = sources.clone();
        Ok(LoadStats {
            total_count,
            skipped_lines: 0,
            load_ms: started.elapsed().as_millis(),
            algorithm: algorithm.to_owned(),
            sources,
            dataset_version,
        })
    })
    .await
    .map_err(|error| format!("演示数据生成任务失败：{error}"))?
}

#[tauri::command]
async fn filter_logs(
    keyword: String,
    match_case: bool,
    whole_word: bool,
    use_regex: bool,
    level: String,
    source_index: i32,
    dataset_version: u64,
    revision: u64,
    state: State<'_, AppState>,
) -> Result<FilterStats, String> {
    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let keyword = keyword.trim();
        let level = level.to_ascii_uppercase();
        let search_regex = build_search_regex(keyword, match_case, whole_word, use_regex)?;
        let current = store.read().map_err(|_| "日志数据锁已失效".to_owned())?;
        let matches = collect_matches(
            &current.records,
            search_regex.as_ref(),
            &level,
            source_index,
        );
        let matched_count = matches.len();
        drop(current);
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        if current.dataset_version != dataset_version {
            return Err("日志数据已更新，请重新筛选".to_owned());
        }
        if revision >= current.filter_revision {
            current.matches = matches;
            current.search_regex = search_regex;
            current.filter_revision = revision;
        }
        Ok(FilterStats {
            matched_count,
            filter_ms: started.elapsed().as_millis(),
        })
    })
    .await
    .map_err(|error| format!("日志筛选任务失败：{error}"))?
}

#[tauri::command]
fn get_logs_page(
    start: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<LogRow>, String> {
    let current = state
        .store
        .read()
        .map_err(|_| "日志数据锁已失效".to_owned())?;
    let start = start.min(current.matches.len());
    let end = start
        .saturating_add(limit.clamp(1, 2_000))
        .min(current.matches.len());
    Ok(current.matches[start..end]
        .iter()
        .map(|&index| {
            let record = &current.records[index];
            let message_highlight_ranges = current
                .search_regex
                .as_ref()
                .map(|regex| highlight_ranges(&record.message, regex))
                .unwrap_or_default();
            let timestamp_highlight_ranges = current
                .search_regex
                .as_ref()
                .map(|regex| highlight_ranges(&record.timestamp, regex))
                .unwrap_or_default();
            LogRow {
                timestamp: record.timestamp.clone(),
                timestamp_highlight_ranges,
                level: record.level.clone(),
                source: current.sources[record.file_index].clone(),
                message: record.message.clone(),
                highlight_ranges: message_highlight_ranges,
            }
        })
        .collect())
}

fn read_log_file(path: &str, file_index: usize) -> Result<(Vec<LogRecord>, usize), String> {
    let file = File::open(path).map_err(|error| format!("无法打开 {path}：{error}"))?;
    let name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut skipped_lines = 0;

    for (line_number, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|error| format!("读取 {name} 第 {} 行失败：{error}", line_number + 1))?;
        let line = line.trim_start_matches('\u{feff}');
        if line.trim().is_empty() {
            continue;
        }
        if let Some(record) = parse_log_line(line, file_index, line_number as u64) {
            records.push(record);
        } else {
            skipped_lines += 1;
        }
    }

    Ok((records, skipped_lines))
}

fn parse_log_line(line: &str, file_index: usize, sequence: u64) -> Option<LogRecord> {
    let bytes = line.as_bytes();
    if bytes.len() < 19
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b' ') | Some(b'T'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }

    let year = parse_digits(&bytes[0..4])?;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    let mut cursor = 19;
    let mut millis = 0;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let fraction_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == fraction_start {
            return None;
        }
        let fraction = &bytes[fraction_start..cursor.min(fraction_start + 3)];
        millis = parse_digits(fraction)? * 10u32.pow(3u32.saturating_sub(fraction.len() as u32));
    }
    if bytes.get(cursor) == Some(&b'Z') {
        cursor += 1;
    }

    let timestamp =
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}");
    let remainder = line.get(cursor..)?.trim_start();
    let (level, message) = parse_level(remainder);
    Some(LogRecord {
        sort_key: pack_time(year, month, day, hour, minute, second, millis),
        file_index,
        sequence,
        timestamp,
        level: level.to_owned(),
        message: message.to_owned(),
    })
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0u32, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(byte - b'0'))
    })
}

fn parse_level(remainder: &str) -> (&str, &str) {
    let (token, rest) = if let Some(without_bracket) = remainder.strip_prefix('[') {
        match without_bracket.find(']') {
            Some(end) => (
                &without_bracket[..end],
                without_bracket[end + 1..].trim_start(),
            ),
            None => ("", remainder),
        }
    } else {
        let end = remainder
            .find(char::is_whitespace)
            .unwrap_or(remainder.len());
        (&remainder[..end], remainder[end..].trim_start())
    };

    let level = match token.to_ascii_uppercase().as_str() {
        "INFO" => "INFO",
        "WARN" | "WARNING" => "WARN",
        "ERROR" | "FATAL" => "ERROR",
        "DEBUG" => "DEBUG",
        "TRACE" => "TRACE",
        _ => return ("INFO", remainder),
    };
    (level, rest)
}

fn pack_time(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
) -> u64 {
    (u64::from(year) << 48)
        | (u64::from(month) << 44)
        | (u64::from(day) << 39)
        | (u64::from(hour) << 34)
        | (u64::from(minute) << 28)
        | (u64::from(second) << 22)
        | (u64::from(millis) << 12)
}

fn compare_records(left: &LogRecord, right: &LogRecord) -> Ordering {
    left.sort_key
        .cmp(&right.sort_key)
        .then_with(|| left.file_index.cmp(&right.file_index))
        .then_with(|| left.sequence.cmp(&right.sequence))
}

fn merge_files(
    left: &mut Vec<LogRecord>,
    right: &mut Vec<LogRecord>,
) -> (Vec<LogRecord>, &'static str) {
    let left_sorted = left
        .windows(2)
        .all(|pair| compare_records(&pair[0], &pair[1]).is_le());
    let right_sorted = right
        .windows(2)
        .all(|pair| compare_records(&pair[0], &pair[1]).is_le());
    let strategy = if left_sorted && right_sorted {
        "双路归并 O(N)"
    } else {
        if !left_sorted {
            left.sort_unstable_by(compare_records);
        }
        if !right_sorted {
            right.sort_unstable_by(compare_records);
        }
        "分文件排序后双路归并"
    };

    let capacity = left.len() + right.len();
    let mut merged = Vec::with_capacity(capacity);
    let mut left = std::mem::take(left).into_iter().peekable();
    let mut right = std::mem::take(right).into_iter().peekable();
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        if compare_records(a, b).is_le() {
            merged.push(left.next().expect("peeked record exists"));
        } else {
            merged.push(right.next().expect("peeked record exists"));
        }
    }
    merged.extend(left);
    merged.extend(right);
    (merged, strategy)
}

fn generate_demo_file(file_index: usize, count: usize) -> Vec<LogRecord> {
    (0..count)
        .map(|index| {
            let elapsed_ms = index as u64 * 2 + file_index as u64;
            let total_seconds = elapsed_ms / 1_000;
            let minute = (total_seconds / 60) as u32;
            let second = (total_seconds % 60) as u32;
            let millis = (elapsed_ms % 1_000) as u32;
            let (level, message) = if index % 100 == 0 {
                (
                    "ERROR",
                    format!("请求失败，trace_id={file_index:02}-{index:06}"),
                )
            } else if index % 20 == 0 {
                ("WARN", format!("响应延迟偏高，request_id={index:06}"))
            } else if index % 10 == 0 {
                ("DEBUG", format!("连接状态检查通过，worker={}", index % 32))
            } else {
                ("INFO", format!("批处理任务完成，记录编号={index:06}"))
            };
            LogRecord {
                sort_key: pack_time(2026, 11, 5, 10, minute, second, millis),
                file_index,
                sequence: index as u64,
                timestamp: format!("2026-11-05 10:{minute:02}:{second:02}.{millis:03}"),
                level: level.to_owned(),
                message,
            }
        })
        .collect()
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn highlight_ranges(value: &str, regex: &Regex) -> Vec<[usize; 2]> {
    regex
        .find_iter(value)
        .filter(|matched| matched.start() < matched.end())
        .map(|matched| {
            [
                value[..matched.start()].chars().count(),
                value[..matched.end()].chars().count(),
            ]
        })
        .collect()
}

fn build_search_regex(
    keyword: &str,
    match_case: bool,
    whole_word: bool,
    use_regex: bool,
) -> Result<Option<Regex>, String> {
    if keyword.is_empty() {
        return Ok(None);
    }

    let pattern = if use_regex {
        keyword.to_owned()
    } else {
        regex::escape(keyword)
    };
    let pattern = if whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };

    RegexBuilder::new(&pattern)
        .case_insensitive(!match_case)
        .build()
        .map(Some)
        .map_err(|error| format!("正则表达式无效：{error}"))
}

fn collect_matches(
    records: &[LogRecord],
    search_regex: Option<&Regex>,
    level: &str,
    source_index: i32,
) -> Vec<usize> {
    records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            let level_matches = level == "ALL" || record.level == level;
            let source_matches = source_index < 0 || record.file_index == source_index as usize;
            let keyword_matches = search_regex.is_none_or(|regex| {
                regex.is_match(&record.message) || regex.is_match(&record.timestamp)
            });
            (level_matches && source_matches && keyword_matches).then_some(index)
        })
        .collect()
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            pick_log_files,
            load_log_files,
            generate_demo_logs,
            filter_logs,
            get_logs_page
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_two_sorted_files_in_timestamp_order() {
        let mut first = vec![
            parse_log_line("2026-11-05 10:00:00.000 [INFO] first", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.002 [WARN] third", 0, 1).unwrap(),
        ];
        let mut second = vec![
            parse_log_line("2026-11-05T10:00:00.001Z [ERROR] second", 1, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.003 [INFO] fourth", 1, 1).unwrap(),
        ];
        let (merged, strategy) = merge_files(&mut first, &mut second);
        assert_eq!(strategy, "双路归并 O(N)");
        assert_eq!(
            merged
                .iter()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third", "fourth"]
        );
    }

    #[test]
    fn sorts_individually_unsorted_file_before_merge() {
        let mut first = vec![
            parse_log_line("2026-11-05 10:00:00.002 [INFO] later", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.000 [INFO] earlier", 0, 1).unwrap(),
        ];
        let mut second =
            vec![parse_log_line("2026-11-05 10:00:00.001 [INFO] middle", 1, 0).unwrap()];
        let (merged, strategy) = merge_files(&mut first, &mut second);
        assert_eq!(strategy, "分文件排序后双路归并");
        assert_eq!(merged[0].message, "earlier");
        assert_eq!(merged[1].message, "middle");
        assert_eq!(merged[2].message, "later");
    }

    #[test]
    fn parses_levels_and_fractional_timestamps() {
        let record =
            parse_log_line("2026-11-05T11:13:18.35Z [WARNING] slow request", 0, 0).unwrap();
        assert_eq!(record.level, "WARN");
        assert_eq!(record.timestamp, "2026-11-05 11:13:18.350");
        assert_eq!(record.message, "slow request");
    }

    #[test]
    fn filters_by_keyword_level_and_source() {
        let records = vec![
            parse_log_line("2026-11-05 10:00:00.000 [ERROR] request failed", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.001 [ERROR] request failed", 1, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.002 [INFO] request complete", 0, 1).unwrap(),
        ];
        let search = build_search_regex("FAILED", false, false, false).unwrap();
        assert_eq!(collect_matches(&records, search.as_ref(), "ERROR", 0), [0]);
        let search = build_search_regex("request", false, false, false).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), "ALL", -1),
            [0, 1, 2]
        );
    }

    #[test]
    fn supports_search_options_and_rejects_invalid_regex() {
        let records = vec![
            parse_log_line("2026-11-05 10:00:00.000 [INFO] Request failed", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.001 [INFO] request failed", 0, 1).unwrap(),
            parse_log_line("2026-11-05 10:00:00.002 [INFO] pre-requested", 0, 2).unwrap(),
        ];

        let search = build_search_regex("Request", true, false, false).unwrap();
        assert_eq!(collect_matches(&records, search.as_ref(), "ALL", -1), [0]);

        let search = build_search_regex("request", false, true, false).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), "ALL", -1),
            [0, 1]
        );

        let search = build_search_regex(r"request\s+failed$", false, false, true).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), "ALL", -1),
            [0, 1]
        );

        assert!(build_search_regex("(", false, false, true).is_err());
    }

    #[test]
    fn computes_unicode_safe_highlight_ranges() {
        let regex = build_search_regex("request", false, false, false)
            .unwrap()
            .unwrap();
        assert_eq!(highlight_ranges("日志 Request 请求", &regex), [[3, 10]]);
        let empty_match = Regex::new(r"^").unwrap();
        assert!(highlight_ranges("Request", &empty_match).is_empty());
    }
}
