use crate::MB;
use crate::config::{Config, norm};
use crate::record::{App, Sample, Sys};
use std::collections::{BTreeMap, HashMap};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemProcessInformation,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId,
};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const SYSTEM_PAGEFILE_INFORMATION: SYSTEM_INFORMATION_CLASS = 18;

/// Processes that start other apps. A group started by one of these gets no "via".
const LAUNCHERS: &[&str] = &[
    "idle", "system", "smss", "wininit", "winlogon", "services", "svchost", "explorer", "sihost",
    "userinit", "taskhostw", "runtimebroker", "dllhost",
];

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// SYSTEM_PROCESS_INFORMATION as the kernel fills it (x64), up to the fields we read.
#[repr(C)]
struct ProcessEntry {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
}

#[repr(C)]
struct PagefileEntry {
    next_entry_offset: u32,
    total_size: u32,
    total_in_use: u32,
    peak_usage: u32,
    name: UnicodeString,
}

pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub create_time: i64,
    /// Private working set, bytes (Task Manager "Memory").
    pub mem: u64,
    /// Private committed bytes.
    pub commit: u64,
    /// Full working set, bytes, shared pages included.
    pub ws: u64,
}

pub struct Snapshotter {
    buf: Vec<u64>,
    small: Vec<u64>,
}

impl Default for Snapshotter {
    fn default() -> Self {
        Self::new()
    }
}

impl Snapshotter {
    pub fn new() -> Self {
        Snapshotter { buf: vec![0; 64 * 1024], small: vec![0; 512] }
    }

    pub fn processes(&mut self) -> Vec<Proc> {
        let Some(len) = query(&mut self.buf, SystemProcessInformation) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(400);
        if len == 0 {
            return out;
        }
        let base = self.buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: the kernel wrote a chain of entries into buf; offsets stay inside it.
            let e: ProcessEntry = unsafe { std::ptr::read_unaligned(base.add(offset).cast()) };
            let pid = e.unique_process_id as u32;
            let name = if e.image_name.buffer.is_null() {
                if pid == 0 { "idle" } else { "system" }.to_string()
            } else {
                let units = unsafe {
                    std::slice::from_raw_parts(e.image_name.buffer, e.image_name.length as usize / 2)
                };
                norm(&String::from_utf16_lossy(units))
            };
            out.push(Proc {
                pid,
                ppid: e.inherited_from_unique_process_id as u32,
                name,
                create_time: e.create_time,
                mem: e.working_set_private_size.max(0) as u64,
                commit: e.pagefile_usage as u64,
                ws: e.working_set_size as u64,
            });
            if e.next_entry_offset == 0 {
                break;
            }
            offset += e.next_entry_offset as usize;
        }
        out
    }

    fn pagefile_in_use_pages(&mut self) -> u64 {
        let Some(len) = query(&mut self.small, SYSTEM_PAGEFILE_INFORMATION) else {
            return 0;
        };
        if len == 0 {
            return 0;
        }
        let base = self.small.as_ptr() as *const u8;
        let (mut offset, mut pages) = (0usize, 0u64);
        loop {
            let e: PagefileEntry = unsafe { std::ptr::read_unaligned(base.add(offset).cast()) };
            pages += e.total_in_use as u64;
            if e.next_entry_offset == 0 {
                break;
            }
            offset += e.next_entry_offset as usize;
        }
        pages
    }

    pub fn system(&mut self, procs: &[Proc]) -> Sys {
        let pi = performance_info();
        let page = pi.PageSize as u64;
        let mb = |pages: usize| (pages as u64 * page / MB) as u32;
        let total = mb(pi.PhysicalTotal);
        let avail = mb(pi.PhysicalAvailable);
        Sys {
            total_mb: total,
            avail_mb: avail,
            used_pct: if total > 0 { (total - avail) * 100 / total } else { 0 },
            commit_mb: mb(pi.CommitTotal),
            commit_limit_mb: mb(pi.CommitLimit),
            pagefile_mb: (self.pagefile_in_use_pages() * page / MB) as u32,
            compressed_mb: procs.iter().find(|p| is_compression(&p.name)).map_or(0, |p| to_mb(p.ws)),
            cache_mb: mb(pi.SystemCache),
            kernel_paged_mb: mb(pi.KernelPaged),
            kernel_nonpaged_mb: mb(pi.KernelNonpaged),
            procs: procs.len() as u32,
        }
    }

    pub fn sample(&mut self, cfg: &Config, ts: String) -> Sample {
        let procs = self.processes();
        let by_pid: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
        let tops = group_tops(&procs, &by_pid, cfg);
        let idle_s = idle_secs();
        let active = idle_s < cfg.idle_after_s;
        let (fg, fg_exe) = match foreground_pid(&procs, &by_pid).and_then(|pid| by_pid.get(&pid)) {
            Some(&i) => (Some(procs[tops[i]].name.clone()), Some(procs[i].name.clone())),
            None => (None, None),
        };
        let sys = self.system(&procs);
        let apps = group_apps(&procs, &by_pid, &tops, cfg);
        Sample {
            ts,
            int_s: if active { cfg.active_interval_s } else { cfg.idle_interval_s },
            idle_s,
            active,
            fg,
            fg_exe,
            sys,
            apps,
        }
    }
}

/// Calls NtQuerySystemInformation, growing buf until the data fits. Returns the bytes written.
fn query(buf: &mut Vec<u64>, class: SYSTEM_INFORMATION_CLASS) -> Option<u32> {
    loop {
        let size = (buf.len() * 8) as u32;
        let mut needed = 0u32;
        let status =
            unsafe { NtQuerySystemInformation(class, buf.as_mut_ptr().cast(), size, &mut needed) };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let want = (needed as usize).max(size as usize * 2) / 8 + 1024;
            buf.resize(want, 0);
            continue;
        }
        return (status >= 0).then_some(needed);
    }
}

fn performance_info() -> PERFORMANCE_INFORMATION {
    let mut pi: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
    pi.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
    unsafe { K32GetPerformanceInfo(&mut pi, pi.cb) };
    if pi.PageSize == 0 {
        pi.PageSize = 4096;
    }
    pi
}

pub fn physical_total_mb() -> u32 {
    let pi = performance_info();
    (pi.PhysicalTotal as u64 * pi.PageSize as u64 / MB) as u32
}

fn is_compression(name: &str) -> bool {
    name == "memory compression" || name == "memcompression"
}

fn to_mb(bytes: u64) -> u32 {
    ((bytes + MB / 2) / MB) as u32
}

/// The parent of procs[i], if it is still the process that started it (PIDs get reused).
fn parent(procs: &[Proc], by_pid: &HashMap<u32, usize>, i: usize) -> Option<usize> {
    let p = &procs[i];
    let j = *by_pid.get(&p.ppid)?;
    (j != i && procs[j].create_time <= p.create_time).then_some(j)
}

/// For each process, the index of the process whose name labels its group:
/// helpers climb to the first non-helper ancestor; everything else is its own top.
fn group_tops(procs: &[Proc], by_pid: &HashMap<u32, usize>, cfg: &Config) -> Vec<usize> {
    (0..procs.len())
        .map(|i| {
            let mut cur = i;
            for _ in 0..32 {
                if !cfg.is_helper(&procs[cur].name) {
                    break;
                }
                match parent(procs, by_pid, cur) {
                    Some(j) => cur = j,
                    None => break,
                }
            }
            cur
        })
        .collect()
}

#[derive(Default)]
struct Acc {
    n: u32,
    mem: u64,
    commit: u64,
    parts: BTreeMap<String, u64>,
    via: Option<String>,
}

fn group_apps(
    procs: &[Proc],
    by_pid: &HashMap<u32, usize>,
    tops: &[usize],
    cfg: &Config,
) -> Vec<App> {
    let mut groups: HashMap<&str, Acc> = HashMap::new();
    for (i, p) in procs.iter().enumerate() {
        if p.pid == 0 || is_compression(&p.name) {
            continue;
        }
        let top = &procs[tops[i]];
        let acc = groups.entry(top.name.as_str()).or_default();
        acc.n += 1;
        acc.mem += p.mem;
        acc.commit += p.commit;
        *acc.parts.entry(p.name.clone()).or_default() += p.mem;
        if tops[i] == i && acc.via.is_none() {
            if let Some(j) = parent(procs, by_pid, i) {
                let launcher = &procs[j].name;
                if launcher != &p.name && !LAUNCHERS.contains(&launcher.as_str()) {
                    acc.via = Some(launcher.clone());
                }
            }
        }
    }

    let min = cfg.min_app_mb as u64 * MB;
    let mut out = Vec::with_capacity(groups.len());
    let mut other = Acc::default();
    for (name, acc) in groups {
        let own = cfg.is_own(name);
        if acc.mem < min && !own {
            other.n += acc.n;
            other.mem += acc.mem;
            other.commit += acc.commit;
            continue;
        }
        let mut parts: BTreeMap<String, u32> =
            acc.parts.into_iter().filter(|(_, m)| *m >= MB).map(|(k, m)| (k, to_mb(m))).collect();
        if parts.len() < 2 {
            parts.clear();
        }
        out.push(App {
            app: name.to_string(),
            own,
            n: acc.n,
            mem_mb: to_mb(acc.mem),
            commit_mb: to_mb(acc.commit),
            via: acc.via,
            parts,
        });
    }
    out.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb).then_with(|| a.app.cmp(&b.app)));
    if other.n > 0 {
        out.push(App {
            app: "(other)".to_string(),
            own: false,
            n: other.n,
            mem_mb: to_mb(other.mem),
            commit_mb: to_mb(other.commit),
            via: None,
            parts: BTreeMap::new(),
        });
    }
    out
}

pub fn idle_secs() -> u32 {
    let mut lii = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if GetLastInputInfo(&mut lii) == 0 {
            return 0;
        }
        GetTickCount().wrapping_sub(lii.dwTime) / 1000
    }
}

fn foreground_pid(procs: &[Proc], by_pid: &HashMap<u32, usize>) -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let is_frame_host = by_pid.get(&pid).is_some_and(|&i| procs[i].name == "applicationframehost");
    if is_frame_host {
        // Store apps draw inside ApplicationFrameHost; the app itself owns a child window.
        if let Some(child) = child_window_pid(hwnd, pid) {
            return Some(child);
        }
    }
    (pid != 0).then_some(pid)
}

fn child_window_pid(hwnd: HWND, frame_pid: u32) -> Option<u32> {
    unsafe extern "system" fn visit(child: HWND, lparam: LPARAM) -> i32 {
        let state = unsafe { &mut *(lparam as *mut (u32, u32)) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(child, &mut pid) };
        if pid != 0 && pid != state.0 {
            state.1 = pid;
            return 0;
        }
        1
    }
    let mut state = (frame_pid, 0u32);
    unsafe { EnumChildWindows(hwnd, Some(visit), &mut state as *mut (u32, u32) as LPARAM) };
    (state.1 != 0).then_some(state.1)
}
