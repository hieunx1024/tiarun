use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

fn detect_active_icon_theme() -> Option<String> {
    // 1. Try GNOME gsettings
    if let Ok(output) = std::process::Command::new("gsettings")
        .args(&["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
    {
        let theme = String::from_utf8_lossy(&output.stdout).trim().trim_matches('\'').trim_matches('"').to_string();
        if !theme.is_empty() && theme != "null" {
            return Some(theme);
        }
    }
    // 2. Try KDE kreadconfig5
    if let Ok(output) = std::process::Command::new("kreadconfig5")
        .args(&["--group", "Icons", "--key", "Theme"])
        .output()
    {
        let theme = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !theme.is_empty() {
            return Some(theme);
        }
    }
    // 3. Try ~/.config/gtk-3.0/settings.ini
    if let Some(mut p) = dirs::config_dir() {
        p.push("gtk-3.0");
        p.push("settings.ini");
        if let Ok(content) = std::fs::read_to_string(&p) {
            for line in content.lines() {
                if let Some(val) = line.trim().strip_prefix("gtk-icon-theme-name=") {
                    let theme = val.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !theme.is_empty() {
                        return Some(theme);
                    }
                }
            }
        }
    }
    None
}

fn collect_theme_roots(bases: &[PathBuf], search_roots: &mut Vec<PathBuf>) {
    for base in bases {
        if !base.exists() {
            continue;
        }
        if base.to_string_lossy().contains("pixmaps") {
            search_roots.push(base.clone());
        } else if let Ok(sub_entries) = std::fs::read_dir(base) {
            for sub in sub_entries.flatten() {
                let sub_path = sub.path();
                if sub_path.is_dir() {
                    search_roots.push(sub_path);
                }
            }
        }
    }
}

pub(super) fn init_unix_icon_index(
    path_index: &Arc<RwLock<HashMap<String, PathBuf>>>,
    indexing_done: &Arc<AtomicBool>,
) {
    // Fast synchronous pass: immediately index /usr/share/pixmaps and standard apps icons
    // so initial launcher pop-in (<2ms) already has icons for common applications.
    let mut fast_map: HashMap<String, PathBuf> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/usr/share/pixmaps") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    fast_map.insert(stem.to_lowercase(), p);
                }
            }
        }
    }
    let quick_hicolor_dirs = [
        PathBuf::from("/usr/share/icons/hicolor/scalable/apps"),
        PathBuf::from("/usr/share/icons/hicolor/256x256/apps"),
        PathBuf::from("/usr/share/icons/hicolor/128x128/apps"),
        PathBuf::from("/usr/share/icons/hicolor/64x64/apps"),
        PathBuf::from("/usr/share/icons/hicolor/48x48/apps"),
    ];
    for qdir in &quick_hicolor_dirs {
        if let Ok(entries) = std::fs::read_dir(qdir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        fast_map.entry(stem.to_lowercase()).or_insert(p);
                    }
                }
            }
        }
    }
    if let Ok(mut lock) = path_index.write() {
        *lock = fast_map;
    }

    let index_clone = path_index.clone();
    let indexing_done_clone = indexing_done.clone();

    std::thread::spawn(move || {
        let active_theme = detect_active_icon_theme();
        let active_theme_lower = active_theme.as_ref().map(|t| t.to_lowercase());

        let mut map: HashMap<String, (PathBuf, i32)> = HashMap::new();
        let mut search_roots = Vec::new();

        // 1. System icon theme roots
        let icon_bases = [
            PathBuf::from("/usr/share/icons"),
            PathBuf::from("/usr/local/share/icons"),
            PathBuf::from("/usr/share/pixmaps"),
            PathBuf::from("/usr/local/share/pixmaps"),
            PathBuf::from("/var/lib/snapd/desktop/icons"),
            PathBuf::from("/snap/gtk-common-themes/current/share/icons"),
            PathBuf::from("/var/lib/flatpak/exports/share/icons"),
        ];
        collect_theme_roots(&icon_bases, &mut search_roots);

        // 2. User icon theme roots
        if let Some(home) = dirs::home_dir() {
            let user_bases = [
                home.join(".local/share/icons"),
                home.join(".icons"),
                home.join(".local/share/pixmaps"),
                home.join(".local/share/flatpak/exports/share/icons"),
            ];
            collect_theme_roots(&user_bases, &mut search_roots);
        }

        // 3. XDG_DATA_DIRS
        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for dir in xdg_data_dirs.split(':') {
                let trimmed = dir.trim();
                if !trimmed.is_empty() {
                    let p_icons = PathBuf::from(trimmed).join("icons");
                    if p_icons.exists() && !search_roots.contains(&p_icons) {
                        if let Ok(sub_entries) = std::fs::read_dir(&p_icons) {
                            for sub in sub_entries.flatten() {
                                let sub_path = sub.path();
                                if sub_path.is_dir() && !search_roots.contains(&sub_path) {
                                    search_roots.push(sub_path);
                                }
                            }
                        }
                    }
                    let p_pix = PathBuf::from(trimmed).join("pixmaps");
                    if p_pix.exists() && !search_roots.contains(&p_pix) {
                        search_roots.push(p_pix);
                    }
                }
            }
        }

        // Prioritize active theme in root ordering if found
        if let Some(ref active) = active_theme_lower {
            search_roots.sort_by_key(|r| {
                let r_lower = r.to_string_lossy().to_lowercase();
                if r_lower.contains(active.as_str()) {
                    0
                } else if r_lower.contains("hicolor") {
                    1
                } else {
                    2
                }
            });
        }

        for root in search_roots {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&root).max_depth(5).into_iter().flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ext_lower != "png" && ext_lower != "svg" && ext_lower != "xpm" && ext_lower != "ico" {
                            continue;
                        }

                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            let key = stem.to_lowercase();
                            let path_str = path.to_string_lossy();
                            let path_lower = path_str.to_lowercase();

                            // Category and theme weighting
                            let is_active = active_theme_lower.as_ref().map_or(false, |t| path_lower.contains(t.as_str()));
                            let is_apps = path_str.contains("/apps/") || path_str.contains("/applications/");
                            let is_pixmaps = path_str.contains("/pixmaps/");
                            let is_panel_or_action = path_str.contains("/panel/")
                                || path_str.contains("/actions/")
                                || path_str.contains("/status/")
                                || path_str.contains("/mimetypes/");
                            let is_symbolic = path_str.contains("symbolic") || key.ends_with("-symbolic");

                            let mut score: i32 = 0;
                            if is_active {
                                score += 40;
                            } else if path_lower.contains("/hicolor/") || path_lower.contains("/papirus/") || path_lower.contains("/yaru/") || path_lower.contains("/breeze/") || path_lower.contains("/adwaita/") {
                                score += 20;
                            } else {
                                score += 10;
                            }

                            if is_apps {
                                score += 30;
                            } else if is_pixmaps {
                                score += 22;
                            } else if is_panel_or_action {
                                score -= 25;
                            }

                            if is_symbolic {
                                score -= 30;
                            }

                            // Resolution weighting - prioritize 32x32 for pixel-perfect 1:1 display
                            if path_str.contains("32x32") {
                                score += 30;
                            } else if ext_lower == "svg" || path_str.contains("scalable") {
                                score += 25;
                            } else if path_str.contains("48x48") || path_str.contains("64x64") {
                                score += 20;
                            } else if path_str.contains("128x128") || path_str.contains("256x256") || path_str.contains("512x512") {
                                score += 10;
                            } else if path_str.contains("24x24") || path_str.contains("22x22") || path_str.contains("16x16") {
                                score += 5;
                            } else {
                                score += 8;
                            }

                            let insert_key = |map: &mut HashMap<String, (PathBuf, i32)>, k: String, p: PathBuf, s: i32| {
                                if let Some((_, old_score)) = map.get(&k) {
                                    if s > *old_score {
                                        map.insert(k, (p, s));
                                    }
                                } else {
                                    map.insert(k, (p, s));
                                }
                            };

                            // 1. Exact stem (e.g. "org.gnome.texteditor", "firefox", "google-chrome")
                            insert_key(&mut map, key.clone(), path.to_path_buf(), score);

                            // 2. Strip "-symbolic" if present
                            if key.ends_with("-symbolic") {
                                let non_sym = key.trim_end_matches("-symbolic").to_string();
                                insert_key(&mut map, non_sym, path.to_path_buf(), score - 15);
                            }

                            // 3. Compact alphanumeric (e.g. "google-chrome" -> "googlechrome")
                            let compact: String = key.chars().filter(|c| c.is_alphanumeric()).collect();
                            if compact.len() > 2 && compact != key {
                                insert_key(&mut map, compact, path.to_path_buf(), score - 2);
                            }

                            // 4. Reverse domain last segment (e.g. "com.mattjakeman.extensionmanager" -> "extensionmanager")
                            if let Some(last_seg) = key.split('.').last() {
                                if last_seg.len() > 2 && last_seg != key {
                                    insert_key(&mut map, last_seg.to_string(), path.to_path_buf(), score - 2);
                                    let last_compact: String = last_seg.chars().filter(|c| c.is_alphanumeric()).collect();
                                    if last_compact.len() > 2 && last_compact != last_seg {
                                        insert_key(&mut map, last_compact, path.to_path_buf(), score - 4);
                                    }
                                }
                            }

                            // 5. Hyphen segment variations (e.g. "spotify-client" -> "spotify", "dbeaver-ce" -> "dbeaver")
                            if key.contains('-') {
                                for part in key.split('-') {
                                    if part.len() > 3 {
                                        insert_key(&mut map, part.to_string(), path.to_path_buf(), score - 5);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let final_map: HashMap<String, PathBuf> = map.into_iter().map(|(k, (p, _))| (k, p)).collect();

        if let Ok(mut lock) = index_clone.write() {
            // Merge with the initial fast_map, with comprehensive results taking precedence
            for (k, v) in final_map {
                lock.insert(k, v);
            }
        }

        indexing_done_clone.store(true, Ordering::SeqCst);
    });
}

pub(super) fn find_icon_path(
    icon_path_index: &Arc<RwLock<HashMap<String, PathBuf>>>,
    icon_hint: Option<&str>,
    app_name: &str,
    exec_or_path: &str,
) -> Option<PathBuf> {
    // 1. Direct path check (e.g. absolute icon paths in .desktop)
    if let Some(hint) = icon_hint {
        let p = Path::new(hint);
        if p.exists() {
            return Some(p.to_path_buf());
        }
        for ext in &["png", "svg", "xpm", "ico"] {
            let p_ext = p.with_extension(ext);
            if p_ext.exists() {
                return Some(p_ext);
            }
        }
        // Check direct /usr/share/pixmaps
        for ext in &["png", "svg", "xpm", "ico"] {
            let pix = PathBuf::from(format!("/usr/share/pixmaps/{}.{}", hint, ext));
            if pix.exists() {
                return Some(pix);
            }
        }
    }

    if let Ok(index) = icon_path_index.read() {
        // 2. Check icon hint variations
        if let Some(hint) = icon_hint {
            let h_lower = hint.to_lowercase();
            // a. Exact lowercase
            if let Some(path) = index.get(&h_lower) {
                return Some(path.clone());
            }
            // b. Strip extension (.png, .svg, .xpm, .ico)
            let h_no_ext = h_lower.trim_end_matches(".png").trim_end_matches(".svg").trim_end_matches(".xpm").trim_end_matches(".ico");
            if let Some(path) = index.get(h_no_ext) {
                return Some(path.clone());
            }
            // c. Strip -symbolic
            if h_no_ext.ends_with("-symbolic") {
                let h_no_sym = h_no_ext.trim_end_matches("-symbolic");
                if let Some(path) = index.get(h_no_sym) {
                    return Some(path.clone());
                }
            }
            // d. Reverse-domain last segment (e.g. "com.mattjakeman.ExtensionManager" -> "extensionmanager")
            if let Some(last_seg) = h_no_ext.split('.').last() {
                if last_seg.len() > 2 && last_seg != h_no_ext {
                    if let Some(path) = index.get(last_seg) {
                        return Some(path.clone());
                    }
                }
            }
            // e. Alphanumeric compact
            let h_compact: String = h_no_ext.chars().filter(|c| c.is_alphanumeric()).collect();
            if !h_compact.is_empty() && h_compact != h_no_ext {
                if let Some(path) = index.get(&h_compact) {
                    return Some(path.clone());
                }
            }
        }

        // 3. Known app aliases (handles apps with distinct icon naming or vendor ids)
        let check_keys: &[&str] = match app_name.to_lowercase().as_str() {
            "chatgpt" => &["chatgpt", "indicator-chatgpt", "openai-chatgpt"],
            "claude" => &["claude-desktop", "claude", "com.anthropic.claude"],
            "spotify" => &["spotify-client", "spotify"],
            "visual studio code" | "code" | "vscode" => &["vscode", "code", "com.visualstudio.code", "com.visualstudio.code.oss"],
            "google chrome" | "chrome" => &["google-chrome", "google-chrome-stable", "chromium", "chrome"],
            "viber" => &["viber", "com.viber.viber"],
            "dbeaver" | "dbeaver ce" => &["dbeaver-ce", "dbeaver"],
            "postman" => &["postman", "com.getpostman.postman"],
            _ => &[],
        };
        for k in check_keys {
            if let Some(path) = index.get(*k) {
                return Some(path.clone());
            }
        }

        // 4. Check app name variations
        let name_key = app_name.to_lowercase();
        if let Some(path) = index.get(&name_key) {
            return Some(path.clone());
        }
        // App name without spaces/punctuation (e.g. "Text Editor" -> "texteditor")
        let name_compact: String = name_key.chars().filter(|c| c.is_alphanumeric()).collect();
        if !name_compact.is_empty() && name_compact != name_key {
            if let Some(path) = index.get(&name_compact) {
                return Some(path.clone());
            }
        }

        // 5. Check executable name variations
        let tokens = crate::launcher::parse_command_line(exec_or_path);
        let bin_target = tokens.iter()
            .find(|t| *t != "env" && !t.contains('='))
            .or_else(|| tokens.first());
        if let Some(bin) = bin_target.and_then(|p| Path::new(p).file_name()).and_then(|f| f.to_str()) {
            let bin_clean = bin.to_lowercase();
            if let Some(path) = index.get(&bin_clean) {
                return Some(path.clone());
            }
            if let Some(stripped) = bin_clean.strip_suffix(".sh") {
                if let Some(path) = index.get(stripped) {
                    return Some(path.clone());
                }
            }
            if let Some(last_bin) = bin_clean.split('-').last() {
                if last_bin.len() > 2 && last_bin != bin_clean {
                    if let Some(path) = index.get(last_bin) {
                        return Some(path.clone());
                    }
                }
            }
        }
    }

    None
}
