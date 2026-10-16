# ramlog

A tiny background logger that records how much RAM each app on a Windows PC uses, all day, every day. The output is plain JSON Lines, made to be handed to an AI agent (or a script) for analysis: which apps hold the most memory, when the PC runs out, what grows over time, and what sits in memory without being used.

- **Small:** about 2 MB of RAM and one system call per sample. Written in Rust, no runtime.
- **Adaptive:** a sample every 30 s while you use the PC, every 2 min while it is idle.
- **Per app, not per process:** helper processes (WebView2, Node, Python, consoles, shells) count under the app that started them, so Teams' WebView2 shows as `ms-teams` and MCP servers started by Claude show as `claude`.
- **Context:** the foreground app, idle time, and sleep, wake, lock, screen and logon events.
- **Daily report:** at the end of each day, a Markdown summary is written next to the raw log.

## Install

Needs Rust (stable, MSVC toolchain).

```powershell
cargo build --release
.\target\release\ramlog.exe install
```

`install` copies `ramlog.exe` and `ramlogd.exe` to `bin\`, adds `ramlogd` to the `HKCU\...\Run` key so it starts at logon (it shows in Task Manager > Startup apps), and starts it. No admin rights needed. Rebuilding while the logger runs is fine, because the running copy lives in `bin\`. Run `install` again to update it.

## Commands

| Command | What it does |
|---|---|
| `ramlog status` | Is the logger running, how much RAM it uses, today's file and the last sample. |
| `ramlog now [N]` | Take one sample now and print the top N apps. Does not write to the log. |
| `ramlog report [DATE]` | Build and print the report for `today` (default), `yesterday`, or `YYYY-MM-DD`. |
| `ramlog start` / `ramlog stop` | Start or stop the background logger. |
| `ramlog install` / `ramlog uninstall` | Turn logon start on or off. Uninstall keeps logs and exes. |
| `ramlog paths` | Show the project, config, log, and exe paths. |

The logger itself (`ramlogd.exe`) has no window and no tray icon. Find it in Task Manager under Details.

## Output

Everything goes to `logs\` in the project folder (git-ignored):

| File | Content |
|---|---|
| `YYYY-MM-DD.jsonl` | Raw data: one header line, then samples and events in time order. |
| `YYYY-MM-DD.report.md` | Daily summary, written when the day ends (or on `ramlog report`). |
| `SCHEMA.md` | Field-by-field description of the JSONL records. |
| `ramlogd-error.log` | Only exists if something went wrong. |

A sample looks like this (shortened):

```json
{"t":"sample","ts":"2026-10-16T22:23:59+05:30","int_s":30,"idle_s":1,"active":true,"fg":"antigravity","fg_exe":"antigravity",
 "sys":{"total_mb":16068,"avail_mb":1487,"used_pct":90,"commit_mb":21318,"commit_limit_mb":38596,"pagefile_mb":1444,"compressed_mb":97,...},
 "apps":[{"app":"claude","n":23,"mem_mb":1519,"commit_mb":2171,"parts":{"claude":1283,"node":201,...}},
         {"app":"afk","own":true,"n":10,"mem_mb":526,"commit_mb":1233,"parts":{"afk":209,"llama-server":252,"msedgewebview2":66}}, ...]}
```

`mem_mb` is the private working set, the same number as the Task Manager "Memory" column. See [SCHEMA.md](SCHEMA.md) for every field. A day of data is about 4 to 6 MB.

## Analysing with an agent

Give the agent the `logs\` folder (or a few days of it) and ask, for example:

> Read SCHEMA.md, then the JSONL files and daily reports in this folder. Find which apps cost the most RAM over these days, when available RAM dropped below 1 GB and why, which apps grow over the day, and which heavy apps sit in the background unused. Suggest concrete changes, ranked by RAM saved.

The daily reports are a good first read; the raw JSONL has the detail.

## Settings

`ramlog.toml` in the project root:

| Key | Default | Meaning |
|---|---|---|
| `active_interval_s` | 30 | Seconds between samples while you use the PC. |
| `idle_interval_s` | 120 | Seconds between samples while idle. |
| `idle_after_s` | 300 | No input for this long counts as idle. |
| `min_app_mb` | 5 | Smaller app groups are summed into `(other)`. |
| `own_apps` | | Exe names of your own apps. They get `"own": true` and their own report table. |
| `helpers` | | Extra helper exes to count under the app that started them. |

Restart the logger after a change (`ramlog stop`, `ramlog start`).

## How it works

- One `NtQuerySystemInformation(SystemProcessInformation)` call per sample gives every process with its parent, private working set and commit, without opening any process.
- `GetPerformanceInfo` gives available RAM, commit, cache and kernel pools; `SystemPageFileInformation` gives page file use.
- A hidden window receives `WM_POWERBROADCAST` (sleep, wake, screen state), `WM_WTSSESSION_CHANGE` (lock) and `WM_ENDSESSION` (logoff, shutdown).
- A gap longer than the planned interval plus 60 s is logged as a `gap` event, so missing time is explicit.

## License

MIT
