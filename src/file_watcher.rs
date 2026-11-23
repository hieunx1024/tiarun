use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use crate::config::Config;
use crate::launcher::{ItemType, LauncherItem};

/// inotify (Linux) needs one watch per directory, so a recursive watch would also
/// register ignored trees (node_modules, .git, target...). Windows'
/// ReadDirectoryChangesW watches a whole tree with a single handle, which is
/// cheaper than thousands of per-directory handles, so it stays recursive there.
const PER_DIRECTORY_WATCHES: bool = cfg!(target_os = "linux");

pub struct FileWatcher {
    _watcher: Arc<Mutex<RecommendedWatcher>>,
    /// Dropping it stops the thread that watches newly created directories.
    _new_dirs: Sender<PathBuf>,
}

impl FileWatcher {
    /// Starts watching hot directories (Desktop, Downloads, Documents, Home non-recursive) in the background.
    /// Newly added files are inserted into `shallow_files` and deleted files are removed.
    pub fn start(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
        search_index: Option<Arc<crate::search_index::SearchIndexEngine>>,
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

        Self::start_with_all_dirs(files, config, recursive_dirs, non_recursive_dirs, search_index)
    }

    /// Starts watching specific directories recursively.
    pub fn start_with_dirs(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
        dirs: Vec<PathBuf>,
    ) -> Option<Self> {
        Self::start_with_all_dirs(files, config, dirs, Vec::new(), None)
    }

    /// Starts watching recursive and non-recursive directories for file creation/modification/removal.
    pub fn start_with_all_dirs(
        files: Arc<RwLock<Vec<LauncherItem>>>,
        config: &Config,
        recursive_dirs: Vec<PathBuf>,
        non_recursive_dirs: Vec<PathBuf>,
        search_index: Option<Arc<crate::search_index::SearchIndexEngine>>,
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

        let scope = Arc::new(crate::search_index::scope::SearchScope::new(&config.search));
        let index_sender = search_index.map(Self::spawn_index_worker);
        let (new_dirs, new_dirs_receiver) = mpsc::channel::<PathBuf>();

        let callback = {
            let files = files.clone();
            let roots = watch_roots.clone();
            let recursive_roots = recursive_dirs.clone();
            let index_sender = index_sender.clone();
            let ignored_dirs = ignored_dirs.clone();
            let ignored_exts = ignored_exts.clone();
            let new_dirs = new_dirs.clone();
            move |res: Result<Event, notify::Error>| {
                let Ok(event) = res else { return };
                // Directories are watched one by one (see below), so a new one needs
                // its own watch. Adding it from this callback would deadlock the
                // inotify event loop, hence the hand-off to another thread.
                // A folder moved in from elsewhere arrives as a rename, not a create.
                // On every platform only the folder itself is reported, not the
                // files already inside it, so it is rescanned (and on Linux watched).
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
                ) {
                    for path in &event.paths {
                        if path.is_dir() && recursive_roots.iter().any(|root| path.starts_with(root)) {
                            let _ = new_dirs.send(path.clone());
                        }
                    }
                }
                Self::handle_event(&files, &event, &ignored_dirs, &ignored_exts, &roots, &index_sender);
            }
        };
        let watcher = Arc::new(Mutex::new(RecommendedWatcher::new(callback, notify::Config::default()).ok()?));

        // Watch each directory the index covers instead of whole trees recursively:
        // a recursive watch also registered node_modules, .git, target... (23k
        // directories, ~1s at startup on a developer machine) and can exceed the
        // inotify watch limit on distros that default to 8192.
        let mut watched_any = false;
        {
            let mut guard = watcher.lock().unwrap_or_else(|error| error.into_inner());
            for dir in &recursive_dirs {
                if PER_DIRECTORY_WATCHES {
                    for sub in Self::watchable_dirs(dir, &scope) {
                        if guard.watch(&sub, RecursiveMode::NonRecursive).is_ok() {
                            watched_any = true;
                        }
                    }
                } else if guard.watch(dir, RecursiveMode::Recursive).is_ok() {
                    watched_any = true;
                }
            }
            for dir in &non_recursive_dirs {
                if guard.watch(dir, RecursiveMode::NonRecursive).is_ok() {
                    watched_any = true;
                }
            }
        }
        if !watched_any {
            return None;
        }

        Self::spawn_new_dir_watcher(
            Arc::downgrade(&watcher),
            new_dirs_receiver,
            scope,
            files,
            ignored_dirs,
            ignored_exts,
            watch_roots,
            index_sender,
        );
        Some(Self { _watcher: watcher, _new_dirs: new_dirs })
    }

    /// `root` and every directory below it that search indexing covers.
    fn watchable_dirs(root: &Path, scope: &crate::search_index::scope::SearchScope) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || (entry.file_type().is_dir() && scope.allows_name(entry.path())))
            .flatten()
            .filter(|entry| entry.file_type().is_dir())
            .map(walkdir::DirEntry::into_path)
            .collect()
    }

    /// Picks up directories that appear after startup: lists the files already
    /// inside them (an extracted archive, a moved-in or renamed folder) and, on
    /// Linux, adds their watches.
    #[allow(clippy::too_many_arguments)]
    fn spawn_new_dir_watcher(
        watcher: Weak<Mutex<RecommendedWatcher>>,
        receiver: mpsc::Receiver<PathBuf>,
        scope: Arc<crate::search_index::scope::SearchScope>,
        files: Arc<RwLock<Vec<LauncherItem>>>,
        ignored_dirs: Vec<String>,
        ignored_exts: Vec<String>,
        watch_roots: Arc<Vec<PathBuf>>,
        index_sender: Option<Sender<PathBuf>>,
    ) {
        let _ = std::thread::Builder::new()
            .name("tiarun-watch-dirs".to_string())
            .spawn(move || {
                while let Ok(dir) = receiver.recv() {
                    if !scope.allows_name(&dir) {
                        continue;
                    }
                    let Some(watcher) = watcher.upgrade() else { break };
                    if PER_DIRECTORY_WATCHES {
                        for sub in Self::watchable_dirs(&dir, &scope) {
                            let _ = watcher.lock().unwrap_or_else(|error| error.into_inner())
                                .watch(&sub, RecursiveMode::NonRecursive);
                        }
                    }
                    drop(watcher);
                    // Files that landed before the watch existed.
                    let created: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
                        .min_depth(1)
                        .into_iter()
                        .filter_entry(|entry| scope.allows_name(entry.path()))
                        .flatten()
                        .map(walkdir::DirEntry::into_path)
                        .collect();
                    if !created.is_empty() {
                        let mut event = Event::new(EventKind::Create(notify::event::CreateKind::Any));
                        event.paths = created;
                        Self::handle_event(&files, &event, &ignored_dirs, &ignored_exts, &watch_roots, &index_sender);
                    }
                }
            });
    }

    /// Editors and downloads emit bursts of modify events while a file is being
    /// written. Indexing on every event re-extracted the document (PDFs included)
    /// each time; this worker waits until a path has been quiet before indexing it.
    fn spawn_index_worker(index: Arc<crate::search_index::SearchIndexEngine>) -> Sender<PathBuf> {
        const QUIET: Duration = Duration::from_millis(500);
        let (send, receive) = mpsc::channel::<PathBuf>();
        let _ = std::thread::Builder::new()
            .name("tiarun-watch-index".to_string())
            .spawn(move || {
                let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
                loop {
                    let next = if pending.is_empty() {
                        receive.recv().map_err(|_| RecvTimeoutError::Disconnected)
                    } else {
                        receive.recv_timeout(QUIET / 4)
                    };
                    match next {
                        Ok(path) => {
                            pending.insert(path, Instant::now());
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    let now = Instant::now();
                    let settled: Vec<PathBuf> = pending
                        .iter()
                        .filter(|(_, changed)| now.duration_since(**changed) >= QUIET)
                        .map(|(path, _)| path.clone())
                        .collect();
                    for path in settled {
                        pending.remove(&path);
                        if path.is_file() {
                            index.index_file(&path);
                        } else if !path.exists() {
                            // A deleted or renamed-away directory removes its whole subtree.
                            index.delete_tree(&path);
                        }
                    }
                }
            });
        send
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
        search_index: &Option<Sender<PathBuf>>,
    ) {
        use notify::event::{ModifyKind, RenameMode};
        // Paths that no longer exist: deletions, and the old name of a rename.
        let gone: &[PathBuf] = match event.kind {
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => &event.paths,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => &event.paths[..event.paths.len().min(1)],
            _ => &[],
        };
        for path in gone.iter().filter(|path| !path.exists()) {
            // A directory takes everything below it along.
            if let Ok(mut lock) = files.write() {
                lock.retain(|f| !Self::is_same_or_inside(Path::new(&f.exec_or_path), path));
            }
            if let Some(index) = search_index {
                let _ = index.send(path.clone());
            }
        }

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

                    if path.is_file() {
                        if let Some(index) = search_index {
                            let _ = index.send(path.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// `path == ancestor` or `path` lies below it (case-insensitive on Windows,
    /// ignoring the `\\?\` verbatim prefix).
    fn is_same_or_inside(path: &Path, ancestor: &Path) -> bool {
        crate::search_index::scope::strip_prefix_case_insensitive(path, ancestor).is_some()
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

    #[test]
    fn test_file_watcher_updates_search_index() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_watch_idx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let files = Arc::new(RwLock::new(Vec::new()));
        let mut config = Config::default();
        config.search.paths.push(crate::config::SearchPathConfig {
            path: temp_dir.to_str().unwrap().to_owned(),
            depth: 1,
            max_depth: Some(1),
            exclude: Vec::new(),
        });
        let db_path = temp_dir.join("index.db");
        let search_index = Arc::new(crate::search_index::SearchIndexEngine::with_db_path(&config, db_path));

        let _watcher = FileWatcher::start_with_all_dirs(
            files.clone(),
            &config,
            vec![temp_dir.clone()],
            Vec::new(),
            Some(search_index.clone()),
        );
        assert!(_watcher.is_some());

        // Create a new text file with unique content
        let new_file = temp_dir.join("realtime_doc.txt");
        std::fs::write(&new_file, "this file has special_unique_phrase_2026 inside").unwrap();

        // Give the file watcher thread a window to process and index the event
        let mut indexed = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let matches = search_index.search_content("special_unique_phrase_2026", 10, &|| false);
            if !matches.is_empty() {
                indexed = true;
                assert_eq!(matches[0].file_name, "realtime_doc.txt");
                break;
            }
        }
        assert!(indexed, "FileWatcher should index newly created file into search index");

        // Remove the file
        let _ = std::fs::remove_file(&new_file);
        let mut removed = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let matches = search_index.search_content("special_unique_phrase_2026", 10, &|| false);
            if matches.is_empty() {
                removed = true;
                break;
            }
        }
        assert!(removed, "FileWatcher should delete removed file from search index");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn watches_directories_created_after_startup() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_watch_newdirs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("existing/deep")).unwrap();
        std::fs::create_dir_all(temp_dir.join("node_modules/pkg")).unwrap();

        let files = Arc::new(RwLock::new(Vec::new()));
        let _watcher = FileWatcher::start_with_dirs(files.clone(), &Config::default(), vec![temp_dir.clone()]);
        assert!(_watcher.is_some());
        let has = |name: &str| files.read().unwrap().iter().any(|f| f.name == name);
        let wait_for = |name: &str| {
            (0..40).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                has(name)
            })
        };

        // Pre-existing nested directory is watched (non-recursive watches per dir).
        std::fs::write(temp_dir.join("existing/deep/old_dir_file.txt"), "x").unwrap();
        assert!(wait_for("old_dir_file.txt"));

        // A directory created later gets its own watch...
        std::fs::create_dir_all(temp_dir.join("fresh")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::fs::write(temp_dir.join("fresh/later_file.txt"), "x").unwrap();
        assert!(wait_for("later_file.txt"));

        // ...and files already inside a moved-in/extracted tree are picked up.
        let staged = std::env::temp_dir().join(format!("tiarun_watch_staged_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(staged.join("inner")).unwrap();
        std::fs::write(staged.join("inner/moved_file.txt"), "x").unwrap();
        std::fs::rename(&staged, temp_dir.join("moved_in")).unwrap();
        assert!(wait_for("moved_file.txt"));

        // Ignored directories stay ignored.
        std::fs::write(temp_dir.join("node_modules/pkg/dep.js"), "x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!has("dep.js"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn renaming_a_directory_updates_paths() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_watch_rename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("old_name/sub")).unwrap();

        let mut config = Config::default();
        config.search.paths.push(crate::config::SearchPathConfig {
            path: temp_dir.to_str().unwrap().to_owned(),
            depth: 5,
            max_depth: Some(5),
            exclude: Vec::new(),
        });
        let index = Arc::new(crate::search_index::SearchIndexEngine::with_db_path(&config, temp_dir.join("index.db")));
        let files = Arc::new(RwLock::new(Vec::new()));
        let _watcher = FileWatcher::start_with_all_dirs(
            files.clone(),
            &config,
            vec![temp_dir.clone()],
            Vec::new(),
            Some(index.clone()),
        );
        let wait = |check: &dyn Fn() -> bool| {
            (0..50).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                check()
            })
        };
        let listed = |path: &Path| files.read().unwrap().iter().any(|f| Path::new(&f.exec_or_path) == path);
        let indexed = |name: &str| index.search_files(name, 5).into_iter().map(|m| PathBuf::from(m.path)).collect::<Vec<_>>();

        let old_file = temp_dir.join("old_name/sub/report_tiarun.txt");
        std::fs::write(&old_file, "x").unwrap();
        assert!(wait(&|| listed(&old_file) && indexed("report_tiarun") == [old_file.clone()]));

        std::fs::rename(temp_dir.join("old_name"), temp_dir.join("new_name")).unwrap();
        let new_file = temp_dir.join("new_name/sub/report_tiarun.txt");
        assert!(
            wait(&|| listed(&new_file) && !listed(&old_file) && indexed("report_tiarun") == [new_file.clone()]),
            "renamed directory should replace old paths"
        );

        // Events inside the renamed directory must carry the new path.
        let later = temp_dir.join("new_name/sub/later_tiarun.txt");
        std::fs::write(&later, "x").unwrap();
        assert!(wait(&|| listed(&later)), "file created after rename should use the new path");
        assert!(!listed(&temp_dir.join("old_name/sub/later_tiarun.txt")));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
