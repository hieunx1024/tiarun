pub mod db;
pub mod extractors;
pub mod crawler;
pub(crate) mod scope;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::content_search::ContentSearchEngine;
use scope::SearchScope;

pub use crate::content_search::ContentMatch;
pub use db::{get_default_db_path, SearchDb, SearchDbFileMatch, SearchDbMatch};
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

pub struct SearchIndexEngine {
    db_path: PathBuf,
    scope: Arc<SearchScope>,
    db: Mutex<Option<SearchDb>>,
    initial_pass_complete: Arc<AtomicBool>,
}

impl SearchIndexEngine {
    pub fn new() -> Self {
        Self::for_config(&Config::default())
    }

    pub fn for_config(config: &Config) -> Self {
        Self::with_db_path(config, get_default_db_path())
    }

    fn with_db_path(config: &Config, db_path: PathBuf) -> Self {
        Self {
            db_path,
            scope: Arc::new(SearchScope::new(&config.search)),
            db: Mutex::new(None),
            initial_pass_complete: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn with_db<T>(&self, query: impl FnOnce(&SearchDb) -> T) -> Option<T> {
        let mut db = self.db.lock().unwrap_or_else(|error| error.into_inner());
        if db.is_none() {
            *db = SearchDb::open_scoped(&self.db_path, self.scope.clone()).ok();
        }
        db.as_ref().map(query)
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
        let ready = self.initial_pass_complete.load(Ordering::Acquire);
        let indexed = self.with_db(|db| db.try_search_content(query, limit)).and_then(Result::ok);
        let mut results = if ready && indexed.is_some() {
            Vec::new()
        } else {
            ContentSearchEngine::search_in_scope(query, &self.scope, limit, cancelled)
        };

        let seen: HashSet<_> = results.iter().map(|result| result.file_path.clone()).collect();
        for m in indexed.into_iter().flatten() {
            if results.len() == limit || cancelled() {
                break;
            }
            if seen.contains(&m.path) {
                continue;
            }

            let path = Path::new(&m.path);
            if !path.is_file() {
                continue;
            }
            let line_number = DocumentExtractor::find_matching_line_number(path, query);
            let is_docx = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("docx"));

            results.push(ContentMatch {
                file_path: m.path,
                file_name: m.filename,
                snippet: m.snippet,
                line_number,
                is_docx,
            });
        }

        results
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
        let index = SearchIndexEngine::with_db_path(&config, db_path);
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
        drop(index);
        std::fs::remove_dir_all(temp_dir).unwrap();
    }
}
