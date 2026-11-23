use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use walkdir::WalkDir;

use super::db::SearchDb;
use super::extractors::DocumentExtractor;
use super::scope::SearchScope;

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
                let mut db = match SearchDb::open(&db_path) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[tiarun-crawler] Failed to open search index database: {e}");
                        return;
                    }
                };

                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    let pass = Self::run_indexing_cycle(&scope, &mut db, &stop_flag);
                    if stop_flag.load(Ordering::Relaxed) {
                        return;
                    }
                    match pass.and_then(|()| db.cleanup_missing_files()) {
                        Ok(_) => initial_pass_complete.store(true, Ordering::Release),
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

        for (dir, max_depth) in scope.crawl_roots() {
            for entry in WalkDir::new(dir)
                .max_depth(max_depth)
                .into_iter()
                .filter_entry(|entry| scope.allows_name(entry.path()))
                .flatten()
            {
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

                let filename = entry.file_name().to_string_lossy();
                let extension = path.extension().and_then(|ext| ext.to_str());

                // Extract text and normalized text
                if let Some(doc) = DocumentExtractor::extract(path) {
                    db.upsert_file(
                        &path_str,
                        &filename,
                        extension,
                        size,
                        mtime,
                        &doc.content,
                        &doc.content_norm,
                    )?;
                } else if previous.is_some() {
                    db.delete_file(&path_str)?;
                }

                processed_in_batch += 1;
                // Throttling: every 50 files, yield CPU and sleep 2ms to keep background smooth
                if processed_in_batch % 50 == 0 {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        Ok(())
    }
}
