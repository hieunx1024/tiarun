use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use walkdir::WalkDir;

use crate::config::Config;

use super::text::{expand_tilde, parse_command_line, quote_arg};
use super::{ItemType, LauncherEngine, LauncherItem};

impl LauncherEngine {
    /// Indexes all standard Linux .desktop application entries + extra paths from config.
    #[cfg(not(target_os = "windows"))]
    pub(super) fn index_apps(&mut self) {
        let mut paths = Vec::new();

        // 1. Extra desktop paths from config (highest priority)
        for extra in &self.config.apps.extra_desktop_paths {
            let expanded = expand_tilde(extra);
            let p = PathBuf::from(expanded);
            if !paths.contains(&p) {
                paths.push(p);
            }
        }

        // 2. User applications directory ($XDG_DATA_HOME/applications or ~/.local/share/applications)
        let data_home = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .ok()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
        if let Some(user_data) = data_home {
            let user_apps = user_data.join("applications");
            if !paths.contains(&user_apps) {
                paths.push(user_apps);
            }
        }

        // 3. User flatpak exports
        if let Some(home) = dirs::home_dir() {
            let flatpak_user = home.join(".local/share/flatpak/exports/share/applications");
            if !paths.contains(&flatpak_user) {
                paths.push(flatpak_user);
            }
        }

        // 4. Standard $XDG_DATA_DIRS (e.g. Ubuntu snap / flatpak / desktop entries)
        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for dir_str in xdg_data_dirs.split(':') {
                let trimmed = dir_str.trim();
                if !trimmed.is_empty() {
                    let mut p = PathBuf::from(trimmed);
                    if p.file_name().map_or(true, |f| f != "applications") {
                        p.push("applications");
                    }
                    if !paths.contains(&p) {
                        paths.push(p);
                    }
                }
            }
        }

        // 5. System fallbacks if not already present from $XDG_DATA_DIRS
        let system_fallbacks = [
            PathBuf::from("/usr/local/share/applications"),
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/var/lib/snapd/desktop/applications"),
            PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        ];
        for p in system_fallbacks {
            if !paths.contains(&p) {
                paths.push(p);
            }
        }

        // Desktop-file ids (basename, per the XDG spec) already indexed, so the same
        // app shipped in multiple XDG_DATA_DIRS is only added once. Distinct apps that
        // merely share a display Name (e.g. Ubuntu 26 ships a new default terminal
        // whose .desktop Name is also just "Terminal", alongside another terminal
        // emulator's .desktop file) must NOT be collapsed, so dedup can't key on `Name`.
        let mut seen_ids: std::collections::HashSet<std::ffi::OsString> = std::collections::HashSet::new();

        for path in paths {
            if !path.exists() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().map_or(false, |ext| ext == "desktop") {
                        let id = file_path.file_name().map(|f| f.to_os_string());
                        if let Some(id) = &id {
                            if seen_ids.contains(id) {
                                continue;
                            }
                        }

                        if let Some(app) = self.parse_desktop_file(&file_path) {
                            // Check if app is in hidden list
                            let is_hidden = self.hidden_apps.iter().any(|h| {
                                app.name.eq_ignore_ascii_case(h) || app.exec_or_path.contains(h)
                            });

                            if let Some(id) = id {
                                seen_ids.insert(id);
                            }

                            if !is_hidden {
                                self.apps.push(app);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Indexes all standard Windows shortcut entries (.lnk and .url) from Start Menu and Desktop.
    #[cfg(target_os = "windows")]
    pub(super) fn index_apps(&mut self) {
        let mut paths = Vec::new();
        if let Some(mut path) = dirs::config_dir() {
            path.push("Microsoft");
            path.push("Windows");
            path.push("Start Menu");
            path.push("Programs");
            paths.push(path);
        }
        paths.push(PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs"));
        if let Some(desktop) = dirs::desktop_dir() {
            paths.push(desktop);
        }
        if let Ok(public_dir) = std::env::var("PUBLIC") {
            let p = PathBuf::from(public_dir).join("Desktop");
            if p.exists() && !paths.contains(&p) {
                paths.push(p);
            }
        }

        for path in paths {
            if !path.exists() {
                continue;
            }
            for entry in WalkDir::new(path).into_iter().flatten() {
                let file_path = entry.path();
                let is_app_shortcut = file_path.extension().map_or(false, |ext| {
                    ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url")
                });

                if is_app_shortcut {
                    let name = file_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let exec = file_path.to_string_lossy().to_string();

                    let is_hidden = self.hidden_apps.iter().any(|h| name.eq_ignore_ascii_case(h));
                    if !is_hidden && !self.apps.iter().any(|item| item.name.eq_ignore_ascii_case(&name)) {
                        self.apps.push(LauncherItem::new(
                            name,
                            exec.clone(),
                            ItemType::App,
                            Some("Windows App".to_string()),
                            false,
                            Some(exec),
                        ));
                    }
                }
            }
        }
    }

    /// Parses a Linux .desktop entry file line by line to extract the core fields.
    #[allow(dead_code)]
    pub(super) fn parse_desktop_file(&self, path: &Path) -> Option<LauncherItem> {
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);

        let mut name = String::new();
        let mut exec = String::new();
        let mut comment = None;
        let mut icon_hint = None;
        let mut desktop_categories = None;
        let mut wm_class = None;
        let mut is_app = false;
        let mut no_display = false;
        let mut hidden = false;
        let mut terminal = false;
        let mut in_desktop_entry = false;

        for line in reader.lines().flatten() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_desktop_entry = trimmed == "[Desktop Entry]";
                continue;
            }

            if !in_desktop_entry {
                continue;
            }

            if let Some(idx) = trimmed.find('=') {
                let key = trimmed[..idx].trim();
                let val = trimmed[idx + 1..].trim();

                match key {
                    "Type" => {
                        if val == "Application" {
                            is_app = true;
                        }
                    }
                    "Name" => {
                        if name.is_empty() {
                            name = val.to_string();
                        }
                    }
                    "Exec" => {
                        let tokens = parse_command_line(val);
                        let mut cleaned_tokens = Vec::new();
                        for token in tokens {
                            // Skip any placeholder starting with '%'
                            if token.starts_with('%') {
                                continue;
                            }
                            cleaned_tokens.push(token);
                        }
                        if let Some(last) = cleaned_tokens.last() {
                            if last == "--" {
                                cleaned_tokens.pop();
                            }
                        }
                        exec = cleaned_tokens.iter().map(|t| quote_arg(t)).collect::<Vec<_>>().join(" ");
                    }
                    "Categories" if !val.is_empty() => {
                        desktop_categories = Some(Arc::from(val));
                    }
                    "StartupWMClass" if !val.is_empty() => {
                        wm_class = Some(Arc::from(val));
                    }
                    "Comment" => {
                        comment = Some(val.to_string());
                    }
                    "Icon" => {
                        if icon_hint.is_none() && !val.is_empty() {
                            icon_hint = Some(val.to_string());
                        }
                    }
                    "NoDisplay" => {
                        if val == "true" {
                            no_display = true;
                        }
                    }
                    "Hidden" => {
                        if val == "true" {
                            hidden = true;
                        }
                    }
                    "Terminal" => {
                        if val == "true" {
                            terminal = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if is_app && !name.is_empty() && !exec.is_empty() && !no_display && !hidden {
            let mut item = LauncherItem::new(
                name,
                exec,
                ItemType::App,
                comment,
                terminal,
                icon_hint,
            );
            item.desktop_categories = desktop_categories;
            item.wm_class = wm_class;
            item.desktop_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .map(Arc::from);
            Some(item)
        } else {
            None
        }
    }

    /// Indexes files based on configured `search.paths` or smart XDG defaults.
    pub(super) fn index_files_impl(config: &Config, files: &mut Vec<LauncherItem>) {
        use std::collections::HashSet;

        const MAX_FILES: usize = 30_000;

        let search_paths = if config.search.paths.is_empty() {
            let mut defaults = Vec::new();

            // Index fast, shallow personal user directories first
            if let Some(dir) = dirs::home_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 1,
                    max_depth: Some(1),
                    exclude: Vec::new(),
                });
            }

            if let Some(dir) = dirs::desktop_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 3,
                    max_depth: Some(3),
                    exclude: Vec::new(),
                });
            }

            if let Some(dir) = dirs::download_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 3,
                    max_depth: Some(3),
                    exclude: Vec::new(),
                });
            }

            if let Some(dir) = dirs::document_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 5,
                    max_depth: Some(5),
                    exclude: Vec::new(),
                });
            }

            if let Some(dir) = dirs::picture_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 3,
                    max_depth: Some(3),
                    exclude: Vec::new(),
                });
            }

            if let Some(dir) = dirs::video_dir() {
                defaults.push(crate::config::SearchPathConfig {
                    path: dir.to_string_lossy().to_string(),
                    depth: 1,
                    max_depth: Some(1),
                    exclude: Vec::new(),
                });
            }

            defaults
        } else {
            config.search.paths.clone()
        };

        let ignored_dirs = &config.search.ignored_dirs;
        let ignored_exts = &config.search.ignored_extensions;
        let mut seen_paths: HashSet<String> = HashSet::new();

        for search_entry in search_paths {
            let expanded_path_str = expand_tilde(&search_entry.path);
            let root_path = PathBuf::from(expanded_path_str);
            if !root_path.exists() {
                continue;
            }

            let max_depth = search_entry.max_depth.unwrap_or(config.search.max_depth);

            for entry in WalkDir::new(&root_path)
                .max_depth(max_depth)
                .into_iter()
                .filter_entry(|e| {
                    if let Some(name) = e.file_name().to_str() {
                        !ignored_dirs.iter().any(|ignored| name == ignored) && !name.starts_with('.')
                    } else {
                        false
                    }
                })
                .flatten()
            {
                if files.len() >= MAX_FILES {
                    return;
                }
                let path = entry.path();
                if path == root_path {
                    continue;
                }

                // Check ignored extensions without allocating lowercase strings
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ignored_exts.iter().any(|ig| {
                        let stripped = ig.strip_prefix('.').unwrap_or(ig.as_str());
                        stripped.eq_ignore_ascii_case(ext)
                    }) {
                        continue;
                    }
                }

                let path_str = path.to_string_lossy().to_string();

                if !seen_paths.insert(path_str.clone()) {
                    continue;
                }

                let name = entry.file_name().to_string_lossy().to_string();

                let is_dir = entry.file_type().is_dir();
                let item_type = if is_dir {
                    ItemType::Dir
                } else {
                    ItemType::File
                };
                let file_size = if is_dir {
                    None
                } else {
                    entry.metadata().ok().map(|m| m.len())
                };

                let home_dir_opt = dirs::home_dir();
                let desc = path.parent().map(|p| {
                    let s = p.to_string_lossy().to_string();
                    if let Some(ref home) = home_dir_opt {
                        let home_str = home.to_string_lossy();
                        if s.starts_with(home_str.as_ref()) {
                            return s.replacen(home_str.as_ref(), "~", 1);
                        }
                    }
                    s
                });

                files.push(
                    LauncherItem::new(
                        name,
                        path_str,
                        item_type,
                        desc,
                        false,
                        None,
                    )
                    .with_file_size(file_size),
                );
            }
        }
    }

    /// Resolves dynamic path searching (e.g. typing `~/Downloads/` directly lists Downloads contents)
    pub fn resolve_path_search(&self, input: &str) -> Option<(PathBuf, String)> {
        let is_path_like = input.contains('/')
            || input.contains('\\')
            || input == "~"
            || (input.len() >= 2 && input.chars().nth(1) == Some(':'));
        if !is_path_like {
            return None;
        }

        let home = dirs::home_dir()?;
        let home_str = home.to_string_lossy();
        let sep = std::path::MAIN_SEPARATOR;
        #[allow(unused_mut)]
        let mut expanded = if input.starts_with("~/") {
            input.replacen("~/", &format!("{home_str}{sep}"), 1)
        } else if input.starts_with(r"~\") {
            input.replacen(r"~\", &format!("{home_str}{sep}"), 1)
        } else if input == "~" {
            format!("{home_str}{sep}")
        } else {
            input.to_string()
        };

        #[cfg(not(target_os = "windows"))]
        {
            expanded = expanded.replace('\\', "/");
        }

        let path = PathBuf::from(&expanded);

        if expanded.ends_with('/') || expanded.ends_with('\\') {
            if path.is_dir() {
                Some((path, String::new()))
            } else {
                None
            }
        } else if let Some(parent) = path.parent() {
            if parent.is_dir() {
                let filter = path.file_name()?.to_string_lossy().to_string();
                Some((parent.to_path_buf(), filter))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Scans a specific directory recursively up to `max_depth` for filtering sub-items.
    pub fn scan_dir_recursive(&self, dir: &Path, max_depth: usize) -> Vec<LauncherItem> {
        let mut items = Vec::new();
        let ignored_dirs = &self.config.search.ignored_dirs;
        let ignored_exts = &self.config.search.ignored_extensions;

        for entry in WalkDir::new(dir)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|e| {
                if let Some(name) = e.file_name().to_str() {
                    !ignored_dirs.iter().any(|ignored| name == ignored) && !name.starts_with('.')
                } else {
                    false
                }
            })
            .flatten()
        {
            if items.len() >= 1000 {
                break;
            }
            let path = entry.path();
            if path == dir {
                continue;
            }

            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_dot = format!(".{}", ext.to_lowercase());
                if ignored_exts.iter().any(|ig| ig.eq_ignore_ascii_case(&ext_dot) || ig.eq_ignore_ascii_case(ext)) {
                    continue;
                }
            }

            let path_str = path.to_string_lossy().to_string();
            let name = entry.file_name().to_string_lossy().to_string();
            let item_type = if entry.file_type().is_dir() {
                ItemType::Dir
            } else {
                ItemType::File
            };

            let desc = path.parent().map(|p| {
                let s = p.to_string_lossy().to_string();
                if let Some(home) = dirs::home_dir() {
                    let home_str = home.to_string_lossy().to_string();
                    if s.starts_with(&home_str) {
                        return s.replacen(&home_str, "~", 1);
                    }
                }
                s
            });

            items.push(LauncherItem::new(
                name,
                path_str,
                item_type,
                desc,
                false,
                None,
            ));
        }
        items
    }

    /// Scans a specific directory on-the-fly for quick sub-folder traversal.
    pub fn scan_dir_on_the_fly(&self, dir: &Path) -> Vec<LauncherItem> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let path_str = path.to_string_lossy().to_string();

                if path.is_dir() {
                    dirs.push(LauncherItem::new(
                        name,
                        path_str,
                        ItemType::Dir,
                        Some(dir.to_string_lossy().to_string()),
                        false,
                        None,
                    ));
                } else {
                    files.push(LauncherItem::new(
                        name,
                        path_str,
                        ItemType::File,
                        Some(dir.to_string_lossy().to_string()),
                        false,
                        None,
                    ));
                }
            }
        }
        dirs.sort_by(|a, b| {
            a.name
                .starts_with('.')
                .cmp(&b.name.starts_with('.'))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        files.sort_by(|a, b| {
            a.name
                .starts_with('.')
                .cmp(&b.name.starts_with('.'))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        dirs.extend(files);
        dirs
    }
}
