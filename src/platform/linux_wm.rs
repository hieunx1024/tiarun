use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use tiarun::AppWindow;

use super::{hide_launcher_window, resize_window_for_state};

pub(crate) fn is_sway_launcher_focused(sway_sock: &str) -> bool {
    sway_ipc_query(sway_sock, 4).is_some_and(|tree| check_node_focused(&tree))
}

pub(crate) fn sway_ipc_query(socket: &str, request_type: u32) -> Option<serde_json::Value> {
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

pub(crate) fn is_launcher_focused(ui: &AppWindow) -> bool {
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

pub(crate) fn send_sway_ipc_command(cmd: &str) {
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

pub(crate) fn apply_x11_skip_taskbar(winit_window: &i_slint_backend_winit::winit::window::Window) {
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
    #[cfg(not(all(target_os = "linux", feature = "x11")))]
    let _ = winit_window;
}

pub(crate) fn focus_wm_window() {
    if let (Ok(runtime_dir), Ok(sig)) = (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE")) {
        let sock_path = PathBuf::from(runtime_dir).join("hypr").join(sig).join(".socket.sock");
        if let Ok(mut stream) = UnixStream::connect(&sock_path) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
            let _ = stream.write_all(b"dispatch focuswindow title:^TiaRun$");
        }
    }
    send_sway_ipc_command("[app_id=\"tiarun\"] focus");
}

pub(crate) fn query_hyprland_ipc(cmd: &str) -> Option<String> {
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

pub(crate) fn send_hyprland_ipc_command(cmd: &str) {
    let _ = query_hyprland_ipc(cmd);
}

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

pub(crate) fn place_hyprland_launcher(ui: &AppWindow) -> Option<()> {
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

pub(crate) fn resize_hyprland_launcher(width: i32, _height: i32) -> Option<()> {
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

pub(crate) fn resize_sway_launcher(width: i32, height: i32) -> Option<()> {
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

pub(crate) fn launcher_is_placed_on_sway(ui: &AppWindow) -> Option<bool> {
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

pub(crate) fn is_gnome_desktop() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    let is_sway = std::env::var_os("SWAYSOCK").is_some() || desktop.contains("sway");
    let is_hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() || desktop.contains("hyprland");
    !is_sway && !is_hyprland && desktop.contains("gnome")
}

pub(crate) fn start_hyprland_focus_watcher(ui_handle: slint::Weak<AppWindow>, shown_instant: Arc<std::sync::RwLock<Instant>>) {
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

pub(crate) fn reveal_after_sway_placement(ui: slint::Weak<AppWindow>, deadline: Instant) {
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

pub(crate) fn reveal_after_hyprland_placement(ui: slint::Weak<AppWindow>, deadline: Instant) {
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

pub(crate) fn try_reposition_wm(ui: &AppWindow) -> bool {
    if std::env::var_os("SWAYSOCK").is_some() {
        let _ = launcher_is_placed_on_sway(ui);
        return true;
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let _ = place_hyprland_launcher(ui);
        return true;
    }
    false
}

pub(crate) fn post_resize_wm(ui: &AppWindow, width: i32, height: i32) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let _ = resize_hyprland_launcher(width, height);
    } else if std::env::var_os("SWAYSOCK").is_none() {
        super::reposition_and_anchor_window(ui);
    }
}

pub(crate) fn schedule_wm_pop_in(ui_weak: &slint::Weak<AppWindow>) -> bool {
    if std::env::var_os("SWAYSOCK").is_some() {
        // Allow the compositor to map the window, then place it before fading in.
        reveal_after_sway_placement(ui_weak.clone(), Instant::now() + Duration::from_millis(120));
        return true;
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        reveal_after_hyprland_placement(ui_weak.clone(), Instant::now() + Duration::from_millis(120));
        return true;
    }
    false
}

pub(crate) fn log_blur_backend_status(ui: &AppWindow) {
    let (_blur_backend, status, tier_name) = ui.window().with_winit_window(|winit_window| {
        tiarun::blur::detect_blur_backend(winit_window)
    }).unwrap_or_else(|| (Box::new(tiarun::blur::fallback::FallbackBlurBackend), tiarun::blur::BlurStatus::Fallback, "Fallback".to_string()));

    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());
    println!(
        "[tiarun] Session: {}, Desktop: {}, Blur: {} ({:?}), Placement: Centered overlay",
        session_type, current_desktop, tier_name, status
    );
    let _ = resize_window_for_state;
}
