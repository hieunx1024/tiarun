use std::path::Path;

use fuzzy_matcher::FuzzyMatcher;
use rayon::prelude::*;

use crate::calc;
use crate::history::HistoryManager;

use super::text::{parse_command_line, remove_vietnamese_accents, strip_vendor_prefix, url_encode_query};
use super::{ItemType, LauncherEngine, LauncherItem};

impl LauncherEngine {
    /// Performs high-performance fuzzy matching and ranking of items.
    /// Mặc định: Chỉ tìm ứng dụng & tính toán (cực nhanh, không lẫn file).
    /// Khi bắt đầu bằng `@f ` hoặc `@file `: Chuyển sang chế độ tìm kiếm file & thư mục.
    pub fn search(&self, query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        self.search_until_cancelled(query, &|| false)
    }

    /// UI-thread fast search: skips blocking `/proc` (`smaps_rollup`) scans in `@w`
    /// window mode and uses any recently cached metrics instead.
    pub fn search_fast(&self, query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        let trimmed_query = query.trim();
        let is_win_mode = trimmed_query.starts_with("@w ")
            || trimmed_query.starts_with("@win ")
            || trimmed_query == "@w"
            || trimmed_query == "@win";
        if is_win_mode {
            return self.search_windows_with(trimmed_query, crate::window_switcher::get_open_windows_fast());
        }
        self.search_until_cancelled(query, &|| false)
    }

    pub fn search_until_cancelled(
        &self,
        query: &str,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        let trimmed_query = query.trim();

        // 0. Shell Command Mode (! <command>)
        if trimmed_query.starts_with('!') {
            return self.search_shell(trimmed_query);
        }

        // 0.5 Help ("?")
        if trimmed_query == "?" {
            return Self::search_help();
        }

        // 1. Window Switcher Mode (@w or @win)
        let is_win_mode = trimmed_query.starts_with("@w ")
            || trimmed_query.starts_with("@win ")
            || trimmed_query == "@w"
            || trimmed_query == "@win";
        if is_win_mode {
            return self.search_windows(trimmed_query);
        }

        // 2. Clipboard History Mode (@c or @clip)
        let is_clip_mode = trimmed_query.starts_with("@c ")
            || trimmed_query.starts_with("@clip ")
            || trimmed_query == "@c"
            || trimmed_query == "@clip";
        if is_clip_mode {
            return self.search_clipboard(trimmed_query);
        }

        // 3. System / Power Actions (@sys or @power or keywords)
        let is_sys_mode = trimmed_query.starts_with("@sys ")
            || trimmed_query.starts_with("@power ")
            || trimmed_query == "@sys"
            || trimmed_query == "@power";
        let is_sys_keyword = [
            "lock", "shutdown", "power off", "restart", "reboot", "sleep", "suspend", "logout",
            "log out",
        ]
        .iter()
        .any(|&k| trimmed_query.eq_ignore_ascii_case(k));

        let mut results = Vec::new();
        if is_sys_mode || (is_sys_keyword && !trimmed_query.is_empty()) {
            results.extend(self.search_system(trimmed_query, is_sys_mode));
            if is_sys_mode {
                return results;
            }
        }

        // 4. File Search Mode (@f or @file)
        let is_file_mode = trimmed_query.starts_with("@f ")
            || trimmed_query.starts_with("@file ")
            || trimmed_query == "@f"
            || trimmed_query == "@file";
        if is_file_mode {
            let shallow_files_guard = self.shallow_files.read().unwrap_or_else(|e| e.into_inner());
            let history_guard = self.history.read().unwrap_or_else(|e| e.into_inner());
            return self.search_files_mode(
                trimmed_query,
                &shallow_files_guard,
                &history_guard,
                cancelled,
            );
        }

        // 5. Theme & Opacity Mode (@theme, @mode, @opacity)
        if trimmed_query.starts_with("@theme")
            || trimmed_query.starts_with("@mode")
            || trimmed_query.starts_with("@opacity")
        {
            return self.search_theme(trimmed_query);
        }

        // 6. Custom Script Plugins (~/.config/tiarun/plugins/) & @ mode index
        if trimmed_query.starts_with('@') {
            if let Some(plugin_results) = self.search_plugins(trimmed_query) {
                return plugin_results;
            }
        }

        // 7. Offline Unit/Currency Converter & Calculator
        results.extend(Self::search_calc_and_convert(trimmed_query));

        // 8. Default App Search (never mixed with shallow files)
        let history_guard = self.history.read().unwrap_or_else(|e| e.into_inner());
        self.search_apps(trimmed_query, results, &history_guard, cancelled)
    }

    /// Shell Command Mode (`! <command>`): run an arbitrary shell command in a terminal.
    fn search_shell(&self, trimmed_query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        let cmd = trimmed_query[1..].trim_start();
        if cmd.is_empty() {
            return Vec::new();
        }
        vec![(
            LauncherItem::new(
                cmd.to_string(),
                cmd.to_string(),
                ItemType::Shell,
                Some("Press Enter to run in terminal".to_string()),
                true,
                None,
            ),
            Vec::new(),
        )]
    }

    /// Help (`?`): lists every keyboard shortcut and search mode.
    fn search_help() -> Vec<(LauncherItem, Vec<usize>)> {
        const SHORTCUTS: &[(&str, &str)] = &[
            ("Ctrl+Alt+Space", "Toggle the launcher window (global shortcut)"),
            ("F1 / ?", "Show this keyboard shortcuts & help cheatsheet"),
            ("Tab / Shift+Tab", "Cycle search scope (Apps ➔ Files ➔ Windows)"),
            ("→ / ←", "Browse into selected folder / step out to parent folder"),
            ("Enter", "Open / run / copy the selected result"),
            ("Up / Down", "Navigate results (or recall past \"!\" commands in shell mode)"),
            ("Backspace", "Delete character; at folder root, go to parent folder"),
            ("Ctrl+C", "Copy selected item (app, file/image, or result)"),
            ("Alt+C / Ctrl+Shift+C", "Copy file path or result to clipboard"),
            ("Alt+T", "Open the selected item's folder in a terminal"),
            ("Alt+K", "Kill the selected process (in @w window mode)"),
            ("Ctrl+,", "Open preferences"),
            ("Esc", "Close preferences, or hide the launcher"),
            ("@", "Browse all search modes and custom script plugins"),
            ("@f <query>", "Search files and folders"),
            ("@w <query>", "Switch between open windows"),
            ("@c <query>", "Browse clipboard history"),
            ("@sys", "Lock, sleep, restart, shutdown"),
            ("@theme", "Switch theme mode / opacity"),
            ("! <command>", "Run a shell command in a terminal"),
            ("5 * 12, 0xFF, ...", "Inline calculator (also unit/currency conversion)"),
            ("anything else", "Search apps; falls back to a web search"),
        ];
        SHORTCUTS
            .iter()
            .map(|(keys, desc)| {
                (
                    LauncherItem::new(
                        (*keys).to_string(),
                        (*keys).to_string(),
                        ItemType::Calc,
                        Some((*desc).to_string()),
                        false,
                        None,
                    ),
                    Vec::new(),
                )
            })
            .collect()
    }

    /// Window Switcher Mode (`@w` / `@win`).
    fn search_windows(&self, trimmed_query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        self.search_windows_with(trimmed_query, crate::window_switcher::get_open_windows())
    }

    fn search_windows_with(
        &self,
        trimmed_query: &str,
        windows: Vec<crate::window_switcher::WindowItem>,
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        let win_query = if trimmed_query.starts_with("@win ") {
            trimmed_query[5..].trim()
        } else if trimmed_query.starts_with("@w ") {
            trimmed_query[3..].trim()
        } else {
            ""
        };
        let win_query_normalized = remove_vietnamese_accents(win_query);
        let items: Vec<LauncherItem> = windows
            .into_iter()
            .map(|w| {
                let icon = self.resolve_window_icon(&w);
                LauncherItem::new(w.title, w.id, ItemType::Window, Some(w.class_name), false, icon)
                    .with_process_stats(w.pid, w.memory_bytes, w.cpu_percent)
            })
            .collect();

        if win_query.is_empty() {
            return items.into_iter().map(|item| (item, Vec::new())).collect();
        }

        let mut matches: Vec<((LauncherItem, Vec<usize>), i64)> = items
            .par_iter()
            .filter_map(|item| {
                let match_res = self.match_precomputed(
                    &item.name,
                    &item.normalized_name,
                    win_query,
                    &win_query_normalized,
                );
                let final_res = if match_res.is_some() {
                    match_res
                } else if let Some(ref desc) = item.description {
                    self.match_item(desc, win_query)
                } else {
                    None
                };
                final_res.map(|(score, indices)| ((item.clone(), indices), score))
            })
            .collect();
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// Resolves the best matching icon hint or path for an open window.
    pub(super) fn resolve_window_icon(&self, win: &crate::window_switcher::WindowItem) -> Option<String> {
        let exact_class = (!win.class_name.is_empty() && !win.class_name.starts_with("PID:"))
            .then(|| win.class_name.to_lowercase());
        let candidates = window_identity_candidates(exact_class.as_deref(), win.executable_path.as_deref());
        let title_lower = win.title.to_lowercase();

        let best = self
            .apps
            .iter()
            .filter_map(|app| {
                let icon = app.icon.as_deref().filter(|s| !s.is_empty())?;
                let score = score_app_for_window(app, icon, &title_lower, exact_class.as_deref(), &candidates);
                (score > 0).then_some((score, icon))
            })
            .max_by_key(|(score, _)| *score);

        if let Some((_, icon)) = best {
            return Some(icon.to_string());
        }

        #[cfg(target_os = "windows")]
        if let Some(ref exe) = win.executable_path {
            if !exe.is_empty() {
                return Some(exe.clone());
            }
        }

        if exact_class.is_some() {
            return Some(win.class_name.clone());
        }

        win.executable_path
            .as_deref()
            .and_then(|p| Path::new(p).file_stem())
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .filter(|s| !s.is_empty() && !is_generic_runtime_name(s))
    }

    /// Clipboard History Mode (`@c` / `@clip`).
    fn search_clipboard(&self, trimmed_query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        let clip_query = if trimmed_query.starts_with("@clip ") {
            trimmed_query[6..].trim()
        } else if trimmed_query.starts_with("@c ") {
            trimmed_query[3..].trim()
        } else {
            ""
        };
        let clip_query_normalized = remove_vietnamese_accents(clip_query);
        let history = self.clipboard.get_history();
        let items: Vec<LauncherItem> = history
            .into_iter()
            .map(|text| {
                let one_line = text.lines().next().unwrap_or("").trim().to_string();
                let char_count = one_line.chars().count();
                let snippet = if char_count > 70 {
                    let truncated: String = one_line.chars().take(70).collect();
                    format!("{}...", truncated)
                } else if one_line.is_empty() {
                    "Clipboard text".to_string()
                } else {
                    one_line
                };
                LauncherItem::new(
                    snippet,
                    text.clone(),
                    ItemType::Clipboard,
                    Some(format!("{} chars", text.len())),
                    false,
                    None,
                )
            })
            .collect();

        if clip_query.is_empty() {
            return items.into_iter().map(|item| (item, Vec::new())).collect();
        }

        let mut matches: Vec<((LauncherItem, Vec<usize>), i64)> = items
            .par_iter()
            .filter_map(|item| {
                let match_res = self.match_precomputed(
                    &item.name,
                    &item.normalized_name,
                    clip_query,
                    &clip_query_normalized,
                );
                let final_res = if match_res.is_some() {
                    match_res
                } else {
                    self.match_item(&item.exec_or_path, clip_query)
                };
                final_res.map(|(score, indices)| ((item.clone(), indices), score))
            })
            .collect();
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// System / Power Actions (`@sys`, `@power`, or keywords).
    fn search_system(&self, trimmed_query: &str, is_sys_mode: bool) -> Vec<(LauncherItem, Vec<usize>)> {
        let sys_query = if trimmed_query.starts_with("@power ") {
            trimmed_query[7..].trim()
        } else if trimmed_query.starts_with("@sys ") {
            trimmed_query[5..].trim()
        } else if is_sys_mode {
            ""
        } else {
            trimmed_query
        };
        let sys_query_normalized = remove_vietnamese_accents(sys_query);
        let actions = crate::system_actions::get_system_actions();

        if sys_query.is_empty() {
            return actions.into_iter().map(|item| (item, Vec::new())).collect();
        }

        let mut matches: Vec<((LauncherItem, Vec<usize>), i64)> = actions
            .into_iter()
            .filter_map(|item| {
                let (score, indices) = self.match_precomputed(
                    &item.name,
                    &item.normalized_name,
                    sys_query,
                    &sys_query_normalized,
                )?;
                Some(((item, indices), score + 300))
            })
            .collect();
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// Matches a resolved directory path (`dir` + optional `filter`), shared between
    /// `@f <path>` and direct path queries like `~/Downloads/`.
    fn search_resolved_dir(
        &self,
        dir: &Path,
        filter: &str,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        if filter.is_empty() {
            return self
                .scan_dir_on_the_fly(dir)
                .into_iter()
                .map(|item| (item, Vec::new()))
                .collect();
        }

        let dir_items = self.scan_dir_recursive(dir, 3);
        let filter_normalized = remove_vietnamese_accents(filter);
        let enable_path_matching = self.config.search.enable_path_matching.unwrap_or(true);
        let mut matched: Vec<((LauncherItem, Vec<usize>), i64)> = dir_items
            .par_iter()
            .filter_map(|item| {
                if cancelled() {
                    return None;
                }
                let match_res = self.match_precomputed(
                    &item.name,
                    &item.normalized_name,
                    filter,
                    &filter_normalized,
                );
                let final_res = if match_res.is_some() {
                    match_res
                } else if enable_path_matching {
                    self.match_precomputed(
                        &item.exec_or_path,
                        &item.normalized_path,
                        filter,
                        &filter_normalized,
                    )
                } else {
                    None
                };
                final_res.map(|(score, indices)| ((item.clone(), indices), score))
            })
            .collect();

        matched.sort_by(|a, b| b.1.cmp(&a.1));
        matched.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// Fuzzy-matches `shallow_files` with optional path matching and history boost.
    fn match_shallow_files(
        &self,
        shallow_files: &[LauncherItem],
        history: &HistoryManager,
        query: &str,
        query_normalized: &str,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        let enable_path_matching = self.config.search.enable_path_matching.unwrap_or(true);
        let mut matches: Vec<((LauncherItem, Vec<usize>), i64)> = shallow_files
            .par_iter()
            .filter_map(|file| {
                if cancelled() {
                    return None;
                }
                let match_res = self.match_precomputed(
                    &file.name,
                    &file.normalized_name,
                    query,
                    query_normalized,
                );
                let final_res = if match_res.is_some() {
                    match_res
                } else if enable_path_matching {
                    self.match_precomputed(
                        &file.exec_or_path,
                        &file.normalized_path,
                        query,
                        query_normalized,
                    )
                } else {
                    None
                };

                final_res.map(|(score, indices)| {
                    let boost = history.get_boost(&file.exec_or_path);
                    ((file.clone(), indices), score + boost)
                })
            })
            .collect();

        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// File Search Mode (`@f` / `@file`).
    fn search_files_mode(
        &self,
        trimmed_query: &str,
        shallow_files: &[LauncherItem],
        history: &HistoryManager,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        let file_query = if trimmed_query.starts_with("@file ") {
            trimmed_query[6..].trim()
        } else if trimmed_query.starts_with("@f ") {
            trimmed_query[3..].trim()
        } else {
            ""
        };
        let file_query_normalized = remove_vietnamese_accents(file_query);

        if file_query.is_empty() {
            return shallow_files
                .iter()
                .take(50)
                .map(|file| (file.clone(), Vec::new()))
                .collect();
        }

        if let Some((dir, filter)) = self.resolve_path_search(file_query) {
            return self.search_resolved_dir(&dir, &filter, cancelled);
        }

        self.match_shallow_files(
            shallow_files,
            history,
            file_query,
            &file_query_normalized,
            cancelled,
        )
    }

    /// Custom Script Plugins (`~/.config/tiarun/plugins/`) & `@` mode index.
    fn search_plugins(&self, trimmed_query: &str) -> Option<Vec<(LauncherItem, Vec<usize>)>> {
        let plugins = crate::plugins::discover_plugins();
        let after_at = &trimmed_query[1..];
        let mut parts = after_at.splitn(2, ' ');
        let target_plugin = parts.next().unwrap_or("").to_lowercase();
        let plugin_query = parts.next().unwrap_or("");

        if let Some(plugin) = plugins.iter().find(|p| p.name == target_plugin) {
            let items = crate::plugins::execute_plugin(plugin, plugin_query);
            return Some(items.into_iter().map(|item| (item, Vec::new())).collect());
        }

        let mut results = vec![
            ("@w - Window Switcher", "@w", "Switch between active running windows"),
            ("@c - Clipboard History", "@c", "Browse and paste clipboard history"),
            ("@sys - System Actions", "@sys", "Lock, Sleep, Restart, Shutdown"),
            ("@f - File Search", "@f", "Search files and browse directories"),
            ("@theme - Switch Theme & Opacity", "@theme", "Dark, Light, System, or Opacity (e.g. @theme 95%)"),
        ]
        .into_iter()
        .map(|(name, exec, desc)| {
            (
                LauncherItem::new(
                    name.to_string(),
                    exec.to_string(),
                    ItemType::Mode,
                    Some(desc.to_string()),
                    false,
                    None,
                ),
                Vec::new(),
            )
        })
        .collect::<Vec<_>>();

        for p in plugins {
            results.push((
                LauncherItem::new(
                    format!("@{} - Custom Script Plugin", p.name),
                    format!("@{}", p.name),
                    ItemType::Mode,
                    Some(format!("Run script: {}", p.path.display())),
                    false,
                    None,
                ),
                Vec::new(),
            ));
        }

        if target_plugin.is_empty() {
            return Some(results);
        }

        if !trimmed_query.contains(' ') {
            let filtered: Vec<(LauncherItem, Vec<usize>)> = results
                .into_iter()
                .filter(|(item, _)| {
                    item.exec_or_path[1..].to_lowercase().starts_with(&target_plugin)
                        || item.name.to_lowercase().contains(&target_plugin)
                })
                .collect();
            if !filtered.is_empty() {
                return Some(filtered);
            }
        }

        None
    }

    /// Theme & Opacity Mode (`@theme`, `@mode`, `@opacity`).
    fn search_theme(&self, trimmed_query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        let theme_query = if trimmed_query.starts_with("@theme ") {
            trimmed_query[7..].trim()
        } else if trimmed_query.starts_with("@mode ") {
            trimmed_query[6..].trim()
        } else if trimmed_query.starts_with("@opacity ") {
            trimmed_query[9..].trim()
        } else if trimmed_query == "@theme" || trimmed_query == "@mode" || trimmed_query == "@opacity" {
            ""
        } else {
            trimmed_query
        };

        let theme_items = vec![
            LauncherItem::new(
                "Dark Theme".to_string(),
                "theme:dark".to_string(),
                ItemType::Theme,
                Some("Switch to sleek dark palette with high contrast".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Light Theme".to_string(),
                "theme:light".to_string(),
                ItemType::Theme,
                Some("Switch to clean, high-visibility light palette".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "System Theme (Auto)".to_string(),
                "theme:system".to_string(),
                ItemType::Theme,
                Some("Automatically match OS dark/light mode".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Window Opacity: 100% (Solid)".to_string(),
                "theme:opacity:100".to_string(),
                ItemType::Theme,
                Some("Set window opacity to 100% (opaque)".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Window Opacity: 95% (Frosted Glass - Recommended)".to_string(),
                "theme:opacity:95".to_string(),
                ItemType::Theme,
                Some("Set window opacity to 95% (modern glassmorphism)".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Window Opacity: 90% (Glass)".to_string(),
                "theme:opacity:90".to_string(),
                ItemType::Theme,
                Some("Set window opacity to 90%".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Window Opacity: 80% (Translucent)".to_string(),
                "theme:opacity:80".to_string(),
                ItemType::Theme,
                Some("Set window opacity to 80%".to_string()),
                false,
                None,
            ),
            LauncherItem::new(
                "Window Opacity: 70% (High Transparency)".to_string(),
                "theme:opacity:70".to_string(),
                ItemType::Theme,
                Some("Set window opacity to 70%".to_string()),
                false,
                None,
            ),
        ];

        if theme_query.is_empty() {
            return theme_items.into_iter().map(|item| (item, Vec::new())).collect();
        }

        let norm_q = remove_vietnamese_accents(theme_query);
        let mut matches: Vec<((LauncherItem, Vec<usize>), i64)> = theme_items
            .into_iter()
            .filter_map(|item| {
                let (score, indices) = self.match_precomputed(
                    &item.name,
                    &item.normalized_name,
                    theme_query,
                    &norm_q,
                )?;
                Some(((item, indices), score + 300))
            })
            .collect();
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(item_with_indices, _)| item_with_indices).collect()
    }

    /// Offline unit/currency conversion and inline math calculator.
    fn search_calc_and_convert(trimmed_query: &str) -> Vec<(LauncherItem, Vec<usize>)> {
        let mut results = Vec::new();
        if let Some(conv) = calc::evaluate_conversion(trimmed_query) {
            results.push((
                LauncherItem::new(
                    conv.title.clone(),
                    conv.value_to_copy.clone(),
                    ItemType::Calc,
                    Some(conv.subtitle),
                    false,
                    None,
                ),
                Vec::new(),
            ));
        }

        if let Some(calc_val) = calc::evaluate(trimmed_query) {
            let formatted = calc::format_result(calc_val);
            results.push((
                LauncherItem::new(
                    formatted.clone(),
                    formatted,
                    ItemType::Calc,
                    Some("Press Enter to copy result to clipboard".to_string()),
                    false,
                    None,
                ),
                Vec::new(),
            ));
        }
        results
    }

    /// Default search: direct path browsing, empty-query app list, or parallel fuzzy
    /// search across apps with trailing web-search fallback (no shallow files mixed in).
    fn search_apps(
        &self,
        trimmed_query: &str,
        mut results: Vec<(LauncherItem, Vec<usize>)>,
        history: &HistoryManager,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<(LauncherItem, Vec<usize>)> {
        if let Some((dir, filter)) = self.resolve_path_search(trimmed_query) {
            results.extend(self.search_resolved_dir(&dir, &filter, cancelled));
            return results;
        }

        if trimmed_query.is_empty() {
            // Dedup by name with a HashSet instead of re-scanning the (growing)
            // result vec with `.any()` for every candidate - O(n) instead of O(n^2),
            // which matters once the app catalog reaches a few hundred entries.
            let mut default_apps = Vec::with_capacity(
                self.pinned_apps.len() + self.custom_apps.len() + self.apps.len(),
            );
            let mut seen_names: std::collections::HashSet<&str> = std::collections::HashSet::new();

            for pinned in &self.pinned_apps {
                if let Some(app) = self
                    .apps
                    .iter()
                    .chain(self.custom_apps.iter())
                    .find(|a| a.name.eq_ignore_ascii_case(pinned))
                {
                    if seen_names.insert(app.name.as_str()) {
                        default_apps.push((app.clone(), Vec::new()));
                    }
                }
            }
            for custom in &self.custom_apps {
                if seen_names.insert(custom.name.as_str()) {
                    default_apps.push((custom.clone(), Vec::new()));
                }
            }
            for app in &self.apps {
                if seen_names.insert(app.name.as_str()) {
                    default_apps.push((app.clone(), Vec::new()));
                }
            }

            return default_apps;
        }

        let query_normalized = remove_vietnamese_accents(trimmed_query);

        let mut custom_matches: Vec<((LauncherItem, Vec<usize>), i64)> = self
            .custom_apps
            .par_iter()
            .filter_map(|custom| {
                let (score, indices) = self.match_precomputed(
                    &custom.name,
                    &custom.normalized_name,
                    trimmed_query,
                    &query_normalized,
                )?;
                let boost = history.get_boost(&custom.name) + 150;
                Some(((custom.clone(), indices), score + boost))
            })
            .collect();

        let mut app_matches: Vec<((LauncherItem, Vec<usize>), i64)> = self
            .apps
            .par_iter()
            .filter_map(|app| {
                let (score, indices) = self.match_precomputed(
                    &app.name,
                    &app.normalized_name,
                    trimmed_query,
                    &query_normalized,
                )?;
                let boost = history.get_boost(&app.name) + 100;
                Some(((app.clone(), indices), score + boost))
            })
            .collect();

        custom_matches.append(&mut app_matches);
        custom_matches.sort_by(|a, b| b.1.cmp(&a.1));

        for (item_with_indices, _) in custom_matches {
            results.push(item_with_indices);
        }

        // Web Search Fallback: always offered as a last-resort action for whatever was typed,
        // appended at the end so it never outranks a real match.
        let url = self
            .config
            .search
            .web_search_url
            .replace("{query}", &url_encode_query(trimmed_query));
        results.push((
            LauncherItem::new(
                format!("Search the web for \"{}\"", trimmed_query),
                url,
                ItemType::WebSearch,
                Some("Opens in your default browser".to_string()),
                false,
                None,
            ),
            Vec::new(),
        ));

        results
    }

    #[inline]
    pub(super) fn match_precomputed(
        &self,
        name: &str,
        name_normalized: &str,
        query: &str,
        query_normalized: &str,
    ) -> Option<(i64, Vec<usize>)> {
        let (score_orig, indices_orig) = self.matcher.fuzzy_indices(name, query).unwrap_or((0, Vec::new()));
        let (score_accent, indices_accent) = if !query_normalized.is_empty()
            && (name != name_normalized || query != query_normalized)
        {
            self.matcher.fuzzy_indices(name_normalized, query_normalized).unwrap_or((0, Vec::new()))
        } else {
            (0, Vec::new())
        };

        let (base_score, indices) = if score_orig >= score_accent && score_orig > 0 {
            (score_orig, indices_orig)
        } else if score_accent > 0 {
            (score_accent, indices_accent)
        } else {
            return None;
        };

        let is_consecutive = indices.windows(2).all(|w| w[1] == w[0] + 1);
        let consecutive_bonus = if is_consecutive && indices.len() > 1 { 40 } else { 0 };

        let bonus = Self::calculate_tier_bonus(name_normalized, query_normalized);
        Some((base_score + bonus + consecutive_bonus, indices))
    }

    #[inline]
    fn calculate_tier_bonus(name_normalized: &str, query_normalized: &str) -> i64 {
        if query_normalized.is_empty() {
            return 0;
        }

        // Tier 0: Exact match ("edge" == "edge") -> +500 bonus
        if name_normalized == query_normalized {
            return 500;
        }

        // Tier 0.5: Exact word match ("microsoft edge" has word "edge") -> +420 bonus
        if name_normalized.split_whitespace().any(|w| w == query_normalized) {
            return 420;
        }

        // Check common vendor/publisher prefixes:
        // "microsoft edge" -> "edge", "google chrome" -> "chrome", "mozilla firefox" -> "firefox"
        let stripped = strip_vendor_prefix(name_normalized);
        if stripped == query_normalized {
            return 450;
        }

        // Tier 1: Exact prefix ("image viewer" starts with "image", or stripped "edge" starts with "ed")
        if name_normalized.starts_with(query_normalized) {
            return 350 + (30usize.saturating_sub(name_normalized.len()) as i64) * 2;
        }
        if stripped.starts_with(query_normalized) {
            return 340 + (30usize.saturating_sub(stripped.len()) as i64) * 2;
        }

        // Tier 2: Word-boundary prefix ("registry editor" has "editor" starting with "ed")
        // Shorter matching words get a higher bonus (e.g. "edge" (4) > "editor" (6) > "educational" (11))
        let mut best_word_bonus = None;
        for (i, word) in name_normalized.split_whitespace().enumerate() {
            if word.starts_with(query_normalized) {
                // Secondary generic words (later in title) get a slight position penalty
                let position_penalty = if i > 0 { 20 } else { 0 };
                let length_bonus = (12usize.saturating_sub(word.len()) as i64) * 8;
                let word_score = 220 + length_bonus - position_penalty;
                best_word_bonus = Some(best_word_bonus.map_or(word_score, |b: i64| b.max(word_score)));
            }
        }
        if let Some(bonus) = best_word_bonus {
            return bonus;
        }

        // Tier 3: Consecutive substring ("fastimage" contains "image") -> +100 bonus
        if name_normalized.contains(query_normalized) {
            return 100;
        }

        0
    }

    fn match_item(&self, text: &str, query: &str) -> Option<(i64, Vec<usize>)> {
        let query_normalized = remove_vietnamese_accents(query);
        let text_normalized = remove_vietnamese_accents(text);
        self.match_precomputed(text, &text_normalized, query, &query_normalized)
    }
}

fn is_generic_runtime_name(name: &str) -> bool {
    matches!(
        name,
        "env"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "python"
            | "python3"
            | "node"
            | "nodejs"
            | "electron"
            | "java"
            | "javaw"
            | "flatpak"
            | "bwrap"
            | "xdg-open"
            | "com"
            | "org"
            | "net"
            | "io"
            | "app"
    )
}

fn push_unique_token(tokens: &mut Vec<String>, candidate: &str) {
    let trimmed = candidate.trim().to_lowercase();
    if trimmed.len() >= 2 && !is_generic_runtime_name(&trimmed) && !tokens.contains(&trimmed) {
        tokens.push(trimmed);
    }
}

fn push_stripped_suffix_tokens(tokens: &mut Vec<String>, base: &str) {
    for suffix in ["-url-handler", "-stable", "-bin"] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            push_unique_token(tokens, stripped);
        }
    }
}

fn window_identity_candidates(exact_class: Option<&str>, executable_path: Option<&str>) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(class) = exact_class {
        push_unique_token(&mut tokens, class);
        push_stripped_suffix_tokens(&mut tokens, class);
        if class.contains('.') {
            for part in class.split('.') {
                push_unique_token(&mut tokens, part);
                push_stripped_suffix_tokens(&mut tokens, &part.to_lowercase());
            }
        }
    }
    if let Some(exe) = executable_path {
        if let Some(stem) = Path::new(exe).file_stem().and_then(|s| s.to_str()) {
            let lower = stem.to_lowercase();
            push_unique_token(&mut tokens, &lower);
            push_stripped_suffix_tokens(&mut tokens, &lower);
        }
    }
    tokens
}

fn primary_exec_stem(exec_or_path: &str) -> Option<String> {
    let tokens = parse_command_line(exec_or_path);
    let bin = tokens
        .iter()
        .find(|t| *t != "env" && !t.contains('='))
        .or_else(|| tokens.first())?;
    let stem = Path::new(bin).file_stem()?.to_str()?.to_lowercase();
    (!stem.is_empty() && !is_generic_runtime_name(&stem)).then_some(stem)
}

fn score_app_for_window(
    app: &LauncherItem,
    icon: &str,
    title_lower: &str,
    exact_class: Option<&str>,
    candidates: &[String],
) -> i32 {
    if let Some(wm_class) = app.wm_class.as_deref() {
        let wm_lower = wm_class.to_lowercase();
        if exact_class == Some(wm_lower.as_str()) {
            return 100;
        }
        if candidates.iter().any(|c| c == &wm_lower) {
            return 95;
        }
    }

    if let Some(desktop_id) = app.desktop_id.as_deref() {
        let desktop_lower = desktop_id.to_lowercase();
        if exact_class == Some(desktop_lower.as_str()) {
            return 90;
        }
        if candidates.iter().any(|c| c == &desktop_lower) {
            return 85;
        }
    }

    let app_name_lower = app.name.to_lowercase();
    if title_lower == app_name_lower || exact_class == Some(app_name_lower.as_str()) {
        return 80;
    }
    if title_lower.ends_with(&format!(" - {}", app_name_lower)) {
        return 75;
    }

    let icon_stem = Path::new(icon)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(icon)
        .to_lowercase();
    let icon_lower = icon.to_lowercase();
    if candidates.iter().any(|c| c == &icon_lower || c == &icon_stem) {
        return 70;
    }
    if candidates.iter().any(|c| {
        c.len() >= 4
            && (icon_stem.starts_with(&format!("{}-", c)) || icon_stem.starts_with(&format!("{}.", c)))
    }) {
        return 65;
    }

    if let Some(exec_stem) = primary_exec_stem(&app.exec_or_path) {
        if candidates.iter().any(|c| c == &exec_stem) {
            return 60;
        }
        if candidates
            .iter()
            .any(|c| c.len() >= 4 && exec_stem.starts_with(&format!("{}-", c)))
        {
            return 55;
        }
    }

    0
}
