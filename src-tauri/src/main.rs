#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::{Arc, RwLock},
    time::Instant,
};
use tauri::State;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadingRule {
    id: String,
    name: String,
    pattern: String,
    time_format: String,
    sample: String,
}

#[tauri::command]
fn default_reading_rules() -> Vec<ReadingRule> {
    vec![
        ReadingRule {
            id: "date-first".into(),
            name: "普通日期日志".into(),
            pattern: r"^(?P<time>\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)[ \t]+(?:\[?(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)(?:\][ \t]*|[ \t]+|$))?(?P<message>.*)$".into(),
            time_format: "%Y-%m-%d%[ T]%H:%M:%S%[.f]%[Z]".into(),
            sample: "2026-11-06 13:06:40.045 INFO [54528] Configuring API with {\n   \"discoverIntervalMs\": 4000\n}".into(),
        },
        ReadingRule {
            id: "jabra-sdk".into(),
            name: "JabraSDK 日志".into(),
            pattern: r"^\[(?P<time>\d{2}:\d{2}:\d{2}:\d{3}[+-]\d{4}[ \t]+\d{4}-\d{2}-\d{2})[ \t]+(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\](?P<message>.*)$".into(),
            time_format: "%H:%M:%S:%f%z %Y-%m-%d".into(),
            sample: "Initializing logger LIBJABRA_TRACE_LEVEL: INFO\n[11:45:06:469+0800 2026-11-06    INFO](t30380): SDK version".into(),
        },
        ReadingRule {
            id: "jabra-direct".into(),
            name: "Jabra Direct 日志".into(),
            pattern: r"^\[(?P<time>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\][ \t]+\[(?P<level>(?i:TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL))\][ \t]*(?P<message>.*)$".into(),
            time_format: "%Y-%m-%d %H:%M:%S.%f".into(),
            sample: "[2026-11-06 11:34:55.197] [info] [\n  'Service',\n  { status: 'started' }\n]\n[2026-11-06 11:35:01.664] [error] TypeError: A number was expected\n    at example.js:1:1".into(),
        },
    ]
}

fn compile_time_format(format: &str) -> Result<Regex, String> {
    let tokens = [
        ("%[ T]", "[ T]"),
        ("%[.f]", r"(?:\.(?P<f>\d{1,9}))?"),
        ("%[Z]", "Z?"),
        ("%Y", r"(?P<Y>\d{4})"),
        ("%m", r"(?P<m>\d{2})"),
        ("%d", r"(?P<d>\d{2})"),
        ("%H", r"(?P<H>\d{2})"),
        ("%M", r"(?P<M>\d{2})"),
        ("%S", r"(?P<S>\d{2})"),
        ("%f", r"(?P<f>\d{1,9})"),
        ("%z", r"(?P<z>[+-]\d{4})"),
        ("%%", "%"),
    ];
    let mut remaining = format;
    let mut pattern = String::from("^");
    while !remaining.is_empty() {
        if remaining.starts_with('%') {
            let (token, expression) = tokens
                .iter()
                .find(|(token, _)| remaining.starts_with(token))
                .ok_or_else(|| format!("不支持的时间格式标记：{remaining}"))?;
            pattern.push_str(expression);
            remaining = &remaining[token.len()..];
        } else {
            let ch = remaining.chars().next().expect("non-empty format");
            if ch == ' ' {
                pattern.push_str("[ \\t]+");
            } else {
                pattern.push_str(&regex::escape(&ch.to_string()));
            }
            remaining = &remaining[ch.len_utf8()..];
        }
    }
    pattern.push('$');
    let regex = Regex::new(&pattern).map_err(|error| format!("时间格式无效：{error}"))?;
    for field in ["Y", "m", "d", "H", "M", "S"] {
        if !regex.capture_names().flatten().any(|name| name == field) {
            return Err(format!("时间格式缺少 %{field}"));
        }
    }
    Ok(regex)
}

struct CompiledRule {
    header: Regex,
    time: Regex,
}

fn compile_rule(rule: &ReadingRule) -> Result<CompiledRule, String> {
    if rule.id.trim().is_empty() || rule.name.trim().is_empty() {
        return Err("规则 ID 和名称不能为空".into());
    }
    Ok(CompiledRule {
        header: compile_reading_rule(&rule.pattern)?,
        time: compile_time_format(&rule.time_format)?,
    })
}

#[tauri::command]
fn validate_reading_rules(rules: Vec<ReadingRule>) -> Result<(), String> {
    if rules.is_empty() {
        return Err("至少保留一条读取规则".into());
    }
    let mut ids = std::collections::HashSet::new();
    for rule in &rules {
        if !ids.insert(&rule.id) {
            return Err(format!("规则 ID 重复：{}", rule.name));
        }
        let compiled = compile_rule(rule).map_err(|error| format!("{}：{error}", rule.name))?;
        if !rule.sample.trim().is_empty() {
            read_records(std::io::Cursor::new(&rule.sample), 0, &compiled)
                .map_err(|error| format!("{}：{error}", rule.name))?;
        }
    }
    Ok(())
}

fn compile_reading_rule(pattern: &str) -> Result<Regex, String> {
    if !pattern.starts_with('^') || !pattern.ends_with('$') {
        return Err("正则必须以 ^ 开始、以 $ 结束，逐行匹配日志头".into());
    }
    let regex = Regex::new(pattern).map_err(|error| format!("读取规则正则无效：{error}"))?;
    for name in ["time", "level", "message"] {
        if !regex
            .capture_names()
            .flatten()
            .any(|capture| capture == name)
        {
            return Err(format!("缺少命名捕获组：{name}"));
        }
    }
    if regex.is_match("") {
        return Err("读取规则不能匹配空行".into());
    }
    Ok(regex)
}

#[derive(Serialize)]
struct RulePreview {
    timestamp: String,
    level: String,
    message: String,
}

#[tauri::command]
fn preview_reading_rule(rule: ReadingRule) -> Result<Vec<RulePreview>, String> {
    let compiled = compile_rule(&rule)?;
    let (records, _) = read_records(std::io::Cursor::new(&rule.sample), 0, &compiled)?;
    Ok(records
        .into_iter()
        .map(|record| RulePreview {
            timestamp: record.timestamp,
            level: record.level,
            message: record.message,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleSuggestion {
    path: String,
    rule_id: Option<String>,
    matched_headers: usize,
    note: String,
}

#[tauri::command]
async fn suggest_reading_rules(
    paths: Vec<String>,
    rules: Vec<ReadingRule>,
) -> Result<Vec<RuleSuggestion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let compiled = rules
            .iter()
            .map(compile_rule)
            .collect::<Result<Vec<_>, _>>()?;
        paths
            .into_iter()
            .map(|path| {
                let file =
                    File::open(&path).map_err(|error| format!("无法打开 {path}：{error}"))?;
                let mut reader = BufReader::new(file);
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                // Bound sampling by bytes as well as lines, including a single huge line.
                while bytes.len() < 64 * 1024 {
                    use std::io::Read;
                    let count = reader
                        .read(&mut buffer)
                        .map_err(|error| format!("读取 {path}：{error}"))?;
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error) if error.error_len().is_none() => {
                        std::str::from_utf8(&bytes[..error.valid_up_to()])
                            .map_err(|error| format!("{path} 不是 UTF-8：{error}"))?
                    }
                    Err(error) => return Err(format!("{path} 不是 UTF-8：{error}")),
                };
                let mut scores = vec![0usize; rules.len()];
                for line in text.trim_start_matches('\u{feff}').lines().take(200) {
                    for (index, rule) in compiled.iter().enumerate() {
                        if let Some(captures) = rule.header.captures(line) {
                            if let Some(time) = captures.name("time") {
                                if normalize_rule_time(time.as_str(), &rule.time).is_ok() {
                                    scores[index] += 1;
                                }
                            }
                        }
                    }
                }
                let best = scores.iter().copied().max().unwrap_or(0);
                let winners = scores
                    .iter()
                    .enumerate()
                    .filter(|(_, score)| **score == best)
                    .collect::<Vec<_>>();
                let rule_id = if best > 0 && winners.len() == 1 {
                    Some(rules[winners[0].0].id.clone())
                } else {
                    None
                };
                let note = if best == 0 {
                    "未匹配到规则，请手动选择或新增规则"
                } else if winners.len() > 1 {
                    "多条规则匹配，请手动确认"
                } else {
                    "根据文件前 64 KiB / 200 行推荐，可手动切换"
                };
                Ok(RuleSuggestion {
                    path,
                    rule_id,
                    matched_headers: best,
                    note: note.into(),
                })
            })
            .collect()
    })
    .await
    .map_err(|error| format!("规则推荐失败：{error}"))?
}

#[derive(Deserialize)]
struct FileReadingRule {
    path: String,
    rule: ReadingRule,
}

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
    search_revision: u64,
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
            .set_title("选择 UTF-8 日志文件（按住 Ctrl 或 Shift 多选）")
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
    files: Vec<FileReadingRule>,
    state: State<'_, AppState>,
) -> Result<LoadStats, String> {
    if files.is_empty() {
        return Err("请至少选择一个日志文件".to_owned());
    }
    let rules = files
        .iter()
        .map(|file| compile_rule(&file.rule))
        .collect::<Result<Vec<_>, _>>()?;

    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let (records, algorithm) = if files.len() == 2 {
            let (left, right) = std::thread::scope(|scope| {
                let left = scope.spawn(|| read_file_records(&files[0].path, 0, &rules[0]));
                let right = scope.spawn(|| read_file_records(&files[1].path, 1, &rules[1]));
                (left.join(), right.join())
            });
            let (mut left, _) = left.map_err(|_| "读取第一个文件时发生线程错误".to_owned())??;
            let (mut right, _) = right.map_err(|_| "读取第二个文件时发生线程错误".to_owned())??;
            merge_files(&mut left, &mut right)
        } else {
            let mut records = Vec::new();
            for (index, file) in files.iter().enumerate() {
                let (mut file_records, _) = read_file_records(&file.path, index, &rules[index])?;
                records.append(&mut file_records);
            }
            let algorithm = if records
                .windows(2)
                .all(|pair| compare_records(&pair[0], &pair[1]).is_le())
            {
                "顺序读取"
            } else {
                records.sort_unstable_by(compare_records);
                "按时间排序"
            };
            (records, algorithm)
        };
        let sources = files
            .iter()
            .map(|file| file_name(&file.path))
            .collect::<Vec<_>>();
        let total_count = records.len();
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        current.dataset_version = current.dataset_version.wrapping_add(1);
        current.filter_revision = 0;
        current.search_revision = 0;
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
    .map_err(|error| format!("日志加载任务失败：{error}"))?
}

#[tauri::command]
async fn filter_logs(
    keyword: String,
    levels: Vec<String>,
    source_indices: Vec<usize>,
    dataset_version: u64,
    revision: u64,
    state: State<'_, AppState>,
) -> Result<FilterStats, String> {
    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let keyword = keyword.trim();
        let filter_regex = build_search_regex(keyword, false, false, false)?;
        let current = store.read().map_err(|_| "日志数据锁已失效".to_owned())?;
        let matches = collect_matches(
            &current.records,
            filter_regex.as_ref(),
            &levels,
            &source_indices,
        );
        let matched_count = matches.len();
        drop(current);
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        if current.dataset_version != dataset_version {
            return Err("日志数据已更新，请重新筛选".to_owned());
        }
        if revision >= current.filter_revision {
            current.matches = matches;
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
async fn set_search(
    keyword: String,
    match_case: bool,
    whole_word: bool,
    use_regex: bool,
    dataset_version: u64,
    revision: u64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || {
        let search_regex = build_search_regex(keyword.trim(), match_case, whole_word, use_regex)?;
        let mut current = store.write().map_err(|_| "日志数据锁已失效".to_owned())?;
        if current.dataset_version != dataset_version {
            return Err("日志数据已更新，请重新搜索".to_owned());
        }
        if revision >= current.search_revision {
            current.search_regex = search_regex;
            current.search_revision = revision;
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("日志搜索任务失败：{error}"))?
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

fn read_file_records(
    path: &str,
    file_index: usize,
    rule: &CompiledRule,
) -> Result<(Vec<LogRecord>, usize), String> {
    let file = File::open(path).map_err(|error| format!("无法打开 {path}：{error}"))?;
    read_records(BufReader::new(file), file_index, rule).map_err(|error| format!("{path}：{error}"))
}

fn normalize_rule_time(time: &str, format: &Regex) -> Result<String, String> {
    let fields = format
        .captures(time)
        .ok_or_else(|| format!("时间与规则格式不匹配：{time}"))?;
    if let Some(offset) = fields.name("z") {
        let bytes = offset.as_str().as_bytes();
        if parse_digits(&bytes[1..3]).is_none_or(|hour| hour > 23)
            || parse_digits(&bytes[3..5]).is_none_or(|minute| minute > 59)
        {
            return Err(format!("时区偏移无效：{time}"));
        }
    }
    let fraction = fields.name("f").map(|value| value.as_str()).unwrap_or("0");
    let normalized = format!(
        "{}-{}-{} {}:{}:{}.{}",
        &fields["Y"],
        &fields["m"],
        &fields["d"],
        &fields["H"],
        &fields["M"],
        &fields["S"],
        fraction
    );
    if parse_log_line(&normalized, 0, 0).is_none() {
        return Err(format!("时间无效：{time}"));
    }
    Ok(normalized)
}

fn read_records(
    reader: impl BufRead,
    file_index: usize,
    rule: &CompiledRule,
) -> Result<(Vec<LogRecord>, usize), String> {
    let mut records: Vec<LogRecord> = Vec::new();
    let mut prefix = String::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| format!("读取第 {} 行失败：{error}", line_number + 1))?;
        let line = if line_number == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            &line
        };
        if let Some(captures) = rule.header.captures(line) {
            let time = captures
                .name("time")
                .ok_or_else(|| format!("第 {} 行缺少时间", line_number + 1))?
                .as_str();
            let normalized_time = normalize_rule_time(time, &rule.time)
                .map_err(|error| format!("第 {} 行：{error}", line_number + 1))?;
            let mut record = parse_log_line(&normalized_time, file_index, line_number as u64)
                .ok_or_else(|| format!("第 {} 行时间无效：{time}", line_number + 1))?;
            if let Some(level) = captures.name("level") {
                let normalized = level.as_str().to_ascii_uppercase();
                if !matches!(
                    normalized.as_str(),
                    "TRACE" | "DEBUG" | "INFO" | "WARN" | "WARNING" | "ERROR" | "FATAL"
                ) {
                    return Err(format!("第 {} 行级别无效：{normalized}", line_number + 1));
                }
                record.level = parse_level(&normalized).0.to_owned();
            }
            record.message = captures
                .name("message")
                .ok_or_else(|| format!("第 {} 行缺少内容捕获组", line_number + 1))?
                .as_str()
                .to_owned();
            if !prefix.is_empty() {
                prefix.push_str(&record.message);
                record.message = std::mem::take(&mut prefix);
            }
            records.push(record);
        } else if let Some(record) = records.last_mut() {
            record.message.push('\n');
            record.message.push_str(line);
        } else {
            prefix.push_str(line);
            prefix.push('\n');
        }
    }
    if records.is_empty() {
        return Err("没有匹配到日志记录，请调整读取规则".into());
    }
    Ok((records, 0))
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
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
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
    levels: &[String],
    source_indices: &[usize],
) -> Vec<usize> {
    records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            let level_matches = levels.contains(&record.level);
            let source_matches = source_indices.contains(&record.file_index);
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
            filter_logs,
            set_search,
            get_logs_page,
            default_reading_rules,
            validate_reading_rules,
            preview_reading_rule,
            suggest_reading_rules
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jabra_direct_levels_objects_arrays_and_stacks() {
        let preset = default_reading_rules().remove(2);
        let rule = compile_rule(&preset).unwrap();
        let text = "[2026-11-06 11:34:55.197] [info] [\n  'Service',\n  { status: 'started' }\n]\n[2026-11-06 11:34:57.611] [debug] (success) {\n  status: 'started'\n}\n[2026-11-06 11:35:01.664] [error] TypeError: A number was expected\n    at example.js:1:1";
        let (records, _) = read_records(std::io::Cursor::new(text), 0, &rule).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .map(|record| record.level.as_str())
                .collect::<Vec<_>>(),
            ["INFO", "DEBUG", "ERROR"]
        );
        assert!(records[0].message.ends_with("\n]"));
        assert!(records[1].message.ends_with("\n}"));
        assert!(records[2].message.ends_with("\n    at example.js:1:1"));
        assert_eq!(
            records
                .iter()
                .map(|record| record.message.matches('\n').count() + 1)
                .sum::<usize>(),
            text.lines().count()
        );
        for other in &default_reading_rules()[..2] {
            assert!(
                read_records(std::io::Cursor::new(text), 0, &compile_rule(other).unwrap()).is_err()
            );
        }
    }

    #[test]
    #[ignore = "requires an explicitly supplied Jabra Direct log directory"]
    fn reads_supplied_jabra_direct_files() {
        let directory =
            std::env::var("LOGSTRYX_TEST_DIRECT_DIR").expect("set LOGSTRYX_TEST_DIRECT_DIR");
        let preset = default_reading_rules().remove(2);
        let rule = compile_rule(&preset).unwrap();
        let mut paths = Vec::new();
        let mut combined = Vec::new();
        let header = Regex::new(r"^\[\d{4}-\d{2}-\d{2} ").unwrap();
        for (index, name) in [
            "JabraDirect.Analytics.main.log",
            "JabraDirect.main.log",
            "JabraDirect.renderer.log",
            "JabraDirect.telemetry.log",
        ]
        .iter()
        .enumerate()
        {
            let path = Path::new(&directory).join(name);
            let text = std::fs::read_to_string(&path).unwrap();
            let (mut records, skipped) =
                read_records(std::io::Cursor::new(&text), index, &rule).unwrap();
            assert_eq!(skipped, 0);
            assert_eq!(
                records.len(),
                text.trim_start_matches('\u{feff}')
                    .lines()
                    .filter(|line| header.is_match(line))
                    .count()
            );
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.message.matches('\n').count() + 1)
                    .sum::<usize>(),
                text.lines().count()
            );
            assert!(records
                .iter()
                .all(|record| ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"]
                    .contains(&record.level.as_str())));
            combined.append(&mut records);
            paths.push(path.to_string_lossy().into_owned());
        }
        let suggestions =
            tauri::async_runtime::block_on(suggest_reading_rules(paths, default_reading_rules()))
                .unwrap();
        assert!(suggestions
            .iter()
            .all(|suggestion| suggestion.rule_id.as_deref() == Some("jabra-direct")));
        combined.sort_unstable_by(compare_records);
        assert!(combined
            .windows(2)
            .all(|pair| compare_records(&pair[0], &pair[1]).is_le()));
        for index in 0..4 {
            assert!(combined.iter().any(|record| record.file_index == index));
        }
    }

    #[test]
    fn validates_independent_rules_and_custom_timestamp_templates() {
        let defaults = default_reading_rules();
        validate_reading_rules(defaults.clone()).unwrap();
        assert!(validate_reading_rules(vec![]).is_err());
        assert!(validate_reading_rules(vec![defaults[0].clone(), defaults[0].clone()]).is_err());
        let mut custom = defaults[0].clone();
        custom.time_format = "%d/%m/%Y %H:%M:%S,%f".into();
        custom.pattern = r"^(?P<time>[^|]+)\|(?P<level>INFO)\|(?P<message>.*)$".into();
        custom.sample = "06/11/2026 13:06:40,123456|INFO|custom time".into();
        let records = preview_reading_rule(custom.clone()).unwrap();
        assert_eq!(records[0].timestamp, "2026-11-06 13:06:40.123");
        validate_reading_rules(vec![custom.clone()]).unwrap();
        custom.time_format = "%Y-%m-%d".into();
        assert!(compile_rule(&custom).is_err());
        custom.time_format = "%d/%m/%Y %H:%M:%S,%f %Q".into();
        assert!(compile_rule(&custom).is_err());
        assert!(preview_reading_rule(ReadingRule {
            sample: defaults[1].sample.clone(),
            ..defaults[0].clone()
        })
        .is_err());
        assert!(preview_reading_rule(ReadingRule {
            sample: defaults[0].sample.clone(),
            ..defaults[1].clone()
        })
        .is_err());
    }

    #[test]
    fn reading_rule_groups_multiline_records_and_preserves_context() {
        let rule = compile_rule(&default_reading_rules()[0]).unwrap();
        let text = "\u{feff}2026-11-06 11:34:56.891 INFO  [39920] [javascript:CameraApi::reConfigure@0] Configuring API with {\n   \"discoverIntervalMs\": 4000\n}\n2026-11-06 11:34:56.891 INFO  [39920] initialized\nlast line";
        let (records, skipped) = read_records(std::io::Cursor::new(text), 0, &rule).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(skipped, 0);
        assert_eq!(records[0].level, "INFO");
        assert_eq!(records[0].timestamp, records[1].timestamp);
        assert!(records[0].message.starts_with("[39920] [javascript:"));
        assert!(records[0]
            .message
            .ends_with("{\n   \"discoverIntervalMs\": 4000\n}"));
        assert!(records[1].message.ends_with("\nlast line"));
        assert!(compare_records(&records[0], &records[1]).is_lt());
    }

    #[test]
    fn reading_rules_validate_patterns_and_dates_without_dropping_prefixes() {
        for pattern in ["(", "^.*$", "^(?P<time>.*)(?P<level>.*)(?P<message>.*)$"] {
            assert!(compile_reading_rule(pattern).is_err());
        }
        let rule = compile_rule(&default_reading_rules()[0]).unwrap();
        for text in [
            "2026-02-30 11:34:56.891 INFO test",
            "",
            "only untimed content",
        ] {
            assert!(read_records(std::io::Cursor::new(text), 0, &rule).is_err());
        }
        let (records, _) = read_records(
            std::io::Cursor::new(
                "2026-11-05T11:13:18.35Z [WARNING] slow request\n2026-11-05 11:13:19 no level",
            ),
            0,
            &rule,
        )
        .unwrap();
        assert_eq!(records[0].level, "WARN");
        assert_eq!(records[0].message, "slow request");
        assert_eq!(records[1].level, "INFO");
        assert_eq!(records[1].message, "no level");
        let custom = r"^(?P<level>INFO)\|(?P<time>[^|]+)\|(?P<message>.*)$";
        let preview = preview_reading_rule(ReadingRule {
            pattern: custom.into(),
            sample: "INFO|2026-11-06 13:06:40.045|hello\nworld".into(),
            ..default_reading_rules().remove(0)
        })
        .unwrap();
        assert_eq!(preview[0].message, "hello\nworld");
    }

    #[test]
    fn reads_sdk_headers_and_attaches_untimed_lines() {
        let rule = compile_rule(&default_reading_rules()[1]).unwrap();
        let text = "Initializing logger LIBJABRA_TRACE_LEVEL: INFO\n[11:45:06:469+0800 2026-11-06    INFO](t30380): Build time\n[11:45:06:470+0800 2026-11-06    INFO](t30380): SDK version\nInitializing logger LIBJABRA_TRACE_LEVEL: INFO\n[11:47:50:053+0800 2026-11-06    INFO](t48752): Build time";
        let (records, _) = read_records(std::io::Cursor::new(text), 1, &rule).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].timestamp, "2026-11-06 11:45:06.469");
        assert_eq!(records[0].level, "INFO");
        assert_eq!(
            records[0].message,
            "Initializing logger LIBJABRA_TRACE_LEVEL: INFO\n(t30380): Build time"
        );
        assert_eq!(
            records[1].message,
            "(t30380): SDK version\nInitializing logger LIBJABRA_TRACE_LEVEL: INFO"
        );
        assert_eq!(records[2].timestamp, "2026-11-06 11:47:50.053");
        assert!(records.iter().all(|record| record.file_index == 1));
        for time in ["25:45:06:469+0800", "11:45:06:469+2460"] {
            let text = format!("[{time} 2026-11-06 INFO](t1): invalid");
            assert!(read_records(std::io::Cursor::new(text), 0, &rule).is_err());
        }
    }

    #[test]
    fn merges_different_formats_with_multiline_content_and_tied_timestamps() {
        let date_rule = compile_rule(&default_reading_rules()[0]).unwrap();
        let sdk_rule = compile_rule(&default_reading_rules()[1]).unwrap();
        let (mut first, _) = read_records(std::io::Cursor::new("2026-11-06 11:45:06.469 INFO config {\n  \"x\": 1\n}\n2026-11-06 11:45:06.471 WARN later"), 0, &date_rule).unwrap();
        let (mut second, _) = read_records(std::io::Cursor::new("Initializing logger\n[11:45:06:469+0800 2026-11-06 INFO](t1): first\n[11:45:06:470+0800 2026-11-06 ERROR](t1): second"), 1, &sdk_rule).unwrap();
        let (records, _) = merge_files(&mut first, &mut second);
        assert_eq!(records.len(), 4);
        assert_eq!(
            records
                .iter()
                .map(|record| record.file_index)
                .collect::<Vec<_>>(),
            [0, 1, 1, 0]
        );
        assert!(records[0].message.ends_with("\n}"));
        assert!(records[1].message.starts_with("Initializing logger\n"));
        assert_eq!(records[2].level, "ERROR");
    }

    #[test]
    #[ignore = "requires two explicitly supplied local log fixtures"]
    fn reads_supplied_two_log_formats() {
        let first = std::env::var("LOGSTRYX_TEST_LOG").expect("set LOGSTRYX_TEST_LOG");
        let second =
            std::env::var("LOGSTRYX_TEST_SECOND_LOG").expect("set LOGSTRYX_TEST_SECOND_LOG");
        let suggestions = tauri::async_runtime::block_on(suggest_reading_rules(
            vec![first.clone(), second.clone()],
            default_reading_rules(),
        ))
        .unwrap();
        assert_eq!(suggestions[0].rule_id.as_deref(), Some("date-first"));
        assert!(suggestions[0].matched_headers >= 9);
        assert_eq!(suggestions[1].rule_id.as_deref(), Some("jabra-sdk"));
        assert!(suggestions[1].matched_headers >= 20);
        let no_match = tauri::async_runtime::block_on(suggest_reading_rules(
            vec![first.clone()],
            vec![default_reading_rules()[1].clone()],
        ))
        .unwrap();
        assert!(no_match[0].rule_id.is_none());
        let ambiguous = tauri::async_runtime::block_on(suggest_reading_rules(
            vec![first.clone()],
            vec![
                default_reading_rules()[0].clone(),
                ReadingRule {
                    id: "another".into(),
                    ..default_reading_rules()[0].clone()
                },
            ],
        ))
        .unwrap();
        assert!(ambiguous[0].rule_id.is_none());
        let date_rule = compile_rule(&default_reading_rules()[0]).unwrap();
        let sdk_rule = compile_rule(&default_reading_rules()[1]).unwrap();
        let first_text = std::fs::read_to_string(&first).unwrap();
        let second_text = std::fs::read_to_string(&second).unwrap();
        let first_count = first_text
            .lines()
            .filter(|line| line.trim_start_matches('\u{feff}').starts_with("2026-"))
            .count();
        let second_count = second_text
            .lines()
            .filter(|line| line.starts_with('['))
            .count();
        let (left, right) = std::thread::scope(|scope| {
            let left =
                scope.spawn(|| read_records(std::io::Cursor::new(&first_text), 0, &date_rule));
            let right =
                scope.spawn(|| read_records(std::io::Cursor::new(&second_text), 1, &sdk_rule));
            (
                left.join().unwrap().unwrap(),
                right.join().unwrap().unwrap(),
            )
        });
        let (mut left, _) = left;
        let (mut right, _) = right;
        assert_eq!(left.len(), first_count);
        assert_eq!(right.len(), second_count);
        assert_eq!(
            right
                .iter()
                .map(|record| record.message.matches("Initializing logger").count())
                .sum::<usize>(),
            second_text.matches("Initializing logger").count()
        );
        let (records, _) = merge_files(&mut left, &mut right);
        assert_eq!(records.len(), first_count + second_count);
        assert!(records
            .windows(2)
            .all(|pair| compare_records(&pair[0], &pair[1]).is_le()));
        assert_eq!(
            records
                .iter()
                .filter(|record| record.file_index == 0)
                .count(),
            first_count
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.message.matches('\n').count() + 1)
                .sum::<usize>(),
            first_text.lines().count() + second_text.lines().count()
        );
    }

    #[test]
    #[ignore = "requires an explicitly supplied local log fixture"]
    fn reads_supplied_panacast_log() {
        let path = std::env::var("LOGSTRYX_TEST_LOG").expect("set LOGSTRYX_TEST_LOG");
        let rule = compile_rule(&default_reading_rules()[0]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let (records, skipped) = read_records(std::io::Cursor::new(&text), 0, &rule).unwrap();
        assert_eq!(
            records.len(),
            text.lines()
                .filter(|line| line.trim_start_matches('\u{feff}').starts_with("2026-"))
                .count()
        );
        assert_eq!(skipped, 0);
        assert_eq!(
            records
                .iter()
                .filter(|record| record.message.contains('\n'))
                .count(),
            text.matches("Configuring API with {").count()
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.message.matches('\n').count() + 1)
                .sum::<usize>(),
            text.lines().count()
        );
        assert!(records.iter().all(|record| record.level == "INFO"));
        assert_eq!(records[0].timestamp, "2026-11-06 11:34:56.891");
        assert_eq!(records[8].timestamp, "2026-11-06 13:06:40.045");
    }

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
        assert_eq!(
            collect_matches(&records, search.as_ref(), &["ERROR".into()], &[0]),
            [0]
        );
        let search = build_search_regex("request", false, false, false).unwrap();
        assert_eq!(
            collect_matches(
                &records,
                search.as_ref(),
                &["ERROR".into(), "INFO".into()],
                &[0, 1]
            ),
            [0, 1, 2]
        );
    }

    #[test]
    fn supports_multiple_levels_and_sources_and_empty_selection() {
        let records = vec![
            parse_log_line("2026-11-05 10:00:00.000 [ERROR] failed", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.001 [WARN] slow", 1, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.002 [INFO] complete", 0, 1).unwrap(),
        ];
        let levels = vec!["ERROR".into(), "WARN".into()];
        assert_eq!(collect_matches(&records, None, &levels, &[0, 1]), [0, 1]);
        assert_eq!(collect_matches(&records, None, &levels, &[1]), [1]);
        assert!(collect_matches(&records, None, &[], &[0, 1]).is_empty());
        assert!(collect_matches(&records, None, &levels, &[]).is_empty());
    }

    #[test]
    fn supports_search_options_and_rejects_invalid_regex() {
        let records = vec![
            parse_log_line("2026-11-05 10:00:00.000 [INFO] Request failed", 0, 0).unwrap(),
            parse_log_line("2026-11-05 10:00:00.001 [INFO] request failed", 0, 1).unwrap(),
            parse_log_line("2026-11-05 10:00:00.002 [INFO] pre-requested", 0, 2).unwrap(),
        ];

        let search = build_search_regex("Request", true, false, false).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), &["INFO".into()], &[0]),
            [0]
        );

        let search = build_search_regex("request", false, true, false).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), &["INFO".into()], &[0]),
            [0, 1]
        );

        let search = build_search_regex(r"request\s+failed$", false, false, true).unwrap();
        assert_eq!(
            collect_matches(&records, search.as_ref(), &["INFO".into()], &[0]),
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
