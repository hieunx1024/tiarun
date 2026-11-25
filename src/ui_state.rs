use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use slint::ComponentHandle;
use tiarun::config::Config;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher::{self, LauncherEngine, LauncherItem};
use tiarun::{AppWindow, LauncherItemData};

use crate::platform::resize_window_for_state;

pub(crate) struct FileSearchResults {
    pub matches: Vec<(LauncherItem, Vec<usize>)>,
    pub file_sizes: std::collections::HashMap<String, String>,
}

pub(crate) fn populate_ui_icons(ui: &AppWindow, icon_resolver: &IconResolver) {
    ui.set_settings_icon(icon_resolver.get_gear_icon());
    ui.set_nav_icon(icon_resolver.get_nav_icon());
    ui.set_enter_icon(icon_resolver.get_enter_icon());
    ui.set_search_icon(icon_resolver.get_search_icon());
    ui.set_folder_icon(icon_resolver.get_folder_icon());
    ui.set_scope_apps_icon(icon_resolver.get_scope_apps_icon());
    ui.set_scope_files_icon(icon_resolver.get_scope_files_icon());
    ui.set_scope_windows_icon(icon_resolver.get_scope_windows_icon());
}

pub(crate) fn apply_settings_to_ui(ui: &AppWindow, cfg: &Config) {
    ui.set_cfg_autostart(tiarun::config::is_autostart_enabled());
    ui.set_cfg_theme_mode(cfg.theme.mode.clone().into());
    ui.set_cfg_theme_opacity(cfg.theme.opacity);
    ui.set_is_dark(cfg.theme.is_dark());
    ui.set_in_settings_mode(true);
    ui.set_settings_status("".into());
    resize_window_for_state(ui);
    ui.invoke_focus_settings();
}

pub(crate) fn format_action_hints(
    selected_item: Option<&launcher::LauncherItem>,
    total_count: usize,
    selected_idx: usize,
) -> (&'static str, String) {
    let hint = if let Some(item) = selected_item {
        match item.item_type {
            launcher::ItemType::App => "↵ Open  •  Ctrl+C Copy  •  Alt+T Terminal",
            launcher::ItemType::File => "↵ Open  •  Ctrl+C Copy  •  Alt+C Copy Path  •  Alt+T Terminal",
            launcher::ItemType::Dir => "↵ Open  •  → Browse  •  Ctrl+C Copy  •  Alt+C Copy Path",
            launcher::ItemType::Window => "↵ Focus Window  •  Alt+K Kill Process  •  Esc Close",
            launcher::ItemType::Clipboard => "↵ Copy  •  Esc Close",
            launcher::ItemType::Calc => "↵ Copy to Clipboard",
            launcher::ItemType::Shell => "↵ Run in Terminal",
            launcher::ItemType::System => "↵ Execute Action",
            launcher::ItemType::Theme => "↵ Apply Theme",
            launcher::ItemType::Mode => "↵ Open Mode  •  Esc Close",
            launcher::ItemType::WebSearch => "↵ Search Web  •  Esc Close",
            launcher::ItemType::Dmenu => "↵ Select  •  Esc Close",
        }
    } else {
        "@ Modes  •  ! Shell  •  Ctrl+, Settings"
    };

    let count_str = if total_count > 0 {
        format!(
            "{}/{}  •  Tab Scope  •  @ Modes  •  F1 Help",
            (selected_idx + 1).min(total_count),
            total_count
        )
    } else {
        "Tab Scope  •  F1 Help".to_string()
    };

    (hint, count_str)
}

pub(crate) fn update_action_hints(
    ui: &AppWindow,
    selected_item: Option<&launcher::LauncherItem>,
    total_count: usize,
    selected_idx: usize,
) {
    let (hint, count_str) = format_action_hints(selected_item, total_count, selected_idx);
    ui.set_action_hint_text(hint.into());
    ui.set_action_item_count(count_str.into());
}

pub(crate) fn effective_search_query(ui: &AppWindow, engine: &LauncherEngine, query: &str) -> String {
    let trimmed = query.trim();

    // Two-way synchronization between search text prefix and active scope chip
    let mut active_scope = ui.get_active_scope().to_string();
    if active_scope.is_empty() {
        active_scope = "apps".to_string();
        ui.set_active_scope("apps".into());
    }
    if trimmed.starts_with("@f") || trimmed.starts_with("@file") {
        active_scope = "files".to_string();
        ui.set_active_scope("files".into());
    } else if trimmed.starts_with("@w") || trimmed.starts_with("@win") {
        active_scope = "windows".to_string();
        ui.set_active_scope("windows".into());
    } else if trimmed.starts_with('@') && trimmed != "@" {
        active_scope = "apps".to_string();
        ui.set_active_scope("apps".into());
    }

    // When the user has not typed an explicit '@', '!', ':' prefix, route search based on active scope
    if trimmed.is_empty() {
        match active_scope.as_str() {
            "files" => "@f".to_string(),
            "windows" => "@w".to_string(),
            _ => "".to_string(),
        }
    } else if !trimmed.starts_with(['@', '!', ':'])
        && trimmed != "?"
        && engine.resolve_path_search(trimmed).is_none()
    {
        match active_scope.as_str() {
            "files" => format!("@f {}", trimmed),
            "windows" => format!("@w {}", trimmed),
            _ => query.to_string(),
        }
    } else {
        query.to_string()
    }
}

pub(crate) fn populate_items(
    ui: &AppWindow,
    engine: &LauncherEngine,
    icon_resolver: &IconResolver,
    query: &str,
) -> Vec<LauncherItem> {
    let effective_query = effective_search_query(ui, engine, query);
    render_search_results(ui, engine, icon_resolver, query, &effective_query, None)
}

pub(crate) fn file_size_label(path: &str) -> String {
    std::fs::metadata(path)
        .map(|metadata| launcher::format_file_size(metadata.len()))
        .unwrap_or_else(|_| "File".to_string())
}

pub(crate) fn render_search_results(
    ui: &AppWindow,
    engine: &LauncherEngine,
    icon_resolver: &IconResolver,
    query: &str,
    effective_query: &str,
    results: Option<FileSearchResults>,
) -> Vec<LauncherItem> {
    let trimmed = query.trim();
    let effective_trimmed = effective_query.trim();

    let is_file_mode = effective_trimmed.starts_with("@f") || effective_trimmed.starts_with("@file");
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
    let mode_content = if is_file_mode {
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
    ui.set_is_file_mode(is_file_mode);
    ui.set_is_shell_mode(is_shell_mode);

    let (results, file_sizes) = if let Some(results) = results {
        (results.matches, Some(results.file_sizes))
    } else {
        (engine.search_fast(effective_query), None)
    };
    let count = results.len();
    ui.set_has_results(count > 0);

    let (mode_title, item_count_suffix) = if is_file_mode {
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
            let is_gnome_wl_blocked = std::env::var_os("WAYLAND_DISPLAY").is_some()
                && crate::platform::is_gnome_desktop()
                && !tiarun::window_switcher::is_gnome_wayland_listing_active();
            ui.set_empty_state_title("No matching windows found".into());
            ui.set_empty_state_subtitle(if is_gnome_wl_blocked {
                "Log out and back in to activate TiaRun's GNOME Shell extension (or enable 'Window Calls')"
            } else if has_mode_content {
                "Try a different window title or app name"
            } else {
                "Make sure target application window is running"
            }.into());
            ui.set_is_empty_folder(false);
        } else if is_clip_mode {
            let is_gnome_wl = std::env::var_os("WAYLAND_DISPLAY").is_some()
                && crate::platform::is_gnome_desktop();
            ui.set_empty_state_title("Clipboard is empty".into());
            ui.set_empty_state_subtitle(if is_gnome_wl {
                "GNOME Wayland restricts background clipboard reading when launcher is hidden"
            } else {
                "Copy some text to see it appear in clipboard history"
            }.into());
            ui.set_is_empty_folder(false);
        } else if is_shell_mode {
            ui.set_empty_state_title("Type a command to run".into());
            ui.set_empty_state_subtitle("e.g. ! ls -la   or   ! echo hello | wc -l".into());
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

    // Cap rendered rows when query is empty so Windows doesn't synchronously
    // extract hundreds of Start Menu shell icons on popup open.
    let render_limit = if trimmed.is_empty() || effective_trimmed == "@w" || effective_trimmed == "@win" {
        (engine.config.search.max_results * 4).clamp(24, 40)
    } else {
        (engine.config.search.max_results + 3).clamp(8, 20)
    };
    let items_iter = results.into_iter().take(render_limit);

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
                let tag = if let Some(bytes) = item.file_size {
                    launcher::format_file_size(bytes)
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
                let class_hint = item
                    .description
                    .as_deref()
                    .filter(|s| !s.is_empty() && !s.starts_with("PID:"));
                let app_hint = class_hint.unwrap_or(&item.name);
                let icon = icon_resolver.resolve_icon(item.icon.as_deref(), app_hint, "");
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
            launcher::ItemType::Mode => {
                (None, "Mode".to_string())
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
            launcher::ItemType::Mode => "mode",
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
                item.exec_or_path.clone()
            }
            launcher::ItemType::Window => {
                let class_desc = item.description.as_deref().unwrap_or("").trim();
                match (class_desc.is_empty() || class_desc.starts_with("PID:"), item.pid) {
                    (false, Some(pid)) => format!("{}  •  PID {}", class_desc, pid),
                    (false, None) => class_desc.to_string(),
                    (true, Some(pid)) => format!("PID {}", pid),
                    (true, None) => item.exec_or_path.clone(),
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

        let cpu_text = if item.item_type == launcher::ItemType::Window {
            item.cpu_percent
                .map(|c| format!("CPU {}", tiarun::window_switcher::format_cpu(c)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let ram_text = if item.item_type == launcher::ItemType::Window {
            item.memory_bytes
                .map(|b| format!("RAM {}", tiarun::window_switcher::format_memory(b)))
                .unwrap_or_default()
        } else {
            String::new()
        };

        slint_items.push(LauncherItemData {
            name: item.name.clone().into(),
            category: category.into(),
            item_type: item_type_str.into(),
            has_icon,
            icon: icon_img,
            exec_or_path: item.exec_or_path.clone().into(),
            subtitle: subtitle.into(),
            cpu_text: cpu_text.into(),
            ram_text: ram_text.into(),
        });

        current_items.push(item);
    }

    let model = std::rc::Rc::new(slint::VecModel::from(slint_items));
    ui.set_items(model.into());

    update_action_hints(ui, current_items.first(), current_items.len(), 0);

    current_items
}

pub(crate) fn apply_theme_command(ui: &AppWindow, cmd: &str) {
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

pub(crate) fn update_preview_for_item(
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
    let cache = preview_cache.clone();
    let current_image_gen = current_image_gen.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(90));
        if current_image_gen.load(Ordering::Relaxed) != generation {
            return;
        }

        let preview = cache.get_or_create_preview(&path);
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
            let dir_items: Vec<tiarun::PreviewDirItem> = if preview.preview_lang == "DIRECTORY"
                && preview.text_snippet != "Empty folder"
            {
                let resolver = IconResolver::new();
                let folder_icon = resolver
                    .resolve_file_type_icon(std::path::Path::new(&path), launcher::ItemType::Dir)
                    .unwrap_or_default();
                let file_icon = resolver
                    .resolve_file_type_icon(std::path::Path::new("file.txt"), launcher::ItemType::File)
                    .unwrap_or_default();
                preview
                    .text_snippet
                    .lines()
                    .map(|line| {
                        if let Some(name) = line.strip_prefix("📁 ") {
                            tiarun::PreviewDirItem {
                                name: name.into(),
                                icon: folder_icon.clone(),
                                has_icon: true,
                            }
                        } else if let Some(name) = line.strip_prefix("📄 ") {
                            tiarun::PreviewDirItem {
                                name: name.into(),
                                icon: file_icon.clone(),
                                has_icon: true,
                            }
                        } else {
                            tiarun::PreviewDirItem {
                                name: line.into(),
                                icon: slint::Image::default(),
                                has_icon: false,
                            }
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            ui.set_preview_dir_items(std::rc::Rc::new(slint::VecModel::from(dir_items)).into());
            ui.set_preview_title(preview.title.into());
            ui.set_preview_type(preview.preview_type.as_str().into());
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

pub(crate) fn ensure_selection_visible(ui: &AppWindow, selected_index: i32) {
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

pub(crate) fn move_selection(
    ui: &AppWindow,
    current_results: &Arc<std::sync::RwLock<Vec<LauncherItem>>>,
    preview_cache: &tiarun::file_preview::PreviewCache,
    current_image_gen: &Arc<std::sync::atomic::AtomicU64>,
    step: i32,
) {
    if step < 0 && ui.get_search_text().is_empty() && ui.get_selected_index() <= 0 && ui.get_is_expanded() {
        ui.set_is_expanded(false);
        ui.set_show_preview(false);
        resize_window_for_state(ui);
        return;
    }
    if step > 0 && ui.get_search_text().is_empty() && !ui.get_is_expanded() {
        ui.set_is_expanded(true);
        resize_window_for_state(ui);
        return;
    }

    let total = current_results.read().map(|lock| lock.len() as i32).unwrap_or(0);
    if total == 0 {
        return;
    }

    let cur = ui.get_selected_index();
    let next = if step < 0 {
        if cur <= 0 { total - 1 } else { cur - 1 }
    } else if cur + 1 >= total {
        0
    } else {
        cur + 1
    };

    ui.set_selected_index(next);
    ensure_selection_visible(ui, next);
    if let Ok(lock) = current_results.read() {
        update_action_hints(ui, lock.get(next as usize), total as usize, next as usize);
    }

    let query = ui.get_search_text();
    let trimmed = query.trim();
    let scope = ui.get_active_scope().to_string();
    let is_file_scope = scope == "files"
        || trimmed.starts_with("@f")
        || trimmed.starts_with("@file")
        || trimmed.starts_with('/')
        || trimmed.starts_with('~')
        || (cfg!(windows) && trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':')
        || trimmed.starts_with('\\');
    let is_showing_list = ui.get_is_showing_list();
    if let Ok(lock) = current_results.read() {
        let item = lock.get(next as usize);
        let should_preview = is_showing_list && (is_file_scope
            || item.map(|it| it.item_type == launcher::ItemType::File || it.item_type == launcher::ItemType::Dir).unwrap_or(false));
        if should_preview {
            update_preview_for_item(ui, ui.as_weak(), item, preview_cache, current_image_gen);
        } else {
            ui.set_show_preview(false);
        }
    }
    resize_window_for_state(ui);
}

pub(crate) fn strip_scope_prefixes(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("@file ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@f ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@win ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("@w ") {
        rest.to_string()
    } else {
        raw.to_string()
    }
}

pub(crate) fn switch_to_scope(ui: &AppWindow, target_scope: &str) -> String {
    ui.set_active_scope(target_scope.into());
    ui.set_is_expanded(true);

    let raw_text = ui.get_search_text().to_string();
    let mut clean_query = strip_scope_prefixes(&raw_text);

    // If moving to a non-files scope (e.g. windows, apps) and the query is a directory path
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

pub(crate) fn step_into_directory(
    ui: &AppWindow,
    current_results: &Arc<std::sync::RwLock<Vec<LauncherItem>>>,
) -> bool {
    let sel_idx = ui.get_selected_index() as usize;
    let item_opt = current_results.read().ok().and_then(|lock| lock.get(sel_idx).cloned());

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
            let sep = std::path::MAIN_SEPARATOR;
            if !path.ends_with('/') && !path.ends_with('\\') {
                path.push(sep);
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

pub(crate) fn step_out_directory(ui: &AppWindow) -> bool {
    let search_text = ui.get_search_text().to_string();
    let is_file_mode = search_text.starts_with("@f ") || search_text.starts_with("@file ");
    let is_path_mode = search_text.starts_with('/')
        || search_text.starts_with('~')
        || (cfg!(windows) && search_text.len() >= 2 && search_text.as_bytes()[1] == b':')
        || search_text.starts_with('\\');
    if (is_file_mode || is_path_mode) && (search_text.ends_with('/') || search_text.ends_with('\\')) {
        let prefix_len = if search_text.starts_with("@file ") {
            6
        } else if search_text.starts_with("@f ") {
            3
        } else {
            0
        };
        let path_part = &search_text[prefix_len..];
        let trimmed_path = path_part.trim_end_matches(|c| c == '/' || c == '\\');
        let new_search_text = if let Some(pos) = trimmed_path.rfind(|c| c == '/' || c == '\\') {
            let parent_path = &trimmed_path[..=pos];
            format!("{}{}", &search_text[..prefix_len], parent_path)
        } else {
            search_text[..prefix_len].to_string()
        };
        ui.set_search_text(new_search_text.clone().into());
        ui.invoke_move_cursor_to_end();
        ui.invoke_search_text_changed(new_search_text.into());
        return true;
    }
    false
}

/// Warms the icon cache a few items at a time via `slint::Timer::single_shot`,
/// re-scheduling itself until the whole list is done. Runs entirely on the
/// event-loop thread, so it keeps the window responsive between chunks instead
/// of resolving every icon in one blocking call at startup.
pub(crate) fn warm_icon_cache_chunked(
    ui_weak: slint::Weak<AppWindow>,
    icon_resolver: Arc<IconResolver>,
    apps: Vec<LauncherItem>,
    start: usize,
) {
    warm_icon_cache_chunked_inner(ui_weak, icon_resolver, apps, start, 0);
}

fn warm_icon_cache_chunked_inner(
    ui_weak: slint::Weak<AppWindow>,
    icon_resolver: Arc<IconResolver>,
    apps: Vec<LauncherItem>,
    start: usize,
    wait_attempts: usize,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    // Pause icon warming while launcher window is open/interactive so UI stays snappy
    if ui.window().is_visible() {
        slint::Timer::single_shot(Duration::from_millis(150), move || {
            warm_icon_cache_chunked_inner(ui_weak, icon_resolver, apps, start, wait_attempts);
        });
        return;
    }
    // Wait briefly for the Unix background icon-theme scan so full theme icons are cached
    if !icon_resolver.is_indexing_done() && wait_attempts < 50 {
        slint::Timer::single_shot(Duration::from_millis(100), move || {
            warm_icon_cache_chunked_inner(ui_weak, icon_resolver, apps, start, wait_attempts + 1);
        });
        return;
    }
    let chunk_size = if cfg!(target_os = "windows") { 2 } else { 4 };
    let end = (start + chunk_size).min(apps.len());
    for app in &apps[start..end] {
        let _ = icon_resolver.resolve_icon(app.icon.as_deref(), &app.name, &app.exec_or_path);
    }
    if end < apps.len() {
        slint::Timer::single_shot(Duration::from_millis(15), move || {
            warm_icon_cache_chunked_inner(ui_weak, icon_resolver, apps, end, wait_attempts);
        });
    }
}

/// Wires the background file/content search worker thread and search/scope UI callbacks.
/// File and content searches can touch the filesystem on every keystroke; this worker
/// coalesces pending queries and only publishes results for the latest generation.
pub(crate) fn wire_search_worker(
    ui: &AppWindow,
    engine: Arc<LauncherEngine>,
    icon_resolver: Arc<IconResolver>,
    current_results: Arc<std::sync::RwLock<Vec<LauncherItem>>>,
    preview_cache: tiarun::file_preview::PreviewCache,
    current_image_gen: Arc<std::sync::atomic::AtomicU64>,
    window_refresh: WindowRefreshTrigger,
) {
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
                (worker_engine.config.search.max_results * 4).clamp(24, 40)
            } else {
                (worker_engine.config.search.max_results + 3).clamp(8, 20)
            };
            let file_sizes = matches
                .iter()
                .take(max_rows)
                .take_while(|_| !cancelled())
                .filter(|(item, _)| item.item_type == launcher::ItemType::File && item.file_size.is_none())
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

    let schedule = move |ui: &AppWindow, text: &str, keep_expanded: bool| {
        let id = generation.fetch_add(1, Ordering::Relaxed) + 1;
        let effective = effective_search_query(ui, &engine, text);
        let scope = ui.get_active_scope();
        let file_scope = scope.as_str() == "files";
        ui.set_is_expanded(keep_expanded || !text.is_empty() || file_scope);
        resize_window_for_state(ui);

        let search_files = ((effective.trim().starts_with("@f") || effective.trim().starts_with("@file")) && file_scope)
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
            let is_win = effective.trim().starts_with("@w") || effective.trim().starts_with("@win");
            if is_win {
                let ui_weak = ui.as_weak();
                let window_refresh = window_refresh.clone();
                slint::Timer::single_shot(Duration::from_millis(40), move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        if ui.window().is_visible() && !ui.get_in_settings_mode() && is_window_mode_active(&ui) {
                            window_refresh.spawn(&ui);
                        }
                    }
                });
            }
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

/// Wires "!" shell command history recall (Up/Down/reset) on `AppWindow`.
pub(crate) fn wire_shell_history_callbacks(ui: &AppWindow, engine: Arc<LauncherEngine>) {
    use std::cell::Cell;
    use std::rc::Rc;

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
    ui.on_shell_history_reset(move || {
        shell_history_cursor.set(-1);
    });
}

pub(crate) fn is_window_mode_active(ui: &AppWindow) -> bool {
    let text = ui.get_search_text().to_string();
    let trimmed = text.trim();
    if trimmed.starts_with("@w") || trimmed.starts_with("@win") {
        return true;
    }
    ui.get_active_scope().as_str() == "windows"
        && !trimmed.starts_with(['@', '!', ':'])
        && trimmed != "?"
}

pub(crate) fn apply_window_refresh_results(
    ui: &AppWindow,
    engine: &LauncherEngine,
    icon_resolver: &IconResolver,
    current_results: &Arc<std::sync::RwLock<Vec<LauncherItem>>>,
    query: &str,
    effective_query: &str,
    results: Option<FileSearchResults>,
) {
    let prev_idx = ui.get_selected_index().max(0) as usize;
    let prev_exec = current_results
        .read()
        .ok()
        .and_then(|lock| lock.get(prev_idx).map(|item| item.exec_or_path.clone()));

    let items = render_search_results(ui, engine, icon_resolver, query, effective_query, results);
    let total = items.len();
    let new_idx = if total == 0 {
        0
    } else if let Some(ref target_exec) = prev_exec {
        items
            .iter()
            .position(|item| &item.exec_or_path == target_exec)
            .unwrap_or_else(|| prev_idx.min(total - 1))
    } else {
        prev_idx.min(total - 1)
    };

    ui.set_selected_index(new_idx as i32);
    if total > 0 {
        ensure_selection_visible(ui, new_idx as i32);
    }
    update_action_hints(ui, items.get(new_idx), total, new_idx);
    if let Ok(mut lock) = current_results.write() {
        *lock = items;
    }
}

#[derive(Clone)]
pub(crate) struct WindowRefreshTrigger {
    engine: Arc<LauncherEngine>,
    tx: std::sync::mpsc::Sender<(String, String, String, Vec<(LauncherItem, Vec<usize>)>)>,
    in_flight: Arc<std::sync::atomic::AtomicBool>,
}

impl WindowRefreshTrigger {
    /// Refreshes the `@w` window list off the UI thread so periodic stats polling
    /// and post-kill refreshes never block the Slint event loop.
    pub(crate) fn spawn(&self, ui: &AppWindow) {
        use std::sync::atomic::Ordering;

        if self.in_flight.swap(true, Ordering::AcqRel) {
            return;
        }

        let query = ui.get_search_text().to_string();
        let scope = ui.get_active_scope().to_string();
        let effective = effective_search_query(ui, &self.engine, &query);
        let ui_weak = ui.as_weak();
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let in_flight = self.in_flight.clone();

        std::thread::spawn(move || {
            let matches = engine.search(&effective);
            in_flight.store(false, Ordering::Release);
            if tx.send((query, scope, effective, matches)).is_ok() {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.invoke_window_refresh_finished();
                    }
                });
            }
        });
    }
}

pub(crate) fn wire_window_refresh_worker(
    ui: &AppWindow,
    engine: Arc<LauncherEngine>,
    icon_resolver: Arc<IconResolver>,
    current_results: Arc<std::sync::RwLock<Vec<LauncherItem>>>,
) -> WindowRefreshTrigger {
    let (tx, rx) = std::sync::mpsc::channel::<(String, String, String, Vec<(LauncherItem, Vec<usize>)>)>();
    let ui_weak = ui.as_weak();
    let engine_ui = engine.clone();

    ui.on_window_refresh_finished(move || {
        let Some((query, scope, effective, matches)) = rx.try_iter().last() else {
            return;
        };
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if ui.window().is_visible()
            && !ui.get_in_settings_mode()
            && is_window_mode_active(&ui)
            && ui.get_search_text() == query.as_str()
            && ui.get_active_scope() == scope.as_str()
        {
            let results = FileSearchResults {
                matches,
                file_sizes: std::collections::HashMap::new(),
            };
            apply_window_refresh_results(
                &ui,
                &engine_ui,
                &icon_resolver,
                &current_results,
                &query,
                &effective,
                Some(results),
            );
        }
    });

    WindowRefreshTrigger {
        engine,
        tx,
        in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::format_action_hints;
    use tiarun::launcher::{ItemType, LauncherItem};

    #[test]
    fn test_action_hints_always_surface_help_and_modes() {
        let app = LauncherItem::new(
            "Firefox".to_string(),
            "firefox".to_string(),
            ItemType::App,
            None,
            false,
            None,
        );
        let (left, right) = format_action_hints(Some(&app), 12, 0);
        assert!(left.contains("↵ Open"));
        assert!(right.starts_with("1/12"));
        assert!(right.contains("Tab Scope"));
        assert!(right.contains("@ Modes"));
        assert!(right.contains("F1 Help"));

        let (empty_left, empty_right) = format_action_hints(None, 0, 0);
        assert!(empty_left.contains("@ Modes"));
        assert!(empty_left.contains("! Shell"));
        assert!(empty_left.contains("Ctrl+, Settings"));
        assert!(empty_right.contains("Tab Scope"));
        assert!(empty_right.contains("F1 Help"));

        let win = LauncherItem::new(
            "Terminal".to_string(),
            "sway:1".to_string(),
            ItemType::Window,
            None,
            false,
            None,
        );
        let (win_left, _) = format_action_hints(Some(&win), 1, 0);
        assert!(win_left.contains("Alt+K Kill Process"));
        assert!(!win_left.contains("Ctrl+K"));

        let clip = LauncherItem::new(
            "copied text".to_string(),
            "copied text".to_string(),
            ItemType::Clipboard,
            None,
            false,
            None,
        );
        let (clip_left, _) = format_action_hints(Some(&clip), 1, 0);
        assert!(clip_left.contains("↵ Copy"));
        assert!(!clip_left.contains("Paste"));
    }
}
