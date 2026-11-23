pub mod db;
pub mod extractors;
pub mod crawler;
pub(crate) mod scope;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::config::Config;
use crate::content_search::ContentSearchEngine;
use scope::SearchScope;

pub use crate::content_search::ContentMatch;
pub use db::{get_default_db_path, temporary_db_path, SearchDb, SearchDbFileMatch, SearchDbMatch};
pub use extractors::DocumentExtractor;

static GLOBAL_INDEXING_PAUSED: AtomicBool = AtomicBool::new(false);

pub fn set_indexing_paused(paused: bool) {
    GLOBAL_INDEXING_PAUSED.store(paused, Ordering::Relaxed);
}

fn is_indexing_paused() -> bool {
    GLOBAL_INDEXING_PAUSED.load(Ordering::Relaxed)
}

pub(crate) fn search_words(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// `index_state` key holding the scope signature of the last completed crawl pass.
pub(crate) const COMPLETED_SCOPE_KEY: &str = "completed_scope";

pub struct SearchIndexEngine {
    db_path: PathBuf,
    scope: Arc<SearchScope>,
    /// Connection used by searches. Kept apart from `write_db` so a search never
    /// waits behind the file watcher extracting/upserting a document.
    read_db: Mutex<Option<SearchDb>>,
    write_db: Mutex<Option<SearchDb>>,
    initial_pass_complete: Arc<AtomicBool>,
}

impl SearchIndexEngine {
    pub fn new() -> Self {
        Self::for_config(&Config::default())
    }

    pub fn for_config(config: &Config) -> Self {
        Self::with_db_path(config, get_default_db_path())
    }

    pub fn with_db_path(config: &Config, db_path: PathBuf) -> Self {
        Self {
            db_path,
            scope: Arc::new(SearchScope::new(&config.search)),
            read_db: Mutex::new(None),
            write_db: Mutex::new(None),
            initial_pass_complete: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn open_db(&self) -> Option<SearchDb> {
        let db = SearchDb::open_scoped(&self.db_path, self.scope.clone()).ok()?;
        // A pass completed by an earlier run with the same scope is still valid:
        // the file watcher and the periodic crawl keep it fresh from here on, so
        // searches do not have to fall back to scanning the disk after a restart.
        if db.get_state(COMPLETED_SCOPE_KEY).is_some_and(|scope| scope == self.scope.signature()) {
            self.initial_pass_complete.store(true, Ordering::Release);
        }
        Some(db)
    }

    fn with_db<T>(&self, query: impl FnOnce(&SearchDb) -> T) -> Option<T> {
        let mut db = self.read_db.lock().unwrap_or_else(|error| error.into_inner());
        if db.is_none() {
            *db = self.open_db();
        }
        db.as_ref().map(query)
    }

    fn with_db_mut<T>(&self, query: impl FnOnce(&mut SearchDb) -> T) -> Option<T> {
        let mut db = self.write_db.lock().unwrap_or_else(|error| error.into_inner());
        if db.is_none() {
            *db = self.open_db();
        }
        db.as_mut().map(query)
    }

    /// Whether the index covers the whole current scope (opens the database once
    /// so a completed pass from an earlier run is recognised).
    fn is_ready(&self) -> bool {
        if !self.initial_pass_complete.load(Ordering::Acquire) {
            self.with_db(|_| ());
        }
        self.initial_pass_complete.load(Ordering::Acquire)
    }

    /// Indexes or re-indexes a single file into the SQLite database in real time.
    pub fn index_file(&self, path: &Path) -> bool {
        let clean = scope::clean_path_prefix(path);
        if !clean.is_file() || !self.scope.contains(clean) {
            return false;
        }

        let Ok(metadata) = clean.metadata() else { return false };
        let size = metadata.len();

        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let path_str = clean.to_string_lossy();

        let is_up_to_date = self.with_db_mut(|db| {
            db.get_file_meta(&path_str).is_some_and(|(_, db_size, db_mtime)| db_size == size && db_mtime == mtime)
        }).unwrap_or(false);

        if is_up_to_date {
            return true;
        }

        let filename = match clean.file_name() {
            Some(name) => name.to_string_lossy(),
            None => return false,
        };
        let extension = clean.extension().and_then(|ext| ext.to_str());

        // Every file is findable by name; only documents carry content.
        let doc = DocumentExtractor::extract_for_index(clean);
        self.with_db_mut(|db| {
            db.upsert_file(&path_str, &filename, extension, size, mtime, &doc.content, &doc.content_norm)
                .is_ok()
        })
        .unwrap_or(false)
    }

    /// Removes `path` and, if it was a directory, everything indexed below it.
    pub fn delete_tree(&self, path: &Path) -> bool {
        let clean = scope::clean_path_prefix(path);
        let root = clean.to_string_lossy();
        self.with_db_mut(|db| db.delete_tree(&root).is_ok()).unwrap_or(false)
    }

    /// Deletes a file from the SQLite search index in real time.
    pub fn delete_file(&self, path: &Path) -> bool {
        let clean = scope::clean_path_prefix(path);
        let path_str = clean.to_string_lossy();
        self.with_db_mut(|db| {
            db.delete_file(&path_str).is_ok()
        }).unwrap_or(false)
    }

    pub(crate) fn search_content(
        &self,
        query: &str,
        limit: usize,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<ContentMatch> {
        if limit == 0 || cancelled() {
            return Vec::new();
        }
        // Read readiness before querying so a concurrent first pass cannot make an
        // earlier snapshot look complete.
        let ready = self.is_ready();
        let indexed = self
            .with_db(|db| Self::run_cancellable(db, cancelled, || db.try_search_content(query, limit)))
            .and_then(Result::ok);
        if cancelled() {
            return Vec::new();
        }

        // Resolving line numbers re-reads each file, so do it in parallel.
        let mut results: Vec<ContentMatch> = indexed
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|m| {
                if cancelled() {
                    return None;
                }
                let path = Path::new(&m.path);
                if !path.is_file() {
                    return None;
                }
                let line_number = DocumentExtractor::find_matching_line_number(path, query);
                let is_docx = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("docx"));
                Some(ContentMatch {
                    file_path: m.path,
                    file_name: m.filename,
                    snippet: m.snippet,
                    line_number,
                    is_docx,
                })
            })
            .collect();

        // Until the index covers the whole scope, top up from a direct disk scan.
        // The index answers first: the scan is slow and only needed for the files
        // the crawler has not reached yet.
        if !ready && results.len() < limit && !cancelled() {
            let seen: HashSet<_> = results.iter().map(|result| result.file_path.clone()).collect();
            let scanned = ContentSearchEngine::search_in_scope(query, &self.scope, limit, cancelled);
            results.extend(
                scanned
                    .into_iter()
                    .filter(|result| !seen.contains(&result.file_path))
                    .take(limit - results.len()),
            );
        }

        results
    }

    /// Runs a query on `db`, interrupting it as soon as `cancelled()` turns true so a
    /// stale keystroke does not hold the worker while the user keeps typing.
    fn run_cancellable<T>(
        db: &SearchDb,
        cancelled: &(impl Fn() -> bool + Sync),
        query: impl FnOnce() -> T,
    ) -> T {
        let interrupt = db.interrupt_handle();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if cancelled() {
                        interrupt.interrupt();
                        return;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(5));
                }
            });
            let result = query();
            done.store(true, Ordering::Release);
            watcher.thread().unpark();
            result
        })
    }

    /// Searches files by name using the indexed database.
    pub fn search_files(&self, query: &str, limit: usize) -> Vec<SearchDbFileMatch> {
        self.with_db(|db| db.search_files_by_name(query, limit)).unwrap_or_default()
    }

    /// Starts the background crawler thread.
    pub fn start_crawler(
        &self,
        stop_flag: Arc<AtomicBool>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        crawler::SearchCrawler::start(
            self.db_path.clone(),
            self.scope.clone(),
            self.initial_pass_complete.clone(),
            stop_flag,
        )
    }
}

impl Default for SearchIndexEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchPathConfig;

    #[test]
    fn searches_current_scope_while_the_index_is_incomplete() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_index_scope_{}", std::process::id()));
        let current = temp_dir.join("notes");
        let previous = temp_dir.join("notes-archive");
        std::fs::create_dir_all(current.join("deep")).unwrap();
        std::fs::create_dir_all(&previous).unwrap();
        let db_path = temp_dir.join("index.db");
        let indexed = current.join("indexed_doc.txt");
        let pending = current.join("pending_doc.txt");
        let outside = previous.join("old_scope.txt");
        let too_deep = current.join("deep").join("old_depth.txt");
        let ignored = current.join("ignored.tmp");
        let content = "Quarterly config_value";

        let mut db = SearchDb::open(&db_path).unwrap();
        for path in [&indexed, &outside, &too_deep, &ignored] {
            let text = if path == &indexed { content } else { "config_value obsolete_scope" };
            std::fs::write(path, text).unwrap();
            db.upsert_file(
                path.to_str().unwrap(),
                path.file_name().unwrap().to_str().unwrap(),
                path.extension().and_then(|ext| ext.to_str()),
                text.len() as u64,
                0,
                text,
                text,
            ).unwrap();
        }
        std::fs::write(&pending, content).unwrap();
        drop(db);

        let mut config = Config::default();
        config.search.paths.push(SearchPathConfig {
            path: current.to_str().unwrap().to_owned(),
            depth: 1,
            max_depth: Some(1),
            exclude: Vec::new(),
        });
        let index = SearchIndexEngine::with_db_path(&config, db_path.clone());
        let search = |query| index.search_content(query, 10, &|| false);

        assert!(search("obsolete_scope").is_empty());
        assert_eq!(index.search_files("old_scope.txt", 1).len(), 0);
        assert_eq!(index.search_files("indexed_doc.txt", 1)[0].filename, "indexed_doc.txt");
        let mut names: Vec<_> = search("config_value").into_iter().map(|m| m.file_name).collect();
        names.sort();
        assert_eq!(names, ["indexed_doc.txt", "pending_doc.txt"]);

        let stop = Arc::new(AtomicBool::new(false));
        let crawler = index.start_crawler(stop.clone()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !index.initial_pass_complete.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        stop.store(true, Ordering::Relaxed);
        crawler.join().unwrap();

        assert!(index.initial_pass_complete.load(Ordering::Acquire));
        assert!(search("obsolete_scope").is_empty());
        let limited = index.search_content("config_value", 1, &|| false);
        assert_eq!(limited.len(), 1);
        assert!(names.contains(&limited[0].file_name));
        assert_eq!(search("config_value").len(), 2);

        // A restart with the same scope trusts the completed pass; a changed scope does not.
        assert!(SearchIndexEngine::with_db_path(&config, db_path.clone()).is_ready());
        config.search.paths[0].max_depth = Some(2);
        assert!(!SearchIndexEngine::with_db_path(&config, db_path).is_ready());
        drop(index);
        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn test_index_file_realtime_and_delete() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_realtime_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("index.db");

        let mut config = Config::default();
        config.search.paths.push(SearchPathConfig {
            path: temp_dir.to_str().unwrap().to_owned(),
            depth: 1,
            max_depth: Some(1),
            exclude: Vec::new(),
        });
        let index = SearchIndexEngine::with_db_path(&config, db_path);

        // Initially no matches
        assert!(index.search_content("tiarun_secret_token", 10, &|| false).is_empty());

        // Create new file
        let new_file = temp_dir.join("secret.txt");
        std::fs::write(&new_file, "this file contains tiarun_secret_token in line 1").unwrap();

        // Index in real time
        let indexed = index.index_file(&new_file);
        assert!(indexed, "File should be indexed successfully");

        // Now search should find it immediately
        let matches = index.search_content("tiarun_secret_token", 10, &|| false);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].file_name, "secret.txt");
        assert!(matches[0].snippet.contains("tiarun_secret_token"));

        // Delete file in real time
        let _ = std::fs::remove_file(&new_file);
        let deleted = index.delete_file(&new_file);
        assert!(deleted, "File should be deleted from index");

        // Now search should be empty again
        assert!(index.search_content("tiarun_secret_token", 10, &|| false).is_empty());

        // Non-documents are findable by name, but their text is not indexed.
        let code = temp_dir.join("payment_service.rs");
        std::fs::write(&code, "fn tiarun_code_token() {}").unwrap();
        assert!(index.index_file(&code));
        assert_eq!(index.search_files("payment service", 5)[0].filename, "payment_service.rs");
        assert!(index.search_content("tiarun_code_token", 10, &|| false).is_empty());

        drop(index);
        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
