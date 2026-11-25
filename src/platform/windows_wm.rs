use std::sync::Arc;
use std::thread;
use std::time::Instant;

use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use tiarun::AppWindow;

use super::{hide_launcher_window, show_and_focus_launcher};

pub(crate) fn is_launcher_focused(ui: &AppWindow) -> bool {
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

pub(crate) fn configure_windows_borderless_transparent(ui: &AppWindow) {
    ui.window().with_winit_window(|winit_window| {
        use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
        };

        if let Ok(handle) = winit_window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = win32_handle.hwnd.get() as HWND;

                unsafe {
                    // 1. Strip titlebar caption, system menu, and minimize/maximize buttons
                    // so Windows DWM never renders any native caption buttons into the launcher window.
                    let current_style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                    let stripped = current_style & !(WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU);
                    if stripped != current_style {
                        SetWindowLongPtrW(hwnd, GWL_STYLE, stripped as isize);
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                        );
                    }

                    // 2. Disable Windows 11 DWM outer rectangular border & rounding;
                    // Slint draws its own anti-aliased rounded liquid-glass pill directly into the transparent canvas.
                    let preference = DWMWCP_DONOTROUND as u32;
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

pub(crate) fn apply_windows_skip_taskbar(winit_window: &i_slint_backend_winit::winit::window::Window) {
    use i_slint_backend_winit::winit::platform::windows::WindowExtWindows;
    winit_window.set_skip_taskbar(true);
}

pub(crate) fn focus_windows_window(winit_window: &i_slint_backend_winit::winit::window::Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
    use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    winit_window.set_visible(true);
    if let Ok(handle) = winit_window.window_handle() {
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            unsafe {
                BringWindowToTop(h.hwnd.get() as _);
                SetForegroundWindow(h.hwnd.get() as _);
            }
        }
    }
}

pub(crate) fn set_windows_outer_position(
    winit_window: &i_slint_backend_winit::winit::window::Window,
    pos_x: i32,
    pos_y: i32,
) {
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

pub(crate) fn start_windows_global_hotkey(
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
