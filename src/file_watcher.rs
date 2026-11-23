use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use crate::config::Config;
use crate::launcher::{ItemType, LauncherItem};

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Starts watching hot directories (Desktop, Downloads, Documents, Home non-recursive) in the background.
    /// Newly added files are inserted into `shallow_files` and deleted files are removed.
    pub fn start(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
    ) -> Option<Self> {
        let mut recursive_dirs = Vec::new();
        if let Some(desktop) = dirs::desktop_dir() {
            if desktop.exists() {
                recursive_dirs.push(desktop);
            }
        }
        if let Some(downloads) = dirs::download_dir() {
            if downloads.exists() {
                recursive_dirs.push(downloads);
            }
        }
        if let Some(documents) = dirs::document_dir() {
            if documents.exists() {
                recursive_dirs.push(documents);
            }
        }
        for entry in &config.search.paths {
            let expanded = crate::launcher::expand_tilde(&entry.path);
            let path = PathBuf::from(expanded);
            if path.exists() && !recursive_dirs.contains(&path) {
                recursive_dirs.push(path);
            }
        }

        let mut non_recursive_dirs = Vec::new();
        if let Some(home) = dirs::home_dir() {
            if home.exists() && !recursive_dirs.contains(&home) {
                non_recursive_dirs.push(home);
            }
        }

        Self::start_with_all_dirs(files, config, recursive_dirs, non_recursive_dirs)
    }

    /// Starts watching specific directories recursively.
    pub fn start_with_dirs(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
        dirs: Vec<PathBuf>,
    ) -> Option<Self> {
        Self::start_with_all_dirs(files, config, dirs, Vec::new())
    }

    /// Starts watching recursive and non-recursive directories for file creation/modification/removal.
    pub fn start_with_all_dirs(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
        recursive_dirs: Vec<PathBuf>,
        non_recursive_dirs: Vec<PathBuf>,
    ) -> Option<Self> {
        let ignored_dirs = config.search.ignored_dirs.clone();
        let ignored_exts = config.search.ignored_extensions.clone();
        let mut all_roots = recursive_dirs.clone();
        for dir in &non_recursive_dirs {
            if !all_roots.contains(dir) {
                all_roots.push(dir.clone());
            }
        }
        let watch_roots = Arc::new(all_roots);

        let files_clone = files.clone();
        let roots_clone = watch_roots.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    Self::handle_event(&files_clone, &event, &ignored_dirs, &ignored_exts, &roots_clone);
                }
            },
            notify::Config::default(),
        ).ok()?;

        let mut watched_any = false;
        for dir in recursive_dirs {
            if watcher.watch(&dir, RecursiveMode::Recursive).is_ok() {
                watched_any = true;
            }
        }
        for dir in non_recursive_dirs {
            if watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                watched_any = true;
            }
        }

        if watched_any {
            Some(Self { _watcher: watcher })
        } else {
            None
        }
    }

    fn should_ignore(
        path: &Path,
        watch_roots: &[PathBuf],
        ignored_dirs: &[String],
        ignored_exts: &[String],
    ) -> bool {
        // Only inspect path components relative to the watched root directory,
        // so that parent directories of the watch root (e.g. AppData in Temp) are never checked.
        let relative_path: PathBuf = watch_roots
            .iter()
            .find_map(|dir| {
                if let Ok(rel) = path.strip_prefix(dir) {
                    Some(rel.to_path_buf())
                } else {
                    let path_s = path.to_string_lossy();
                    let dir_s = dir.to_string_lossy();
                    let clean_path = path_s.strip_prefix(r"\\?\").unwrap_or(&path_s);
                    let clean_dir = dir_s.strip_prefix(r"\\?\").unwrap_or(&dir_s);
                    Path::new(clean_path).strip_prefix(clean_dir).ok().map(|p| p.to_path_buf())
                }
            })
            .unwrap_or_else(|| {
                path.file_name().map(PathBuf::from).unwrap_or_else(|| path.to_path_buf())
            });

        // Ignore hidden files and directories
        for comp in relative_path.components() {
            if let Some(name) = comp.as_os_str().to_str() {
                if name.starts_with('.') && name != "." && name != ".." {
                    return true;
                }
                if ignored_dirs.iter().any(|ig| ig == name) {
                    return true;
                }
            }
        }

        // Check extension
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_dot = format!(".{}", ext.to_lowercase());
            if ignored_exts.iter().any(|ig| ig.eq_ignore_ascii_case(&ext_dot) || ig.eq_ignore_ascii_case(ext)) {
                return true;
            }
        }

        false
    }

    fn make_launcher_item(path: &Path) -> Option<LauncherItem> {
        let name = path.file_name()?.to_string_lossy().to_string();
        if name.is_empty() {
            return None;
        }

        let path_str = path.to_string_lossy().to_string();
        let item_type = if path.is_dir() {
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

        Some(LauncherItem::new(
            name,
            path_str,
            item_type,
            desc,
            false,
            None,
        ))
    }

    fn handle_event(
        files: &Arc<RwLock<Vec<LauncherItem>>>,
        event: &Event,
        ignored_dirs: &[String],
        ignored_exts: &[String],
        watch_roots: &[PathBuf],
    ) {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                for path in &event.paths {
                    if !path.exists() || Self::should_ignore(path, watch_roots, ignored_dirs, ignored_exts) {
                        continue;
                    }

                    if let Some(new_item) = Self::make_launcher_item(path) {
                        if let Ok(mut lock) = files.write() {
                            let path_str = &new_item.exec_or_path;
                            // Check if already present
                            if let Some(existing) = lock.iter_mut().find(|f| {
                                #[cfg(target_os = "windows")]
                                {
                                    f.exec_or_path.eq_ignore_ascii_case(path_str)
                                }
                                #[cfg(not(target_os = "windows"))]
                                {
                                    &f.exec_or_path == path_str
                                }
                            }) {
                                *existing = new_item;
                            } else {
                                lock.push(new_item);
                            }
                        }
                    }
                }
            }
            EventKind::Remove(_) => {
                for path in &event.paths {
                    let path_str = path.to_string_lossy();
                    let clean_path = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);
                    if let Ok(mut lock) = files.write() {
                        lock.retain(|f| {
                            let f_clean = f.exec_or_path.strip_prefix(r"\\?\").unwrap_or(&f.exec_or_path);
                            #[cfg(target_os = "windows")]
                            {
                                !f_clean.eq_ignore_ascii_case(clean_path)
                            }
                            #[cfg(not(target_os = "windows"))]
                            {
                                f_clean != clean_path
                            }
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_watcher_create_and_remove() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_watch_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let files = Arc::new(RwLock::new(Vec::new()));
        let config = Config::default();

        let _watcher = FileWatcher::start_with_dirs(
            files.clone(),
            &config,
            vec![temp_dir.clone()],
        );
        assert!(_watcher.is_some(), "Watcher should start successfully on valid temp dir");

        // Create a new file
        let new_file = temp_dir.join("test_download.pdf");
        std::fs::write(&new_file, b"content").unwrap();

        // Give the file watcher thread a window to process the event (up to 3s for CI)
        let mut found = false;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Ok(lock) = files.read() {
                if lock.iter().any(|f| f.name == "test_download.pdf") {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "File watcher should detect newly created file in watched directory");

        // Remove the file
        let _ = std::fs::remove_file(&new_file);
        let mut removed = false;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Ok(lock) = files.read() {
                if !lock.iter().any(|f| f.name == "test_download.pdf") {
                    removed = true;
                    break;
                }
            }
        }
        assert!(removed, "File watcher should remove deleted file from launcher items");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

