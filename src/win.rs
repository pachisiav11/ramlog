use crate::DAEMON_WINDOW_CLASS;
use std::io;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ramlog";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The hidden window of the running logger, if any.
pub fn daemon_window() -> Option<HWND> {
    let class = wide(DAEMON_WINDOW_CLASS);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

pub fn daemon_pid() -> Option<u32> {
    let hwnd = daemon_window()?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (pid != 0).then_some(pid)
}

/// Waits until the process exits. True if it exited (or was already gone).
pub fn wait_exit(pid: u32, ms: u32) -> bool {
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let done = WaitForSingleObject(handle, ms) == WAIT_OBJECT_0;
        CloseHandle(handle);
        done
    }
}

pub fn run_key_get() -> Option<String> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub fn run_key_set(command: &str) -> io::Result<()> {
    let (key, value, data) = (wide(RUN_KEY), wide(RUN_VALUE), wide(command));
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    win32_result(status)
}

pub fn run_key_delete() -> io::Result<()> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
    // ERROR_FILE_NOT_FOUND: nothing to remove.
    if status == 2 { Ok(()) } else { win32_result(status) }
}

fn win32_result(status: u32) -> io::Result<()> {
    if status == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(status as i32)) }
}

/// Converts a FILETIME value (100 ns ticks since 1601) to local time.
pub fn filetime_local(ft: i64) -> Option<chrono::DateTime<chrono::Local>> {
    let unix_100ns = ft - 116_444_736_000_000_000;
    let secs = unix_100ns.div_euclid(10_000_000);
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.with_timezone(&chrono::Local))
}
