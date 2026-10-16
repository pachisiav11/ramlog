# ramlog log schema (version 1)

ramlog writes one JSON Lines file per local day: `YYYY-MM-DD.jsonl`. Every line is one JSON object. The `t` field says which kind of record it is: `header`, `sample`, or `event`. Lines are in time order. A daily summary, `YYYY-MM-DD.report.md`, is written next to each finished day.

All memory values are whole megabytes (MiB). All times are local time with the UTC offset, for example `2026-10-16T22:15:30+05:30`.

## header

The first line of every file.

| Field | Meaning |
|---|---|
| `schema` | Schema version of this file (1). |
| `tool`, `version` | `ramlog` and its version. |
| `host` | Computer name. |
| `date` | The day this file covers. |
| `total_mb` | Usable physical RAM. |
| `own_apps` | Apps the user wrote; samples mark them with `"own": true`. |
| `doc` | Pointer to this file. |

## sample

One snapshot of memory use.

| Field | Meaning |
|---|---|
| `ts` | When the sample was taken. |
| `int_s` | Seconds this sample stands for: the planned wait until the next sample. 30 while the user is active, 120 while idle (configurable). Weight averages by this value. |
| `idle_s` | Seconds since the last keyboard or mouse input. |
| `active` | `true` when `idle_s` is below the idle threshold (300 s by default). |
| `fg` | App group of the foreground window (see "App groups"). Missing when no window has focus. |
| `fg_exe` | Exe of the foreground window. Store apps are resolved through ApplicationFrameHost. |
| `sys` | System totals, see below. |
| `apps` | One entry per app group, largest `mem_mb` first. Groups under the size threshold (5 MB by default) are summed into one `(other)` entry at the end. Own apps are always listed. |

### sys

| Field | Meaning |
|---|---|
| `total_mb` | Usable physical RAM. |
| `avail_mb` | Available RAM (free + standby). The main signal for memory pressure. |
| `used_pct` | `(total_mb - avail_mb) / total_mb` in percent. |
| `commit_mb`, `commit_limit_mb` | Committed memory and the commit limit (RAM + page file). When commit nears the limit, Windows grows the page file or allocations fail. |
| `pagefile_mb` | Page file space in use. |
| `compressed_mb` | RAM held by the Memory Compression process (compressed pages). |
| `cache_mb` | System file cache. |
| `kernel_paged_mb`, `kernel_nonpaged_mb` | Kernel pools. |
| `procs` | Number of processes. |

### apps[]

| Field | Meaning |
|---|---|
| `app` | Group name: lowercase exe name without `.exe`. |
| `own` | Present and `true` for the user's own apps. |
| `n` | Number of processes in the group. |
| `mem_mb` | Sum of private working sets: the RAM only this group uses. Matches the Task Manager "Memory" column. |
| `commit_mb` | Sum of private committed memory (in RAM or in the page file). |
| `via` | The process that started this group, when it is not a normal launcher (Explorer, services, svchost and similar). Example: a native messaging host started by Chrome has `"via": "chrome"`. |
| `parts` | When a group has more than one exe: memory per exe (`mem_mb`), for exes of 1 MB or more. |

## event

Things that happened between samples. Fields: `ts`, `ev`, and extra fields per kind.

| `ev` | Meaning | Extra fields |
|---|---|---|
| `start` | Logger started (usually at logon). | `version`, `pid`, `boot` (last full restart; Fast Startup shutdowns do not reset it), `uptime_s`, `active_interval_s`, `idle_interval_s`, `idle_after_s` |
| `stop` | Logger stopped with `ramlog stop`. | |
| `suspend`, `resume` | System sleep and wake. | |
| `display_off`, `display_on`, `display_dim` | Screen state. On Modern Standby laptops, display off often comes before sleep. One `display_on` is logged right after `start`. | |
| `lock`, `unlock` | Session lock and unlock. | |
| `logoff`, `shutdown` | The session or system is ending. | |
| `gap` | Within one logger run, the time since the last sample was more than its `int_s` + 60 s. Usually sleep or hibernate. Time while the logger was not running shows as a `stop` (or nothing, after a crash or power loss) followed by a `start`. | `since` (time of the previous sample), `secs` (seconds not covered) |

## App groups

Every process belongs to the group of its own exe name, except helper processes. A helper is counted under the nearest ancestor that is not a helper. Helpers are `msedgewebview2`, `conhost`, `openconsole`, `crashpad_handler`, `node`, `python`, `pythonw`, `py`, `uv`, `cmd`, `powershell`, `pwsh`, `bash`, `sh`, `git`, plus the `helpers` list in `ramlog.toml`. So a WebView2 runtime inside Teams counts as `ms-teams`, and Node MCP servers started by Claude count as `claude`. Processes with the same exe name are always in the same group (all Chrome processes are `chrome`). The Memory Compression process is reported in `sys.compressed_mb`, not in `apps`.

## Tips for analysis

- Weight every sample by `int_s`. Idle samples stand for 4 times as much time as active ones.
- Days can have gaps (sleep, shutdown). Check `gap`, `suspend`, and `start` events before reading a trend across them.
- Low `avail_mb` together with high `pagefile_mb` or `compressed_mb` is real memory pressure.
- For leaks, follow one app's `mem_mb` across consecutive samples while `fg` and `active` stay the same.
- A `(other)` entry holds many small processes; its size is the sum of them.
