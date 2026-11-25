use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use crate::config::Config;
use crate::launcher::{ItemType, LauncherItem};

/// inotify (Linux) needs one watch per directory, so a recursive watch would also
/// register ignored trees (node_modules, .git, target...). Windows'
/// ReadDirectoryChangesW watches a whole tree with a single handle, which is
/// cheaper than thousands of per-directory handles, so it stays recursive there.
const PER_DIRECTORY_WATCHES: bool = cfg!(target_os = "linux");

const HARD_IGNORED_NAMES: &[&str] = &[
    "node_modules", ".git", "target", "build", "dist", ".cache", "venv", ".venv",
    "__pycache__", ".cargo", ".rustup", ".local", "AppData", ".Trash", ".Trash-1000",
    ".idea", ".vscode", "vendor", "bin", "obj", ".next", ".nuxt", ".gradle", ".m2",
    ".npm", ".nvm", "proc", "sys", "dev",
];

fn allows_watch_dir(path: &Path, ignored_dirs: &[String]) -> bool {
    path.file_name().is_none_or(|name| {
        name.to_str().is_some_and(|name| {
            !name.starts_with('.')
                && !HARD_IGNORED_NAMES.iter().any(|ignored| ignored.eq_ignore_ascii_case(name))
                && !ignored_dirs.iter().any(|ignored| ignored.eq_ignore_ascii_case(name))
        })
    })
}

fn clean_path_prefix(path: &Path) -> &Path {
    let s = path.to_str().unwrap_or("");
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        Path::new(stripped)
    } else {
        path
    }
}

fn strip_prefix_case_insensitive(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let clean_path = clean_path_prefix(path);
    let clean_prefix = clean_path_prefix(prefix);

    #[cfg(target_os = "windows")]
    {
        use std::path::Component;
        let mut path_comps = clean_path.components();
        let mut prefix_comps = clean_prefix.components();

        loop {
            match (prefix_comps.next(), path_comps.next()) {
                (None, Some(rem)) => {
                    let mut result = PathBuf::from(rem.as_os_str());
                    for comp in path_comps {
                        result.push(comp.as_os_str());
                    }
                    return Some(result);
                }
                (None, None) => return Some(PathBuf::new()),
                (Some(p_comp), Some(c_comp)) => {
                    let match_comp = match (p_comp, c_comp) {
                        (Component::Prefix(p1), Component::Prefix(p2)) => {
                            p1.as_os_str().to_string_lossy().eq_ignore_ascii_case(&p2.as_os_str().to_string_lossy())
                        }
                        (Component::RootDir, Component::RootDir) => true,
                        (Component::CurDir, Component::CurDir) => true,
                        (Component::ParentDir, Component::ParentDir) => true,
                        (Component::Normal(n1), Component::Normal(n2)) => {
                            n1.to_string_lossy().eq_ignore_ascii_case(&n2.to_string_lossy())
                        }
                        _ => false,
                    };
                    if !match_comp {
                        return None;
                    }
                }
                (Some(_), None) => return None,
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        clean_path.strip_prefix(clean_prefix).ok().map(|p| p.to_path_buf())
    }
}

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
        let (new_dirs, new_dirs_receiver) = mpsc::channel::<PathBuf>();

        let callback = {
            let files = files.clone();
            let roots = watch_roots.clone();
            let recursive_roots = recursive_dirs.clone();
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
                Self::handle_event(&files, &event, &ignored_dirs, &ignored_exts, &roots);
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
                    for sub in Self::watchable_dirs(dir, &ignored_dirs) {
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
            files,
            ignored_dirs,
            ignored_exts,
            watch_roots,
        );
        Some(Self { _watcher: watcher, _new_dirs: new_dirs })
    }

    /// `root` and every directory below it that file watching covers.
    fn watchable_dirs(root: &Path, ignored_dirs: &[String]) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || (entry.file_type().is_dir() && allows_watch_dir(entry.path(), ignored_dirs)))
            .flatten()
            .filter(|entry| entry.file_type().is_dir())
            .map(walkdir::DirEntry::into_path)
            .collect()
    }

    /// Picks up directories that appear after startup: lists the files already
    /// inside them (an extracted archive, a moved-in or renamed folder) and, on
    /// Linux, adds their watches.
    fn spawn_new_dir_watcher(
        watcher: Weak<Mutex<RecommendedWatcher>>,
        receiver: mpsc::Receiver<PathBuf>,
        files: Arc<RwLock<Vec<LauncherItem>>>,
        ignored_dirs: Vec<String>,
        ignored_exts: Vec<String>,
        watch_roots: Arc<Vec<PathBuf>>,
    ) {
        let _ = std::thread::Builder::new()
            .name("tiarun-watch-dirs".to_string())
            .spawn(move || {
                while let Ok(dir) = receiver.recv() {
                    if !allows_watch_dir(&dir, &ignored_dirs) {
                        continue;
                    }
                    let Some(watcher) = watcher.upgrade() else { break };
                    if PER_DIRECTORY_WATCHES {
                        for sub in Self::watchable_dirs(&dir, &ignored_dirs) {
                            let _ = watcher.lock().unwrap_or_else(|error| error.into_inner())
                                .watch(&sub, RecursiveMode::NonRecursive);
                        }
                    }
                    drop(watcher);
                    // Files that landed before the watch existed.
                    let created: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
                        .min_depth(1)
                        .into_iter()
                        .filter_entry(|entry| allows_watch_dir(entry.path(), &ignored_dirs))
                        .flatten()
                        .map(walkdir::DirEntry::into_path)
                        .collect();
                    if !created.is_empty() {
                        let mut event = Event::new(EventKind::Create(notify::event::CreateKind::Any));
                        event.paths = created;
                        Self::handle_event(&files, &event, &ignored_dirs, &ignored_exts, &watch_roots);
                    }
                }
            });
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
        let meta = std::fs::metadata(path).ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or_else(|| path.is_dir());
        let item_type = if is_dir {
            ItemType::Dir
        } else {
            ItemType::File
        };
        let file_size = if is_dir { None } else { meta.map(|m| m.len()) };

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

        Some(
            LauncherItem::new(name, path_str, item_type, desc, false, None)
                .with_file_size(file_size),
        )
    }

    fn handle_event(
        files: &Arc<RwLock<Vec<LauncherItem>>>,
        event: &Event,
        ignored_dirs: &[String],
        ignored_exts: &[String],
        watch_roots: &[PathBuf],
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
                }
            }
            _ => {}
        }
    }

    /// `path == ancestor` or `path` lies below it (case-insensitive on Windows,
    /// ignoring the `\\?\` verbatim prefix).
    fn is_same_or_inside(path: &Path, ancestor: &Path) -> bool {
        strip_prefix_case_insensitive(path, ancestor).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_watcher_create_and_remove() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_watch_test_{}", std::process::id()));
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
        let files = Arc::new(RwLock::new(Vec::new()));
        let _watcher = FileWatcher::start_with_all_dirs(
            files.clone(),
            &config,
            vec![temp_dir.clone()],
            Vec::new(),
        );
        let wait = |check: &dyn Fn() -> bool| {
            (0..50).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                check()
            })
        };
        let listed = |path: &Path| files.read().unwrap().iter().any(|f| Path::new(&f.exec_or_path) == path);

        let old_file = temp_dir.join("old_name/sub/report_tiarun.txt");
        std::fs::write(&old_file, "x").unwrap();
        assert!(wait(&|| listed(&old_file)));

        std::fs::rename(temp_dir.join("old_name"), temp_dir.join("new_name")).unwrap();
        let new_file = temp_dir.join("new_name/sub/report_tiarun.txt");
        assert!(
            wait(&|| listed(&new_file) && !listed(&old_file)),
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
