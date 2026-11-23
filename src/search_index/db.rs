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
             PRAGMA cache_size = -2000;",
        )?;

        Self::init_schema(&mut conn)?;
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
        Ok(Self { conn })
    }

    fn init_schema(conn: &mut Connection) -> Result<()> {
        let version = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = tx.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version == SCHEMA_VERSION {
            return tx.commit();
        }
        if version != 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }

        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS files_meta (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT UNIQUE NOT NULL,
                filename TEXT NOT NULL,
                filename_norm TEXT NOT NULL,
                extension TEXT,
                size INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_files_meta_path ON files_meta(path);
            CREATE INDEX IF NOT EXISTS idx_files_meta_mtime ON files_meta(mtime);",
        )?;
        let has_legacy_fts = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'files_fts')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if has_legacy_fts {
            tx.execute_batch("ALTER TABLE files_fts RENAME TO legacy_files_fts;")?;
        }

        tx.execute_batch(
            "CREATE VIRTUAL TABLE files_fts USING fts5(
                path UNINDEXED,
                filename,
                filename_norm,
                content,
                content_norm,
                tokenize = 'unicode61 remove_diacritics 2'
            );",
        )?;
        if has_legacy_fts {
            tx.execute_batch(
                "INSERT OR REPLACE INTO files_fts (rowid, path, filename, filename_norm, content, content_norm)
                 SELECT m.id, m.path, m.filename, m.filename_norm, f.content, f.content_norm
                 FROM legacy_files_fts f JOIN files_meta m ON m.path = f.path
                 WHERE m.id = CAST(f.file_id AS INTEGER)
                 ORDER BY f.rowid;
                 DELETE FROM files_meta WHERE id NOT IN (SELECT rowid FROM files_fts);
                 DROP TABLE legacy_files_fts;",
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()
    }

    /// Retrieves existing file size and mtime to determine if re-indexing is required.
    pub fn get_file_meta(&self, path: &str) -> Option<(i64, u64, i64)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, size, mtime FROM files_meta WHERE path = ?1 LIMIT 1"
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

        let tx = self.conn.transaction()?;

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
        let tx = self.conn.transaction()?;
        if let Some(file_id) = tx.query_row(
            "SELECT id FROM files_meta WHERE path = ?1",
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
        let mut to_delete = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT path FROM files_meta")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let p: String = row.get(0)?;
                if !Path::new(&p).exists() {
                    to_delete.push(p);
                }
            }
        }

        let count = to_delete.len();
        for p in to_delete {
            self.delete_file(&p)?;
        }
        Ok(count)
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

        let sql = "
            SELECT rowid, path, filename,
                   COALESCE(NULLIF(snippet(files_fts, 3, '', '', '...', 25), ''), snippet(files_fts, 4, '', '', '...', 25)) AS snip,
                   bm25(files_fts, 0.0, 1.0, 1.0, 10.0, 8.0) AS rank
            FROM files_fts
            WHERE files_fts MATCH ?1 AND in_search_scope(path)
            ORDER BY rank
            LIMIT ?2
        ";

        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![&fts_query, i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            let file_id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let filename: String = row.get(2)?;
            let raw_snippet: String = row.get(3)?;
            let snippet = raw_snippet.replace('\n', " ").replace('\r', " ").trim().to_string();

            Ok(SearchDbMatch {
                file_id,
                path,
                filename,
                snippet,
            })
        })?;
        rows.collect()
    }

    /// Searches files by name using indexed prefix matching + Vietnamese accent insensitivity.
    pub fn search_files_by_name(&self, query: &str, limit: usize) -> Vec<SearchDbFileMatch> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let words = super::search_words(trimmed);

        if words.is_empty() {
            return Vec::new();
        }

        let is_vietnamese_accented = words.iter().any(|w| remove_vietnamese_accents(w) != *w);

        let fts_query = if is_vietnamese_accented {
            let orig = words.iter().map(|w| format!("\"{}\"*", w)).collect::<Vec<_>>().join(" AND ");
            let norm = words.iter().map(|w| format!("\"{}\"*", remove_vietnamese_accents(w))).collect::<Vec<_>>().join(" AND ");
            format!("(filename: ({orig})) OR (filename_norm: ({norm}))")
        } else {
            let norm = words.iter().map(|w| format!("\"{}\"*", w)).collect::<Vec<_>>().join(" AND ");
            format!("(filename_norm: ({norm}))")
        };

        let sql = "
            SELECT path, filename
            FROM files_fts
            WHERE files_fts MATCH ?1 AND in_search_scope(path)
            ORDER BY bm25(files_fts, 0.0, 10.0, 8.0, 0.0, 0.0)
            LIMIT ?2
        ";

        let mut stmt = match self.conn.prepare_cached(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map(params![&fts_query, i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            let path: String = row.get(0)?;
            let filename: String = row.get(1)?;
            Ok(SearchDbFileMatch { path, filename })
        }) {
            Ok(mapped) => mapped,
            Err(_) => return Vec::new(),
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
    let cache_dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    cache_dir.join("tiarun").join("search_index.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_index_and_reuses_metadata_id_for_updates() {
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

        let mut db = SearchDb::open(&db_path).unwrap();
        assert_eq!(db.count_indexed_files(), 1);
        assert_eq!(db.search_files_by_name("billing_config.txt", 1)[0].path, path);
        assert_eq!(db.search_content("config_value", 1)[0].file_id, 41);

        db.upsert_file(path, "billing_config.txt", Some("txt"), 14, 2, "updated_value", "updated_value")
            .unwrap();
        assert!(db.search_content("config_value", 1).is_empty());
        assert_eq!(db.search_content("updated_value", 1)[0].file_id, 41);
        drop(db);

        let mut db = SearchDb::open(&db_path).unwrap();
        assert_eq!(db.search_content("updated_value", 1)[0].file_id, 41);
        db.delete_file(path).unwrap();
        assert!(db.search_content("updated_value", 1).is_empty());
        assert!(db.search_files_by_name("billing_config.txt", 1).is_empty());
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

