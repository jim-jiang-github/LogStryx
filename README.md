# LogStryx

> A cross-platform log analysis workbench for fast search, powerful filtering, and AI-assisted troubleshooting.

LogStryx is a desktop log-query workbench built with Tauri 2, Rust, and native HTML/CSS/JavaScript. The current implementation focuses on merging two timestamped log files, fast filtering, and a virtualized list. AI-assisted troubleshooting is part of the product direction and is not implemented in this initial build.

## Features

- Load one or more UTF-8 `.log` or `.txt` files and order records by timestamp. Two already-sorted inputs use an O(N) two-way merge; an unsorted input is sorted before merging.
- Manage independent named rules in **Edit > Log reading rules** (编辑 > 日志读取规则): add/delete rules, edit each header regex and timestamp format, and preview multiline samples. Rules are saved locally and applied on the next import.
- Search messages and timestamps, with case-sensitive, whole-word, and regular-expression options.
- Filter by log level and source file; matching text is highlighted.
- Keep records and filtering in the Rust backend and virtualize the frontend list so only nearby rows are rendered.

The three built-in rules separately recognize date-first headers (optional level), JabraSDK headers, and Jabra Direct bracketed headers (case-insensitive levels):

```text
2026-11-05 11:13:18.353 [INFO] Service started
2026-11-05T11:13:18.354Z [ERROR] Request failed
[11:45:06:469+0800 2026-11-06    INFO](t30380): SDK version
[2026-11-06 11:34:55.197] [info] Service started
```

Use Ctrl or Shift in the file picker to select two files simultaneously. An assignment dialog recommends a rule separately for each file using its first 64 KiB / 200 lines. Ambiguous or unmatched samples require manual selection; recommendations are not guarantees. You can override recommendations or open rule management before importing. Confirmed bindings are remembered per file path. Each file uses its assigned rule; two selected files are read concurrently and merged with stable ordering for equal timestamps.

Supported levels are `INFO`, `WARN`/`WARNING`, `ERROR`/`FATAL`, `DEBUG`, and `TRACE`; omitted levels default to `INFO`. Non-header lines are appended to the preceding record, preserving multiline JSON and stack traces. Leading untimed lines (such as `Initializing logger`) are prepended to the first record instead. An invalid captured timestamp or a file with no matching records causes an explicit import error; the current dataset remains unchanged.

Custom rules use Rust `regex` syntax and must start with `^`, end with `$`, and define named captures `time`, `level`, and `message`, for example `(?P<time>...)`. Lookaround and backreferences are unsupported. Each rule has its own timestamp template: `%Y` year, `%m` month, `%d` day, `%H` hour, `%M` minute, `%S` second, `%f` fraction (1–9 digits, truncated to milliseconds), `%z` signed offset such as `+0800`, and `%%` literal percent. Other characters are literal; spaces match one or more spaces/tabs. Extensions `%[ T]`, `%[.f]`, and `%[Z]` accept a space-or-T separator, optional fractional seconds, and optional Z respectively.

The date-first preset uses `%Y-%m-%d%[ T]%H:%M:%S%[.f]%[Z]`; the SDK preset uses `%H:%M:%S:%f%z %Y-%m-%d`. Timestamps are ordered by local fields without timezone conversion; ensure files use the same timezone. Messages retain IDs and call locations. Preview and file import use the same parser. Previously saved unmodified defaults become the two separate presets; custom patterns are retained as a migrated rule whose timestamp template should be checked. Cancelling rule management discards draft edits; saving validates all rules and any non-empty samples before replacing local settings.

The Jabra Direct preset covers Analytics.main, main, renderer, and telemetry logs, retaining multiline objects, arrays, and exception stacks as message text (not necessarily JSON). Newly shipped presets are added to existing local rule lists without overwriting customizations; deleting a known preset does not cause it to reappear on restart.

## Development

Install the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your operating system, Rust, Node.js, and Tauri CLI 2:

```sh
cargo install tauri-cli --version "^2"
cd src-tauri
cargo tauri dev
```

The project keeps Tauri's cross-platform structure; build desktop releases on each target operating system with its corresponding native toolchain. For a Windows x64 executable without an installer:

```powershell
rustup target add x86_64-pc-windows-msvc
cd src-tauri
cargo tauri build --target x86_64-pc-windows-msvc --no-bundle
```

To create a Windows NSIS installer when needed:

```powershell
cd src-tauri
cargo tauri build --target x86_64-pc-windows-msvc --bundles nsis
```

The Windows installer uses Tauri's WebView2 download bootstrapper (`downloadBootstrapper`); WebView2 Runtime is not bundled. On Windows, the target machine must already have the Evergreen WebView2 Runtime or be able to download it during installation. macOS and Linux use their platform webviews and native build prerequisites.
