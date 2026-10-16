#![windows_subsystem = "windows"]

//! The background logger. No window is shown; a hidden one receives power, session and stop messages.

use ramlog::config::{self, Config};
use ramlog::daily::{self, DailyLog};
use ramlog::record::{Event, Record, now_ts, today};
use ramlog::snapshot::{self, Snapshotter};
use ramlog::{DAEMON_WINDOW_CLASS, VERSION, report, win};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows_sys::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow, DispatchMessageW,
    ENDSESSION_LOGOFF, GetMessageW, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND,
    PBT_POWERSETTINGCHANGE, PostQuitMessage, RegisterClassW, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WNDCLASSW,
    WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};

enum Msg {
    SampleNow,
    Report(String),
    Quit,
}

static LOG: OnceLock<Mutex<DailyLog>> = OnceLock::new();
static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static ERROR_LOG: OnceLock<PathBuf> = OnceLock::new();

/// Wake-ups closer than this to the last sample do not take a new one.
const MIN_GAP: Duration = Duration::from_secs(10);
/// While idle, check this often whether you are back.
const IDLE_POLL: Duration = Duration::from_secs(15);

fn main() {
    let root = config::root();
    let logs = config::logs_dir(&root);
    let _ = std::fs::create_dir_all(&logs);
    let _ = ERROR_LOG.set(logs.join("ramlogd-error.log"));
    std::panic::set_hook(Box::new(|info| error(&format!("panic: {info}"))));

    let cfg = match Config::load(&root) {
        Ok(cfg) => cfg,
        Err(e) => return error(&format!("config: {e}")),
    };

    let name = win::wide(r"Local\ramlogd");
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return;
    }

    let log = match DailyLog::new(logs.clone(), snapshot::physical_total_mb(), cfg.own_apps.clone()) {
        Ok(log) => log,
        Err(e) => return error(&format!("cannot open log folder {}: {e}", logs.display())),
    };
    let _ = LOG.set(Mutex::new(log));
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(tx);

    let ts = now_ts();
    let uptime_ms = unsafe { GetTickCount64() };
    let boot = chrono::Local::now() - chrono::Duration::milliseconds(uptime_ms as i64);
    record(Event::new(&ts, "start")
        .with("version", VERSION)
        .with("pid", std::process::id())
        .with("boot", boot.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
        .with("uptime_s", uptime_ms / 1000)
        .with("active_interval_s", cfg.active_interval_s)
        .with("idle_interval_s", cfg.idle_interval_s)
        .with("idle_after_s", cfg.idle_after_s));

    let sampler = std::thread::spawn(move || sample_loop(cfg, logs, rx));
    run_window();
    send(Msg::Quit);
    let _ = sampler.join();
}

fn error(text: &str) {
    let Some(path) = ERROR_LOG.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {text}", now_ts());
    }
}

fn send(msg: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(msg);
    }
}

fn record(event: Event) {
    let ts = event.ts.clone();
    write(&ts, &Record::Event(event));
}

fn write(ts: &str, rec: &Record) {
    let Some(log) = LOG.get() else { return };
    let result = log.lock().unwrap_or_else(|e| e.into_inner()).append(ts, rec);
    match result {
        Ok(Some(finished_day)) => send(Msg::Report(finished_day)),
        Ok(None) => {}
        Err(e) => error(&format!("write failed: {e}")),
    }
}

fn make_report(logs: &Path, date: &str) {
    if let Err(e) = report::write(logs, date) {
        error(&format!("report for {date} failed: {e}"));
    }
}

fn sample_loop(cfg: Config, logs: PathBuf, rx: Receiver<Msg>) {
    let today = today();
    for date in daily::logged_dates(&logs) {
        if date < today && !daily::report_path(&logs, &date).exists() {
            make_report(&logs, &date);
        }
    }

    let mut snap = Snapshotter::new();
    let mut last: Option<(SystemTime, String, u32)> = None;
    loop {
        let now = SystemTime::now();
        let ts = now_ts();
        if let Some((then, prev_ts, int_s)) = &last {
            let elapsed = now.duration_since(*then).map_or(0, |d| d.as_secs());
            if elapsed > *int_s as u64 + 60 {
                record(Event::new(&ts, "gap")
                    .with("since", prev_ts.clone())
                    .with("secs", elapsed - *int_s as u64));
            }
        }
        let sample = snap.sample(&cfg, ts.clone());
        let (int_s, active) = (sample.int_s, sample.active);
        write(&ts, &Record::Sample(sample));
        last = Some((now, ts, int_s));

        let deadline = now + Duration::from_secs(int_s as u64);
        loop {
            let left = deadline
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO)
                .min(Duration::from_secs(int_s as u64));
            if left.is_zero() {
                break;
            }
            let step = if active { left } else { left.min(IDLE_POLL) };
            match rx.recv_timeout(step) {
                Ok(Msg::SampleNow) => {
                    if now.elapsed().unwrap_or(Duration::ZERO) >= MIN_GAP {
                        break;
                    }
                }
                Ok(Msg::Report(date)) => make_report(&logs, &date),
                Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    if !active && snapshot::idle_secs() < cfg.idle_after_s {
                        break;
                    }
                }
            }
        }
    }
}

fn run_window() {
    unsafe {
        let class = win::wide(DAEMON_WINDOW_CLASS);
        let instance = GetModuleHandleW(std::ptr::null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        // A hidden top-level window (not message-only) so that broadcast messages reach it.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return error("cannot create the hidden window");
        }
        RegisterPowerSettingNotification(hwnd, &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE);
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn event(ev: &str) {
    record(Event::new(&now_ts(), ev));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            match wparam as u32 {
                PBT_APMSUSPEND => event("suspend"),
                PBT_APMRESUMEAUTOMATIC => {
                    event("resume");
                    send(Msg::SampleNow);
                }
                PBT_POWERSETTINGCHANGE => {
                    let setting = unsafe { &*(lparam as *const POWERBROADCAST_SETTING) };
                    let guid = setting.PowerSetting;
                    let want = GUID_CONSOLE_DISPLAY_STATE;
                    let is_display = guid.data1 == want.data1
                        && guid.data2 == want.data2
                        && guid.data3 == want.data3
                        && guid.data4 == want.data4;
                    if is_display && setting.DataLength >= 4 {
                        let state =
                            unsafe { std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32) };
                        match state {
                            0 => event("display_off"),
                            1 => {
                                event("display_on");
                                send(Msg::SampleNow);
                            }
                            _ => event("display_dim"),
                        }
                    }
                }
                _ => {}
            }
            1
        }
        WM_WTSSESSION_CHANGE => {
            match wparam as u32 {
                WTS_SESSION_LOCK => event("lock"),
                WTS_SESSION_UNLOCK => {
                    event("unlock");
                    send(Msg::SampleNow);
                }
                _ => {}
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                event(if lparam as u32 & ENDSESSION_LOGOFF != 0 { "logoff" } else { "shutdown" });
            }
            0
        }
        WM_CLOSE => {
            event("stop");
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
