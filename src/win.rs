use crate::DAEMON_WINDOW_CLASS;
use std::io;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ,
    RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_SETTINGCHANGE,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ramlog";
const ENV_KEY: &str = "Environment";

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

fn same_dir(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('\\').eq_ignore_ascii_case(b.trim().trim_end_matches('\\'))
}

/// `raw` (a PATH value) with `dir` appended, or None when it is already there.
pub fn path_with(raw: &str, dir: &str) -> Option<String> {
    if raw.split(';').any(|p| same_dir(p, dir)) {
        return None;
    }
    let mut out = raw.trim_end_matches(';').to_string();
    if !out.is_empty() {
        out.push(';');
    }
    out.push_str(dir);
    Some(out)
}

/// `raw` (a PATH value) without `dir`, or None when it is not there.
pub fn path_without(raw: &str, dir: &str) -> Option<String> {
    let kept: Vec<&str> = raw.split(';').filter(|p| !same_dir(p, dir)).collect();
    (kept.len() != raw.split(';').count()).then(|| kept.join(";"))
}

/// The user PATH as stored (variables such as %USERPROFILE% not expanded), and its value type.
fn user_path() -> io::Result<(String, REG_VALUE_TYPE)> {
    let (key, value) = (wide(ENV_KEY), wide("Path"));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut kind: REG_VALUE_TYPE = 0;
    let mut bytes = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            flags,
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == 2 {
        return Ok((String::new(), REG_EXPAND_SZ));
    }
    win32_result(status)?;
    let mut buf = vec![0u16; bytes as usize / 2 + 1];
    let mut bytes = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            flags,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    win32_result(status)?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok((String::from_utf16_lossy(&buf[..len]), kind))
}

fn set_user_path(value: &str, kind: REG_VALUE_TYPE) -> io::Result<()> {
    let (key, name, data) = (wide(ENV_KEY), wide("Path"), wide(value));
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            kind,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    win32_result(status)?;
    // Explorer reloads the environment on this broadcast, so new terminals see the change.
    let env = wide(ENV_KEY);
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        )
    };
    Ok(())
}

pub fn user_path_has(dir: &str) -> bool {
    user_path().is_ok_and(|(raw, _)| raw.split(';').any(|p| same_dir(p, dir)))
}

/// Adds `dir` to the user PATH. Ok(false) when it was already there.
pub fn user_path_add(dir: &str) -> io::Result<bool> {
    let (raw, kind) = user_path()?;
    match path_with(&raw, dir) {
        Some(new) => set_user_path(&new, kind).map(|()| true),
        None => Ok(false),
    }
}

/// Removes `dir` from the user PATH. Ok(false) when it was not there.
pub fn user_path_remove(dir: &str) -> io::Result<bool> {
    let (raw, kind) = user_path()?;
    match path_without(&raw, dir) {
        Some(new) => set_user_path(&new, kind).map(|()| true),
        None => Ok(false),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_edits_keep_other_entries() {
        let raw = r"C:\a;%USERPROFILE%\b;";
        assert_eq!(path_with(raw, r"D:\ramlog\bin").as_deref(), Some(r"C:\a;%USERPROFILE%\b;D:\ramlog\bin"));
        assert_eq!(path_with(r"C:\a;d:\RAMLOG\bin\", r"D:\ramlog\bin"), None);
        assert_eq!(path_with("", r"D:\ramlog\bin").as_deref(), Some(r"D:\ramlog\bin"));
        assert_eq!(
            path_without(r"C:\a;D:\ramlog\bin;%USERPROFILE%\b", r"d:\ramlog\bin\").as_deref(),
            Some(r"C:\a;%USERPROFILE%\b")
        );
        assert_eq!(path_without(r"C:\a", r"D:\ramlog\bin"), None);
    }
}
