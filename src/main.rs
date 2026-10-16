use ramlog::config::{self, CONFIG_FILE, Config};
use ramlog::daily;
use ramlog::record::{Record, now_ts, today};
use ramlog::snapshot::Snapshotter;
use ramlog::{MB, report, win};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

const USAGE: &str = "\
ramlog: logs RAM use per app on Windows, for later analysis.

Usage:
  ramlog status          Show if the logger runs, its own RAM, and today's log
  ramlog now [N]         Take one sample now and print the top N apps (default 15)
  ramlog report [DATE]   Write and print the report for DATE: today (default), yesterday, or YYYY-MM-DD
  ramlog start           Start the background logger
  ramlog stop            Stop the background logger
  ramlog install         Copy the exes to bin\\, add bin\\ to PATH, start the logger at logon and now
  ramlog uninstall       Stop the logger, remove it from logon and PATH (files stay)
  ramlog paths           Show the project, config, log, and exe paths
";

const DETACHED_PROCESS: u32 = 0x0000_0008;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = config::root();
    let logs = config::logs_dir(&root);
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("status") => status(&root, &logs),
        Some("now") => Config::load(&root).and_then(|cfg| now(&cfg, args.get(1))),
        Some("report") => report_cmd(&logs, args.get(1)),
        Some("start") => start(&root),
        Some("stop") => stop(),
        Some("install") => install(&root),
        Some("uninstall") => uninstall(&root),
        Some("paths") => paths(&root, &logs),
        Some(other) => Err(format!("unknown command: {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ramlog: {e}");
            ExitCode::FAILURE
        }
    }
}

fn gb(mb: u32) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

fn status(root: &Path, logs: &Path) -> Result<(), String> {
    match win::daemon_pid() {
        Some(pid) => {
            let procs = Snapshotter::new().processes();
            match procs.iter().find(|p| p.pid == pid) {
                Some(p) => {
                    let started = win::filetime_local(p.create_time)
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    println!(
                        "Logger: running (pid {pid}, {:.1} MB RAM, started {started})",
                        p.mem as f64 / MB as f64
                    );
                }
                None => println!("Logger: running (pid {pid})"),
            }
        }
        None => println!("Logger: not running. Start it with `ramlog start`."),
    }
    match win::run_key_get() {
        Some(cmd) => println!("Logon start: on ({cmd})"),
        None => println!("Logon start: off. Turn it on with `ramlog install`."),
    }
    let bin = root.join("bin");
    if win::user_path_has(&bin.display().to_string()) {
        println!("PATH: on (`ramlog` works in any new terminal)");
    } else {
        println!("PATH: off. `ramlog install` adds {}", bin.display());
    }
    if let Err(e) = Config::load(root) {
        println!("Config: ERROR {e}");
    }
    println!("Logs: {}", logs.display());

    let date = today();
    let path = daily::day_path(logs, &date);
    let Ok(text) = std::fs::read_to_string(&path) else {
        println!("Today: no log file yet");
        return Ok(());
    };
    let is_sample = |l: &&str| l.starts_with(r#"{"t":"sample""#);
    let samples = text.lines().filter(is_sample).count();
    let events = text.lines().filter(|l| l.starts_with(r#"{"t":"event""#)).count();
    println!(
        "Today: {} ({} KB), {samples} samples, {events} events",
        path.file_name().unwrap_or_default().to_string_lossy(),
        text.len() / 1024
    );
    if let Some(Ok(Record::Sample(s))) =
        text.lines().rev().find(is_sample).map(serde_json::from_str::<Record>)
    {
        let top: Vec<String> =
            s.apps.iter().take(3).map(|a| format!("{} {} MB", a.app, a.mem_mb)).collect();
        println!(
            "Last sample: {}, {} available, top: {}",
            s.ts.get(11..19).unwrap_or(&s.ts),
            gb(s.sys.avail_mb),
            top.join(", ")
        );
    }
    let errors = logs.join("ramlogd-error.log");
    if errors.is_file() {
        println!("Errors were logged: {}", errors.display());
    }
    Ok(())
}

fn now(cfg: &Config, n: Option<&String>) -> Result<(), String> {
    let top = match n {
        Some(s) => s.parse::<usize>().map_err(|_| format!("not a number: {s}"))?,
        None => 15,
    };
    let s = Snapshotter::new().sample(cfg, now_ts());
    let sys = &s.sys;
    println!(
        "{}  RAM {} total, {} available ({}% used)",
        s.ts.get(..19).unwrap_or(&s.ts).replace('T', " "),
        gb(sys.total_mb),
        gb(sys.avail_mb),
        sys.used_pct
    );
    println!(
        "Commit {} of {}, page file {}, compressed {}, cache {}",
        gb(sys.commit_mb),
        gb(sys.commit_limit_mb),
        gb(sys.pagefile_mb),
        gb(sys.compressed_mb),
        gb(sys.cache_mb)
    );
    println!(
        "Focus: {}   Idle: {} s   Processes: {}\n",
        s.fg.as_deref().unwrap_or("-"),
        s.idle_s,
        sys.procs
    );
    println!("{:<40} {:>5} {:>10} {:>10}", "App", "Procs", "Memory MB", "Commit MB");
    for a in s.apps.iter().take(top) {
        let name = if a.own { format!("{} (own)", a.app) } else { a.app.clone() };
        println!("{name:<40} {:>5} {:>10} {:>10}", a.n, a.mem_mb, a.commit_mb);
    }
    Ok(())
}

fn report_cmd(logs: &Path, arg: Option<&String>) -> Result<(), String> {
    let date = match arg.map(String::as_str) {
        None | Some("today") => today(),
        Some("yesterday") => (chrono::Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string(),
        Some(d) => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map(|_| d.to_string())
            .map_err(|_| format!("not a date: {d} (use YYYY-MM-DD, today, or yesterday)"))?,
    };
    let (path, text) =
        report::write(logs, &date).map_err(|e| format!("no report for {date}: {e}"))?;
    print!("{text}");
    println!("\nSaved: {}", path.display());
    Ok(())
}

fn daemon_exe(root: &Path) -> Result<PathBuf, String> {
    let installed = root.join("bin").join("ramlogd.exe");
    if installed.is_file() {
        return Ok(installed);
    }
    let sibling = exe_dir().join("ramlogd.exe");
    if sibling.is_file() {
        return Ok(sibling);
    }
    Err("ramlogd.exe not found. Build it with `cargo build --release`.".to_string())
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn start(root: &Path) -> Result<(), String> {
    if let Some(pid) = win::daemon_pid() {
        println!("Already running (pid {pid}).");
        return Ok(());
    }
    let exe = daemon_exe(root)?;
    Command::new(&exe)
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(pid) = win::daemon_pid() {
            println!("Started {} (pid {pid}).", exe.display());
            return Ok(());
        }
    }
    Err(format!(
        "started {} but it is not running. See {}",
        exe.display(),
        config::logs_dir(root).join("ramlogd-error.log").display()
    ))
}

fn stop() -> Result<(), String> {
    let (Some(hwnd), Some(pid)) = (win::daemon_window(), win::daemon_pid()) else {
        println!("Not running.");
        return Ok(());
    };
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    if win::wait_exit(pid, 10_000) {
        println!("Stopped (pid {pid}).");
        Ok(())
    } else {
        Err(format!("pid {pid} did not stop within 10 s"))
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn install(root: &Path) -> Result<(), String> {
    let here = exe_dir();
    let bin = root.join("bin");
    stop()?;
    if !same_dir(&here, &bin) {
        std::fs::create_dir_all(&bin).map_err(|e| format!("cannot create {}: {e}", bin.display()))?;
        for name in ["ramlog.exe", "ramlogd.exe"] {
            let from = here.join(name);
            std::fs::copy(&from, bin.join(name))
                .map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
        }
        println!("Copied ramlog.exe and ramlogd.exe to {}", bin.display());
    }
    let command = format!("\"{}\"", bin.join("ramlogd.exe").display());
    win::run_key_set(&command).map_err(|e| format!("cannot set the logon entry: {e}"))?;
    println!("Logon start: on ({command})");
    let bin_dir = bin.display().to_string();
    match win::user_path_add(&bin_dir) {
        Ok(true) => println!("Added {bin_dir} to your user PATH. New terminals can run `ramlog` from any folder."),
        Ok(false) => println!("PATH already has {bin_dir}"),
        Err(e) => println!("Could not add {bin_dir} to your user PATH: {e}"),
    }
    start(root)
}

fn uninstall(root: &Path) -> Result<(), String> {
    stop()?;
    win::run_key_delete().map_err(|e| format!("cannot remove the logon entry: {e}"))?;
    println!("Logon start: off.");
    let bin_dir = root.join("bin").display().to_string();
    match win::user_path_remove(&bin_dir) {
        Ok(true) => println!("Removed {bin_dir} from your user PATH."),
        Ok(false) => {}
        Err(e) => println!("Could not remove {bin_dir} from your user PATH: {e}"),
    }
    println!("Logs and exes are kept.");
    Ok(())
}

fn paths(root: &Path, logs: &Path) -> Result<(), String> {
    let cfg = root.join(CONFIG_FILE);
    println!("Project: {}", root.display());
    println!("Config:  {}{}", cfg.display(), if cfg.is_file() { "" } else { " (missing, defaults used)" });
    println!("Logs:    {}", logs.display());
    match daemon_exe(root) {
        Ok(exe) => println!("Logger:  {}", exe.display()),
        Err(e) => println!("Logger:  {e}"),
    }
    Ok(())
}
