#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(unused_imports)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::rc::Rc;
use std::cell::Cell;

use slint::ComponentHandle;
use tiarun::config::Config;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher::{self, LauncherEngine, LauncherItem, open_config_file};
use tiarun::{AppWindow, LauncherItemData};
use i_slint_backend_winit::WinitWindowAccessor;

use std::io::{Read, Write};

use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
use std::net::{TcpListener, TcpStream, SocketAddr};

#[cfg(unix)]
fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("tiarun.sock")
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        PathBuf::from(format!("/tmp/tiarun-{}.sock", user))
    }
}

#[cfg(unix)]
fn try_send_to_existing_instance(msg: &[u8]) -> bool {
    let socket_path = get_socket_path();
    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
        let _ = stream.write_all(msg);
        return true;
    }
    false
}

#[cfg(windows)]
fn try_send_to_existing_instance(msg: &[u8]) -> bool {
    let addr: SocketAddr = "127.0.0.1:42425".parse().unwrap();
    for _ in 0..2 {
        if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            if stream.write_all(msg).is_ok() {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(target_os = "windows")]
fn is_launcher_focused(ui: &AppWindow) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Some(focused) = ui.window().with_winit_window(|w| {
        if let Ok(handle) = w.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let fg = unsafe { GetForegroundWindow() };
                return fg == (h.hwnd.get() as _);
            }
        }
        w.has_focus()
    }) {
        return focused;
    }
    false
}

#[cfg(target_os = "windows")]
fn apply_windows_rounded_corners(ui: &AppWindow) {
    ui.window().with_winit_window(|winit_window| {
        use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::Controls::MARGINS;
        use windows_sys::Win32::Graphics::Dwm::{
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };

        if let Ok(handle) = winit_window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = win32_handle.hwnd.get() as HWND;

                unsafe {
                    // 1. DWM Glass/Alpha Composition across full window area
                    let margins = MARGINS {
                        cxLeftWidth: -1,
                        cxRightWidth: -1,
                        cyTopHeight: -1,
                        cyBottomHeight: -1,
                    };
                    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

                    // 2. Windows 11 Native Rounded Corners (anti-aliased at DWM compositor level)
                    let preference = DWMWCP_ROUND as u32;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                        &preference as *const _ as *const _,
                        std::mem::size_of_val(&preference) as u32,
                    );
                }
            }
        }
    });
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_sway_launcher_focused(sway_sock: &str) -> bool {
    sway_ipc_query(sway_sock, 4).is_some_and(|tree| check_node_focused(&tree))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn sway_ipc_query(socket: &str, request_type: u32) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(40))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(40))).ok()?;
    let mut request = *b"i3-ipc\0\0\0\0\0\0\0\0";
    request[10..].copy_from_slice(&request_type.to_le_bytes());
    stream.write_all(&request).ok()?;
    let mut header = [0u8; 14];
    stream.read_exact(&mut header).ok()?;
    let len = u32::from_le_bytes(header[6..10].try_into().ok()?) as usize;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).ok()?;
    serde_json::from_slice(&payload).ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn check_node_focused(val: &serde_json::Value) -> bool {
    if val.get("focused").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(app_id) = val.get("app_id").and_then(|v| v.as_str()) {
            return app_id == "tiarun" || app_id == "view-launcher";
        }
        return false;
    }
    if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
        for n in nodes {
            if check_node_focused(n) {
                return true;
            }
        }
    }
    if let Some(floating) = val.get("floating_nodes").and_then(|v| v.as_array()) {
        for n in floating {
            if check_node_focused(n) {
                return true;
            }
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "macos")))]
fn sway_launcher_node(node: &serde_json::Value) -> Option<&serde_json::Value> {
    if node["app_id"].as_str() == Some("tiarun") || node["app_id"].as_str() == Some("view-launcher") {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|key| node[*key].as_array())
        .flatten()
        .find_map(sway_launcher_node)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_launcher_focused(ui: &AppWindow) -> bool {
    // 1. If running under Hyprland, query Hyprland IPC in <0.05ms
    if let (Ok(runtime_dir), Ok(sig)) = (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE")) {
        let sock_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket.sock");
        if let Ok(mut stream) = UnixStream::connect(&sock_path) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
            if stream.write_all(b"j/activewindow").is_ok() {
                let mut buf = [0u8; 1024];
                if let Ok(n) = stream.read(&mut buf) {
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        return s.contains("\"class\": \"tiarun\"") || s.contains("\"class\": \"view-launcher\"");
                    }
                }
            }
        }
    }

    // 2. If running under Sway, query Sway IPC tree directly (<0.6ms)
    if let Ok(sway_sock) = std::env::var("SWAYSOCK") {
        return is_sway_launcher_focused(&sway_sock);
    }

    // 3. Fallback to winit focus status
    ui.window().with_winit_window(|w| w.has_focus()).unwrap_or(false)
}

#[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos")))))]
fn is_launcher_focused(ui: &AppWindow) -> bool {
    ui.window().with_winit_window(|w| w.has_focus()).unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_sway_ipc_command(cmd: &str) {
    if let Ok(sway_sock) = std::env::var("SWAYSOCK") {
        if let Ok(mut stream) = UnixStream::connect(&sway_sock) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(30)));
            let _ = stream.set_read_timeout(Some(Duration::from_millis(30)));
            let mut msg = Vec::with_capacity(14 + cmd.len());
            msg.extend_from_slice(b"i3-ipc");
            msg.extend_from_slice(&(cmd.len() as u32).to_le_bytes());
            msg.extend_from_slice(&0u32.to_le_bytes()); // TYPE 0 = RUN_COMMAND
            msg.extend_from_slice(cmd.as_bytes());
            if stream.write_all(&msg).is_ok() {
                // Keep the connection open until Sway processes the command.
                let mut header = [0; 14];
                let _ = stream.read_exact(&mut header);
            }
        }
    }
}

fn apply_skip_taskbar(winit_window: &i_slint_backend_winit::winit::window::Window) {
    #[cfg(target_os = "windows")]
    {
        use i_slint_backend_winit::winit::platform::windows::WindowExtWindows;
        winit_window.set_skip_taskbar(true);
    }

    #[cfg(all(target_os = "linux", feature = "x11"))]
    {
        use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = winit_window.window_handle() {
            let xid = match handle.as_raw() {
                RawWindowHandle::Xlib(h) => Some(h.window as u32),
                RawWindowHandle::Xcb(h) => Some(h.window.get()),
                _ => None,
            };
            if let Some(xid) = xid {
                if let Ok((conn, _)) = x11rb::connect(None) {
                    use x11rb::connection::Connection;
                    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
                    use x11rb::wrapper::ConnectionExt as _;

                    // 1. _NET_WM_WINDOW_TYPE -> _NET_WM_WINDOW_TYPE_UTILITY
                    if let Ok(reply) = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE") {
                        if let Ok(atom_type) = reply.reply() {
                            if let Ok(dock_reply) = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE_UTILITY") {
                                if let Ok(atom_dock) = dock_reply.reply() {
                                    let _ = conn.change_property32(
                                        PropMode::REPLACE,
                                        xid,
                                        atom_type.atom,
                                        AtomEnum::ATOM,
                                        &[atom_dock.atom],
                                    );
                                }
                            }
                        }
                    }

                    // 2. _NET_WM_STATE -> SKIP_TASKBAR, SKIP_PAGER, ABOVE
                    if let Ok(state_reply) = conn.intern_atom(false, b"_NET_WM_STATE") {
                        if let Ok(atom_state) = state_reply.reply() {
                            let above = conn.intern_atom(false, b"_NET_WM_STATE_ABOVE").ok().and_then(|r| r.reply().ok()).map(|r| r.atom);
                            let skip_tb = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR").ok().and_then(|r| r.reply().ok()).map(|r| r.atom);
                            let skip_pg = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER").ok().and_then(|r| r.reply().ok()).map(|r| r.atom);
                            let mut states = Vec::new();
                            if let Some(a) = above { states.push(a); }
                            if let Some(s) = skip_tb { states.push(s); }
                            if let Some(p) = skip_pg { states.push(p); }
                            if !states.is_empty() {
                                let _ = conn.change_property32(
                                    PropMode::REPLACE,
                                    xid,
                                    atom_state.atom,
                                    AtomEnum::ATOM,
                                    &states,
                                );
                            }
                        }
                    }
                    let _ = conn.flush();
                }
            }
        }
    }
}

fn focus_launcher_window(ui: &AppWindow) {
    ui.window().with_winit_window(|w| {
        apply_skip_taskbar(w);
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
            use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            w.set_visible(true);
            if let Ok(handle) = w.window_handle() {
                if let RawWindowHandle::Win32(h) = handle.as_raw() {
                    unsafe {
                        BringWindowToTop(h.hwnd.get() as _);
                        SetForegroundWindow(h.hwnd.get() as _);
                    }
                }
            }
        }
        w.focus_window();
    });

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let (Ok(runtime_dir), Ok(sig)) = (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE")) {
            let sock_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket.sock");
            if let Ok(mut stream) = UnixStream::connect(&sock_path) {
                let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
                let _ = stream.write_all(b"dispatch focuswindow title:^TiaRun$");
            }
        }
        send_sway_ipc_command("[app_id=\"tiarun\"] focus");
    }
}

static LAST_WINDOW_SIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn resize_window_for_state(ui: &AppWindow) {
    let width = ui.get_launcher_width() as i32;
    let height = ui.get_launcher_height() as i32;

    let packed = ((width as u64) << 32) | (height as u32 as u64);
    if LAST_WINDOW_SIZE.swap(packed, std::sync::atomic::Ordering::SeqCst) == packed {
        return;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    if ui.window().is_visible() {
        if let Some(()) = resize_sway_launcher(width, height) {
            return;
        }
    }

    ui.window().set_size(slint::LogicalSize::new(width as f32, height as f32));
    ui.window().with_winit_window(move |winit_window| {
        let _ = winit_window.request_inner_size(i_slint_backend_winit::winit::dpi::LogicalSize::new(width as f32, height as f32));
    });

    #[cfg(target_os = "windows")]
    {
        apply_windows_rounded_corners(ui);
        reposition_and_anchor_window(ui);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            let _ = resize_hyprland_launcher(width, height);
        } else if std::env::var_os("SWAYSOCK").is_none() {
            reposition_and_anchor_window(ui);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn query_hyprland_ipc(cmd: &str) -> Option<String> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let sock_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket.sock");
    let mut stream = UnixStream::connect(&sock_path).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(40))).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(40))).ok()?;
    stream.write_all(cmd.as_bytes()).ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok()?;
    Some(resp)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_hyprland_ipc_command(cmd: &str) {
    let _ = query_hyprland_ipc(cmd);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn get_hyprland_focused_monitor() -> Option<(i64, i64, i64, i64)> {
    let json_str = query_hyprland_ipc("j/monitors")?;
    let val: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    let monitors = val.as_array()?;
    let mon = monitors.iter().find(|m| m["focused"].as_bool() == Some(true))
        .or_else(|| monitors.first())?;

    let mon_x = mon["x"].as_i64()?;
    let mon_y = mon["y"].as_i64()?;
    let scale = mon["scale"].as_f64().unwrap_or(1.0);
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    let raw_w = mon["width"].as_f64()?;
    let raw_h = mon["height"].as_f64()?;
    let mon_w = (raw_w / scale).round() as i64;
    let mon_h = (raw_h / scale).round() as i64;
    Some((mon_x, mon_y, mon_w, mon_h))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn place_hyprland_launcher(ui: &AppWindow) -> Option<()> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return None;
    }
    let (mon_x, mon_y, mon_w, mon_h) = get_hyprland_focused_monitor()?;
    let width = ui.get_launcher_width() as i64;
    let x = mon_x + (mon_w - width).max(0) / 2;
    let y = mon_y + mon_h * 15 / 100;

    send_hyprland_ipc_command(&format!(
        "dispatch movewindowpixel exact {} {},title:^(TiaRun|View Launcher)$",
        x, y
    ));
    Some(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn resize_hyprland_launcher(width: i32, _height: i32) -> Option<()> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return None;
    }
    let (mon_x, mon_y, mon_w, mon_h) = get_hyprland_focused_monitor()?;
    let x = mon_x + (mon_w - (width as i64)).max(0) / 2;
    let y = mon_y + mon_h * 15 / 100;

    send_hyprland_ipc_command(&format!(
        "dispatch movewindowpixel exact {} {},title:^(TiaRun|View Launcher)$",
        x, y
    ));
    slint::Timer::single_shot(Duration::from_millis(15), move || {
        send_hyprland_ipc_command(&format!(
            "dispatch movewindowpixel exact {} {},title:^(TiaRun|View Launcher)$",
            x, y
        ));
    });
    Some(())
}

fn reposition_and_anchor_window(ui: &AppWindow) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var_os("SWAYSOCK").is_some() {
            let _ = launcher_is_placed_on_sway(ui);
            return;
        }
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            let _ = place_hyprland_launcher(ui);
            return;
        }
    }

    ui.window().with_winit_window(|winit_window| {
        let monitor_opt = winit_window
            .current_monitor()
            .or_else(|| winit_window.primary_monitor())
            .or_else(|| winit_window.available_monitors().next());

        if let Some(monitor) = monitor_opt {
            let screen_size = monitor.size();
            let scale_factor = monitor.scale_factor();
            let monitor_pos = monitor.position();
            let window_width = (f64::from(ui.get_launcher_width()) * scale_factor) as i32;
            let window_height = (f64::from(ui.get_launcher_height()) * scale_factor) as i32;
            #[allow(unused_variables)]
            let _ = window_height;

            let pos_x = monitor_pos.x + (screen_size.width as i32 - window_width).max(0) / 2;
            let pos_y = monitor_pos.y + (screen_size.height as f64 * 0.15) as i32;

            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
                use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                use windows_sys::Win32::Foundation::HWND;

                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                        let hwnd = win32_handle.hwnd.get() as HWND;
                        unsafe {
                            SetWindowPos(
                                hwnd,
                                std::ptr::null_mut(),
                                pos_x,
                                pos_y,
                                0,
                                0,
                                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE,
                            );
                        }
                    }
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                winit_window.set_outer_position(i_slint_backend_winit::winit::dpi::PhysicalPosition::new(pos_x, pos_y));
            }
        }
    });
}

#[cfg(all(unix, not(target_os = "macos")))]
fn resize_sway_launcher(width: i32, height: i32) -> Option<()> {
    let socket = std::env::var("SWAYSOCK").ok()?;
    let tree = sway_ipc_query(&socket, 4)?;
    let node = sway_launcher_node(&tree)?;
    let id = node["id"].as_i64()?;
    let rect = &node["rect"];
    let height_delta = i64::from(height) - rect["height"].as_i64()?;

    // Resize the bottom edge first so a compositor minimum height cannot
    // shift the top edge when applying the width.
    let mut command = format!("[con_id={id}] ");
    if height_delta != 0 {
        let direction = if height_delta > 0 { "grow" } else { "shrink" };
        command.push_str(&format!("resize {direction} down {} px, ", height_delta.abs()));
    }
    command.push_str(&format!("resize set width {width} px"));
    send_sway_ipc_command(&command);
    Some(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launcher_is_placed_on_sway(ui: &AppWindow) -> Option<bool> {
    if !ui.window().is_visible() {
        return None;
    }
    let socket = std::env::var("SWAYSOCK").ok()?;
    let tree = sway_ipc_query(&socket, 4)?;
    let node = sway_launcher_node(&tree)?;
    let id = node["id"].as_i64()?;
    let workspaces = sway_ipc_query(&socket, 1)?;
    let workspace = workspaces
        .as_array()?
        .iter()
        .find(|ws| ws["focused"].as_bool() == Some(true))
        .or_else(|| workspaces.as_array()?.first())?;
    let workspace_rect = &workspace["rect"];
    let width = workspace_rect["width"].as_i64()?;
    let height = workspace_rect["height"].as_i64()?;
    let ws_x = workspace_rect["x"].as_i64()?;
    let ws_y = workspace_rect["y"].as_i64()?;
    let launcher_w = ui.get_launcher_width() as i32;
    let launcher_h = ui.get_launcher_height() as i32;
    let target_x = ws_x + (width - i64::from(launcher_w)).max(0) / 2;
    let target_y = ws_y + height * 15 / 100;
    let rect = &node["rect"];
    let current_x = rect["x"].as_i64()?;
    let current_y = rect["y"].as_i64()?;
    let current_w = rect["width"].as_i64()?;
    let current_h = rect["height"].as_i64()?;

    let positioned = current_x == target_x
        && current_y == target_y
        && current_w == i64::from(launcher_w)
        && current_h == i64::from(launcher_h);
    if !positioned {
        send_sway_ipc_command(&format!(
            "[con_id={id}] floating enable, resize set {launcher_w} px {launcher_h} px, move absolute position {target_x} px {target_y} px, focus",
        ));
    }
    Some(positioned)
}

fn is_gnome_desktop() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let is_sway = std::env::var_os("SWAYSOCK").is_some() || desktop.contains("sway");
        let is_hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() || desktop.contains("hyprland");
        !is_sway && !is_hyprland && desktop.contains("gnome")
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    false
}

fn show_launcher_window(ui: &AppWindow) -> Result<(), slint::PlatformError> {
    let result = ui.show();
    tiarun::search_index::set_indexing_paused(ui.window().is_visible());
    result
}

fn hide_launcher_window(ui: &AppWindow) {
    let _ = ui.hide();
    tiarun::search_index::set_indexing_paused(ui.window().is_visible());
}

fn show_and_focus_launcher(
    ui: &AppWindow,
    ui_weak: slint::Weak<AppWindow>,
    shown_instant: &Arc<std::sync::RwLock<Instant>>,
    query: &str,
) {
    let was_visible = ui.window().is_visible();
    if !was_visible {
        LAST_WINDOW_SIZE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    let cfg = Config::load();
    ui.set_is_dark(cfg.theme.is_dark());
    ui.set_cfg_compact_empty(cfg.theme.compact_empty_view.unwrap_or(true));
    ui.set_cfg_view_style(cfg.theme.view_style.clone().into());
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }
    ui.set_search_text(query.into());
    ui.set_active_category("".into());
    ui.set_active_scope("apps".into());
    ui.set_is_expanded(!query.is_empty());
    ui.set_show_preview(false);
    ui.set_selected_index(0);
    ui.set_scroll_viewport_y(0.0);
    resize_window_for_state(ui);
    if !was_visible {
        reposition_and_anchor_window(ui);
    }
    ui.invoke_search_text_changed(query.into());
    if let Ok(mut lock) = shown_instant.write() {
        *lock = Instant::now();
    }
    if !was_visible {
        animate_window_pop_in(ui_weak.clone());
    }
    let _ = show_launcher_window(ui);
    focus_launcher_window(ui);
    ui.invoke_focus_search();
    if !was_visible {
        slint::Timer::single_shot(Duration::from_millis(30), move || {
            if let Some(ui) = ui_weak.upgrade().filter(|ui| ui.window().is_visible()) {
                reposition_and_anchor_window(&ui);
                ui.invoke_focus_search();
            }
        });
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn start_hyprland_focus_watcher(ui_handle: slint::Weak<AppWindow>, shown_instant: Arc<std::sync::RwLock<Instant>>) {
    if let (Ok(runtime_dir), Ok(sig)) = (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE")) {
        let sock2_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket2.sock");
        thread::spawn(move || {
            use std::io::BufRead;
            if let Ok(stream) = UnixStream::connect(&sock2_path) {
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break; };
                    if line.starts_with("activewindow>>") {
                        let active = line.trim_start_matches("activewindow>>");
                        if !active.starts_with("tiarun,") && !active.starts_with("view-launcher,") {
                            let ui_weak = ui_handle.clone();
                            let shown = shown_instant.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    if ui.window().is_visible() && !ui.get_in_settings_mode() {
                                        let elapsed = shown.read().map(|t| t.elapsed()).unwrap_or_default();
                                        if elapsed > Duration::from_millis(150) {
                                            hide_launcher_window(&ui);
                                            ui.set_search_text("".into());
                                            ui.set_is_expanded(false);
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
        });
    }
}

#[cfg(unix)]
fn start_daemon_listener(
    ui_handle: slint::Weak<AppWindow>,
    shown_instant: Arc<std::sync::RwLock<Instant>>,
    existing_listener: Option<UnixListener>,
) {
    let socket_path = get_socket_path();
    let listener_res = if let Some(l) = existing_listener {
        Ok(l)
    } else {
        let _ = std::fs::remove_file(&socket_path);
        UnixListener::bind(&socket_path)
    };

    if let Ok(listener) = listener_res {
        thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 512];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let cmd = String::from_utf8_lossy(&buf[..n]);

                    if cmd.starts_with("quit") {
                        let _ = slint::quit_event_loop();
                        break;
                    }

                    if cmd.starts_with("escape") || cmd.starts_with("close") {
                        let ui_weak = ui_handle.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.invoke_close_window();
                            }
                        });
                        continue;
                    }

                    if cmd.starts_with("ping") {
                        continue;
                    }

                    if cmd.starts_with("settings") {
                        let ui_weak = ui_handle.clone();
                        let shown = shown_instant.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                                let cfg = Config::load();
                                ui.set_cfg_autostart(tiarun::config::is_autostart_enabled());
                                ui.set_cfg_theme_mode(cfg.theme.mode.clone().into());
                                ui.set_cfg_theme_opacity(cfg.theme.opacity);
                                ui.set_is_dark(cfg.theme.is_dark());
                                ui.set_in_settings_mode(true);
                                ui.set_settings_status("".into());
                                resize_window_for_state(&ui);
                                ui.invoke_focus_settings();
                            }
                        });
                        continue;
                    }

                    if let Some(query) = cmd.strip_prefix("query:") {
                        let query_str = query.trim().to_string();
                        let ui_weak = ui_handle.clone();
                        let shown = shown_instant.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_in_settings_mode(false);
                                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, &query_str);
                            }
                        });
                        continue;
                    }

                    let ui_weak = ui_handle.clone();
                    let shown = shown_instant.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let is_visible = ui.window().is_visible();
                            let is_focused = is_launcher_focused(&ui);
                            if is_visible && is_focused {
                                hide_launcher_window(&ui);
                            } else {
                                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                            }
                        }
                    });
                }
            }
        });
    }
}

#[cfg(windows)]
fn start_windows_global_hotkey(
    ui_handle: slint::Weak<AppWindow>,
    shown_instant: Arc<std::sync::RwLock<Instant>>,
) {
    thread::spawn(move || {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            RegisterHotKey, MOD_CONTROL, MOD_ALT, MOD_NOREPEAT,
        };

        const HOTKEY_ID_CTRL_ALT_SPACE: i32 = 4242;
        let vk_space = 0x20u32; // VK_SPACE

        // Register default hotkey: Ctrl + Alt + Space
        unsafe {
            let res = RegisterHotKey(
                std::ptr::null_mut(),
                HOTKEY_ID_CTRL_ALT_SPACE,
                (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT) as u32,
                vk_space,
            );
            if res == 0 {
                RegisterHotKey(
                    std::ptr::null_mut(),
                    HOTKEY_ID_CTRL_ALT_SPACE,
                    (MOD_CONTROL | MOD_ALT) as u32,
                    vk_space,
                );
            }
        };

        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            if msg.message == WM_HOTKEY && msg.wParam == HOTKEY_ID_CTRL_ALT_SPACE as usize {
                let ui_weak = ui_handle.clone();
                let shown = shown_instant.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        let is_visible = ui.window().is_visible();
                        let is_focused = is_launcher_focused(&ui);
                        if is_visible && is_focused {
                            hide_launcher_window(&ui);
                        } else {
                            show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                        }
                    }
                });
            }
        }
    });
}

#[cfg(windows)]
fn start_daemon_listener(
    ui_handle: slint::Weak<AppWindow>,
    shown_instant: Arc<std::sync::RwLock<Instant>>,
) {
    // 1. Native Win32 global hotkey thread (Ctrl + Alt + Space) with 0ms spawn latency
    start_windows_global_hotkey(ui_handle.clone(), shown_instant.clone());

    // 2. TCP listener for IPC toggle commands from external processes
    let addr: SocketAddr = "127.0.0.1:42425".parse().unwrap();
    if let Ok(listener) = TcpListener::bind(addr) {
        thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 8];
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                    let _ = stream.read(&mut buf);
                    let cmd = String::from_utf8_lossy(&buf);

                    if cmd.starts_with("quit") {
                        let _ = slint::quit_event_loop();
                        break;
                    }

                    if cmd.starts_with("escape") || cmd.starts_with("close") {
                        let ui_weak = ui_handle.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.invoke_close_window();
                            }
                        });
                        continue;
                    }

                    if cmd.starts_with("ping") {
                        continue;
                    }

                    if cmd.starts_with("settings") {
                        let ui_weak = ui_handle.clone();
                        let shown = shown_instant.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                                let cfg = Config::load();
                                ui.set_cfg_autostart(tiarun::config::is_autostart_enabled());
                                ui.set_cfg_theme_mode(cfg.theme.mode.clone().into());
                                ui.set_cfg_theme_opacity(cfg.theme.opacity);
                                ui.set_is_dark(cfg.theme.is_dark());
                                ui.set_in_settings_mode(true);
                                ui.set_settings_status("".into());
                                resize_window_for_state(&ui);
                                ui.invoke_focus_settings();
                            }
                        });
                        continue;
                    }

                    let ui_weak = ui_handle.clone();
                    let shown = shown_instant.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let is_visible = ui.window().is_visible();
                            let is_focused = is_launcher_focused(&ui);
                            if is_visible && is_focused {
                                hide_launcher_window(&ui);
                            } else {
                                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                            }
                        }
                    });
                }
            }
        });
    }
}

fn update_action_hints(
    ui: &AppWindow,
    selected_item: Option<&launcher::LauncherItem>,
    total_count: usize,
    selected_idx: usize,
) {
    let hint = if let Some(item) = selected_item {
        match item.item_type {
            launcher::ItemType::App => "↵ Open  •  Ctrl+C Copy  •  Alt+T Terminal",
            launcher::ItemType::File => "↵ Open  •  Ctrl+C Copy  •  Alt+C Copy Path  •  Alt+T Terminal",
            launcher::ItemType::Dir => "↵ Open  •  → Browse  •  Ctrl+C Copy  •  Alt+C Copy Path",
            launcher::ItemType::Window => "↵ Focus Window  •  Esc Close",
            launcher::ItemType::Clipboard => "↵ Paste  •  Esc Close",
            launcher::ItemType::Calc => "↵ Copy to Clipboard",
            launcher::ItemType::Shell => "↵ Run in Terminal",
            launcher::ItemType::System => "↵ Execute Action",
            launcher::ItemType::Theme => "↵ Apply Theme",
            launcher::ItemType::WebSearch => "↵ Search Web  •  Esc Close",
            launcher::ItemType::Dmenu => "↵ Select  •  Esc Close",
        }
    } else {
        "↵ Open  •  Tab Scope  •  F1 Help"
    };
    ui.set_action_hint_text(hint.into());

    let count_str = if total_count > 0 {
        format!("{}/{}  •  Tab Scope", (selected_idx + 1).min(total_count), total_count)
    } else {
        "Tab Scope".to_string()
    };
    ui.set_action_item_count(count_str.into());
}

fn effective_search_query(ui: &AppWindow, engine: &LauncherEngine, query: &str) -> String {
    let trimmed = query.trim();

    // Two-way synchronization between search text prefix and active scope chip
    let mut active_scope = ui.get_active_scope().to_string();
    if active_scope.is_empty() {
        active_scope = "apps".to_string();
        ui.set_active_scope("apps".into());
    }
    if (trimmed.starts_with("@f") && !trimmed.starts_with("@fc")) || trimmed.starts_with("@file") {
        active_scope = "files".to_string();
        ui.set_active_scope("files".into());
    } else if trimmed.starts_with("@content")
        || trimmed.starts_with("@text")
        || trimmed.starts_with("@grep")
        || trimmed.starts_with("@fc")
        || trimmed.starts_with("@t")
    {
        active_scope = "text".to_string();
        ui.set_active_scope("text".into());
    } else if trimmed.starts_with("@w") || trimmed.starts_with("@win") {
        active_scope = "windows".to_string();
        ui.set_active_scope("windows".into());
    }

    // When the user has not typed an explicit '@', '!', ':' prefix, route search based on active scope
    if trimmed.is_empty() {
        match active_scope.as_str() {
            "files" => "@f".to_string(),
            "windows" => "@w".to_string(),
            "text" => "@t".to_string(),
            _ => "".to_string(),
        }
    } else if !trimmed.starts_with(['@', '!', ':'])
        && trimmed != "?"
        && engine.resolve_path_search(trimmed).is_none()
    {
        match active_scope.as_str() {
            "files" => format!("@f {}", trimmed),
            "text" => format!("@t {}", trimmed),
            "windows" => format!("@w {}", trimmed),
            _ => query.to_string(),
        }
    } else {
        query.to_string()
    }
}

fn populate_items(
    ui: &AppWindow,
    engine: &LauncherEngine,
    icon_resolver: &IconResolver,
    query: &str,
) -> Vec<LauncherItem> {
    let effective_query = effective_search_query(ui, engine, query);
    render_search_results(ui, engine, icon_resolver, query, &effective_query, None)
}

struct FileSearchResults {
    matches: Vec<(LauncherItem, Vec<usize>)>,
    file_sizes: std::collections::HashMap<String, String>,
}

fn file_size_label(path: &str) -> String {
    std::fs::metadata(path)
        .map(|metadata| launcher::format_file_size(metadata.len()))
        .unwrap_or_else(|_| "File".to_string())
}

fn render_search_results(
    ui: &AppWindow,
    engine: &LauncherEngine,
    icon_resolver: &IconResolver,
    query: &str,
    effective_query: &str,
    results: Option<FileSearchResults>,
) -> Vec<LauncherItem> {
    let trimmed = query.trim();
    let effective_trimmed = effective_query.trim();

    let is_content_mode = effective_trimmed.starts_with("@content")
        || effective_trimmed.starts_with("@text")
        || effective_trimmed.starts_with("@grep")
        || effective_trimmed.starts_with("@fc")
        || effective_trimmed.starts_with("@t");
    let is_file_mode = (effective_trimmed.starts_with("@f") && !effective_trimmed.starts_with("@fc")) || effective_trimmed.starts_with("@file");
    let is_win_mode = effective_trimmed.starts_with("@w") || effective_trimmed.starts_with("@win");
    let is_clip_mode = effective_trimmed.starts_with("@c") || effective_trimmed.starts_with("@clip");
    let is_sys_mode = effective_trimmed.starts_with("@sys") || effective_trimmed.starts_with("@power");
    let is_theme_mode = effective_trimmed.starts_with("@theme") || effective_trimmed.starts_with("@mode") || effective_trimmed.starts_with("@opacity");
    let is_shell_mode = effective_trimmed.starts_with('!');
    let is_help_mode = effective_trimmed == "?";

    // Content typed after the mode trigger itself (e.g. the path after "@f ",
    // the command after "!"). Once there's real content, hide the mode badge
    // so it stops eating into the width available to show that content -
    // long file paths / shell commands would otherwise get cut off under it.
    let mode_content = if is_content_mode {
        effective_trimmed.strip_prefix("@content")
            .or_else(|| effective_trimmed.strip_prefix("@text"))
            .or_else(|| effective_trimmed.strip_prefix("@grep"))
            .or_else(|| effective_trimmed.strip_prefix("@fc"))
            .or_else(|| effective_trimmed.strip_prefix("@t"))
    } else if is_file_mode {
        effective_trimmed.strip_prefix("@file").or_else(|| effective_trimmed.strip_prefix("@f"))
    } else if is_win_mode {
        effective_trimmed.strip_prefix("@win").or_else(|| effective_trimmed.strip_prefix("@w"))
    } else if is_clip_mode {
        effective_trimmed.strip_prefix("@clip").or_else(|| effective_trimmed.strip_prefix("@c"))
    } else if is_sys_mode {
        effective_trimmed.strip_prefix("@power").or_else(|| effective_trimmed.strip_prefix("@sys"))
    } else if is_theme_mode {
        effective_trimmed.strip_prefix("@theme").or_else(|| effective_trimmed.strip_prefix("@mode")).or_else(|| effective_trimmed.strip_prefix("@opacity"))
    } else if is_shell_mode {
        effective_trimmed.strip_prefix('!')
    } else {
        None
    };
    let has_mode_content = mode_content.map_or(false, |rest| !rest.trim().is_empty());

    let mode_badge = if has_mode_content {
        ""
    } else if is_content_mode {
        "Text"
    } else if is_file_mode {
        "Files"
    } else if is_win_mode {
        "Windows"
    } else if is_clip_mode {
        "Clipboard"
    } else if is_sys_mode {
        "System"
    } else if is_theme_mode {
        "Theme"
    } else if is_shell_mode {
        "Shell"
    } else if is_help_mode {
        "Help"
    } else {
        ""
    };
    ui.set_mode_badge_text(mode_badge.into());
    ui.set_is_file_mode(is_file_mode || is_content_mode);
    ui.set_is_shell_mode(is_shell_mode);

    use tiarun::apps::AppCategoryFilter;
    let is_category_search = !effective_trimmed.starts_with(['@', '!', ':'])
        && effective_trimmed != "?"
        && engine.resolve_path_search(effective_trimmed).is_none();
    ui.set_categories_enabled(is_category_search);
    let filter = if ui.get_cfg_view_style() == "liquid_glass" && is_category_search {
        AppCategoryFilter::from_chip_name(&ui.get_active_category())
    } else {
        AppCategoryFilter::All
    };

    let (mut results, file_sizes) = if let Some(results) = results {
        (results.matches, Some(results.file_sizes))
    } else {
        let matches = if filter == AppCategoryFilter::Actions {
            engine.search(&format!("@sys {effective_trimmed}"))
        } else {
            engine.search(effective_query)
        };
        (matches, None)
    };
    if filter != AppCategoryFilter::All {
        results.retain(|(item, _)| filter.matches(item));
    }
    if filter == AppCategoryFilter::Help && effective_trimmed.is_empty() {
        results.extend(engine.search("?"));
    }
    let count = results.len();
    ui.set_has_results(count > 0);

    let (mode_title, item_count_suffix) = if is_content_mode {
        ("CONTENT SEARCH", "matches")
    } else if is_file_mode {
        ("FILE SEARCH", "files")
    } else if is_win_mode {
        ("WINDOWS", "windows")
    } else if is_clip_mode {
        ("CLIPBOARD", "items")
    } else if is_sys_mode {
        ("SYSTEM", "actions")
    } else if is_theme_mode {
        ("THEME & OPACITY", "options")
    } else if is_shell_mode {
        ("RUN COMMAND", "command")
    } else if is_help_mode {
        ("KEYBOARD SHORTCUTS", "shortcuts")
    } else {
        ("APPLICATIONS", "apps")
    };
    ui.set_mode_title(mode_title.into());
    ui.set_status_item_count(format!("{} {}", count, item_count_suffix).into());

    let mut current_items = Vec::new();
    let mut slint_items = Vec::new();

    let file_query = if effective_trimmed.starts_with("@file ") {
        effective_trimmed[6..].trim()
    } else if effective_trimmed.starts_with("@f ") {
        effective_trimmed[3..].trim()
    } else if is_file_mode {
        ""
    } else {
        effective_trimmed
    };

    if count == 0 {
        // Only resolve the path (an extra dirs::home_dir() + stat call) when there
        // are actually no results to show - `engine.search()` already did the
        // equivalent resolution internally to produce those (or zero) results, so
        // doing it again here unconditionally on every keystroke was pure waste on
        // the common "there are results" path.
        let is_dir_view = if let Some((dir, filter)) = engine.resolve_path_search(file_query) {
            filter.is_empty() && dir.is_dir()
        } else {
            false
        };
        if is_dir_view {
            ui.set_empty_state_title("Folder is empty".into());
            ui.set_empty_state_subtitle("Press Backspace to go back to parent folder".into());
            ui.set_is_empty_folder(true);
        } else if is_win_mode {
            ui.set_empty_state_title("No matching windows found".into());
            ui.set_empty_state_subtitle("Make sure target application window is running".into());
            ui.set_is_empty_folder(false);
        } else if is_clip_mode {
            ui.set_empty_state_title("Clipboard is empty".into());
            ui.set_empty_state_subtitle("Copy some text to see it appear in clipboard history".into());
            ui.set_is_empty_folder(false);
        } else if is_shell_mode {
            ui.set_empty_state_title("Type a command to run".into());
            ui.set_empty_state_subtitle("e.g. ! ls -la   or   ! echo hello | wc -l".into());
            ui.set_is_empty_folder(false);
        } else if is_content_mode {
            ui.set_empty_state_title(format!("No text matches for '{}'", trimmed).into());
            ui.set_empty_state_subtitle("Search indexed text, documents, and code files".into());
            ui.set_is_empty_folder(false);
        } else if is_file_mode {
            ui.set_empty_state_title(format!("No files found for '{}'", trimmed).into());
            ui.set_empty_state_subtitle("Try searching with another keyword or path".into());
            ui.set_is_empty_folder(false);
        } else if !trimmed.is_empty() {
            ui.set_empty_state_title(format!("No results for '{}'", trimmed).into());
            ui.set_empty_state_subtitle("Try searching for an app, file, or math command".into());
            ui.set_is_empty_folder(false);
        } else {
            ui.set_empty_state_title("No applications found".into());
            ui.set_empty_state_subtitle("Try searching for an app, file, or math command".into());
            ui.set_is_empty_folder(false);
        }
    } else {
        ui.set_is_empty_folder(false);
    }

    let is_unlimited = filter != AppCategoryFilter::All || trimmed.is_empty();
    let items_iter: Box<dyn Iterator<Item = (launcher::LauncherItem, Vec<usize>)>> = if is_unlimited {
        Box::new(results.into_iter())
    } else {
        let compute_limit = (engine.config.search.max_results + 3).clamp(8, 20);
        Box::new(results.into_iter().take(compute_limit))
    };

    for (item, _indices) in items_iter {
        let (slint_icon, category) = match item.item_type {
            launcher::ItemType::App => {
                let icon = icon_resolver.resolve_icon(item.icon.as_deref(), &item.name, &item.exec_or_path);
                (icon, item.get_category_tag().to_string())
            }
            launcher::ItemType::Dir => {
                let icon = icon_resolver.resolve_file_type_icon(Path::new(&item.exec_or_path), launcher::ItemType::Dir);
                (icon, "Folder".to_string())
            }
            launcher::ItemType::File => {
                let icon = icon_resolver.resolve_file_type_icon(Path::new(&item.exec_or_path), launcher::ItemType::File);
                let tag = if let Some(line) = item.description.as_deref().and_then(|desc| desc.strip_prefix("Line ")) {
                    line.split_once(':').map_or_else(|| "Content".to_string(), |(number, _)| format!("Line {number}"))
                } else if let Some(file_sizes) = &file_sizes {
                    file_sizes.get(&item.exec_or_path).cloned().unwrap_or_else(|| "File".to_string())
                } else {
                    file_size_label(&item.exec_or_path)
                };
                (icon, tag)
            }
            launcher::ItemType::Calc => {
                (None, item.description.clone().unwrap_or_else(|| "Calc".to_string()))
            }
            launcher::ItemType::Window => {
                let icon = icon_resolver.resolve_icon(item.icon.as_deref(), &item.name, &item.exec_or_path);
                (icon, item.description.clone().unwrap_or_else(|| "Window".to_string()))
            }
            launcher::ItemType::Clipboard => {
                (None, item.description.clone().unwrap_or_else(|| "Clipboard".to_string()))
            }
            launcher::ItemType::System => {
                (None, "System".to_string())
            }
            launcher::ItemType::Theme => {
                (None, "Theme".to_string())
            }
            launcher::ItemType::Dmenu => {
                (None, "".to_string())
            }
            launcher::ItemType::Shell => {
                (None, "Run".to_string())
            }
            launcher::ItemType::WebSearch => {
                (None, "Web".to_string())
            }
        };

        let has_icon = slint_icon.is_some();
        let icon_img = slint_icon.unwrap_or_default();

        let item_type_str = match item.item_type {
            launcher::ItemType::App => "app",
            launcher::ItemType::File => "file",
            launcher::ItemType::Dir => "dir",
            launcher::ItemType::Calc => "calc",
            launcher::ItemType::Window => "win",
            launcher::ItemType::Clipboard => "clip",
            launcher::ItemType::System => "sys",
            launcher::ItemType::Theme => "theme",
            launcher::ItemType::Dmenu => "dmenu",
            launcher::ItemType::Shell => "shell",
            launcher::ItemType::WebSearch => "web",
        };

        let subtitle = match item.item_type {
            launcher::ItemType::App => {
                if let Some(ref desc) = item.description {
                    let desc_trimmed = desc.trim();
                    if !desc_trimmed.is_empty() && !desc_trimmed.eq_ignore_ascii_case(&item.name) {
                        desc_trimmed.to_string()
                    } else {
                        launcher::clean_app_exec(&item.exec_or_path)
                    }
                } else {
                    launcher::clean_app_exec(&item.exec_or_path)
                }
            }
            launcher::ItemType::File | launcher::ItemType::Dir => {
                if let Some(ref desc) = item.description {
                    if desc.starts_with("Line ") {
                        desc.clone()
                    } else {
                        item.exec_or_path.clone()
                    }
                } else {
                    item.exec_or_path.clone()
                }
            }
            _ => {
                if let Some(ref desc) = item.description {
                    desc.clone()
                } else {
                    item.exec_or_path.clone()
                }
            }
        };

        slint_items.push(LauncherItemData {
            name: item.name.clone().into(),
            category: category.into(),
            item_type: item_type_str.into(),
            has_icon,
            icon: icon_img,
            exec_or_path: item.exec_or_path.clone().into(),
            subtitle: subtitle.into(),
        });

        current_items.push(item);
    }

    let model = std::rc::Rc::new(slint::VecModel::from(slint_items));
    ui.set_items(model.into());

    update_action_hints(ui, current_items.first(), current_items.len(), 0);

    current_items
}

fn apply_theme_command(ui: &AppWindow, cmd: &str) {
    let mut cfg = Config::load();
    if cmd == "theme:dark" {
        cfg.theme.mode = "dark".to_string();
    } else if cmd == "theme:light" {
        cfg.theme.mode = "light".to_string();
    } else if cmd == "theme:system" {
        cfg.theme.mode = "system".to_string();
    } else if let Some(op_str) = cmd.strip_prefix("theme:opacity:") {
        if let Ok(val) = op_str.parse::<f32>() {
            cfg.theme.opacity = (val / 100.0).clamp(0.50, 1.00);
        }
    }
    let _ = cfg.save();
    let is_dark = cfg.theme.is_dark();
    ui.set_is_dark(is_dark);
    ui.set_cfg_theme_mode(cfg.theme.mode.clone().into());
    ui.set_cfg_theme_opacity(cfg.theme.opacity);
}

fn update_preview_for_item(
    ui: &AppWindow,
    ui_weak: slint::Weak<AppWindow>,
    item: Option<&launcher::LauncherItem>,
    preview_cache: &tiarun::file_preview::PreviewCache,
    current_image_gen: &Arc<std::sync::atomic::AtomicU64>,
) {
    use std::sync::atomic::Ordering;

    let generation = current_image_gen.fetch_add(1, Ordering::Relaxed) + 1;
    let Some(item) = item.filter(|item| matches!(item.item_type, launcher::ItemType::File | launcher::ItemType::Dir)) else {
        ui.set_show_preview(false);
        return;
    };

    let path = item.exec_or_path.clone();
    let line = item.description.as_deref()
        .and_then(|description| description.strip_prefix("Line "))
        .and_then(|description| description.split_once(':'))
        .and_then(|(number, _)| number.parse::<usize>().ok());
    let cache = preview_cache.clone();
    let current_image_gen = current_image_gen.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(35));
        if current_image_gen.load(Ordering::Relaxed) != generation {
            return;
        }

        let mut preview = cache.get_or_create_preview(&path);
        let contextual_text = line.and_then(|line| {
            tiarun::content_search::ContentSearchEngine::get_contextual_preview(&path, line)
        });
        let has_context = contextual_text.is_some();
        if let Some(snippet) = contextual_text {
            let extension = Path::new(&path).extension().and_then(|ext| ext.to_str())
                .filter(|ext| !ext.is_empty()).unwrap_or("text");
            preview.preview_lang = extension.to_uppercase();
            preview.text_snippet = snippet;
        }
        if current_image_gen.load(Ordering::Relaxed) != generation {
            return;
        }

        let thumbnail = preview.image_path.as_deref().and_then(|image_path| {
            cache.get_cached_thumbnail(image_path).or_else(|| {
                let (width, height, bytes) = tiarun::file_preview::decode_thumbnail_rgba(image_path, 720, 500)?;
                cache.insert_cached_thumbnail(image_path, width, height, bytes.clone());
                Some((width, height, bytes))
            })
        });
        let _ = slint::invoke_from_event_loop(move || {
            if current_image_gen.load(Ordering::Relaxed) != generation {
                return;
            }
            let Some(ui) = ui_weak.upgrade().filter(|ui| {
                ui.window().is_visible() && ui.get_is_showing_list() && !ui.get_in_settings_mode()
            }) else { return };

            let image = thumbnail.map(|(width, height, bytes)| {
                let mut pixels = slint::SharedPixelBuffer::new(width, height);
                pixels.make_mut_bytes().copy_from_slice(&bytes);
                slint::Image::from_rgba8(pixels)
            }).unwrap_or_default();
            ui.set_preview_title(preview.title.into());
            ui.set_preview_type(if has_context {
                "text".into()
            } else {
                preview.preview_type.as_str().into()
            });
            ui.set_preview_lang(preview.preview_lang.into());
            ui.set_preview_text(preview.text_snippet.into());
            ui.set_preview_file_size(preview.file_size.into());
            ui.set_preview_file_modified(preview.modified_time.into());
            ui.set_preview_file_location(preview.location.into());
            ui.set_preview_image(image);
            ui.set_show_preview(true);
        });
    });
}

fn ensure_selection_visible(ui: &AppWindow, selected_index: i32) {
    if ui.get_cfg_view_style() == "liquid_glass" {
        let theme = ui.global::<tiarun::Theme>();
        let row_top = theme.get_panel_padding()
            + selected_index as f32 * (theme.get_row_height() + theme.get_row_gap());
        let row_bottom = row_top + theme.get_row_height();
        let viewport_top = -ui.get_scroll_viewport_y();
        let viewport_height = ui.get_results_height();
        if row_top < viewport_top {
            ui.set_scroll_viewport_y(-row_top);
        } else if row_bottom > viewport_top + viewport_height {
            ui.set_scroll_viewport_y(viewport_height - row_bottom);
        }
        return;
    }
    let row_height = 44.0;
    let row_top = selected_index as f32 * row_height;
    let viewport_top = -ui.get_scroll_viewport_y();
    let viewport_height = ui.get_results_height();
    if row_top < viewport_top {
        ui.set_scroll_viewport_y(-row_top);
    } else if row_top + row_height > viewport_top + viewport_height {
        ui.set_scroll_viewport_y(viewport_height - row_top - row_height);
    }
}

/// Warms the icon cache a few items at a time via `slint::Timer::single_shot`,
/// re-scheduling itself until the whole list is done. Runs entirely on the
/// event-loop thread, so it keeps the window responsive between chunks instead
/// of resolving every icon in one blocking call at startup.
#[cfg(target_os = "windows")]
fn warm_icon_cache_chunked(icon_resolver: Arc<IconResolver>, apps: Vec<LauncherItem>, start: usize) {
    const CHUNK_SIZE: usize = 5;
    let end = (start + CHUNK_SIZE).min(apps.len());
    for app in &apps[start..end] {
        let _ = icon_resolver.resolve_icon(app.icon.as_deref(), &app.name, &app.exec_or_path);
    }
    if end < apps.len() {
        slint::Timer::single_shot(std::time::Duration::from_millis(15), move || {
            warm_icon_cache_chunked(icon_resolver, apps, end);
        });
    }
}

fn run_dmenu_mode(prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        if std::env::var("SLINT_BACKEND").is_err() {
            unsafe {
                std::env::set_var("SLINT_BACKEND", "winit-software");
            }
        }
    }

    use std::io::BufRead;
    let stdin = std::io::stdin();
    let raw_lines: Vec<String> = stdin.lock().lines().filter_map(|l| l.ok()).collect();
    if raw_lines.is_empty() {
        return Ok(());
    }

    init_platform_with_app_id("tiarun")?;
    let ui = AppWindow::new()?;
    #[cfg(target_os = "windows")]
    ui.set_cfg_enable_shadow_margin(false);
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }
    ui.set_categories_enabled(false);
    ui.set_mode_title(prompt.to_string().into());
    ui.set_mode_badge_text("dmenu".into());
    ui.set_is_file_mode(false);
    ui.set_cfg_show_icons(false);
    ui.set_cfg_show_status_bar(true);
    ui.set_cfg_max_results(8);
    ui.set_cfg_theme_mode("dark".into());
    ui.set_cfg_theme_opacity(0.95);
    ui.set_is_dark(true);

    let icon_resolver = IconResolver::new();
    ui.set_settings_icon(icon_resolver.get_gear_icon());
    ui.set_nav_icon(icon_resolver.get_nav_icon());
    ui.set_enter_icon(icon_resolver.get_enter_icon());
    ui.set_search_icon(icon_resolver.get_search_icon());
    ui.set_folder_icon(icon_resolver.get_folder_icon());
    ui.set_scope_apps_icon(icon_resolver.get_scope_apps_icon());
    ui.set_scope_files_icon(icon_resolver.get_scope_files_icon());
    ui.set_scope_text_icon(icon_resolver.get_scope_text_icon());
    ui.set_scope_windows_icon(icon_resolver.get_scope_windows_icon());

    ui.window().with_winit_window(|winit_window| {
        winit_window.set_transparent(true);
        winit_window.set_decorations(false);
        apply_skip_taskbar(winit_window);
        #[cfg(target_os = "windows")]
        {
            winit_window.set_visible(true);
            winit_window.focus_window();
        }
        if let Some(monitor) = winit_window.current_monitor().or_else(|| winit_window.primary_monitor()) {
            let screen_size = monitor.size();
            let scale_factor = monitor.scale_factor();
            let monitor_pos = monitor.position();
            let window_width = (f64::from(ui.get_launcher_width()) * scale_factor) as u32;
            let _window_height = (f64::from(ui.get_launcher_height()) * scale_factor) as u32;
            let pos_x = monitor_pos.x + (screen_size.width.saturating_sub(window_width) / 2) as i32;
            let pos_y = monitor_pos.y + (screen_size.height as f64 * 0.15) as i32;
            winit_window.set_outer_position(i_slint_backend_winit::winit::dpi::PhysicalPosition::new(pos_x, pos_y));
        }
    });

    #[cfg(target_os = "windows")]
    apply_windows_rounded_corners(&ui);

    let current_filtered = std::rc::Rc::new(std::cell::RefCell::new(raw_lines.clone()));

    let update_dmenu_view = {
        let ui_weak = ui.as_weak();
        let raw_lines = raw_lines.clone();
        let current_filtered = current_filtered.clone();
        move |query: &str| {
            let Some(ui) = ui_weak.upgrade() else { return; };
            let trimmed = query.trim();
            let filtered: Vec<String> = if trimmed.is_empty() {
                raw_lines.clone()
            } else {
                let norm_query = launcher::remove_vietnamese_accents(trimmed);
                let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
                use fuzzy_matcher::FuzzyMatcher;
                let mut matches: Vec<(String, i64)> = raw_lines
                    .iter()
                    .filter_map(|line| {
                        let norm_line = launcher::remove_vietnamese_accents(line);
                        let s1 = matcher.fuzzy_match(line, trimmed).unwrap_or(0);
                        let s2 = matcher.fuzzy_match(&norm_line, &norm_query).unwrap_or(0);
                        let max_s = s1.max(s2);
                        if max_s > 0 { Some((line.clone(), max_s)) } else { None }
                    })
                    .collect();
                matches.sort_by(|a, b| b.1.cmp(&a.1));
                matches.into_iter().map(|(l, _)| l).collect()
            };

            *current_filtered.borrow_mut() = filtered.clone();
            ui.set_has_results(!filtered.is_empty());
            ui.set_status_item_count(format!("{} items", filtered.len()).into());
            ui.set_selected_index(0);
            ui.set_scroll_viewport_y(0.0);

            let slint_items: Vec<LauncherItemData> = filtered
                .iter()
                .take(50)
                .map(|l| LauncherItemData {
                    name: l.clone().into(),
                    category: "".into(),
                    item_type: "dmenu".into(),
                    has_icon: false,
                    icon: slint::Image::default(),
                    exec_or_path: l.clone().into(),
                    subtitle: l.clone().into(),
                })
                .collect();
            ui.set_items(std::rc::Rc::new(slint::VecModel::from(slint_items)).into());
        }
    };

    update_dmenu_view("");

    {
        let update_view = update_dmenu_view.clone();
        ui.on_search_text_changed(move |text| {
            update_view(text.as_str());
        });
    }

    {
        let current_filtered = current_filtered.clone();
        ui.on_item_activated(move |idx| {
            let list = current_filtered.borrow();
            if let Some(item) = list.get(idx as usize) {
                println!("{}", item);
                std::process::exit(0);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let current_filtered = current_filtered.clone();
        ui.on_move_up(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let cur = ui.get_selected_index();
                if cur > 0 {
                    let next = cur - 1;
                    ui.set_selected_index(next);
                    ensure_selection_visible(&ui, next);
                }
            }
        });
        let ui_weak = ui.as_weak();
        ui.on_move_down(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let total = current_filtered.borrow().len() as i32;
                let cur = ui.get_selected_index();
                if cur + 1 < total {
                    let next = cur + 1;
                    ui.set_selected_index(next);
                    ensure_selection_visible(&ui, next);
                }
            }
        });
    }

    ui.on_close_window(|| {
        std::process::exit(1);
    });

    ui.run()?;
    Ok(())
}

/// Explicitly selects the winit platform backend (mirroring the same `SLINT_BACKEND`
/// handling Slint's own default backend selector does for the "winit" case) and sets
/// the XDG app id, so it takes effect on the window about to be created.
///
/// This can't be done by just calling `slint::set_xdg_app_id()` right after
/// `AppWindow::new()`: Slint's winit backend creates the real native window (and
/// sends its Wayland app_id / X11 WM_CLASS) *eagerly*, as part of constructing the
/// window adapter, which happens inside `AppWindow::new()` itself - by the time that
/// call returns, the window already exists without an app_id, and there's no way to
/// change it afterwards. `set_xdg_app_id()` also requires the platform to already be
/// selected (it errors otherwise), so the backend has to be initialized manually here
/// rather than relying on Slint's normal lazy auto-selection on first window creation.
fn init_platform_with_app_id(app_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let backend_config = std::env::var("SLINT_BACKEND").unwrap_or_default().to_lowercase();
    let renderer_name = backend_config.split_once('-').map(|(_, renderer)| renderer).filter(|r| !r.is_empty());
    let backend = i_slint_backend_winit::Backend::new_with_renderer_by_name(renderer_name)?;
    slint::platform::set_platform(Box::new(backend))
        .map_err(|e| format!("failed to set slint platform: {e}"))?;
    slint::set_xdg_app_id(app_id)?;
    Ok(())
}

/// Plays the "pop in" appear animation: a plain fade-in-place (no position
/// movement - animating position together with the panel's drop-shadow proved too
/// heavy for rapid show/hide cycles and crashed the compositor). Puts the window
/// at opacity 0, then (after a short delay so at least one frame actually renders
/// that state - a 1ms delay isn't enough, since the compositor's first post-show
/// frame can take longer than that to reach the screen) flips it to opacity 1, so
/// `main_card`'s `animate`d `opacity` in app_window.slint eases it in instead of
/// it just snapping into view. Must be called on the UI/event-loop thread.
#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_after_sway_placement(ui: slint::Weak<AppWindow>, deadline: Instant) {
    slint::Timer::single_shot(Duration::from_millis(8), move || {
        if let Some(window) = ui.upgrade().filter(|ui| ui.window().is_visible()) {
            if launcher_is_placed_on_sway(&window) == Some(true) || Instant::now() >= deadline {
                window.invoke_focus_search();
                window.set_is_pop_visible(true);
            } else {
                reveal_after_sway_placement(ui, deadline);
            }
        }
    });
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_after_hyprland_placement(ui: slint::Weak<AppWindow>, deadline: Instant) {
    slint::Timer::single_shot(Duration::from_millis(8), move || {
        if let Some(window) = ui.upgrade().filter(|ui| ui.window().is_visible()) {
            if place_hyprland_launcher(&window).is_some() || Instant::now() >= deadline {
                window.invoke_focus_search();
                window.set_is_pop_visible(true);
            } else {
                reveal_after_hyprland_placement(ui, deadline);
            }
        }
    });
}

fn animate_window_pop_in(ui_weak: slint::Weak<AppWindow>) {
    if let Some(ui) = ui_weak.upgrade() {
        ui.set_is_pop_visible(false);
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if std::env::var_os("SWAYSOCK").is_some() {
                // Allow the compositor to map the window, then place it before fading in.
                reveal_after_sway_placement(ui_weak, Instant::now() + Duration::from_millis(120));
                return;
            }
            if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
                reveal_after_hyprland_placement(ui_weak, Instant::now() + Duration::from_millis(120));
                return;
            }
        }
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(8), move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_is_pop_visible(true);
        }
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        if std::env::var("SLINT_BACKEND").is_err() {
            // Default to winit-software on Windows for seamless window transparency and guaranteed DWM composition compatibility
            unsafe {
                std::env::set_var("SLINT_BACKEND", "winit-software");
            }
        }
    }

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("tiarun {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "TiaRun {}\nUsage: tiarun [OPTIONS]\n\nOptions:\n  -b, --daemon        Start resident background daemon without showing window\n  -d, --dmenu         Run in dmenu pipe mode\n  -p, --prompt <TEXT> Custom prompt in dmenu mode\n  -q, --quit          Quit running background launcher daemon\n  -v, --version       Show version\n  -h, --help          Show help",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--quit" || a == "-q") {
        #[cfg(unix)]
        {
            let socket_path = get_socket_path();
            if let Ok(mut stream) = UnixStream::connect(&socket_path) {
                let _ = stream.write_all(b"quit");
                println!("tiarun daemon stopped.");
                return Ok(());
            }
        }
        #[cfg(windows)]
        {
            let addr: SocketAddr = "127.0.0.1:42425".parse().unwrap();
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
                let _ = stream.write_all(b"quit");
                println!("tiarun daemon stopped.");
                return Ok(());
            }
        }
        println!("tiarun is not running.");
        return Ok(());
    }

    let is_dmenu = args.iter().any(|a| a == "--dmenu" || a == "-d");
    if is_dmenu {
        let mut prompt = "Select:".to_string();
        let mut i = 1;
        while i < args.len() {
            if (args[i] == "-p" || args[i] == "--prompt") && i + 1 < args.len() {
                prompt = args[i + 1].clone();
                i += 1;
            }
            i += 1;
        }
        return run_dmenu_mode(&prompt);
    }

    let mut is_daemon = args.iter().any(|a| a == "--daemon" || a == "-b" || a == "--background");

    let non_flag_args: Vec<&String> = args.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
    let mut initial_query = if !non_flag_args.is_empty() {
        let q = non_flag_args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
        Some(q)
    } else {
        None
    };

    // 0. Super-fast single-instance check BEFORE allocating GPU or Window (<0.2ms!)
    let msg_to_send = if is_daemon {
        b"ping".to_vec()
    } else if let Some(ref q) = initial_query {
        format!("query:{}", q).into_bytes()
    } else {
        b"toggle".to_vec()
    };
    if try_send_to_existing_instance(&msg_to_send) {
        if is_daemon {
            println!("tiarun daemon is already running.");
        }
        return Ok(());
    }

    let config = Config::load();
    let search_index = Arc::new(tiarun::search_index::SearchIndexEngine::for_config(&config));
    let indexing_stop = Arc::new(AtomicBool::new(false));
    if let Err(error) = search_index.start_crawler(indexing_stop.clone()) {
        eprintln!("[tiarun-crawler] Failed to start: {error}");
    }

    #[cfg(unix)]
    let mut daemon_listener = None;

    #[cfg(unix)]
    if is_daemon {
        let socket_path = get_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        if let Ok(listener) = UnixListener::bind(&socket_path) {
            // Setup global shortcut and currency rate cache in background while idle
            std::thread::spawn(|| {
                tiarun::config::setup_global_shortcut();
            });
            tiarun::calc::init_currency_rates();

            // Quiet resident daemon: do NOT create any window, GPU context, or Wayland surface
            // at startup. Block silently on the socket until the user actually summons the launcher.
            loop {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 512];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let cmd = String::from_utf8_lossy(&buf[..n]);

                    if cmd.starts_with("quit") {
                        indexing_stop.store(true, Ordering::Relaxed);
                        let _ = std::fs::remove_file(&socket_path);
                        return Ok(());
                    }
                    if cmd.starts_with("ping") {
                        continue;
                    }
                    if let Some(query) = cmd.strip_prefix("query:") {
                        initial_query = Some(query.trim().to_string());
                    }
                    // For "toggle" or "settings" or query, wake up and instantiate window!
                    break;
                }
            }
            daemon_listener = Some(listener);
            is_daemon = false;
        }
    }

    // Identify this window to the compositor/WM as "tiarun" so Wayland
    // (app_id) and X11 (WM_CLASS) window managers can match it to the installed
    // .desktop entry (assets/tiarun.desktop: Icon=tiarun,
    // StartupWMClass=tiarun) and show the real app icon in the dock/
    // taskbar instead of falling back to a generic one. Must run before the
    // window is created (see init_platform_with_app_id's doc comment).
    init_platform_with_app_id("tiarun")?;

    // 1. Create main Slint Window for primary resident daemon
    let ui = AppWindow::new()?;
    #[cfg(target_os = "windows")]
    ui.set_cfg_enable_shadow_margin(false);

    let shown_instant = Arc::new(std::sync::RwLock::new(Instant::now()));

    // 2. Start background socket listener for instant wakeup
    #[cfg(unix)]
    start_daemon_listener(ui.as_weak(), shown_instant.clone(), daemon_listener);
    #[cfg(windows)]
    start_daemon_listener(ui.as_weak(), shown_instant.clone());

    #[cfg(all(unix, not(target_os = "macos")))]
    start_hyprland_focus_watcher(ui.as_weak(), shown_instant.clone());

    // Universal auto-hide on blur (focus loss) timer for Windows, X11, Wayland
    let focus_check_timer = Rc::new(slint::Timer::default());
    {
        let ui_weak = ui.as_weak();
        let shown = shown_instant.clone();
        focus_check_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(120),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    if ui.window().is_visible() && !ui.get_in_settings_mode() {
                        let elapsed = shown.read().map(|t| t.elapsed()).unwrap_or_default();
                        if elapsed > std::time::Duration::from_millis(600) {
                            if !is_launcher_focused(&ui) {
                                hide_launcher_window(&ui);
                                ui.set_search_text("".into());
                                ui.set_is_expanded(false);
                            }
                        }
                    }
                }
            },
        );
    }

    // Setup global shortcut and currency rate cache in background (non-blocking)
    std::thread::spawn(|| {
        tiarun::config::setup_global_shortcut();
    });
    tiarun::calc::init_currency_rates();

    // 3. Apply Config & Initialize Engine
    ui.set_cfg_autostart(tiarun::config::is_autostart_enabled());
    ui.set_cfg_show_icons(config.theme.show_icons.unwrap_or(true));
    ui.set_cfg_show_status_bar(config.theme.show_status_bar.unwrap_or(true));
    ui.set_cfg_compact_empty(config.theme.compact_empty_view.unwrap_or(true));
    ui.set_cfg_view_style(config.theme.view_style.clone().into());
    ui.set_cfg_enable_path_matching(config.search.enable_path_matching.unwrap_or(true));
    ui.set_cfg_max_results(config.search.max_results as i32);
    ui.set_cfg_max_depth(config.search.max_depth as i32);
    ui.set_cfg_theme_mode(config.theme.mode.clone().into());
    ui.set_cfg_theme_opacity(config.theme.opacity);
    ui.set_is_dark(config.theme.is_dark());
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }

    let engine = Arc::new(LauncherEngine::with_search_index(config, search_index));
    let icon_resolver = Arc::new(IconResolver::new());

    let categories: Vec<slint::SharedString> = engine
        .available_categories()
        .into_iter()
        .map(Into::into)
        .collect();
    ui.set_categories(std::rc::Rc::new(slint::VecModel::from(categories)).into());

    // Set UI action icons
    ui.set_settings_icon(icon_resolver.get_gear_icon());
    ui.set_nav_icon(icon_resolver.get_nav_icon());
    ui.set_enter_icon(icon_resolver.get_enter_icon());
    ui.set_search_icon(icon_resolver.get_search_icon());
    ui.set_folder_icon(icon_resolver.get_folder_icon());
    ui.set_scope_apps_icon(icon_resolver.get_scope_apps_icon());
    ui.set_scope_files_icon(icon_resolver.get_scope_files_icon());
    ui.set_scope_text_icon(icon_resolver.get_scope_text_icon());
    ui.set_scope_windows_icon(icon_resolver.get_scope_windows_icon());

    // Window properties (Skip Taskbar / Tool Window overlay)
    ui.window().with_winit_window(move |winit_window| {
        winit_window.set_title("TiaRun");
        winit_window.set_transparent(true);
        winit_window.set_decorations(false);
        apply_skip_taskbar(winit_window);
        #[cfg(target_os = "windows")]
        {
            if !is_daemon {
                winit_window.set_visible(true);
                winit_window.focus_window();
            }
        }
    });

    let center_window = |ui: &AppWindow| {
        reposition_and_anchor_window(ui);
    };

    center_window(&ui);
    #[cfg(target_os = "windows")]
    apply_windows_rounded_corners(&ui);

    // Runtime blur backend detection and startup status reporting
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (_blur_backend, status, tier_name) = ui.window().with_winit_window(|winit_window| {
            tiarun::blur::detect_blur_backend(winit_window)
        }).unwrap_or_else(|| (Box::new(tiarun::blur::fallback::FallbackBlurBackend), tiarun::blur::BlurStatus::Fallback, "Fallback".to_string()));

        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
        let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());
        println!(
            "[tiarun] Session: {}, Desktop: {}, Blur: {} ({:?}), Placement: Centered overlay",
            session_type, current_desktop, tier_name, status
        );

        let has_blur = status != tiarun::blur::BlurStatus::Fallback;
        ui.set_has_blur(has_blur);
    }

    // Re-center when Wayland surface configure event resolves
    let center_timer = slint::Timer::default();
    {
        let ui_weak = ui.as_weak();
        center_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(60),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    center_window(&ui);
                    #[cfg(target_os = "windows")]
                    apply_windows_rounded_corners(&ui);
                }
            },
        );
    }

    // 4. Initial Population
    let current_results = Arc::new(std::sync::RwLock::new(populate_items(
        &ui,
        &engine,
        &icon_resolver,
        "",
    )));

    // Preload icons on startup. On Linux this is a fast O(1) index lookup, but on
    // Windows each icon requires a synchronous SHGetFileInfoW/PrivateExtractIconsW
    // shell call (which internally resolves .lnk targets via COM), so warming the
    // whole Start Menu in one blocking call can noticeably delay the window becoming
    // interactive on machines with a large Start Menu. `slint::Image` is not
    // Send/Sync, so this can't be offloaded to a background thread without either
    // moving Image across threads (unsound) or re-deriving pixel data on that thread
    // and initializing COM on it (fragile, unverifiable without a Windows machine).
    // Instead, warm the cache in small chunks via the event loop's own timer so the
    // window can paint and respond to input between chunks, rather than blocking
    // startup on the full list.
    #[cfg(target_os = "windows")]
    {
        if is_daemon {
            let resolver = icon_resolver.clone();
            let apps = engine.apps.clone();
            slint::Timer::single_shot(Duration::from_secs(3), move || {
                warm_icon_cache_chunked(resolver, apps, 0);
            });
        } else {
            warm_icon_cache_chunked(icon_resolver.clone(), engine.apps.clone(), 0);
        }
    }
    #[cfg(not(target_os = "windows"))]
    icon_resolver.preload_icons(&engine.apps);

    // Automatically re-populate initial items once background icon indexing finishes
    // so any newly resolved icons appear on screen without waiting for user typing.
    let refresh_timer = Rc::new(slint::Timer::default());
    {
        let engine = engine.clone();
        let icon_resolver = icon_resolver.clone();
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();
        let refresh_timer_clone = refresh_timer.clone();
        let mut check_count = 0;
        refresh_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(80),
            move || {
                check_count += 1;
                if icon_resolver.is_indexing_done() || check_count >= 25 {
                    if let Some(ui) = ui_weak.upgrade() {
                        if ui.get_search_text().is_empty() && !ui.get_in_settings_mode() {
                            let items = populate_items(&ui, &engine, &icon_resolver, "");
                            if let Ok(mut lock) = current_results.write() {
                                *lock = items;
                            }
                        }
                    }
                    refresh_timer_clone.stop();
                }
            },
        );
    }

    let preview_cache = tiarun::file_preview::PreviewCache::new();
    let current_image_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // File and content searches can touch the filesystem on every keystroke.
    // One worker coalesces pending queries and only publishes the current one.
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::mpsc::{self, RecvTimeoutError};

        let (send, receive) = mpsc::channel::<(u64, String, String, String)>();
        let (send_results, receive_results) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let publish = {
            let engine = engine.clone();
            let icon_resolver = icon_resolver.clone();
            let current_results = current_results.clone();
            let preview_cache = preview_cache.clone();
            let current_image_gen = current_image_gen.clone();
            move |ui: &AppWindow, query: &str, effective: &str, results| {
                let items = render_search_results(ui, &engine, &icon_resolver, query, effective, results);
                let preview_item = items.first().filter(|_| ui.get_is_showing_list());
                update_preview_for_item(ui, ui.as_weak(), preview_item, &preview_cache, &current_image_gen);
                if let Ok(mut lock) = current_results.write() {
                    *lock = items;
                }
                ui.set_selected_index(0);
                ui.set_scroll_viewport_y(0.0);
            }
        };

        let worker_engine = engine.clone();
        let worker_generation = generation.clone();
        let ui_weak = ui.as_weak();
        std::thread::spawn(move || {
            while let Ok(mut request) = receive.recv() {
                loop {
                    match receive.recv_timeout(Duration::from_millis(20)) {
                        Ok(newer) => request = newer,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                let (id, query, scope, effective) = request;
                if worker_generation.load(Ordering::Relaxed) != id {
                    continue;
                }
                let cancelled = || worker_generation.load(Ordering::Relaxed) != id;
                let matches = worker_engine.search_until_cancelled(&effective, &cancelled);
                if cancelled() {
                    continue;
                }
                let max_rows = if query.trim().is_empty() {
                    matches.len()
                } else {
                    (worker_engine.config.search.max_results + 3).clamp(8, 20)
                };
                let file_sizes = matches.iter().take(max_rows)
                    .take_while(|_| !cancelled())
                    .filter(|(item, _)| item.item_type == launcher::ItemType::File
                        && !item.description.as_deref().is_some_and(|desc| desc.starts_with("Line ")))
                    .map(|(item, _)| (item.exec_or_path.clone(), file_size_label(&item.exec_or_path)))
                    .collect();
                if cancelled() {
                    continue;
                }
                let results = FileSearchResults { matches, file_sizes };
                if send_results.send((id, query, scope, effective, results)).is_err() {
                    break;
                }
                let ui_weak = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.invoke_file_search_finished();
                    }
                });
            }
        });

        {
            let generation = generation.clone();
            let publish = publish.clone();
            let ui_weak = ui.as_weak();
            ui.on_file_search_finished(move || {
                if let Some((id, query, scope, effective, results)) = receive_results.try_iter().last() {
                    if let Some(ui) = ui_weak.upgrade().filter(|ui| {
                        generation.load(Ordering::Relaxed) == id
                            && ui.window().is_visible()
                            && !ui.get_in_settings_mode()
                            && ui.get_search_text() == query.as_str()
                            && ui.get_active_scope() == scope.as_str()
                    }) {
                        publish(&ui, &query, &effective, Some(results));
                    }
                }
            });
        }

        let engine = engine.clone();
        let current_results = current_results.clone();
        let current_image_gen = current_image_gen.clone();
        let schedule = move |ui: &AppWindow, text: &str, keep_expanded: bool| {
            let id = generation.fetch_add(1, Ordering::Relaxed) + 1;
            let effective = effective_search_query(ui, &engine, text);
            let scope = ui.get_active_scope();
            let file_scope = matches!(scope.as_str(), "files" | "text");
            ui.set_is_expanded(keep_expanded || !text.is_empty() || file_scope
                || (ui.get_cfg_view_style() == "liquid_glass" && !ui.get_active_category().is_empty()));
            resize_window_for_state(ui);

            let search_files = effective.trim().starts_with('@') && file_scope
                || effective.trim().starts_with(['/', '~'])
                || (!effective.trim().starts_with(['@', '!', ':'])
                    && (effective.contains(['/', '\\']) || effective.as_bytes().get(1) == Some(&b':')));
            if search_files {
                // Keep the previous rows visible while typing, but prevent
                // activating them after they no longer match the query.
                if let Ok(mut lock) = current_results.write() {
                    lock.clear();
                }
                current_image_gen.fetch_add(1, Ordering::Relaxed);
                let _ = send.send((id, text.to_string(), scope.to_string(), effective));
            } else {
                publish(ui, text, &effective, None);
            }
        };
        let ui_weak = ui.as_weak();
        let schedule_edit = schedule.clone();
        ui.on_search_text_changed(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                schedule_edit(&ui, &text, false);
            }
        });
        let ui_weak = ui.as_weak();
        ui.on_scope_selected(move |scope| {
            if let Some(ui) = ui_weak.upgrade() {
                let query = switch_to_scope(&ui, &scope);
                schedule(&ui, &query, true);
            }
        });
    }

    // 5.1 Category Chip Selected (macOS Liquid Glass chips)
    {
        let engine = engine.clone();
        let icon_resolver = icon_resolver.clone();
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();
        ui.on_chip_selected(move |category| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_active_category(category.clone());
                ui.set_is_expanded(true);
                // When selecting a category chip, automatically clear the search text so all apps
                // in the chosen category appear immediately instead of being filtered by previous text.
                if !category.is_empty() {
                    ui.set_search_text("".into());
                }
                resize_window_for_state(&ui);
                let items = populate_items(&ui, &engine, &icon_resolver, &ui.get_search_text());
                ui.set_scroll_viewport_y(0.0);
                if let Ok(mut lock) = current_results.write() {
                    *lock = items;
                }
                ui.set_selected_index(0);
            }
        });
    }

fn strip_scope_prefixes(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("@file ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@f ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@text ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@content ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@grep ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@fc ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@t ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@win ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@w ") {
        rest.to_string()
    } else {
        raw.to_string()
    }
}

fn switch_to_scope(ui: &AppWindow, target_scope: &str) -> String {
    ui.set_active_scope(target_scope.into());
    ui.set_is_expanded(true);

    let raw_text = ui.get_search_text().to_string();
    let mut clean_query = strip_scope_prefixes(&raw_text);

    // If moving to a non-files scope (e.g. text, windows, apps) and the query is a directory path
    // (ends with '/', or starts with '~' or '/', or Windows drive path), clear it so it doesn't pollute the new scope.
    if target_scope != "files" {
        let trimmed_q = clean_query.trim();
        let is_dir_path = trimmed_q.ends_with('/')
            || trimmed_q.ends_with('\\')
            || trimmed_q.starts_with('~')
            || trimmed_q.starts_with('/')
            || (cfg!(windows) && trimmed_q.len() >= 2 && trimmed_q.as_bytes()[1] == b':');
        if is_dir_path {
            clean_query = String::new();
        }
    }

    if clean_query != raw_text {
        ui.set_search_text(clean_query.clone().into());
        ui.invoke_move_cursor_to_end();
    }

    clean_query
}

    // 5.0 Shell ("!") command history recall — Up/Down cycle through past commands,
    // like a real shell. `shell_history_cursor` is -1 when not currently browsing
    // (i.e. sitting on the live, possibly-edited command line); otherwise it's an
    // index into `engine.history`'s ordered shell command log.
    let shell_history_cursor: Rc<Cell<i64>> = Rc::new(Cell::new(-1));
    {
        let engine = engine.clone();
        let cursor = shell_history_cursor.clone();
        ui.on_shell_history_prev(move || -> slint::SharedString {
            let history = match engine.history.read() {
                Ok(h) => h,
                Err(_) => return "!".into(),
            };
            let commands = history.shell_commands();
            if commands.is_empty() {
                return "!".into();
            }
            let idx = if cursor.get() < 0 {
                commands.len() as i64 - 1
            } else {
                (cursor.get() - 1).max(0)
            };
            cursor.set(idx);
            format!("!{}", commands[idx as usize]).into()
        });
    }
    {
        let engine = engine.clone();
        let cursor = shell_history_cursor.clone();
        ui.on_shell_history_next(move || -> slint::SharedString {
            if cursor.get() < 0 {
                return "!".into();
            }
            let history = match engine.history.read() {
                Ok(h) => h,
                Err(_) => return "!".into(),
            };
            let commands = history.shell_commands();
            let idx = cursor.get() + 1;
            if idx as usize >= commands.len() {
                cursor.set(-1);
                return "!".into();
            }
            cursor.set(idx);
            format!("!{}", commands[idx as usize]).into()
        });
    }
    {
        let cursor = shell_history_cursor.clone();
        ui.on_shell_history_reset(move || {
            cursor.set(-1);
        });
    }

    // 5.1 Connect Move Up with Auto-Scroll
    {
        let ui_weak = ui.as_weak();
        let current_results = current_results.clone();
        let preview_cache = preview_cache.clone();
        let current_image_gen = current_image_gen.clone();
        ui.on_move_up(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if ui.get_search_text().is_empty() && ui.get_selected_index() <= 0 && ui.get_is_expanded() {
                    ui.set_is_expanded(false);
                    ui.set_show_preview(false);
                    resize_window_for_state(&ui);
                    return;
                }
                let total = if let Ok(lock) = current_results.read() {
                    lock.len() as i32
                } else {
                    0
                };
                if total == 0 { return; }
                let cur = ui.get_selected_index();
                let next = if cur <= 0 { total - 1 } else { cur - 1 };
                ui.set_selected_index(next);
                ensure_selection_visible(&ui, next);
                if let Ok(lock) = current_results.read() {
                    update_action_hints(&ui, lock.get(next as usize), total as usize, next as usize);
                }
                let query = ui.get_search_text();
                let trimmed = query.trim();
                let scope = ui.get_active_scope().to_string();
                let is_file_or_content = scope == "files" || scope == "text"
                    || trimmed.starts_with("@f")
                    || trimmed.starts_with("@file")
                    || trimmed.starts_with("@content")
                    || trimmed.starts_with("@text")
                    || trimmed.starts_with("@grep")
                    || trimmed.starts_with("@fc")
                    || trimmed.starts_with("@t")
                    || trimmed.starts_with('/')
                    || trimmed.starts_with('~');
                let is_showing_list = ui.get_is_showing_list();
                if let Ok(lock) = current_results.read() {
                    let item = lock.get(next as usize);
                    let should_preview = is_showing_list && (is_file_or_content
                        || item.map(|it| it.item_type == launcher::ItemType::File || it.item_type == launcher::ItemType::Dir).unwrap_or(false));
                    if should_preview {
                        update_preview_for_item(&ui, ui.as_weak(), item, &preview_cache, &current_image_gen);
                    } else {
                        ui.set_show_preview(false);
                    }
                }
                resize_window_for_state(&ui);
            }
        });
    }

    // 5.2 Connect Move Down with Auto-Scroll
    {
        let ui_weak = ui.as_weak();
        let current_results = current_results.clone();
        let preview_cache = preview_cache.clone();
        let current_image_gen = current_image_gen.clone();
        ui.on_move_down(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if ui.get_search_text().is_empty() && !ui.get_is_expanded() {
                    ui.set_is_expanded(true);
                    resize_window_for_state(&ui);
                    return;
                }
                let total = if let Ok(lock) = current_results.read() {
                    lock.len() as i32
                } else {
                    0
                };
                if total == 0 { return; }
                let cur = ui.get_selected_index();
                let next = if cur + 1 >= total { 0 } else { cur + 1 };
                ui.set_selected_index(next);
                ensure_selection_visible(&ui, next);
                if let Ok(lock) = current_results.read() {
                    update_action_hints(&ui, lock.get(next as usize), total as usize, next as usize);
                }
                let query = ui.get_search_text();
                let trimmed = query.trim();
                let scope = ui.get_active_scope().to_string();
                let is_file_or_content = scope == "files" || scope == "text"
                    || trimmed.starts_with("@f")
                    || trimmed.starts_with("@file")
                    || trimmed.starts_with("@content")
                    || trimmed.starts_with("@text")
                    || trimmed.starts_with("@grep")
                    || trimmed.starts_with("@fc")
                    || trimmed.starts_with("@t")
                    || trimmed.starts_with('/')
                    || trimmed.starts_with('~');
                let is_showing_list = ui.get_is_showing_list();
                if let Ok(lock) = current_results.read() {
                    let item = lock.get(next as usize);
                    let should_preview = is_showing_list && (is_file_or_content
                        || item.map(|it| it.item_type == launcher::ItemType::File || it.item_type == launcher::ItemType::Dir).unwrap_or(false));
                    if should_preview {
                        update_preview_for_item(&ui, ui.as_weak(), item, &preview_cache, &current_image_gen);
                    } else {
                        ui.set_show_preview(false);
                    }
                }
                resize_window_for_state(&ui);
            }
        });
    }

    fn step_into_directory(
        ui: &AppWindow,
        current_results: &Arc<std::sync::RwLock<Vec<launcher::LauncherItem>>>,
    ) -> bool {
        let sel_idx = ui.get_selected_index() as usize;
        let item_opt = {
            if let Ok(lock) = current_results.read() {
                lock.get(sel_idx).cloned()
            } else {
                None
            }
        };

        if let Some(item) = item_opt {
            if item.item_type == launcher::ItemType::Dir {
                let search_text = ui.get_search_text().to_string();
                let mut path = item.exec_or_path;
                if let Some(home) = dirs::home_dir() {
                    let home_str = home.to_string_lossy().to_string();
                    if path.starts_with(&home_str) {
                        path = format!("~{}", &path[home_str.len()..]);
                    }
                }
                if !path.ends_with('/') {
                    path.push('/');
                }
                let prefix = if search_text.starts_with("@file ") || search_text == "@file" {
                    "@file "
                } else if search_text.starts_with("@f ") || search_text == "@f" {
                    "@f "
                } else {
                    ""
                };
                let new_search_text = format!("{}{}", prefix, path);
                ui.set_search_text(new_search_text.clone().into());
                ui.invoke_move_cursor_to_end();
                ui.invoke_search_text_changed(new_search_text.into());
                return true;
            }
        }
        false
    }

    // 5.3 Connect Handle Right Arrow (Drill Into Directory)
    {
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();

        ui.on_handle_right_arrow(move || -> bool {
            if let Some(ui) = ui_weak.upgrade() {
                step_into_directory(&ui, &current_results)
            } else {
                false
            }
        });
    }

    // 5.3b Connect Tab and Shift+Tab (Scope cycling loop)
    {
        let ui_weak = ui.as_weak();
        ui.on_tab_pressed(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let cur_scope = ui.get_active_scope().to_string();
                let next_scope = match cur_scope.as_str() {
                    "apps" => "files",
                    "files" => "text",
                    "text" => "windows",
                    _ => "apps",
                };
                ui.invoke_scope_selected(next_scope.into());
                ui.invoke_move_cursor_to_end();
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.on_backtab_pressed(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let cur_scope = ui.get_active_scope().to_string();
                let prev_scope = match cur_scope.as_str() {
                    "windows" => "text",
                    "text" => "files",
                    "files" => "apps",
                    _ => "windows",
                };
                ui.invoke_scope_selected(prev_scope.into());
                ui.invoke_move_cursor_to_end();
            }
        });
    }

fn step_out_directory(
    ui: &AppWindow,
) -> bool {
    let search_text = ui.get_search_text().to_string();
    let is_file_mode = search_text.starts_with("@f ") || search_text.starts_with("@file ");
    let is_path_mode = search_text.starts_with('/') || search_text.starts_with('~');
    if (is_file_mode || is_path_mode) && search_text.ends_with('/') {
        let prefix_len = if search_text.starts_with("@file ") {
            6
        } else if search_text.starts_with("@f ") {
            3
        } else {
            0
        };
        let path_part = &search_text[prefix_len..];
        let trimmed_path = path_part.trim_end_matches('/');
        if let Some(pos) = trimmed_path.rfind('/') {
            let parent_path = &trimmed_path[..=pos];
            let new_search_text = format!("{}{}", &search_text[..prefix_len], parent_path);
            ui.set_search_text(new_search_text.clone().into());
            ui.invoke_move_cursor_to_end();
            ui.invoke_search_text_changed(new_search_text.into());
            return true;
        } else {
            let new_search_text = format!("{}", &search_text[..prefix_len]);
            ui.set_search_text(new_search_text.clone().into());
            ui.invoke_move_cursor_to_end();
            ui.invoke_search_text_changed(new_search_text.into());
            return true;
        }
    }
    false
}

    // 5.4 Connect Handle Backspace & Left Arrow (Step Out to Parent Directory)
    {
        let ui_weak = ui.as_weak();

        ui.on_handle_backspace(move || -> bool {
            if let Some(ui) = ui_weak.upgrade() {
                step_out_directory(&ui)
            } else {
                false
            }
        });
    }

    {
        let ui_weak = ui.as_weak();

        ui.on_handle_left_arrow(move || -> bool {
            if let Some(ui) = ui_weak.upgrade() {
                step_out_directory(&ui)
            } else {
                false
            }
        });
    }

    // 6. Connect Item Activated (Enter key)
    {
        let engine = engine.clone();
        let icon_resolver = icon_resolver.clone();
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();

        ui.on_item_activated(move |idx| {
            let idx = idx as usize;
            let item_opt = {
                if let Ok(lock) = current_results.read() {
                    lock.get(idx).cloned()
                } else {
                    None
                }
            };

            if let Some(item) = item_opt {
                if item.item_type == launcher::ItemType::Theme {
                    if let Some(ui) = ui_weak.upgrade() {
                        apply_theme_command(&ui, &item.exec_or_path);
                        let items = populate_items(&ui, &engine, &icon_resolver, &ui.get_search_text());
                        if let Ok(mut lock) = current_results.write() {
                            *lock = items;
                        }
                    }
                    return;
                }

                if let Some(ui) = ui_weak.upgrade() {
                    hide_launcher_window(&ui);
                    ui.set_search_text("".into());
                    ui.set_active_scope("apps".into());
                    ui.set_is_expanded(false);
                }
                engine.launch(&item);
            }
        });
    }

    // 7. Connect Open In Terminal (Alt+T)
    {
        let engine = engine.clone();
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();

        ui.on_open_in_terminal(move |idx| {
            let idx = idx as usize;
            let item_opt = {
                if let Ok(lock) = current_results.read() {
                    lock.get(idx).cloned()
                } else {
                    None
                }
            };

            if let Some(item) = item_opt {
                if let Some(ui) = ui_weak.upgrade() {
                    hide_launcher_window(&ui);
                    ui.set_search_text("".into());
                    ui.set_active_scope("apps".into());
                    ui.set_is_expanded(false);
                }
                engine.open_in_terminal(&item);
            }
        });
    }

    // 8. Connect Copy Path (Alt+C)
    {
        let engine = engine.clone();
        let current_results = current_results.clone();

        ui.on_copy_path(move |idx| {
            let idx = idx as usize;
            let item_opt = {
                if let Ok(lock) = current_results.read() {
                    lock.get(idx).cloned()
                } else {
                    None
                }
            };

            if let Some(item) = item_opt {
                engine.copy_to_clipboard(&item.exec_or_path);
            }
        });
    }

    // 8.1 Connect Smart Copy Item (Ctrl+C)
    {
        let engine = engine.clone();
        let current_results = current_results.clone();

        ui.on_copy_item(move |idx| {
            let idx = idx as usize;
            let item_opt = {
                if let Ok(lock) = current_results.read() {
                    lock.get(idx).cloned()
                } else {
                    None
                }
            };

            if let Some(item) = item_opt {
                engine.copy_item_smart(&item);
            }
        });
    }

    // 9. Connect Open Settings View (Ctrl+,)
    {
        let ui_weak = ui.as_weak();
        ui.on_open_config(move || {
            let start = std::time::Instant::now();
            if std::env::var("TIARUN_DEBUG").is_ok() || std::env::var("VIEW_LAUNCHER_DEBUG").is_ok() || std::env::var("RUST_LOG").is_ok() {
                eprintln!("[DEBUG] on_open_config ENTER at {:?}", start);
            }
            if let Some(ui) = ui_weak.upgrade() {
                let cfg = Config::load();
                ui.set_cfg_autostart(tiarun::config::is_autostart_enabled());
                ui.set_cfg_theme_mode(cfg.theme.mode.clone().into());
                ui.set_cfg_theme_opacity(cfg.theme.opacity);
                ui.set_is_dark(cfg.theme.is_dark());
                ui.set_in_settings_mode(true);
                ui.set_settings_status("".into());
                resize_window_for_state(&ui);
                ui.invoke_focus_settings();
            }
            if std::env::var("TIARUN_DEBUG").is_ok() || std::env::var("VIEW_LAUNCHER_DEBUG").is_ok() || std::env::var("RUST_LOG").is_ok() {
                eprintln!("[DEBUG] on_open_config EXIT in {:?}", start.elapsed());
            }
        });
    }

    // 9.1 Connect Close Settings View
    {
        let ui_weak = ui.as_weak();
        ui.on_close_settings(move || {
            let start = std::time::Instant::now();
            if std::env::var("TIARUN_DEBUG").is_ok() || std::env::var("VIEW_LAUNCHER_DEBUG").is_ok() || std::env::var("RUST_LOG").is_ok() {
                eprintln!("[DEBUG] on_close_settings ENTER at {:?}", start);
            }
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_in_settings_mode(false);
                resize_window_for_state(&ui);
                ui.invoke_focus_search();
            }
            if std::env::var("TIARUN_DEBUG").is_ok() || std::env::var("VIEW_LAUNCHER_DEBUG").is_ok() || std::env::var("RUST_LOG").is_ok() {
                eprintln!("[DEBUG] on_close_settings EXIT in {:?}", start.elapsed());
            }
        });
    }

    // 9.2 Connect Save Settings
    {
        let ui_weak = ui.as_weak();
        let engine = engine.clone();
        let icon_resolver = icon_resolver.clone();
        let current_results = current_results.clone();
        ui.on_save_settings(move |show_icons, show_status_bar, enable_path, max_results, max_depth, autostart, compact_empty, theme_mode, theme_opacity, view_style| {
            let mut cfg = Config::load();
            cfg.general.autostart = autostart;
            cfg.theme.show_icons = Some(show_icons);
            cfg.theme.show_status_bar = Some(show_status_bar);
            cfg.theme.compact_empty_view = Some(compact_empty);
            cfg.theme.mode = theme_mode.to_string();
            cfg.theme.opacity = theme_opacity;
            cfg.theme.view_style = view_style.to_string();
            cfg.search.enable_path_matching = Some(enable_path);
            cfg.search.max_results = max_results as usize;
            cfg.search.max_depth = max_depth as usize;
            let _ = cfg.save();

            let is_dark = cfg.theme.is_dark();
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_is_dark(is_dark);
                ui.set_cfg_theme_mode(theme_mode);
                ui.set_cfg_theme_opacity(theme_opacity);
                ui.set_cfg_view_style(view_style);
                ui.set_settings_status("✔ Saved automatically".into());
                let items = populate_items(&ui, &engine, &icon_resolver, &ui.get_search_text());
                if let Ok(mut lock) = current_results.write() {
                    *lock = items;
                }
            }
        });
    }

    // 9.3 Connect Open Raw config.toml in Editor
    ui.on_open_raw_config(move || {
        open_config_file();
    });

    // 9.4 Connect Immediate System Theme Request
    {
        let ui_weak = ui.as_weak();
        ui.on_request_system_theme(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let is_dark = tiarun::config::is_system_dark_mode();
                ui.set_is_dark(is_dark);
            }
        });
    }

    // 10. Connect Close Window (Esc)
    {
        let ui_weak = ui.as_weak();
        ui.on_close_window(move || {
            if let Some(ui) = ui_weak.upgrade() {
                hide_launcher_window(&ui);
                ui.set_search_text("".into());
                ui.set_active_scope("apps".into());
                ui.set_is_expanded(false);
                ui.set_show_preview(false);
            }
        });
    }

    let ui_weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.invoke_close_window();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });

    // 10.1 Native Window Dragging on Wayland / X11 / Windows
    {
        let ui_weak = ui.as_weak();
        ui.on_start_window_drag(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.window().with_winit_window(|winit_window| {
                    let _ = winit_window.drag_window();
                });
            }
        });
    }

    // 10.2 Real-time System Theme Watcher Thread (non-blocking)
    {
        let ui_weak = ui.as_weak();
        std::thread::spawn(move || {
            loop {
                #[cfg(target_os = "windows")]
                std::thread::sleep(std::time::Duration::from_secs(3));
                #[cfg(not(target_os = "windows"))]
                std::thread::sleep(std::time::Duration::from_millis(400));

                let current_dark = tiarun::config::is_system_dark_mode();
                let ui_w = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_w.upgrade() {
                        if ui.get_cfg_theme_mode() == "system" && ui.get_is_dark() != current_dark {
                            ui.set_is_dark(current_dark);
                        }
                    }
                });
            }
        });
    }

    // 11. Run Slint Event Loop
    // The window is first shown as part of `run()` itself (cold start becoming the
    // daemon); schedule the same pop-in transition for that initial appearance too.
    if !is_daemon {
        if let Ok(mut lock) = shown_instant.write() {
            *lock = Instant::now();
        }
        if let Some(ref q) = initial_query {
            ui.set_search_text(q.clone().into());
            ui.set_is_expanded(true);
            ui.invoke_search_text_changed(q.clone().into());
        }
        animate_window_pop_in(ui.as_weak());
        show_launcher_window(&ui)?;
        focus_launcher_window(&ui);
        ui.invoke_focus_search();
        let ui_w = ui.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
            if let Some(ui) = ui_w.upgrade() {
                ui.invoke_focus_search();
            }
        });
    }
    let result = slint::run_event_loop_until_quit();
    indexing_stop.store(true, Ordering::Relaxed);

    // Cleanup socket on exit
    #[cfg(unix)]
    {
        let socket_path = get_socket_path();
        let _ = std::fs::remove_file(&socket_path);
    }

    result?;
    Ok(())
}
