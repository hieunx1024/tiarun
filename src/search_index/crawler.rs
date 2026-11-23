use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use walkdir::WalkDir;

use super::db::SearchDb;
use super::extractors::{DocumentExtractor, ExtractedDocument};
use super::scope::SearchScope;

/// Writes are flushed in one transaction per batch: FTS5 creates an index segment
/// per transaction, so per-file commits fragment the index and slow down queries.
const BATCH_FILES: usize = 64;
const BATCH_BYTES: usize = 8 * 1024 * 1024;

enum PendingWrite {
    Upsert {
        path: String,
        filename: String,
        extension: Option<String>,
        size: u64,
        mtime: i64,
        doc: ExtractedDocument,
    },
}

pub struct SearchCrawler;

impl SearchCrawler {
    /// Starts the background crawler thread with lowest CPU & I/O priority.
    pub(super) fn start(
        db_path: PathBuf,
        scope: Arc<SearchScope>,
        initial_pass_complete: Arc<AtomicBool>,
        stop_flag: Arc<AtomicBool>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("tiarun-crawler".to_string())
            .spawn(move || {
                Self::set_idle_priority();

                // Initial quiet delay to let app UI settle on cold boot
                std::thread::sleep(Duration::from_millis(100));

                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                // Opening can fail while another connection holds the write lock
                // (e.g. rebuilding an old index); keep retrying instead of leaving
                // the index unmaintained for the whole session.
                let mut db = loop {
                    match SearchDb::open(&db_path) {
                        Ok(db) => break db,
                        Err(error) => {
                            eprintln!("[tiarun-crawler] Failed to open search index database: {error}; retrying");
                            for _ in 0..10 {
                                if stop_flag.load(Ordering::Relaxed) {
                                    return;
                                }
                                std::thread::sleep(Duration::from_secs(1));
                            }
                        }
                    }
                };

                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    let pass = db
                        .strip_non_document_content()
                        .and_then(|_| Self::run_indexing_cycle(&scope, &mut db, &stop_flag));
                    if stop_flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let pass = pass
                        .and_then(|()| db.cleanup_missing_files())
                        .and_then(|_| db.cleanup_out_of_scope(&scope))
                        .and_then(|_| db.set_state(super::COMPLETED_SCOPE_KEY, &scope.signature()));
                    match pass {
                        Ok(()) => initial_pass_complete.store(true, Ordering::Release),
                        Err(error) => eprintln!("[tiarun-crawler] Indexing pass failed: {error}"),
                    }

                    // Sleep 5 minutes between incremental maintenance passes
                    for _ in 0..300 {
                        if stop_flag.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            })
    }

    /// Sets the thread to the absolute lowest CPU priority.
    fn set_idle_priority() {
        #[cfg(target_os = "linux")]
        unsafe {
            // Lower priority to maximum niceness (19)
            libc::nice(19);
        }

        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Threading::SetThreadPriority(
                windows_sys::Win32::System::Threading::GetCurrentThread(),
                windows_sys::Win32::System::Threading::THREAD_PRIORITY_IDLE,
            );
        }
    }

    /// Runs a single indexing cycle across configured and standard user folders.
    fn run_indexing_cycle(
        scope: &SearchScope,
        db: &mut SearchDb,
        stop_flag: &AtomicBool,
    ) -> rusqlite::Result<()> {
        let mut processed_in_batch = 0usize;
        let mut pending = Vec::new();
        let mut pending_bytes = 0usize;
        let flush = |db: &mut SearchDb, pending: &mut Vec<PendingWrite>| -> rusqlite::Result<()> {
            if pending.is_empty() {
                return Ok(());
            }
            db.in_transaction(|db| {
                for write in pending.drain(..) {
                    match write {
                        PendingWrite::Upsert { path, filename, extension, size, mtime, doc } => db.upsert_file(
                            &path,
                            &filename,
                            extension.as_deref(),
                            size,
                            mtime,
                            &doc.content,
                            &doc.content_norm,
                        )?,
                    }
                }
                Ok(())
            })
        };

        for (dir, max_depth) in scope.crawl_roots() {
            for entry in WalkDir::new(dir)
                .max_depth(max_depth)
                .into_iter()
                .filter_entry(|entry| scope.allows_name(entry.path()))
                .flatten()
            {
                if super::is_indexing_paused() {
                    // Do not hold the write lock while waiting.
                    flush(db, &mut pending)?;
                    pending_bytes = 0;
                }
                while super::is_indexing_paused() {
                    if stop_flag.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if stop_flag.load(Ordering::Relaxed) {
                    return Ok(());
                }

                let path = entry.path();
                if !entry.file_type().is_file() || !scope.allows_extension(path) {
                    continue;
                }

                let Ok(metadata) = entry.metadata() else { continue };
                let size = metadata.len();
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                let path_str = path.to_string_lossy();

                // Incremental check: check if file is already up-to-date in DB
                let previous = db.get_file_meta(&path_str);
                if previous.is_some_and(|(_, db_size, db_mtime)| db_size == size && db_mtime == mtime) {
                    continue;
                }

                // Every file is findable by name; only documents carry content.
                let doc = DocumentExtractor::extract_for_index(path);
                pending_bytes += doc.content.len() + doc.content_norm.len() + 256;
                pending.push(PendingWrite::Upsert {
                    path: path_str.into_owned(),
                    filename: entry.file_name().to_string_lossy().into_owned(),
                    extension: path.extension().and_then(|ext| ext.to_str()).map(str::to_owned),
                    size,
                    mtime,
                    doc,
                });
                if pending.len() >= BATCH_FILES || pending_bytes >= BATCH_BYTES {
                    flush(db, &mut pending)?;
                    pending_bytes = 0;
                }

                processed_in_batch += 1;
                // Throttling: every 50 files, yield CPU and sleep 2ms to keep background smooth
                if processed_in_batch % 50 == 0 {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        flush(db, &mut pending)
    }
}
