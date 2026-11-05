# LogStryx

> A cross-platform log analysis workbench for fast search, powerful filtering, and AI-assisted troubleshooting.

LogStryx is a desktop log-query workbench built with Tauri 2, Rust, and native HTML/CSS/JavaScript. The current implementation focuses on merging two timestamped log files, fast filtering, and a virtualized list. AI-assisted troubleshooting is part of the product direction and is not implemented in this initial build.

## Features

- Load two UTF-8 `.log` or `.txt` files and merge records by timestamp. Already-sorted inputs use an O(N) two-way merge; an unsorted input is sorted before merging.
- Search messages and timestamps, with case-sensitive, whole-word, and regular-expression options.
- Filter by log level and source file; matching text is highlighted.
- Keep records and filtering in the Rust backend and virtualize the frontend list so only nearby rows are rendered.
- Generate one million demo records for performance testing.

Each non-empty input line must start with a timestamp. The level and message are optional:

```text
2026-11-05 11:13:18.353 [INFO] Service started
2026-11-05T11:13:18.354Z [ERROR] Request failed
```

Supported levels are `INFO`, `WARN`/`WARNING`, `ERROR`/`FATAL`, `DEBUG`, and `TRACE`. Lines without a supported timestamp are skipped and counted. Timestamps are ordered by their local fields without timezone conversion; normalize offset timestamps to one timezone before loading.

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
