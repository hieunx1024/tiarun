use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension, Result, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::launcher::remove_vietnamese_accents;

use super::scope::SearchScope;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone)]
pub struct SearchDbMatch {
    pub file_id: i64,
    pub path: String,
    pub filename: String,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct SearchDbFileMatch {
    pub path: String,
    pub filename: String,
}

pub struct SearchDb {
    conn: Connection,
}

impl SearchDb {
    /// Opens or creates the SQLite index database with WAL mode enabled.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_scope(path, None)
    }

    pub(super) fn open_scoped(path: &Path, scope: Arc<SearchScope>) -> Result<Self> {
        Self::open_with_scope(path, Some(scope))
    }

    fn open_with_scope(path: &Path, scope: Option<Arc<SearchScope>>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut conn = Connection::open(path)?;

        // Performance & concurrency pragmas
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -8000;
             PRAGMA journal_size_limit = 67108864;",
        )?;

        let discarded_legacy = Self::init_schema(&mut conn)?;
        // Idempotent additions that do not need a schema version bump.
        // The NOCASE index keeps the case-insensitive `path` lookups below from
        // scanning the whole table (once per crawled file).
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_files_meta_path_nocase ON files_meta(path COLLATE NOCASE);
             CREATE TABLE IF NOT EXISTS index_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        conn.create_scalar_function(
            "in_search_scope",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| {
                Ok(scope.as_ref().is_none_or(|scope| {
                    matches!(ctx.get_raw(0), ValueRef::Text(path)
                        if std::str::from_utf8(path).is_ok_and(|path| scope.contains(Path::new(path))))
                }))
            },
        )?;
        let db = Self { conn };
        if discarded_legacy {
            // Best effort: another connection may hold a read transaction.
            let _ = db.reclaim_free_space();
        }
        Ok(db)
    }

    /// Creates the schema. Returns true when an index from a pre-1.0 build was
    /// discarded (its caller then reclaims the space).
    fn init_schema(conn: &mut Connection) -> Result<bool> {
        let version = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version == SCHEMA_VERSION {
            return Ok(false);
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = tx.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version == SCHEMA_VERSION {
            tx.commit()?;
            return Ok(false);
        }
        if version != 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }

        // Pre-1.0 indexes are rebuilt rather than migrated: copying them meant
        // rewriting every row (code and package caches included) only for the
        // next crawl pass to delete most of it. The crawler re-indexes in the background.
        let legacy = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('files_fts', 'files_meta'))",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        tx.execute_batch(
            "DROP TABLE IF EXISTS files_fts;
             DROP TABLE IF EXISTS files_meta;

             CREATE TABLE files_meta (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT UNIQUE NOT NULL,
                filename TEXT NOT NULL,
                filename_norm TEXT NOT NULL,
                extension TEXT,
                size INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL
             );
             CREATE INDEX idx_files_meta_path ON files_meta(path);
             CREATE INDEX idx_files_meta_mtime ON files_meta(mtime);

             CREATE VIRTUAL TABLE files_fts USING fts5(
                path UNINDEXED,
                filename,
                filename_norm,
                content,
                content_norm,
                tokenize = 'unicode61 remove_diacritics 2'
             );",
        )?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(legacy)
    }

    /// Retrieves existing file size and mtime to determine if re-indexing is required.
    pub fn get_file_meta(&self, path: &str) -> Option<(i64, u64, i64)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, size, mtime FROM files_meta WHERE path = ?1 COLLATE NOCASE LIMIT 1"
        ).ok()?;

        stmt.query_row(params![path], |row| {
            let id: i64 = row.get(0)?;
            let size: i64 = row.get(1)?;
            let mtime: i64 = row.get(2)?;
            Ok((id, size as u64, mtime))
        }).ok()
    }

    /// Upserts a document into `files_meta` and `files_fts`.
    pub fn upsert_file(
        &mut self,
        path: &str,
        filename: &str,
        extension: Option<&str>,
        size: u64,
        mtime: i64,
        content: &str,
        content_norm: &str,
    ) -> Result<()> {
        let filename_norm = remove_vietnamese_accents(filename);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let tx = self.conn.savepoint()?;

        let file_id: i64 = tx.query_row(
            "INSERT INTO files_meta (path, filename, filename_norm, extension, size, mtime, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                 filename = excluded.filename,
                 filename_norm = excluded.filename_norm,
                 extension = excluded.extension,
                 size = excluded.size,
                 mtime = excluded.mtime,
                 indexed_at = excluded.indexed_at
             RETURNING id",
            params![
                path,
                filename,
                &filename_norm,
                extension,
                size as i64,
                mtime,
                now
            ],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT OR REPLACE INTO files_fts (rowid, path, filename, filename_norm, content, content_norm)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                file_id,
                path,
                filename,
                &filename_norm,
                content,
                content_norm,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Removes a deleted file from the index.
    pub fn delete_file(&mut self, path: &str) -> Result<()> {
        let tx = self.conn.savepoint()?;
        if let Some(file_id) = tx.query_row(
            "SELECT id FROM files_meta WHERE path = ?1 COLLATE NOCASE",
            params![path],
            |row| row.get::<_, i64>(0),
        ).optional()? {
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![file_id])?;
            tx.execute("DELETE FROM files_meta WHERE id = ?1", params![file_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Cleans up index entries for files that no longer exist on disk.
    pub fn cleanup_missing_files(&mut self) -> Result<usize> {
        self.delete_where(|path| !Path::new(path).exists())
    }

    /// Removes `root` and every entry below it (a deleted or renamed directory).
    pub fn delete_tree(&mut self, root: &str) -> Result<usize> {
        let separator = std::path::MAIN_SEPARATOR;
        let prefix = format!("{}{separator}", root.trim_end_matches(separator));
        // Range scan on the path index: every path that starts with `prefix`
        // sorts between `prefix` and `prefix` with its separator bumped by one.
        let upper = format!("{}{}", root.trim_end_matches(separator), char::from(separator as u8 + 1));
        let mut paths: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT path FROM files_meta WHERE path = ?1 COLLATE NOCASE
                 UNION SELECT path FROM files_meta WHERE path >= ?2 AND path < ?3",
            )?;
            let rows = stmt.query_map(params![root, prefix, upper], |row| row.get(0))?;
            rows.collect::<Result<_>>()?
        };
        paths.dedup();
        for chunk in paths.chunks(256) {
            self.in_transaction(|db| {
                for path in chunk {
                    db.delete_file(path)?;
                }
                Ok(())
            })?;
        }
        Ok(paths.len())
    }

    /// Removes entries a narrower scope no longer covers (e.g. a newly excluded
    /// directory), so they stop costing query time and disk space.
    pub(super) fn cleanup_out_of_scope(&mut self, scope: &SearchScope) -> Result<usize> {
        let removed = self.delete_where(|path| !scope.contains(Path::new(path)))?;
        if removed > 0 {
            self.reclaim_free_space()?;
        }
        Ok(removed)
    }

    fn delete_where(&mut self, should_delete: impl Fn(&str) -> bool) -> Result<usize> {
        let mut to_delete = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT path FROM files_meta")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let path: String = row.get(0)?;
                if should_delete(&path) {
                    to_delete.push(path);
                }
            }
        }

        // Short transactions so real-time watcher updates are not locked out.
        for chunk in to_delete.chunks(256) {
            self.in_transaction(|db| {
                for path in chunk {
                    db.delete_file(path)?;
                }
                Ok(())
            })?;
        }
        Ok(to_delete.len())
    }

    /// One-time cleanup for indexes built before content indexing was limited to
    /// documents: drops the stored text of source code and other non-documents
    /// (their names stay searchable).
    pub(super) fn strip_non_document_content(&mut self) -> Result<usize> {
        const KEY: &str = "content_policy";
        const POLICY: &str = "documents-v1";
        if self.get_state(KEY).as_deref() == Some(POLICY) {
            return Ok(0);
        }
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT id, path FROM files_meta")?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, path) = row?;
                if !super::extractors::DocumentExtractor::is_document(Path::new(&path)) {
                    ids.push(id);
                }
            }
            ids
        };
        for chunk in ids.chunks(256) {
            self.in_transaction(|db| {
                let mut stmt = db
                    .conn
                    .prepare_cached("UPDATE files_fts SET content = '', content_norm = '' WHERE rowid = ?1")?;
                for id in chunk {
                    stmt.execute(params![id])?;
                }
                Ok(())
            })?;
        }
        if !ids.is_empty() {
            self.reclaim_free_space()?;
        }
        self.set_state(KEY, POLICY)?;
        Ok(ids.len())
    }

    /// Shrinks the file after large deletions (SQLite otherwise keeps the pages).
    fn reclaim_free_space(&self) -> Result<()> {
        let page_size: i64 = self.conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
        let pages: i64 = self.conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
        let free: i64 = self.conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
        if free * page_size >= 64 * 1024 * 1024 && free * 2 >= pages {
            // In WAL mode VACUUM rewrites the database through the WAL; truncate it
            // afterwards or the reclaimed space just moves into the -wal file.
            self.conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(())
    }

    /// Runs several writes in one transaction. FTS5 writes a new index segment per
    /// transaction, so batching keeps the index compact and indexing fast.
    pub fn in_transaction<T>(&mut self, writes: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match writes(self) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn get_state(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM index_state WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO index_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub(super) fn interrupt_handle(&self) -> rusqlite::InterruptHandle {
        self.conn.get_interrupt_handle()
    }

    /// Sanitizes and prepares a search query for FTS5 boolean match.
    fn prepare_fts_query(raw: &str) -> String {
        let all_words = super::search_words(raw);

        if all_words.is_empty() {
            return String::new();
        }

        // Single character tokens in full-text content search cause catastrophic FTS5 posting-list scans
        // (e.g. "b"* matches every single word starting with 'b' across 8000+ files, taking >18s).
        // - If only 1 word and length < 2, return empty (don't scan millions of rows).
        // - If multiple words, discard 1-character tokens that are still being typed.
        let words: Vec<String> = if all_words.len() == 1 {
            if all_words[0].chars().count() < 2 {
                return String::new();
            }
            all_words
        } else {
            let filtered: Vec<String> = all_words.into_iter().filter(|w| w.chars().count() >= 2).collect();
            if filtered.is_empty() {
                return String::new();
            }
            filtered
        };

        let is_vietnamese_accented = words.iter().any(|w| remove_vietnamese_accents(w) != *w);

        // Build tokens: exact word for earlier words; prefix wildcard ONLY for the last word if >= 3 chars
        let make_tokens = |list: &[String]| -> String {
            list.iter()
                .enumerate()
                .map(|(idx, w)| {
                    if idx == list.len() - 1 && w.chars().count() >= 3 {
                        format!("\"{}\"*", w)
                    } else {
                        format!("\"{}\"", w)
                    }
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        };

        let norm_words: Vec<String> = words.iter().map(|w| remove_vietnamese_accents(w)).collect();
        let norm_tokens = make_tokens(&norm_words);

        // Build exact phrase if multiple words
        let phrase_part = if words.len() > 1 {
            let phrase_norm = norm_words.join(" ");
            let phrase_orig = words.join(" ");
            format!("(content: \"{phrase_orig}\") OR (content_norm: \"{phrase_norm}\") OR ")
        } else {
            String::new()
        };

        if is_vietnamese_accented {
            let orig_tokens = make_tokens(&words);
            format!("{phrase_part}(content: ({orig_tokens})) OR (content_norm: ({norm_tokens}))")
        } else {
            format!("{phrase_part}(content: ({norm_tokens})) OR (content_norm: ({norm_tokens}))")
        }
    }

    /// Searches file content and metadata using FTS5 with BM25 ranking.
    pub fn search_content(&self, query: &str, limit: usize) -> Vec<SearchDbMatch> {
        self.try_search_content(query, limit).unwrap_or_default()
    }

    pub(super) fn try_search_content(&self, query: &str, limit: usize) -> Result<Vec<SearchDbMatch>> {
        let fts_query = Self::prepare_fts_query(query);
        if fts_query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        // Ranking through FTS5's hidden `rank` column lets FTS5 order the matches
        // itself, so the per-row work below only runs for the rows that are
        // actually returned instead of for every match.
        //
        // The snippet is cut around the first occurrence of the query instead of
        // using FTS5 `snippet()`, which re-tokenizes the whole document: for large
        // PDFs/logs that took seconds per query. `content_norm` is a char-for-char
        // accent-folded copy of `content`, so a position found in one is valid in
        // the other.
        let words: Vec<String> = super::search_words(query)
            .iter()
            .map(|word| remove_vietnamese_accents(word))
            .filter(|word| word.chars().count() >= 2)
            .collect();
        let phrase = words.join(" ");
        let longest = words.iter().max_by_key(|word| word.len()).cloned().unwrap_or_default();

        let sql = "
            SELECT rowid, path, filename,
                   COALESCE(NULLIF(instr(content_norm, ?3), 0), NULLIF(instr(content_norm, ?4), 0), 1)
            FROM files_fts
            WHERE files_fts MATCH ?1
              AND rank MATCH 'bm25(0.0, 1.0, 1.0, 10.0, 8.0)'
              AND in_search_scope(path)
            ORDER BY rank
            LIMIT ?2
        ";
        let mut stmt = self.conn.prepare_cached(sql)?;
        let found = stmt
            .query_map(
                params![&fts_query, i64::try_from(limit).unwrap_or(i64::MAX), &phrase, &longest],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?)),
            )?
            .collect::<Result<Vec<_>>>()?;

        const BEFORE: i64 = 30;
        const WINDOW: i64 = 110;
        let mut excerpt = self
            .conn
            .prepare_cached("SELECT substr(content, ?2, ?3) FROM files_fts WHERE rowid = ?1")?;
        found
            .into_iter()
            .map(|(file_id, path, filename, position)| {
                let start = (position - BEFORE).max(1);
                let text: String = excerpt
                    .query_row(params![file_id, start, WINDOW + 1], |row| row.get(0))
                    .optional()?
                    .unwrap_or_default();
                Ok(SearchDbMatch { file_id, path, filename, snippet: Self::format_excerpt(&text, start > 1, WINDOW) })
            })
            .collect()
    }

    /// Flattens an excerpt to one line and marks truncated ends with `...`.
    fn format_excerpt(text: &str, cut_start: bool, window: i64) -> String {
        let window = usize::try_from(window).unwrap_or(usize::MAX);
        let cut_end = text.chars().count() > window;
        let mut body: String = text.chars().take(window).collect();
        if cut_start {
            // Drop the partial first word (never past the match itself).
            if let Some(space) = body.find(char::is_whitespace).filter(|&index| index < 30) {
                body.drain(..space);
            }
        }
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        format!(
            "{}{}{}",
            if cut_start { "..." } else { "" },
            body,
            if cut_end { "..." } else { "" }
        )
    }

    /// Searches files by name (accent-insensitive substring match on every word).
    ///
    /// This scans the small `files_meta` table instead of FTS5: FTS5 shares one
    /// index for names and contents, so a short prefix like `"te"*` had to merge
    /// the posting lists of every content token and could take seconds on a cold cache.
    pub fn search_files_by_name(&self, query: &str, limit: usize) -> Vec<SearchDbFileMatch> {
        let words: Vec<String> = super::search_words(query)
            .iter()
            .map(|word| remove_vietnamese_accents(word))
            .collect();
        if words.is_empty() || limit == 0 {
            return Vec::new();
        }

        // Words only contain alphanumerics, so they need no LIKE escaping.
        let conditions = (1..=words.len())
            .map(|index| format!("filename_norm LIKE ?{index}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let first = words.len() + 1;
        let sql = format!(
            "SELECT path, filename FROM (
                SELECT path, filename FROM files_meta
                WHERE {conditions}
                ORDER BY CASE WHEN filename_norm LIKE ?{first} THEN 0 ELSE 1 END, length(filename)
                LIMIT ?{limit_param}
             )
             WHERE in_search_scope(path)
             LIMIT ?{limit_out}",
            limit_param = first + 1,
            limit_out = first + 2,
        );

        let Ok(mut stmt) = self.conn.prepare_cached(&sql) else {
            return Vec::new();
        };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut values: Vec<rusqlite::types::Value> = words
            .iter()
            .map(|word| format!("%{word}%").into())
            .collect();
        values.push(format!("{}%", words[0]).into());
        // Over-fetch so rows outside the current scope do not starve the result.
        values.push(limit.saturating_mul(4).max(200).into());
        values.push(limit.into());

        let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(SearchDbFileMatch { path: row.get(0)?, filename: row.get(1)? })
        }) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok).collect()
    }

    /// Returns the total count of indexed documents.
    pub fn count_indexed_files(&self) -> usize {
        let count: i64 = self.conn
            .query_row("SELECT COUNT(*) FROM files_meta", [], |row| row.get(0))
            .unwrap_or(0);
        count as usize
    }
}

/// Helper function to get the default search index database path.
pub fn get_default_db_path() -> PathBuf {
    // Unit tests must never touch (or migrate) the user's real index.
    if cfg!(test) {
        return temporary_db_path();
    }
    let cache_dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    cache_dir.join("tiarun").join("search_index.db")
}

/// A per-process database in the temp directory, for tests and throwaway engines.
pub fn temporary_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("tiarun_search_index_{}.db", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_pre_1_0_index_instead_of_migrating() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_legacy_index_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("legacy.db");
        let path = "/legacy/billing_config.txt";
        {
            let legacy = Connection::open(&db_path).unwrap();
            legacy.execute_batch(
                "CREATE TABLE files_meta (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT UNIQUE NOT NULL,
                    filename TEXT NOT NULL,
                    filename_norm TEXT NOT NULL,
                    extension TEXT,
                    size INTEGER NOT NULL,
                    mtime INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL
                 );
                 CREATE VIRTUAL TABLE files_fts USING fts5(
                    file_id UNINDEXED, path UNINDEXED, filename, filename_norm, content, content_norm,
                    tokenize = 'unicode61 remove_diacritics 2'
                 );
                 INSERT INTO files_meta VALUES
                    (41, '/legacy/billing_config.txt', 'billing_config.txt', 'billing_config.txt', 'txt', 12, 1, 1),
                    (42, '/legacy/incomplete.txt', 'incomplete.txt', 'incomplete.txt', 'txt', 12, 1, 1);
                 INSERT INTO files_fts VALUES
                    ('41', '/legacy/billing_config.txt', 'billing_config.txt',
                     'billing_config.txt', 'config_value', 'config_value');",
            ).unwrap();
        }

        // The pre-1.0 index is discarded and rebuilt by the crawler, not migrated.
        let mut db = SearchDb::open(&db_path).unwrap();
        assert_eq!(db.count_indexed_files(), 0);
        assert!(db.search_content("config_value", 1).is_empty());
        let version: i64 = db.conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        db.upsert_file(path, "billing_config.txt", Some("txt"), 14, 2, "updated_value", "updated_value")
            .unwrap();
        drop(db);
        let mut db = SearchDb::open(&db_path).unwrap();
        assert_eq!(db.search_content("updated_value", 1)[0].path, path);
        assert_eq!(db.search_files_by_name("billing config", 1)[0].path, path);
        db.delete_file(path).unwrap();
        assert_eq!(db.count_indexed_files(), 0);
        drop(db);
        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn test_search_db_vietnamese_accent_and_unaccented() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_test_db_{}", std::process::id()));
        let db_path = temp_dir.join("test_search.db");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut db = SearchDb::open(&db_path).expect("Failed to open test db");

        let content = "Hợp đồng mua bán thiết bị và phần mềm công nghệ thông tin năm 2026. Điều khoản thanh toán đợt 1.";
        let content_norm = remove_vietnamese_accents(content);

        db.upsert_file(
            "/home/user/Documents/Hop_dong_2026.docx",
            "Hop_dong_2026.docx",
            Some("docx"),
            1024,
            1234567,
            content,
            &content_norm,
        ).expect("Failed to upsert file");

        assert_eq!(db.count_indexed_files(), 1);

        // 1. Search with unaccented query "hop dong" -> must match
        let matches = db.search_content("hop dong", 10);
        assert!(!matches.is_empty(), "Should match unaccented 'hop dong'");
        assert_eq!(matches[0].filename, "Hop_dong_2026.docx");

        // 2. Search with accented query "hợp đồng" -> must match
        let matches_accented = db.search_content("hợp đồng", 10);
        assert!(!matches_accented.is_empty(), "Should match accented 'hợp đồng'");

        // 3. Search with unaccented "thanh toan dot 1" -> must match
        let matches_payment = db.search_content("thanh toan", 10);
        assert!(!matches_payment.is_empty(), "Should match 'thanh toan'");

        // 4. Search by filename
        let file_matches = db.search_files_by_name("hop dong", 10);
        assert!(!file_matches.is_empty(), "Should find file by name");

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn snippet_is_cut_around_the_match_in_long_documents() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_test_snippet_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let mut db = SearchDb::open(&temp_dir.join("snippet.db")).unwrap();

        let filler = "lorem ipsum dolor sit amet ".repeat(400);
        let content = format!("{filler}Điều khoản thanh toán đợt 1 {filler}");
        db.upsert_file("/docs/contract.txt", "contract.txt", Some("txt"), 1, 1, &content, &remove_vietnamese_accents(&content))
            .unwrap();

        for query in ["thanh toan", "thanh toán"] {
            let matches = db.search_content(query, 5);
            assert_eq!(matches.len(), 1);
            let snippet = &matches[0].snippet;
            assert!(snippet.contains("thanh toán đợt 1"), "{snippet}");
            assert!(snippet.starts_with("...") && snippet.ends_with("..."), "{snippet}");
            assert!(snippet.chars().count() < 130, "{snippet}");
        }
        assert_eq!(db.search_files_by_name("tract", 5)[0].filename, "contract.txt");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn strips_stored_code_content_once_and_keeps_names() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_test_strip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let mut db = SearchDb::open(&temp_dir.join("strip.db")).unwrap();
        db.upsert_file("/p/server.go", "server.go", Some("go"), 1, 1, "func legacy_token()", "func legacy_token()").unwrap();
        db.upsert_file("/p/notes.md", "notes.md", Some("md"), 1, 1, "meeting legacy_token", "meeting legacy_token").unwrap();

        assert_eq!(db.strip_non_document_content().unwrap(), 1);
        let matches = db.search_content("legacy_token", 10);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].filename, "notes.md");
        assert_eq!(db.search_files_by_name("server", 5)[0].filename, "server.go");
        assert_eq!(db.strip_non_document_content().unwrap(), 0, "runs only once");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn delete_tree_removes_only_the_directory_subtree() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_test_tree_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let mut db = SearchDb::open(&temp_dir.join("tree.db")).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let paths = [
            format!("{sep}docs{sep}fo"),
            format!("{sep}docs{sep}fo{sep}a.txt"),
            format!("{sep}docs{sep}fo{sep}deep{sep}b.txt"),
            format!("{sep}docs{sep}foo{sep}c.txt"),
            format!("{sep}docs{sep}fo.txt"),
        ];
        for path in &paths {
            db.upsert_file(path, "x.txt", Some("txt"), 1, 1, "", "").unwrap();
        }
        assert_eq!(db.delete_tree(&paths[0]).unwrap(), 3);
        let left: Vec<String> = db
            .conn
            .prepare("SELECT path FROM files_meta ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(left, [paths[4].clone(), paths[3].clone()]);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_phrase_and_int_main() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_test_phrase_{}", std::process::id()));
        let db_path = temp_dir.join("test_phrase.db");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut db = SearchDb::open(&db_path).expect("Failed to open test db");

        // 1. hello.cpp with 'int main(){'
        let hello_code = "#include<iostream>\nusing namespace std;\nint Tsum(int a, int b){\n    return a+b;\n}\nint main(){\n    cout<<\"nhap vao so a: \";\n}";
        let hello_norm = remove_vietnamese_accents(hello_code);
        db.upsert_file(
            "/home/hieunx/hello.cpp",
            "hello.cpp",
            Some("cpp"),
            274,
            1000,
            hello_code,
            &hello_norm,
        ).unwrap();

        // 2. internalOnboarding.jsp (starts with 'int', content has 'main')
        let jsp_code = "<%@page import=\"java.util.Collections\"%>\nString id = \"internalOnboarding-\";\n// path: webapp/src/main/webapp/WEB-INF";
        let jsp_norm = remove_vietnamese_accents(jsp_code);
        db.upsert_file(
            "/home/hieunx/Desktop/projects/exo-social/webapp/src/main/webapp/WEB-INF/jsp/portlet/internalOnboarding.jsp",
            "internalOnboarding.jsp",
            Some("jsp"),
            599,
            1000,
            jsp_code,
            &jsp_norm,
        ).unwrap();

        // Query: "int main()"
        let matches = db.search_content("int main()", 10);
        assert!(!matches.is_empty(), "Should return matches for 'int main()'");
        assert_eq!(matches[0].filename, "hello.cpp", "hello.cpp MUST be #1 match for 'int main()'");
        assert!(!matches.iter().any(|m| m.filename == "internalOnboarding.jsp"),
            "internalOnboarding.jsp must NOT match content search for 'int main()'");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

