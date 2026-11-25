#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dmenu;
mod platform;
mod ui_state;

use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
use std::net::{SocketAddr, TcpListener, TcpStream};

use i_slint_backend_winit::{WinitWindowAccessor, WinitWindowEventResult, winit};
use slint::ComponentHandle;
use tiarun::AppWindow;
use tiarun::config::Config;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher::{self, LauncherEngine, open_config_file};

use crate::dmenu::run_dmenu_mode;
use crate::platform::{
    animate_window_pop_in, apply_skip_taskbar, configure_windows_borderless_transparent,
    focus_launcher_window, hide_launcher_window, init_platform_with_app_id, is_gnome_desktop,
    is_launcher_focused, log_blur_backend_status, reposition_and_anchor_window,
    resize_window_for_state, show_and_focus_launcher, show_launcher_window,
};
use crate::ui_state::{
    apply_settings_to_ui, apply_theme_command, is_window_mode_active, move_selection,
    populate_items, populate_ui_icons, step_into_directory, step_out_directory,
    wire_search_worker, wire_shell_history_callbacks, wire_window_refresh_worker,
};

#[cfg(unix)]
fn try_send_to_existing_instance(msg: &[u8]) -> bool {
    let socket_path = tiarun::ipc::get_socket_path();
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

/// Shared daemon command dispatcher for both Unix socket and Windows TCP listeners.
/// Returns `true` when `"quit"` is received and the listener loop should terminate.
fn handle_daemon_command(
    cmd: &str,
    ui_handle: &slint::Weak<AppWindow>,
    shown_instant: &Arc<std::sync::RwLock<Instant>>,
) -> bool {
    if cmd.starts_with("quit") {
        let _ = slint::quit_event_loop();
        return true;
    }

    if cmd.starts_with("escape") || cmd.starts_with("close") {
        let ui_weak = ui_handle.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_close_window();
            }
        });
        return false;
    }

    if cmd.starts_with("ping") {
        return false;
    }

    if cmd.starts_with("settings") {
        let ui_weak = ui_handle.clone();
        let shown = shown_instant.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                show_and_focus_launcher(&ui, ui_weak.clone(), &shown, "");
                let cfg = Config::load();
                apply_settings_to_ui(&ui, &cfg);
            }
        });
        return false;
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
        return false;
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
    false
}

#[cfg(unix)]
fn start_daemon_listener(
    ui_handle: slint::Weak<AppWindow>,
    shown_instant: Arc<std::sync::RwLock<Instant>>,
    existing_listener: Option<UnixListener>,
) {
    let socket_path = tiarun::ipc::get_socket_path();
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
                    if handle_daemon_command(&cmd, &ui_handle, &shown_instant) {
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(windows)]
fn start_daemon_listener(
    ui_handle: slint::Weak<AppWindow>,
    shown_instant: Arc<std::sync::RwLock<Instant>>,
) {
    // 1. Native Win32 global hotkey thread (Ctrl + Alt + Space) with 0ms spawn latency
    platform::start_windows_global_hotkey(ui_handle.clone(), shown_instant.clone());

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
                    if handle_daemon_command(&cmd, &ui_handle, &shown_instant) {
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(unix)]
fn bind_daemon_listener() -> Option<UnixListener> {
    let socket_path = tiarun::ipc::get_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    UnixListener::bind(&socket_path).ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Allow Slint's default GPU renderer (FemtoVG) on all platforms including Windows,
    // with winit's built-in automatic fallback to software rendering if GL init fails.
    // Users can still override via `SLINT_BACKEND` (e.g. `winit-software`).
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
        if try_send_to_existing_instance(b"quit") {
            println!("tiarun daemon stopped.");
            return Ok(());
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

    let is_daemon = args.iter().any(|a| a == "--daemon" || a == "-b" || a == "--background");

    let non_flag_args: Vec<&String> = args.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
    let initial_query = if !non_flag_args.is_empty() {
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

    // Bind the IPC socket early so any toggle arriving while the daemon warms up
    // queues cleanly in the kernel listen backlog instead of failing to connect.
    #[cfg(unix)]
    let daemon_listener = bind_daemon_listener();

    let config = Config::load_fast();

    // Identify this window to the compositor/WM as "tiarun" so Wayland
    // (app_id) and X11 (WM_CLASS) window managers can match it to the installed
    // .desktop entry (assets/tiarun.desktop: Icon=tiarun,
    // StartupWMClass=tiarun) and show the real app icon in the dock/
    // taskbar instead of falling back to a generic one. Must run before the
    // window is created (see init_platform_with_app_id's doc comment).
    init_platform_with_app_id("tiarun")?;

    // 1. Create main Slint Window for primary resident daemon
    let ui = AppWindow::new()?;

    let shown_instant = Arc::new(std::sync::RwLock::new(Instant::now()));

    // 2. Start background socket listener for instant wakeup
    #[cfg(unix)]
    start_daemon_listener(ui.as_weak(), shown_instant.clone(), daemon_listener);
    #[cfg(windows)]
    start_daemon_listener(ui.as_weak(), shown_instant.clone());

    #[cfg(all(unix, not(target_os = "macos")))]
    platform::start_hyprland_focus_watcher(ui.as_weak(), shown_instant.clone());

    // Universal auto-hide on blur (focus loss) timer for Windows, X11, Wayland
    let focus_check_timer = Rc::new(slint::Timer::default());
    {
        let ui_weak = ui.as_weak();
        let shown = shown_instant.clone();
        focus_check_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(120),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    if ui.window().is_visible() && !ui.get_in_settings_mode() {
                        let elapsed = shown.read().map(|t| t.elapsed()).unwrap_or_default();
                        if elapsed > Duration::from_millis(600) && !is_launcher_focused(&ui) {
                            hide_launcher_window(&ui);
                            ui.set_search_text("".into());
                            ui.set_is_expanded(false);
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
    ui.set_cfg_enable_path_matching(config.search.enable_path_matching.unwrap_or(true));
    ui.set_cfg_max_results(config.search.max_results as i32);
    ui.set_cfg_max_depth(config.search.max_depth as i32);
    ui.set_cfg_theme_mode(config.theme.mode.clone().into());
    ui.set_cfg_theme_opacity(config.theme.opacity);
    let initial_dark = if is_daemon && config.theme.mode.eq_ignore_ascii_case("system") {
        true
    } else {
        config.theme.is_dark()
    };
    ui.set_is_dark(initial_dark);
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }

    let engine = Arc::new(LauncherEngine::new(config));
    let icon_resolver = Arc::new(IconResolver::new());

    // Set UI action icons
    populate_ui_icons(&ui, &icon_resolver);

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

    if !is_daemon {
        reposition_and_anchor_window(&ui);
    }
    configure_windows_borderless_transparent(&ui);
    log_blur_backend_status(&ui);

    // Re-center when Wayland surface configure event resolves
    let center_timer = slint::Timer::default();
    if !is_daemon {
        let ui_weak = ui.as_weak();
        center_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(60),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    reposition_and_anchor_window(&ui);
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

    // Warm the icon cache in small non-blocking chunks via the event loop's timer
    // rather than blocking startup on decoding every installed app icon on the main
    // thread. When running as a hidden daemon, warming proceeds silently in the
    // background; when visible, it pauses automatically to keep the UI responsive.
    if is_daemon {
        let ui_weak = ui.as_weak();
        let resolver = icon_resolver.clone();
        let apps = engine.apps.clone();
        let delay = if cfg!(target_os = "windows") {
            Duration::from_secs(3)
        } else {
            Duration::from_millis(150)
        };
        slint::Timer::single_shot(delay, move || {
            ui_state::warm_icon_cache_chunked(ui_weak, resolver, apps, 0);
        });
    } else {
        ui_state::warm_icon_cache_chunked(ui.as_weak(), icon_resolver.clone(), engine.apps.clone(), 0);
    }

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
            Duration::from_millis(80),
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
    let window_refresh = wire_window_refresh_worker(
        &ui,
        engine.clone(),
        icon_resolver.clone(),
        current_results.clone(),
    );

    wire_search_worker(
        &ui,
        engine.clone(),
        icon_resolver.clone(),
        current_results.clone(),
        preview_cache.clone(),
        current_image_gen.clone(),
        window_refresh.clone(),
    );

    // 5.0 Shell ("!") command history recall — Up/Down cycle through past commands.
    wire_shell_history_callbacks(&ui, engine.clone());

    // 5.1 & 5.2 Connect Move Up / Down with Auto-Scroll
    {
        let ui_weak = ui.as_weak();
        let current_results = current_results.clone();
        let preview_cache = preview_cache.clone();
        let current_image_gen = current_image_gen.clone();
        ui.on_move_up(move || {
            if let Some(ui) = ui_weak.upgrade() {
                move_selection(&ui, &current_results, &preview_cache, &current_image_gen, -1);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let current_results = current_results.clone();
        let preview_cache = preview_cache.clone();
        let current_image_gen = current_image_gen.clone();
        ui.on_move_down(move || {
            if let Some(ui) = ui_weak.upgrade() {
                move_selection(&ui, &current_results, &preview_cache, &current_image_gen, 1);
            }
        });
    }

    // 5.3 Connect Handle Right Arrow (Drill Into Directory)
    {
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();
        ui.on_handle_right_arrow(move || -> bool {
            ui_weak.upgrade().is_some_and(|ui| step_into_directory(&ui, &current_results))
        });
    }

    // 5.3b Connect Tab and Shift+Tab (Scope cycling loop)
    {
        let ui_weak = ui.as_weak();
        ui.on_tab_pressed(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let next_scope = match ui.get_active_scope().as_str() {
                    "apps" => "files",
                    "files" => "windows",
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
                let prev_scope = match ui.get_active_scope().as_str() {
                    "windows" => "files",
                    "files" => "apps",
                    _ => "windows",
                };
                ui.invoke_scope_selected(prev_scope.into());
                ui.invoke_move_cursor_to_end();
            }
        });
    }

    // 5.4 Connect Handle Backspace & Left Arrow (Step Out to Parent Directory)
    {
        let ui_weak = ui.as_weak();
        ui.on_handle_backspace(move || -> bool {
            ui_weak.upgrade().is_some_and(|ui| step_out_directory(&ui))
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_handle_left_arrow(move || -> bool {
            ui_weak.upgrade().is_some_and(|ui| step_out_directory(&ui))
        });
    }

    // 6. Connect Item Activated (Enter key)
    {
        let engine = engine.clone();
        let icon_resolver = icon_resolver.clone();
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();

        ui.on_item_activated(move |idx| {
            let item_opt = current_results.read().ok().and_then(|lock| lock.get(idx as usize).cloned());
            if let Some(item) = item_opt {
                if item.item_type == launcher::ItemType::Mode {
                    if let Some(ui) = ui_weak.upgrade() {
                        let next_query = format!("{} ", item.exec_or_path);
                        ui.set_search_text(next_query.clone().into());
                        ui.set_is_expanded(true);
                        ui.invoke_move_cursor_to_end();
                        ui.invoke_search_text_changed(next_query.into());
                    }
                    return;
                }

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
            let item_opt = current_results.read().ok().and_then(|lock| lock.get(idx as usize).cloned());
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
            let item_opt = current_results.read().ok().and_then(|lock| lock.get(idx as usize).cloned());
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
            let item_opt = current_results.read().ok().and_then(|lock| lock.get(idx as usize).cloned());
            if let Some(item) = item_opt {
                engine.copy_item_smart(&item);
            }
        });
    }

    // 8.2 Connect Kill Window Process (Alt+K in @w mode)
    {
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();
        let window_refresh = window_refresh.clone();

        ui.on_kill_process(move |idx| {
            let item_opt = current_results
                .read()
                .ok()
                .and_then(|lock| lock.get(idx as usize).cloned());
            if let Some(item) = item_opt {
                if item.item_type == launcher::ItemType::Window {
                    if let Some(pid) = item.pid {
                        if std::env::var("TIARUN_DEBUG").is_ok() {
                            eprintln!("[DEBUG] on_kill_process idx={} pid={} title={}", idx, pid, item.name);
                        }
                        let ui_w = ui_weak.clone();
                        let refresh = window_refresh.clone();
                        std::thread::spawn(move || {
                            if tiarun::window_switcher::kill_process_graceful_then_force(pid) {
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = ui_w.upgrade() {
                                        if ui.window().is_visible() && is_window_mode_active(&ui) {
                                            refresh.spawn(&ui);
                                        }
                                    }
                                });
                            }
                        });
                    }
                }
            }
        });
    }

    // Unified winit shortcut & focus handler using real compositor/OS modifier state so
    // Wayland/IME modifier quirks in Slint's internal tracking cannot swallow shortcuts.
    {
        use std::cell::Cell;
        use winit::event::{ElementState, KeyEvent as WinitKeyEvent, WindowEvent};
        use winit::keyboard::{Key, ModifiersState, NamedKey};
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

        fn key_matches_char(event: &WinitKeyEvent, ch: &str) -> bool {
            matches!(&event.logical_key, Key::Character(s) if s.eq_ignore_ascii_case(ch))
                || matches!(event.key_without_modifiers(), Key::Character(ref s) if s.eq_ignore_ascii_case(ch))
        }

        let ui_weak = ui.as_weak();
        let engine_focus = engine.clone();
        let winit_mods = Rc::new(Cell::new(ModifiersState::empty()));
        ui.window().on_winit_window_event(move |_win, event| {
            match event {
                WindowEvent::ModifiersChanged(mods) => {
                    winit_mods.set(mods.state());
                }
                WindowEvent::Focused(false) => {
                    winit_mods.set(ModifiersState::empty());
                }
                WindowEvent::Focused(true) => {
                    let clip = engine_focus.clipboard.clone();
                    std::thread::spawn(move || {
                        clip.refresh_now();
                    });
                }
                WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                    let pressed = event.state == ElementState::Pressed;
                    if let Key::Named(named) = event.logical_key {
                        let bit = match named {
                            NamedKey::Control => Some(ModifiersState::CONTROL),
                            NamedKey::Alt => Some(ModifiersState::ALT),
                            NamedKey::Shift => Some(ModifiersState::SHIFT),
                            NamedKey::Super => Some(ModifiersState::SUPER),
                            _ => None,
                        };
                        if let Some(bit) = bit {
                            let mut mods = winit_mods.get();
                            mods.set(bit, pressed);
                            winit_mods.set(mods);
                            return WinitWindowEventResult::Propagate;
                        }
                    }

                    if pressed && !event.repeat {
                        let mods = winit_mods.get();
                        let ctrl = mods.control_key() && !mods.alt_key() && !mods.super_key();
                        let alt = mods.alt_key() && !mods.control_key() && !mods.super_key();
                        let Some(ui) = ui_weak.upgrade() else {
                            return WinitWindowEventResult::Propagate;
                        };
                        if !ui.window().is_visible() || ui.get_in_settings_mode() {
                            return WinitWindowEventResult::Propagate;
                        }

                        enum ShortcutAction {
                            OpenConfig,
                            OpenInTerminal,
                            CopyPath,
                            CopyItem,
                            KillProcess,
                        }

                        let action = if ctrl && !mods.shift_key() && key_matches_char(event, ",") {
                            Some(ShortcutAction::OpenConfig)
                        } else if alt && !mods.shift_key() && key_matches_char(event, "t") {
                            Some(ShortcutAction::OpenInTerminal)
                        } else if (ctrl && mods.shift_key() || alt && !mods.shift_key())
                            && key_matches_char(event, "c")
                        {
                            Some(ShortcutAction::CopyPath)
                        } else if ctrl
                            && !mods.shift_key()
                            && key_matches_char(event, "c")
                            && !ui.get_has_search_selection()
                        {
                            Some(ShortcutAction::CopyItem)
                        } else if alt
                            && !mods.shift_key()
                            && key_matches_char(event, "k")
                            && is_window_mode_active(&ui)
                        {
                            Some(ShortcutAction::KillProcess)
                        } else {
                            None
                        };

                        if let Some(action) = action {
                            let ui_w = ui.as_weak();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_w.upgrade() {
                                    let idx = ui.get_selected_index();
                                    match action {
                                        ShortcutAction::OpenConfig => ui.invoke_open_config(),
                                        ShortcutAction::OpenInTerminal => ui.invoke_open_in_terminal(idx),
                                        ShortcutAction::CopyPath => ui.invoke_copy_path(idx),
                                        ShortcutAction::CopyItem => ui.invoke_copy_item(idx),
                                        ShortcutAction::KillProcess => ui.invoke_kill_process(idx),
                                    }
                                }
                            });
                            return WinitWindowEventResult::PreventDefault;
                        }
                    }
                }
                _ => {}
            }
            WinitWindowEventResult::Propagate
        });
    }

    // 9. Connect Open Settings View (Ctrl+,)
    {
        let ui_weak = ui.as_weak();
        ui.on_open_config(move || {
            let start = Instant::now();
            if std::env::var("TIARUN_DEBUG").is_ok() || std::env::var("VIEW_LAUNCHER_DEBUG").is_ok() || std::env::var("RUST_LOG").is_ok() {
                eprintln!("[DEBUG] on_open_config ENTER at {:?}", start);
            }
            if let Some(ui) = ui_weak.upgrade() {
                let cfg = Config::load();
                apply_settings_to_ui(&ui, &cfg);
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
            let start = Instant::now();
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
        ui.on_save_settings(move |show_icons, show_status_bar, enable_path, max_results, max_depth, autostart, compact_empty, theme_mode, theme_opacity| {
            let mut cfg = Config::load();
            cfg.general.autostart = autostart;
            cfg.theme.show_icons = Some(show_icons);
            cfg.theme.show_status_bar = Some(show_status_bar);
            cfg.theme.compact_empty_view = Some(compact_empty);
            cfg.theme.mode = theme_mode.to_string();
            cfg.theme.opacity = theme_opacity;
            cfg.search.enable_path_matching = Some(enable_path);
            cfg.search.max_results = max_results as usize;
            cfg.search.max_depth = max_depth as usize;
            let _ = cfg.save();

            let is_dark = cfg.theme.is_dark();
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_is_dark(is_dark);
                ui.set_cfg_theme_mode(theme_mode);
                ui.set_cfg_theme_opacity(theme_opacity);
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
            let mut first = true;
            loop {
                if !first {
                    #[cfg(target_os = "windows")]
                    std::thread::sleep(Duration::from_secs(3));
                    #[cfg(not(target_os = "windows"))]
                    std::thread::sleep(Duration::from_millis(400));
                }
                first = false;

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

    // 10.3 Live CPU & RAM refresh timer while @w Window Switcher is visible
    let window_stats_timer = Rc::new(slint::Timer::default());
    {
        let ui_weak = ui.as_weak();
        let window_refresh = window_refresh.clone();
        window_stats_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(2500),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    if ui.window().is_visible()
                        && !ui.get_in_settings_mode()
                        && is_window_mode_active(&ui)
                    {
                        window_refresh.spawn(&ui);
                    }
                }
            },
        );
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
        if initial_query.is_some() {
            ui.invoke_move_cursor_to_end();
        }
        let ui_w = ui.as_weak();
        let initial_query_for_timer = initial_query.clone();
        slint::Timer::single_shot(Duration::from_millis(30), move || {
            if let Some(ui) = ui_w.upgrade() {
                ui.invoke_focus_search();
                if let Some(ref q) = initial_query_for_timer {
                    if ui.get_search_text().as_str() == q {
                        ui.invoke_move_cursor_to_end();
                    }
                }
            }
        });
    }
    let result = slint::run_event_loop_until_quit();

    // Cleanup socket on exit
    #[cfg(unix)]
    {
        let socket_path = tiarun::ipc::get_socket_path();
        let _ = std::fs::remove_file(&socket_path);
    }

    result?;
    Ok(())
}
