#[cfg(all(unix, not(target_os = "macos")))]
mod linux_wm;
#[cfg(target_os = "windows")]
mod windows_wm;

use std::sync::Arc;
use std::time::{Duration, Instant};

use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use tiarun::config::Config;
use tiarun::AppWindow;

static LAST_WINDOW_SIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "windows")]
pub(crate) use windows_wm::{configure_windows_borderless_transparent, start_windows_global_hotkey};

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) use linux_wm::{log_blur_backend_status, start_hyprland_focus_watcher};

#[cfg(not(target_os = "windows"))]
pub(crate) fn configure_windows_borderless_transparent(_ui: &AppWindow) {}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub(crate) fn log_blur_backend_status(_ui: &AppWindow) {}

pub(crate) fn is_launcher_focused(ui: &AppWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_wm::is_launcher_focused(ui)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_wm::is_launcher_focused(ui)
    }
    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos")))))]
    {
        ui.window().with_winit_window(|w| w.has_focus()).unwrap_or(false)
    }
}

pub(crate) fn apply_skip_taskbar(winit_window: &i_slint_backend_winit::winit::window::Window) {
    #[cfg(target_os = "windows")]
    windows_wm::apply_windows_skip_taskbar(winit_window);

    #[cfg(all(unix, not(target_os = "macos")))]
    linux_wm::apply_x11_skip_taskbar(winit_window);
}

pub(crate) fn focus_launcher_window(ui: &AppWindow) {
    ui.window().with_winit_window(|w| {
        w.set_title("TiaRun");
        w.set_transparent(true);
        w.set_decorations(false);
        apply_skip_taskbar(w);
        #[cfg(target_os = "windows")]
        windows_wm::focus_windows_window(w);
        w.focus_window();
    });

    #[cfg(all(unix, not(target_os = "macos")))]
    linux_wm::focus_wm_window();
}

pub(crate) fn resize_window_for_state(ui: &AppWindow) {
    let width = ui.get_launcher_width() as i32;
    let height = ui.get_launcher_height() as i32;

    let packed = ((width as u64) << 32) | (height as u32 as u64);
    if LAST_WINDOW_SIZE.swap(packed, std::sync::atomic::Ordering::SeqCst) == packed {
        return;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    if ui.window().is_visible() {
        if let Some(()) = linux_wm::resize_sway_launcher(width, height) {
            return;
        }
    }

    ui.window().set_size(slint::LogicalSize::new(width as f32, height as f32));
    ui.window().with_winit_window(move |winit_window| {
        let _ = winit_window.request_inner_size(i_slint_backend_winit::winit::dpi::LogicalSize::new(width as f32, height as f32));
    });

    #[cfg(target_os = "windows")]
    {
        let _ = height;
        reposition_and_anchor_window(ui);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    linux_wm::post_resize_wm(ui, width, height);
}

pub(crate) fn reposition_and_anchor_window(ui: &AppWindow) {
    #[cfg(all(unix, not(target_os = "macos")))]
    if linux_wm::try_reposition_wm(ui) {
        return;
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
            windows_wm::set_windows_outer_position(winit_window, pos_x, pos_y);

            #[cfg(not(target_os = "windows"))]
            winit_window.set_outer_position(i_slint_backend_winit::winit::dpi::PhysicalPosition::new(pos_x, pos_y));
        }
    });
}

pub(crate) fn is_gnome_desktop() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_wm::is_gnome_desktop()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    false
}

pub(crate) fn show_launcher_window(ui: &AppWindow) -> Result<(), slint::PlatformError> {
    ui.show()
}

pub(crate) fn hide_launcher_window(ui: &AppWindow) {
    let _ = ui.hide();
}

pub(crate) fn show_and_focus_launcher(
    ui: &AppWindow,
    ui_weak: slint::Weak<AppWindow>,
    shown_instant: &Arc<std::sync::RwLock<Instant>>,
    query: &str,
) {
    let was_visible = ui.window().is_visible();
    if !was_visible {
        LAST_WINDOW_SIZE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    let cfg = Config::load_fast();
    ui.set_is_dark(cfg.theme.is_dark_cached());
    ui.set_cfg_compact_empty(cfg.theme.compact_empty_view.unwrap_or(true));
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }
    ui.set_search_text(query.into());
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
        if is_gnome_desktop() {
            // GNOME Mutter already animates window map/unmap; starting at opacity 0
            // on first Wayland surface map can leave the initial buffer transparent.
            ui.set_is_pop_visible(true);
        } else {
            animate_window_pop_in(ui_weak.clone());
        }
    }
    let _ = show_launcher_window(ui);
    focus_launcher_window(ui);
    ui.invoke_focus_search();
    if !query.is_empty() {
        ui.invoke_move_cursor_to_end();
    }
    if !was_visible {
        let initial_query_owned = query.to_string();
        slint::Timer::single_shot(Duration::from_millis(30), move || {
            if let Some(ui) = ui_weak.upgrade().filter(|ui| ui.window().is_visible()) {
                reposition_and_anchor_window(&ui);
                focus_launcher_window(&ui);
                ui.set_is_pop_visible(true);
                ui.window().request_redraw();
                ui.invoke_focus_search();
                if !initial_query_owned.is_empty() && ui.get_search_text().as_str() == initial_query_owned {
                    ui.invoke_move_cursor_to_end();
                }
            }
        });
        if is_gnome_desktop() {
            let ui_weak_gnome = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(120), move || {
                if let Some(ui) = ui_weak_gnome.upgrade().filter(|ui| ui.window().is_visible()) {
                    let _ = show_launcher_window(&ui);
                    focus_launcher_window(&ui);
                    ui.window().request_redraw();
                    ui.invoke_focus_search();
                }
            });
        }
    }
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
pub(crate) fn init_platform_with_app_id(app_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let backend_config = std::env::var("SLINT_BACKEND").unwrap_or_default().to_lowercase();
    let renderer_name = backend_config.split_once('-').map(|(_, renderer)| renderer).filter(|r| !r.is_empty());

    let mut builder = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attrs| {
            attrs
                .with_title("TiaRun")
                .with_transparent(true)
                .with_decorations(false)
        });
    if let Some(name) = renderer_name {
        builder = builder.with_renderer_name(name);
    }

    // On GNOME Wayland, native Wayland xdg_toplevel windows have no protocol to
    // skip the dock/taskbar or self-center. Using XWayland when DISPLAY is
    // available preserves WAYLAND_DISPLAY for clipboard/extensions while letting
    // Mutter honor _NET_WM_STATE_SKIP_TASKBAR, ABOVE, and exact centering.
    #[cfg(all(target_os = "linux", feature = "x11"))]
    if is_gnome_desktop()
        && std::env::var_os("DISPLAY").is_some()
        && std::env::var_os("SLINT_BACKEND").is_none()
    {
        use i_slint_backend_winit::winit::platform::x11::EventLoopBuilderExtX11;
        let mut el_builder = i_slint_backend_winit::winit::event_loop::EventLoop::with_user_event();
        el_builder.with_any_thread(true);
        el_builder.with_x11();
        builder = builder.with_event_loop_builder(el_builder);
    }

    let backend = builder.build()?;
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
pub(crate) fn animate_window_pop_in(ui_weak: slint::Weak<AppWindow>) {
    if let Some(ui) = ui_weak.upgrade() {
        ui.set_is_pop_visible(false);
        #[cfg(all(unix, not(target_os = "macos")))]
        if linux_wm::schedule_wm_pop_in(&ui_weak) {
            return;
        }
    }
    slint::Timer::single_shot(Duration::from_millis(8), move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_is_pop_visible(true);
        }
    });
}
