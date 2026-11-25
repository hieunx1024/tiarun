use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher;
use tiarun::{AppWindow, LauncherItemData};

use crate::platform::{
    apply_skip_taskbar, configure_windows_borderless_transparent, init_platform_with_app_id,
    is_gnome_desktop,
};
use crate::ui_state::{ensure_selection_visible, populate_ui_icons};

pub(crate) fn run_dmenu_mode(prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let raw_lines: Vec<String> = stdin.lock().lines().filter_map(|l| l.ok()).collect();
    if raw_lines.is_empty() {
        return Ok(());
    }

    init_platform_with_app_id("tiarun")?;
    let ui = AppWindow::new()?;
    if is_gnome_desktop() {
        ui.set_cfg_fixed_width(true);
    }
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
    populate_ui_icons(&ui, &icon_resolver);

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

    configure_windows_borderless_transparent(&ui);

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
                    cpu_text: "".into(),
                    ram_text: "".into(),
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
