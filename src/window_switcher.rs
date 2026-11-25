use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt as WindowsCommandExt;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone)]
pub struct WindowItem {
    pub id: String,
    pub title: String,
    pub class_name: String,
    pub pid: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub executable_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessStats {
    pub memory_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub executable_path: Option<String>,
}

static CPU_SAMPLE_CACHE: OnceLock<Mutex<HashMap<u32, (u64, Instant)>>> = OnceLock::new();
static PROCESS_STATS_CACHE: OnceLock<Mutex<HashMap<u32, (ProcessStats, Instant)>>> = OnceLock::new();

fn cpu_sample_cache() -> &'static Mutex<HashMap<u32, (u64, Instant)>> {
    CPU_SAMPLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn process_stats_cache() -> &'static Mutex<HashMap<u32, (ProcessStats, Instant)>> {
    PROCESS_STATS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn format_memory(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_cpu(cpu: f32) -> String {
    if cpu >= 10.0 {
        format!("{:.0}%", cpu)
    } else {
        format!("{:.1}%", cpu)
    }
}

fn enumerate_open_windows() -> Vec<WindowItem> {
    let mut windows = Vec::new();
    #[cfg(unix)]
    let self_pid = std::process::id();

    // 1. Try Hyprland (Wayland) — direct IPC socket first, CLI fallback
    #[cfg(unix)]
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        let json_str = hyprland_ipc_query("j/clients").or_else(|| {
            let output = Command::new("hyprctl").args(&["clients", "-j"]).output().ok()?;
            String::from_utf8(output.stdout).ok()
        });
        if let Some(json_str) = json_str {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(arr) = val.as_array() {
                    for item in arr {
                        let title = item["title"].as_str().unwrap_or("").to_string();
                        let class_name = item["class"].as_str().unwrap_or("").to_string();
                        let address = item["address"].as_str().unwrap_or("").to_string();
                        let pid = item["pid"]
                            .as_u64()
                            .and_then(|p| u32::try_from(p).ok())
                            .filter(|&p| p > 0);
                        if pid == Some(self_pid) {
                            continue;
                        }
                        if !title.is_empty() && !address.is_empty() {
                            windows.push(WindowItem {
                                id: format!("hypr:{}", address),
                                title,
                                class_name,
                                pid,
                                memory_bytes: None,
                                cpu_percent: None,
                                executable_path: None,
                            });
                        }
                    }
                    if !windows.is_empty() {
                        return windows;
                    }
                }
            }
        }
    }

    // 2. Try Sway (Wayland) — query Sway IPC socket directly (<1ms) with CLI fallback
    #[cfg(unix)]
    if let Ok(sway_sock) = std::env::var("SWAYSOCK") {
        let tree_val = sway_ipc_get_tree(&sway_sock).or_else(|| {
            let output = Command::new("swaymsg").args(&["-t", "get_tree"]).output().ok()?;
            let json_str = String::from_utf8(output.stdout).ok()?;
            serde_json::from_str::<serde_json::Value>(&json_str).ok()
        });
        if let Some(val) = tree_val {
            collect_sway_windows(&val, &mut windows, self_pid);
            if !windows.is_empty() {
                return windows;
            }
        }
    }

    // 2.5 Try GNOME Wayland ("Window Calls" extension over D-Bus) if present
    #[cfg(target_os = "linux")]
    if is_gnome_wayland() {
        let gnome_windows = enumerate_gnome_window_calls(self_pid);
        if !gnome_windows.is_empty() {
            return gnome_windows;
        }
    }

    // 2.6 Try KDE Plasma Wayland via kdotool if present
    #[cfg(target_os = "linux")]
    if is_kde_wayland() {
        let kde_windows = enumerate_kde_kdotool(self_pid);
        if !kde_windows.is_empty() {
            return kde_windows;
        }
    }

    // 3. Try pure-Rust X11 (x11rb) for X11 & XWayland without external wmctrl
    #[cfg(all(target_os = "linux", feature = "x11"))]
    {
        let x11_windows = enumerate_x11_windows(self_pid);
        if !x11_windows.is_empty() {
            return x11_windows;
        }
    }

    // 3.5 Fallback to wmctrl (X11 & XWayland) if x11 feature is disabled or unavailable
    #[cfg(unix)]
    if let Ok(output) = Command::new("wmctrl").args(&["-l", "-p", "-x"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let id = parts[0].to_string();
                let pid = parts[2].parse::<u32>().ok().filter(|&p| p > 0);
                if pid == Some(self_pid) {
                    continue;
                }
                let class_name = parts[3].to_string();
                let title = parts[5..].join(" ");
                if !title.is_empty()
                    && !title.eq_ignore_ascii_case("Desktop")
                    && !class_name.contains("gnome-shell")
                {
                    windows.push(WindowItem {
                        id: format!("wmctrl:{}", id),
                        title,
                        class_name,
                        pid,
                        memory_bytes: None,
                        cpu_percent: None,
                        executable_path: None,
                    });
                }
            }
        }
        if !windows.is_empty() {
            return windows;
        }
    }

    // 4. Windows 10/11 native active window switcher (<1ms via Win32 EnumWindows)
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongW,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
            WS_EX_TOOLWINDOW,
        };

        /// For UWP apps hosted inside `ApplicationFrameWindow` (`ApplicationFrameHost.exe`),
        /// resolve the actual child app PID instead of reporting the shared frame host PID.
        fn resolve_uwp_child_pid(hwnd: HWND, host_pid: u32) -> u32 {
            struct SearchState {
                host_pid: u32,
                found_pid: u32,
            }

            unsafe extern "system" fn enum_child_proc(child: HWND, lparam: LPARAM) -> BOOL {
                unsafe {
                    let state = &mut *(lparam as *mut SearchState);
                    let mut child_pid: u32 = 0;
                    GetWindowThreadProcessId(child, &mut child_pid);
                    if child_pid != 0 && child_pid != state.host_pid {
                        state.found_pid = child_pid;
                        return 0;
                    }
                    1
                }
            }

            let mut class_buf = [0u16; 64];
            let class_len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32) };
            if class_len > 0 {
                let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
                if class_name == "ApplicationFrameWindow" {
                    let mut state = SearchState {
                        host_pid,
                        found_pid: 0,
                    };
                    unsafe {
                        EnumChildWindows(hwnd, Some(enum_child_proc), &mut state as *mut _ as LPARAM);
                    }
                    if state.found_pid != 0 {
                        return state.found_pid;
                    }
                }
            }
            host_pid
        }

        unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            unsafe {
                let list = &mut *(lparam as *mut Vec<WindowItem>);

                if IsWindowVisible(hwnd) == 0 {
                    return 1;
                }

                // Filter out tool windows or overlay utility popups
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                if (ex_style & WS_EX_TOOLWINDOW) != 0 {
                    return 1;
                }

                let len = GetWindowTextLengthW(hwnd);
                if len > 0 {
                    let mut title_buf = vec![0u16; (len + 1) as usize];
                    let read_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), len + 1);
                    if read_len > 0 {
                        let title = String::from_utf16_lossy(&title_buf[..read_len as usize])
                            .trim()
                            .to_string();
                        if !title.is_empty() && title != "View Launcher" && title != "TiaRun" {
                            let mut pid: u32 = 0;
                            GetWindowThreadProcessId(hwnd, &mut pid);
                            if pid > 0 {
                                pid = resolve_uwp_child_pid(hwnd, pid);
                            }
                            if pid != std::process::id() {
                                list.push(WindowItem {
                                    id: format!("winhwnd:{}", hwnd as isize),
                                    title,
                                    class_name: format!("PID: {}", pid),
                                    pid: (pid > 0).then_some(pid),
                                    memory_bytes: None,
                                    cpu_percent: None,
                                    executable_path: None,
                                });
                            }
                        }
                    }
                }
                1
            }
        }

        unsafe {
            let mut win_list: Vec<WindowItem> = Vec::new();
            EnumWindows(Some(enum_windows_callback), &mut win_list as *mut _ as LPARAM);
            windows.extend(win_list);
        }
    }

    windows
}

/// Enumerates open windows and collects full process metrics (PSS memory + CPU%).
/// Intended for background refresh workers rather than synchronous UI-thread queries.
pub fn get_open_windows() -> Vec<WindowItem> {
    let mut windows = enumerate_open_windows();
    populate_process_metrics(&mut windows);
    windows
}

/// Enumerates open windows without blocking on `/proc` or `smaps_rollup`;
/// fills any recently cached process stats so UI renders immediately (<2ms).
pub fn get_open_windows_fast() -> Vec<WindowItem> {
    let mut windows = enumerate_open_windows();
    apply_cached_process_metrics(&mut windows);
    windows
}

fn apply_cached_process_metrics(windows: &mut [WindowItem]) {
    let now = Instant::now();
    let mut cache = process_stats_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (_, ts)| now.duration_since(*ts) <= Duration::from_secs(15));
    for win in windows.iter_mut() {
        if let Some(pid) = win.pid {
            if let Some((stats, _)) = cache.get(&pid) {
                win.memory_bytes = stats.memory_bytes;
                win.cpu_percent = stats.cpu_percent;
                win.executable_path = stats.executable_path.clone();
            }
        }
    }
}

fn populate_process_metrics(windows: &mut [WindowItem]) {
    if windows.is_empty() {
        return;
    }

    let mut stats_by_pid: HashMap<u32, ProcessStats> = HashMap::new();
    for win in windows.iter() {
        if let Some(pid) = win.pid {
            stats_by_pid.entry(pid).or_default();
        }
    }
    if stats_by_pid.is_empty() {
        return;
    }

    collect_stats_for_pids(&mut stats_by_pid);

    let now = Instant::now();
    {
        let mut cache = process_stats_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (_, ts)| now.duration_since(*ts) <= Duration::from_secs(15));
        for (&pid, stats) in &stats_by_pid {
            cache.insert(pid, (stats.clone(), now));
        }
    }

    for win in windows.iter_mut() {
        if let Some(pid) = win.pid {
            if let Some(stats) = stats_by_pid.get(&pid) {
                win.memory_bytes = stats.memory_bytes;
                win.cpu_percent = stats.cpu_percent;
                win.executable_path = stats.executable_path.clone();
            }
        }
    }
}

pub fn get_process_stats(pid: u32) -> ProcessStats {
    let mut map = HashMap::from([(pid, ProcessStats::default())]);
    collect_stats_for_pids(&mut map);
    map.remove(&pid).unwrap_or_default()
}

/// Collects `root_pid` and all descendant PIDs via BFS, guarding against PID <= 1 and cycles.
fn collect_descendant_pids(
    root_pid: u32,
    children_by_ppid: &HashMap<u32, Vec<u32>>,
) -> Vec<u32> {
    const MAX_TREE_PIDS: usize = 512;
    if root_pid <= 1 {
        return Vec::new();
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut out = Vec::new();
    visited.insert(root_pid);
    queue.push_back(root_pid);

    while let Some(pid) = queue.pop_front() {
        out.push(pid);
        if out.len() >= MAX_TREE_PIDS {
            break;
        }
        if let Some(children) = children_by_ppid.get(&pid) {
            for &child in children {
                if child > 1 && visited.insert(child) {
                    queue.push_back(child);
                }
            }
        }
    }
    out
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_proc_stat(stat: &str) -> Option<(u32, u64, u64)> {
    let rparen = stat.rfind(')')?;
    let after = stat.get(rparen + 2..)?;
    let mut fields = after.split_whitespace();
    // Fields after ") ": state(0), ppid(1), pgrp(2)..cmajflt(10), utime(11), stime(12), ..., starttime(19)
    let _state = fields.next()?;
    let ppid = fields.next()?.parse::<u32>().ok()?;
    let utime = fields.nth(9)?.parse::<u64>().ok()?;
    let stime = fields.next()?.parse::<u64>().ok()?;
    let starttime = fields.nth(6)?.parse::<u64>().ok()?;
    Some((ppid, utime.saturating_add(stime), starttime))
}

#[cfg(any(target_os = "linux", test))]
fn parse_smaps_rollup_pss_bytes(contents: &str) -> Option<u64> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("Pss:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn build_linux_proc_snapshot() -> (HashMap<u32, Vec<u32>>, HashMap<u32, (u64, u64)>) {
    let mut children_by_ppid: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut stat_by_pid: HashMap<u32, (u64, u64)> = HashMap::new();

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return (children_by_ppid, stat_by_pid);
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid <= 1 {
            continue;
        }
        let stat_path = entry.path().join("stat");
        let Ok(stat) = std::fs::read_to_string(stat_path) else {
            continue;
        };
        if let Some((ppid, total_ticks, start_ticks)) = parse_linux_proc_stat(&stat) {
            if ppid > 1 && ppid != pid {
                children_by_ppid.entry(ppid).or_default().push(pid);
            }
            stat_by_pid.insert(pid, (total_ticks, start_ticks));
        }
    }

    (children_by_ppid, stat_by_pid)
}

#[cfg(target_os = "linux")]
fn collect_stats_for_pids(stats_by_pid: &mut HashMap<u32, ProcessStats>) {
    static PAGE_SIZE: OnceLock<u64> = OnceLock::new();
    static CLK_TCK: OnceLock<f64> = OnceLock::new();

    let page_size = *PAGE_SIZE.get_or_init(|| {
        let val = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if val > 0 { val as u64 } else { 4096 }
    });
    let clk_tck = *CLK_TCK.get_or_init(|| {
        let val = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if val > 0 { val as f64 } else { 100.0 }
    });
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0)
        .max(1.0);

    let (children_by_ppid, stat_by_pid) = build_linux_proc_snapshot();

    let now = Instant::now();
    let mut uptime_cache: Option<f64> = None;
    let mut cache_guard = cpu_sample_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache_guard.retain(|_, (_, ts)| now.duration_since(*ts) <= Duration::from_secs(30));

    let mut per_pid_mem: HashMap<u32, Option<u64>> = HashMap::new();
    let mut per_pid_cpu: HashMap<u32, Option<f32>> = HashMap::new();

    for (&root_pid, stats) in stats_by_pid.iter_mut() {
        stats.executable_path = read_linux_exe_path(root_pid);
        let tree_pids = collect_descendant_pids(root_pid, &children_by_ppid);

        let mut total_mem: u64 = 0;
        let mut has_mem = false;
        let mut total_cpu: f32 = 0.0;
        let mut has_cpu = false;

        for pid in tree_pids {
            let mem = *per_pid_mem
                .entry(pid)
                .or_insert_with(|| read_linux_process_memory_bytes(pid, page_size));
            if let Some(bytes) = mem {
                total_mem = total_mem.saturating_add(bytes);
                has_mem = true;
            }

            let cpu = *per_pid_cpu.entry(pid).or_insert_with(|| {
                let (total_ticks, start_ticks) = stat_by_pid
                    .get(&pid)
                    .copied()
                    .or_else(|| read_linux_proc_stat(pid))?;
                let cpu = if let Some(&(prev_ticks, prev_at)) = cache_guard.get(&pid) {
                    let elapsed = now.duration_since(prev_at);
                    if elapsed >= Duration::from_millis(80) && elapsed <= Duration::from_secs(15) {
                        let delta_secs = total_ticks.saturating_sub(prev_ticks) as f64 / clk_tck;
                        let wall_secs = elapsed.as_secs_f64();
                        ((delta_secs / wall_secs) * 100.0 / num_cpus).clamp(0.0, 100.0) as f32
                    } else {
                        let uptime = *uptime_cache.get_or_insert_with(read_linux_uptime);
                        lifetime_cpu_percent(total_ticks, start_ticks, uptime, clk_tck, num_cpus)
                    }
                } else {
                    let uptime = *uptime_cache.get_or_insert_with(read_linux_uptime);
                    lifetime_cpu_percent(total_ticks, start_ticks, uptime, clk_tck, num_cpus)
                };
                cache_guard.insert(pid, (total_ticks, now));
                Some(cpu)
            });

            if let Some(pct) = cpu {
                total_cpu += pct;
                has_cpu = true;
            }
        }

        stats.memory_bytes = has_mem.then_some(total_mem);
        stats.cpu_percent = has_cpu.then_some(total_cpu.clamp(0.0, 100.0));
    }
}

#[cfg(target_os = "linux")]
fn lifetime_cpu_percent(
    total_ticks: u64,
    start_ticks: u64,
    uptime_secs: f64,
    clk_tck: f64,
    num_cpus: f64,
) -> f32 {
    if uptime_secs <= 0.0 {
        return 0.0;
    }
    let proc_age_secs = (uptime_secs - (start_ticks as f64 / clk_tck)).max(0.1);
    let proc_cpu_secs = total_ticks as f64 / clk_tck;
    ((proc_cpu_secs / proc_age_secs) * 100.0 / num_cpus).clamp(0.0, 100.0) as f32
}

#[cfg(target_os = "linux")]
fn read_linux_exe_path(pid: u32) -> Option<String> {
    let raw = std::fs::read_link(format!("/proc/{}/exe", pid))
        .ok()?
        .to_string_lossy()
        .into_owned();
    let trimmed = raw.strip_suffix(" (deleted)").unwrap_or(&raw).trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(target_os = "linux")]
fn read_linux_uptime() -> f64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
fn read_linux_process_memory_bytes(pid: u32, page_size: u64) -> Option<u64> {
    // Prefer PSS (Proportional Set Size) so summing across multi-process app trees
    // splits shared libraries proportionally instead of double-counting them.
    // This runs on the background window-refresh thread so it never stalls the UI.
    if let Ok(rollup) = std::fs::read_to_string(format!("/proc/{}/smaps_rollup", pid)) {
        if let Some(pss) = parse_smaps_rollup_pss_bytes(&rollup) {
            return Some(pss);
        }
    }
    read_linux_rss_bytes(pid, page_size)
}

#[cfg(target_os = "linux")]
fn read_linux_rss_bytes(pid: u32, page_size: u64) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{}/statm", pid)).ok()?;
    let rss_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(rss_pages.saturating_mul(page_size))
}

#[cfg(target_os = "linux")]
fn read_linux_proc_stat(pid: u32) -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let (_, total_ticks, start_ticks) = parse_linux_proc_stat(&stat)?;
    Some((total_ticks, start_ticks))
}

#[cfg(target_os = "windows")]
fn build_windows_process_children() -> HashMap<u32, Vec<u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut children_by_ppid: HashMap<u32, Vec<u32>> = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return children_by_ppid;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let pid = entry.th32ProcessID;
                let ppid = entry.th32ParentProcessID;
                if pid > 1 && ppid > 1 && ppid != pid {
                    children_by_ppid.entry(ppid).or_default().push(pid);
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    children_by_ppid
}

#[cfg(target_os = "windows")]
fn collect_stats_for_pids(stats_by_pid: &mut HashMap<u32, ProcessStats>) {
    use std::time::SystemTime;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn filetime_to_100ns(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    let children_by_ppid = build_windows_process_children();

    let now = Instant::now();
    let now_filetime_100ns = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() + 11_644_473_600) * 10_000_000 + (d.subsec_nanos() as u64 / 100))
        .unwrap_or(0);
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0)
        .max(1.0);

    let mut cache_guard = cpu_sample_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache_guard.retain(|_, (_, ts)| now.duration_since(*ts) <= Duration::from_secs(30));

    let mut per_pid_stats: HashMap<u32, (Option<u64>, Option<f32>)> = HashMap::new();

    for (&root_pid, stats) in stats_by_pid.iter_mut() {
        unsafe {
            let root_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, root_pid);
            if !root_handle.is_null() {
                let mut exe_buf = [0u16; 1024];
                let mut exe_len = exe_buf.len() as u32;
                if QueryFullProcessImageNameW(root_handle, 0, exe_buf.as_mut_ptr(), &mut exe_len) != 0
                    && exe_len > 0
                {
                    stats.executable_path =
                        Some(String::from_utf16_lossy(&exe_buf[..exe_len as usize]));
                }
                CloseHandle(root_handle);
            }
        }

        let tree_pids = collect_descendant_pids(root_pid, &children_by_ppid);
        let mut total_mem: u64 = 0;
        let mut has_mem = false;
        let mut total_cpu: f32 = 0.0;
        let mut has_cpu = false;

        for pid in tree_pids {
            let (mem, cpu) = *per_pid_stats.entry(pid).or_insert_with(|| unsafe {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if handle.is_null() {
                    return (None, None);
                }

                // Prefer PrivateUsage so shared DLLs aren't double-counted across multi-process trees.
                let mut pmc_ex: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
                pmc_ex.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
                let mem = if K32GetProcessMemoryInfo(
                    handle,
                    &mut pmc_ex as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                    pmc_ex.cb,
                ) != 0
                {
                    let private = pmc_ex.PrivateUsage as u64;
                    let ws = pmc_ex.WorkingSetSize as u64;
                    Some(if private > 0 { private } else { ws })
                } else {
                    None
                };

                let mut creation = std::mem::zeroed();
                let mut exit = std::mem::zeroed();
                let mut kernel = std::mem::zeroed();
                let mut user = std::mem::zeroed();
                let cpu = if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0 {
                    let total_100ns = filetime_to_100ns(kernel).saturating_add(filetime_to_100ns(user));
                    let cpu = if let Some(&(prev_100ns, prev_at)) = cache_guard.get(&pid) {
                        let elapsed = now.duration_since(prev_at);
                        if elapsed >= Duration::from_millis(80) && elapsed <= Duration::from_secs(15) {
                            let delta_secs = total_100ns.saturating_sub(prev_100ns) as f64 / 10_000_000.0;
                            let wall_secs = elapsed.as_secs_f64();
                            ((delta_secs / wall_secs) * 100.0 / num_cpus).clamp(0.0, 100.0) as f32
                        } else {
                            let age_100ns = now_filetime_100ns
                                .saturating_sub(filetime_to_100ns(creation))
                                .max(1_000_000);
                            ((total_100ns as f64 / age_100ns as f64) * 100.0 / num_cpus)
                                .clamp(0.0, 100.0) as f32
                        }
                    } else {
                        let age_100ns = now_filetime_100ns
                            .saturating_sub(filetime_to_100ns(creation))
                            .max(1_000_000);
                        ((total_100ns as f64 / age_100ns as f64) * 100.0 / num_cpus)
                            .clamp(0.0, 100.0) as f32
                    };
                    cache_guard.insert(pid, (total_100ns, now));
                    Some(cpu)
                } else {
                    None
                };

                CloseHandle(handle);
                (mem, cpu)
            });

            if let Some(bytes) = mem {
                total_mem = total_mem.saturating_add(bytes);
                has_mem = true;
            }
            if let Some(pct) = cpu {
                total_cpu += pct;
                has_cpu = true;
            }
        }

        stats.memory_bytes = has_mem.then_some(total_mem);
        stats.cpu_percent = has_cpu.then_some(total_cpu.clamp(0.0, 100.0));
    }
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
fn collect_stats_for_pids(_stats_by_pid: &mut HashMap<u32, ProcessStats>) {}

/// Checks whether a Unix process ID is currently alive.
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if unsafe { libc::kill(pid as libc::pid_t, 0) == 0 } {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Sends `SIGKILL` on Unix (or delegates to `kill_process` on Windows).
pub fn force_kill_process(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
    }
    #[cfg(not(unix))]
    {
        kill_process(pid)
    }
}

/// Sends `SIGTERM` first, waits briefly off the UI thread, and escalates to `SIGKILL`
/// if a Unix process ignores `SIGTERM` (e.g. Electron/tray apps).
pub fn kill_process_graceful_then_force(pid: u32) -> bool {
    if !kill_process(pid) {
        return false;
    }
    #[cfg(unix)]
    {
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(30));
            if !is_process_alive(pid) {
                return true;
            }
        }
        let _ = force_kill_process(pid);
    }
    true
}

/// Terminates the process owning a window (`SIGTERM` on Unix, `TerminateProcess` on Windows).
pub fn kill_process(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }

    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
    }

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };

        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let ok = TerminateProcess(handle, 1) != 0;
            CloseHandle(handle);
            ok
        }
    }

    #[cfg(all(not(unix), not(target_os = "windows")))]
    {
        false
    }
}

#[cfg(unix)]
fn hyprland_ipc_query(cmd: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let sock_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket.sock");
    let mut stream = UnixStream::connect(&sock_path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(60))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(60))).ok()?;
    stream.write_all(cmd.as_bytes()).ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok()?;
    Some(resp)
}

#[cfg(unix)]
fn sway_ipc_request(socket: &str, request_type: u32, payload_bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(60))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(60))).ok()?;
    let mut request = Vec::with_capacity(14 + payload_bytes.len());
    request.extend_from_slice(b"i3-ipc");
    request.extend_from_slice(&(payload_bytes.len() as u32).to_le_bytes());
    request.extend_from_slice(&request_type.to_le_bytes());
    request.extend_from_slice(payload_bytes);
    stream.write_all(&request).ok()?;
    let mut header = [0u8; 14];
    stream.read_exact(&mut header).ok()?;
    let len = u32::from_le_bytes(header[6..10].try_into().ok()?) as usize;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).ok()?;
    Some(payload)
}

#[cfg(unix)]
fn sway_ipc_get_tree(socket: &str) -> Option<serde_json::Value> {
    let payload = sway_ipc_request(socket, 4, &[])?;
    serde_json::from_slice(&payload).ok()
}

#[cfg(target_os = "linux")]
fn is_gnome_wayland() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && (desktop.contains("gnome") || desktop.contains("ubuntu"))
}

#[cfg(target_os = "linux")]
fn is_kde_wayland() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    std::env::var_os("WAYLAND_DISPLAY").is_some() && desktop.contains("kde")
}

#[cfg(any(target_os = "linux", test))]
fn parse_gnome_window_calls_output(raw: &str, self_pid: u32) -> Vec<WindowItem> {
    let trimmed = raw.trim();
    // `gdbus` prints GVariant tuple: `('[...]',)`
    let json_slice = trimmed
        .strip_prefix("('")
        .and_then(|s| s.strip_suffix("',)"))
        .map(|s| s.replace("\\'", "'"))
        .unwrap_or_else(|| trimmed.to_string());
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_slice) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let pid = item["pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .filter(|&p| p > 0);
        if pid == Some(self_pid) {
            continue;
        }
        let Some(id) = item["id"].as_u64() else {
            continue;
        };
        let title = item["title"].as_str().unwrap_or("").trim().to_string();
        let class_name = item["wm_class"].as_str().unwrap_or("").trim().to_string();
        if !title.is_empty() && !class_name.eq_ignore_ascii_case("tiarun") {
            out.push(WindowItem {
                id: format!("gnome:{}", id),
                title,
                class_name,
                pid,
                memory_bytes: None,
                cpu_percent: None,
                executable_path: None,
            });
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn enumerate_gnome_window_calls(self_pid: u32) -> Vec<WindowItem> {
    let Ok(out) = Command::new("gdbus")
        .args(&[
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell/Extensions/Windows",
            "--method",
            "org.gnome.Shell.Extensions.Windows.List",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_gnome_window_calls_output(&text, self_pid)
}

#[cfg(target_os = "linux")]
fn enumerate_kde_kdotool(self_pid: u32) -> Vec<WindowItem> {
    let Ok(out) = Command::new("kdotool").args(&["search", "--name", "."]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut windows = Vec::new();
    for id in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
        let pid = Command::new("kdotool")
            .args(&["getwindowpid", id])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
            .filter(|&p| p > 0);
        if pid == Some(self_pid) {
            continue;
        }
        let title = Command::new("kdotool")
            .args(&["getwindowname", id])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let class_name = Command::new("kdotool")
            .args(&["getwindowclassname", id])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if class_name.eq_ignore_ascii_case("tiarun") {
            continue;
        }
        windows.push(WindowItem {
            id: format!("kdo:{}", id),
            title,
            class_name,
            pid,
            memory_bytes: None,
            cpu_percent: None,
            executable_path: None,
        });
    }
    windows
}

#[cfg(any(all(target_os = "linux", feature = "x11"), test))]
fn parse_wm_class_bytes(bytes: &[u8]) -> String {
    let parts: Vec<&str> = bytes
        .split(|&b| b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .filter(|s| !s.is_empty())
        .collect();
    // Standard WM_CLASS is "<instance>\0<class>\0"; prefer the class name when present.
    parts.last().copied().unwrap_or("").to_string()
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn enumerate_x11_windows(self_pid: u32) -> Vec<WindowItem> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return Vec::new();
    };
    let Some(screen) = conn.setup().roots.get(screen_num) else {
        return Vec::new();
    };
    let root = screen.root;

    let intern = |name: &[u8]| -> Option<u32> {
        conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
    };
    let Some(atom_client_list) = intern(b"_NET_CLIENT_LIST") else { return Vec::new(); };
    let atom_net_wm_name = intern(b"_NET_WM_NAME");
    let atom_utf8 = intern(b"UTF8_STRING");
    let atom_net_wm_pid = intern(b"_NET_WM_PID");
    let atom_wm_type = intern(b"_NET_WM_WINDOW_TYPE");
    let atom_type_dock = intern(b"_NET_WM_WINDOW_TYPE_DOCK");
    let atom_type_desktop = intern(b"_NET_WM_WINDOW_TYPE_DESKTOP");

    let Ok(list_cookie) = conn.get_property(false, root, atom_client_list, AtomEnum::WINDOW, 0, 1024) else {
        return Vec::new();
    };
    let Ok(list_reply) = list_cookie.reply() else {
        return Vec::new();
    };
    let Some(win_ids) = list_reply.value32() else {
        return Vec::new();
    };

    let mut windows = Vec::new();
    for win_id in win_ids {
        if let Some(atom_wm_type) = atom_wm_type {
            if let Ok(r) = conn
                .get_property(false, win_id, atom_wm_type, AtomEnum::ATOM, 0, 16)
                .and_then(|c| Ok(c.reply()))
            {
                if let Ok(reply) = r {
                    if let Some(mut types) = reply.value32() {
                        if types.any(|t| Some(t) == atom_type_dock || Some(t) == atom_type_desktop) {
                            continue;
                        }
                    }
                }
            }
        }

        let pid = atom_net_wm_pid
            .and_then(|atom| {
                conn.get_property(false, win_id, atom, AtomEnum::CARDINAL, 0, 1)
                    .ok()?
                    .reply()
                    .ok()?
                    .value32()?
                    .next()
            })
            .filter(|&p| p > 0);
        if pid == Some(self_pid) {
            continue;
        }

        let class_name = conn
            .get_property(false, win_id, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| parse_wm_class_bytes(&r.value))
            .unwrap_or_default();
        if class_name.contains("gnome-shell") || class_name.eq_ignore_ascii_case("tiarun") {
            continue;
        }

        let title = atom_net_wm_name
            .zip(atom_utf8)
            .and_then(|(atom_name, utf8)| {
                let r = conn.get_property(false, win_id, atom_name, utf8, 0, 512).ok()?.reply().ok()?;
                String::from_utf8(r.value).ok()
            })
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                let r = conn
                    .get_property(false, win_id, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 512)
                    .ok()?
                    .reply()
                    .ok()?;
                Some(String::from_utf8_lossy(&r.value).into_owned())
            })
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if title.is_empty() || title.eq_ignore_ascii_case("Desktop") {
            continue;
        }

        windows.push(WindowItem {
            id: format!("x11:{}", win_id),
            title,
            class_name,
            pid,
            memory_bytes: None,
            cpu_percent: None,
            executable_path: None,
        });
    }

    windows
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn focus_x11_window(win_id: u32) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};

    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return false;
    };
    let Some(screen) = conn.setup().roots.get(screen_num) else {
        return false;
    };
    let Ok(atom_reply) = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").and_then(|c| Ok(c.reply())) else {
        return false;
    };
    let Ok(atom) = atom_reply else {
        return false;
    };
    let event = ClientMessageEvent::new(
        32,
        win_id,
        atom.atom,
        [2, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    let ok = conn
        .send_event(
            false,
            screen.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .is_ok();
    let _ = conn.flush();
    ok
}

#[allow(dead_code)]
fn collect_sway_windows(node: &serde_json::Value, list: &mut Vec<WindowItem>, self_pid: u32) {
    if let Some(nodes) = node["nodes"].as_array() {
        for n in nodes {
            collect_sway_windows(n, list, self_pid);
        }
    }
    if let Some(fnodes) = node["floating_nodes"].as_array() {
        for n in fnodes {
            collect_sway_windows(n, list, self_pid);
        }
    }
    if let Some(name) = node["name"].as_str() {
        if let Some(id) = node["id"].as_i64() {
            let app_id = node["app_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| node["window_properties"]["class"].as_str())
                .unwrap_or("");
            let pid = node["pid"]
                .as_u64()
                .and_then(|p| u32::try_from(p).ok())
                .filter(|&p| p > 0);
            if pid == Some(self_pid) {
                return;
            }
            if !name.is_empty()
                && (node["type"].as_str() == Some("con")
                    || node["type"].as_str() == Some("floating_con"))
            {
                list.push(WindowItem {
                    id: format!("sway:{}", id),
                    title: name.to_string(),
                    class_name: app_id.to_string(),
                    pid,
                    memory_bytes: None,
                    cpu_percent: None,
                    executable_path: None,
                });
            }
        }
    }
}

pub fn focus_window(id: &str) {
    if let Some(addr) = id.strip_prefix("hypr:") {
        #[cfg(unix)]
        if hyprland_ipc_query(&format!("dispatch focuswindow address:{}", addr)).is_some() {
            return;
        }
        let _ = Command::new("hyprctl")
            .args(&["dispatch", "focuswindow", &format!("address:{}", addr)])
            .spawn();
        return;
    }

    if let Some(con_id) = id.strip_prefix("sway:") {
        #[cfg(unix)]
        if let Ok(sway_sock) = std::env::var("SWAYSOCK") {
            let cmd = format!("[con_id={}] focus", con_id);
            if sway_ipc_request(&sway_sock, 0, cmd.as_bytes()).is_some() {
                return;
            }
        }
        let _ = Command::new("swaymsg")
            .args(&[&format!("[con_id={}] focus", con_id)])
            .spawn();
        return;
    }

    if let Some(gnome_id) = id.strip_prefix("gnome:") {
        let _ = Command::new("gdbus")
            .args(&[
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell/Extensions/Windows",
                "--method",
                "org.gnome.Shell.Extensions.Windows.Activate",
                gnome_id,
            ])
            .spawn();
        return;
    }

    if let Some(kdo_id) = id.strip_prefix("kdo:") {
        let _ = Command::new("kdotool").args(&["windowactivate", kdo_id]).spawn();
        return;
    }

    #[cfg(all(target_os = "linux", feature = "x11"))]
    if let Some(x11_id) = id.strip_prefix("x11:") {
        if let Ok(win_id) = x11_id.parse::<u32>() {
            let _ = focus_x11_window(win_id);
        }
        return;
    }

    if let Some(win_id) = id.strip_prefix("wmctrl:") {
        if let Ok(_) = Command::new("wmctrl").args(&["-i", "-a", win_id]).spawn() {
            return;
        }
        let _ = Command::new("xdotool").args(&["windowactivate", win_id]).spawn();
        return;
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(hwnd_str) = id.strip_prefix("winhwnd:") {
            if let Ok(hwnd_val) = hwnd_str.parse::<isize>() {
                use windows_sys::Win32::Foundation::HWND;
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    BringWindowToTop, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
                };
                let hwnd = hwnd_val as HWND;
                unsafe {
                    if IsIconic(hwnd) != 0 {
                        ShowWindow(hwnd, SW_RESTORE);
                    }
                    BringWindowToTop(hwnd);
                    SetForegroundWindow(hwnd);
                }
                return;
            }
        }

        if let Some(pid) = id.strip_prefix("winpid:") {
            let script = format!("$w = (New-Object -ComObject WScript.Shell); $w.AppActivate({})", pid);
            let _ = Command::new("powershell")
                .args(&["-NoProfile", "-NonInteractive", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_memory_and_cpu() {
        assert_eq!(format_memory(512), "512 B");
        assert_eq!(format_memory(1536), "2 KB");
        assert_eq!(format_memory(256 * 1024 * 1024), "256 MB");
        assert_eq!(format_memory(1_610_612_736), "1.5 GB");

        assert_eq!(format_cpu(0.4), "0.4%");
        assert_eq!(format_cpu(9.94), "9.9%");
        assert_eq!(format_cpu(12.6), "13%");
    }

    #[test]
    fn test_collect_descendant_pids_handles_trees_and_cycles() {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        children.insert(100, vec![101, 102]);
        children.insert(101, vec![103, 104]);
        // Cycle back to root and invalid PID 1 should both be safely ignored.
        children.insert(104, vec![100, 1, 105]);

        let tree = collect_descendant_pids(100, &children);
        assert_eq!(tree, vec![100, 101, 102, 103, 104, 105]);
        assert!(collect_descendant_pids(1, &children).is_empty());
        assert!(collect_descendant_pids(0, &children).is_empty());
    }

    #[test]
    fn test_parse_linux_proc_stat_and_smaps_rollup() {
        // Real-world comm with spaces and nested parentheses like "(Web Content)"
        let stat = "4242 (Isolated Web (Co)) S 1000 4242 4242 0 -1 4194304 100 0 0 0 150 50 0 0 20 0 8 0 98765 0 0";
        assert_eq!(parse_linux_proc_stat(stat), Some((1000, 200, 98765)));

        let rollup = "\
5e6907145000-7fff3732f000 ---p 00000000 00:00 0                          [rollup]
Rss:                3928 kB
Pss:                1024 kB
Private_Dirty:       288 kB
";
        assert_eq!(parse_smaps_rollup_pss_bytes(rollup), Some(1024 * 1024));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_process_stats_and_kill() {
        let stats = get_process_stats(std::process::id());
        assert!(stats.memory_bytes.is_some_and(|b| b > 0));
        assert!(stats.cpu_percent.is_some_and(|c| (0.0..=100.0).contains(&c)));
        assert!(stats.executable_path.is_some());

        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert!(kill_process(pid));
        let status = child.wait().unwrap();
        assert!(!status.success());

        // Process ignoring SIGTERM must be terminated via SIGKILL escalation.
        let mut stubborn = Command::new("sh")
            .args(&["-c", "trap '' TERM; sleep 30"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let stubborn_pid = stubborn.id();
        assert!(kill_process_graceful_then_force(stubborn_pid));
        let stubborn_status = stubborn.wait().unwrap();
        assert!(!stubborn_status.success());
    }

    #[test]
    fn test_parse_gnome_window_calls_and_x11_wm_class() {
        let raw = r#"('[{"id":123,"title":"Firefox","wm_class":"firefox","pid":4242},{"id":124,"title":"TiaRun","wm_class":"tiarun","pid":9999}]',)"#;
        let wins = parse_gnome_window_calls_output(raw, 9999);
        assert_eq!(wins.len(), 1);
        assert_eq!(wins[0].id, "gnome:123");
        assert_eq!(wins[0].title, "Firefox");
        assert_eq!(wins[0].class_name, "firefox");
        assert_eq!(wins[0].pid, Some(4242));

        assert_eq!(parse_wm_class_bytes(b"Navigator\0firefox\0"), "firefox");
        assert_eq!(parse_wm_class_bytes(b"alacritty\0"), "alacritty");
        assert_eq!(parse_wm_class_bytes(b""), "");
    }
}
