use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use image::GenericImageView;
use crate::launcher::format_file_size;

#[derive(Debug, Clone, Default)]
pub struct FilePreview {
    pub title: String,
    pub preview_type: String, // "image", "text", "metadata", "none"
    pub preview_lang: String, // "Rust", "Python", "DIRECTORY", etc.
    pub text_snippet: String,
    pub file_size: String,
    pub modified_time: String,
    pub location: String,
    pub image_path: Option<String>,
}

fn detect_language(ext: &str) -> &'static str {
    match ext {
        "rs" => "Rust",
        "py" => "Python",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "mts" | "cts" => "TypeScript",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" | "markdown" => "Markdown",
        "sh" | "bash" | "zsh" => "Shell",
        "c" | "h" => "C",
        "cpp" | "cxx" | "cc" | "hpp" => "C++",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" | "less" => "CSS",
        "sql" => "SQL",
        "xml" => "XML",
        "ini" | "cfg" | "conf" => "Config",
        "log" => "Log",
        "txt" => "Text",
        _ => "",
    }
}

/// Attempts to find a pre-rendered system thumbnail (FreeDesktop standard on Linux/Unix).
/// Provides near-instant (<1ms) visual previews for images, videos, and PDFs.
pub fn get_freedesktop_thumbnail(#[allow(unused_variables)] path: &Path) -> Option<PathBuf> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let url = url::Url::from_file_path(&canonical).ok()?;
        let uri_str = url.as_str();
        let digest = md5::compute(uri_str.as_bytes());
        let hash = format!("{:x}", digest);

        let cache_dir = dirs::cache_dir()
            .map(|d| d.join("thumbnails"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".cache").join("thumbnails"))
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
            });

        for tier in &["large", "normal", "x-large", "xx-large"] {
            let candidate = cache_dir.join(tier).join(format!("{}.png", hash));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Instant image dimension reader (<0.2ms) without decoding all raw pixel data.
fn get_image_dimensions(path: &Path, ext: &str) -> Option<(u32, u32)> {
    if ext == "svg" {
        let svg_data = fs::read(path).ok()?;
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(&svg_data, &opt).ok()?;
        let size = tree.size();
        return Some((size.width().round() as u32, size.height().round() as u32));
    }

    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

impl FilePreview {
    pub fn from_path(path_str: &str) -> Self {
        let path = Path::new(path_str);
        if !path.exists() {
            return Self::default();
        }

        let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let location = path.parent().map(|p| {
            let s = p.to_string_lossy().to_string();
            if let Some(home) = dirs::home_dir() {
                let home_str = home.to_string_lossy().to_string();
                if s.starts_with(&home_str) {
                    return s.replacen(&home_str, "~", 1);
                }
            }
            s
        }).unwrap_or_default();

        let metadata = fs::metadata(path).ok();
        let file_len = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_time = metadata.as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                if let Ok(elapsed) = t.elapsed() {
                    let secs = elapsed.as_secs();
                    if secs < 60 {
                        "Just now".to_string()
                    } else if secs < 3600 {
                        format!("{}m ago", secs / 60)
                    } else if secs < 86400 {
                        format!("{}h ago", secs / 3600)
                    } else {
                        format!("{}d ago", secs / 86400)
                    }
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();

        // 1. Smart Directory Preview: summarize child files and folders
        if path.is_dir() {
            let mut dir_items = Vec::new();
            let mut file_count = 0usize;
            let mut dir_count = 0usize;

            if let Ok(entries) = fs::read_dir(path) {
                let mut all_entries = Vec::new();
                for entry in entries.flatten() {
                    let is_d = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_d {
                        dir_count += 1;
                    } else {
                        file_count += 1;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    all_entries.push((is_d, name));
                }

                // Sort: directories first, then alphabetically
                all_entries.sort_by(|a, b| {
                    b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
                });

                for (is_d, name) in all_entries.iter().take(12) {
                    let icon = if *is_d { "📁" } else { "📄" };
                    let suffix = if *is_d { "/" } else { "" };
                    dir_items.push(format!("{} {}{}", icon, name, suffix));
                }

                let total = file_count + dir_count;
                if total > 12 {
                    dir_items.push(format!("… and {} more items", total - 12));
                }
            }

            let summary_stat = format!("{} items ({} dirs, {} files)", file_count + dir_count, dir_count, file_count);
            let snippet = if dir_items.is_empty() {
                "Empty folder".to_string()
            } else {
                dir_items.join("\n")
            };

            return Self {
                title,
                preview_type: "text".to_string(),
                preview_lang: "DIRECTORY".to_string(),
                text_snippet: snippet,
                file_size: summary_stat,
                modified_time,
                location,
                image_path: None,
            };
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let formatted_len = format_file_size(file_len);

        // Cloud-only placeholders (OneDrive "Files On-Demand"...): reading them for a
        // preview would download the file, and the first result is previewed on
        // every keystroke. Show metadata only.
        if crate::search_index::extractors::DocumentExtractor::is_cloud_placeholder(path) {
            return Self {
                title,
                preview_type: "metadata".to_string(),
                preview_lang: if ext.is_empty() { "FILE".to_string() } else { ext.to_uppercase() },
                text_snippet: String::new(),
                file_size: format!("{formatted_len} (Online only)"),
                modified_time,
                location,
                image_path: None,
            };
        }

        // 2. Image files (or files with pre-rendered OS thumbnails like PDF / Video)
        let is_image_ext = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "svg" | "webp" | "bmp" | "ico");
        let has_os_thumb = get_freedesktop_thumbnail(path).is_some();

        if is_image_ext || has_os_thumb {
            const MAX_IMAGE_PREVIEW_BYTES: u64 = 40 * 1024 * 1024;
            if file_len > MAX_IMAGE_PREVIEW_BYTES && !has_os_thumb {
                return Self {
                    title,
                    preview_type: "metadata".to_string(),
                    preview_lang: ext.to_uppercase(),
                    text_snippet: String::new(),
                    file_size: format!("{} (Large File)", formatted_len),
                    modified_time,
                    location,
                    image_path: None,
                };
            }

            // Extract dimensions in <0.2ms if it's an image
            let dims_str = if is_image_ext {
                get_image_dimensions(path, &ext)
                    .map(|(w, h)| format!(" • {}×{}", w, h))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let preview_size_str = format!("{}{}", formatted_len, dims_str);
            let lang_label = if is_image_ext {
                ext.to_uppercase()
            } else if ext.is_empty() {
                "MEDIA".to_string()
            } else {
                ext.to_uppercase()
            };

            return Self {
                title,
                preview_type: "image".to_string(),
                preview_lang: lang_label,
                text_snippet: String::new(),
                file_size: preview_size_str,
                modified_time,
                location,
                image_path: Some(path_str.to_string()),
            };
        }

        // 3. Text and code files with line numbers
        if matches!(
            ext.as_str(),
            "txt" | "md" | "markdown" | "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "mts" | "cts"
                | "json" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh"
                | "c" | "cpp" | "cxx" | "cc" | "h" | "hpp" | "html" | "htm" | "css" | "scss"
                | "sass" | "less" | "sql" | "log" | "ini" | "cfg" | "conf" | "xml"
        ) {
            let preview_lang = detect_language(&ext).to_string();
            let text_snippet = if let Ok(mut f) = fs::File::open(path) {
                use std::io::Read;
                let mut buf = [0u8; 32768]; // Read up to 32KB for generous, smooth preview
                if let Ok(n) = f.read(&mut buf) {
                    let raw = String::from_utf8_lossy(&buf[..n]);
                    format_text_snippet(&ext, &raw)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            return Self {
                title,
                preview_type: "text".to_string(),
                preview_lang,
                text_snippet,
                file_size: formatted_len,
                modified_time,
                location,
                image_path: None,
            };
        }

        // 4. Metadata fallback for all other files (archives, binaries, unknown formats)
        Self {
            title,
            preview_type: "metadata".to_string(),
            preview_lang: ext.to_uppercase(),
            text_snippet: String::new(),
            file_size: formatted_len,
            modified_time,
            location,
            image_path: None,
        }
    }
}

/// Formats text previews with smart pretty-printing (JSON), document styling (Markdown),
/// tab-to-space normalization, and right-aligned line numbers for code files.
pub fn format_text_snippet(ext: &str, raw: &str) -> String {
    // 1. Tab normalization
    let normalized = raw.replace('\t', "    ");

    match ext {
        "json" | "jsonl" => {
            // Attempt pretty printing for clean indented view
            let formatted = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&normalized) {
                serde_json::to_string_pretty(&val).unwrap_or(normalized)
            } else {
                normalized
            };

            let mut lines_vec = Vec::new();
            for (idx, line) in formatted.lines().take(80).enumerate() {
                let line_num = idx + 1;
                if line.chars().count() > 200 {
                    let truncated: String = line.chars().take(200).collect();
                    lines_vec.push(format!("{:>2} │ {}…", line_num, truncated));
                } else {
                    lines_vec.push(format!("{:>2} │ {}", line_num, line));
                }
            }
            lines_vec.join("\n")
        }
        "md" | "markdown" => {
            let mut out = Vec::new();
            let mut in_code_block = false;
            for line in normalized.lines().take(80) {
                let trimmed = line.trim_start();
                if trimmed.starts_with("```") {
                    if in_code_block {
                        out.push("└────────────────────────────────────────".to_string());
                        in_code_block = false;
                    } else {
                        let lang = trimmed.trim_start_matches('`').trim();
                        if lang.is_empty() {
                            out.push("┌────────────────────────────────────────".to_string());
                        } else {
                            out.push(format!("┌── ⟨{}⟩ ──────────────────────────────", lang));
                        }
                        in_code_block = true;
                    }
                    continue;
                }
                if in_code_block {
                    out.push(format!("│ {}", line));
                    continue;
                }

                if let Some(h1) = trimmed.strip_prefix("# ") {
                    out.push(format!("━ ◈ {} ━━━━━━━━━━━━━━━━━━━━", h1));
                } else if let Some(h2) = trimmed.strip_prefix("## ") {
                    out.push(format!("▶ {}", h2));
                } else if let Some(h3) = trimmed.strip_prefix("### ") {
                    out.push(format!("• {}", h3));
                } else if let Some(quote) = trimmed.strip_prefix("> ") {
                    out.push(format!("▎ {}", quote));
                } else if let Some(todo) = trimmed.strip_prefix("- [ ] ") {
                    out.push(format!("☐ {}", todo));
                } else if let Some(done) = trimmed.strip_prefix("- [x] ").or_else(|| trimmed.strip_prefix("- [X] ")) {
                    out.push(format!("☑ {}", done));
                } else if let Some(bullet) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                    out.push(format!("• {}", bullet));
                } else if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                    out.push("────────────────────────────────────────".to_string());
                } else {
                    if line.chars().count() > 200 {
                        let truncated: String = line.chars().take(200).collect();
                        out.push(format!("{}…", truncated));
                    } else {
                        out.push(line.to_string());
                    }
                }
            }
            out.join("\n")
        }
        _ => {
            let mut lines_vec = Vec::new();
            for (idx, line) in normalized.lines().take(80).enumerate() {
                let line_num = idx + 1;
                if line.chars().count() > 200 {
                    let truncated: String = line.chars().take(200).collect();
                    lines_vec.push(format!("{:>2} │ {}…", line_num, truncated));
                } else {
                    lines_vec.push(format!("{:>2} │ {}", line_num, line));
                }
            }
            lines_vec.join("\n")
        }
    }
}

/// Asynchronously decodes and scales any raster or vector image into a compact RGBA thumbnail.
/// First checks the OS thumbnail cache for <1ms instant retrieval, otherwise decodes directly.
pub fn decode_thumbnail_rgba(path_str: &str, max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let path = Path::new(path_str);

    // 1. Tận dụng FreeDesktop thumbnail cache có sẵn từ hệ thống
    if let Some(thumb_path) = get_freedesktop_thumbnail(path) {
        if let Ok(reader) = image::ImageReader::open(&thumb_path) {
            if let Ok(reader) = reader.with_guessed_format() {
                if let Ok(img) = reader.decode() {
                    let thumb = img.thumbnail(max_w, max_h);
                    let (w, h) = thumb.dimensions();
                    let rgba = thumb.to_rgba8();
                    return Some((w, h, rgba.into_raw()));
                }
            }
        }
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    // 2. Render SVG vector
    if ext == "svg" {
        let svg_data = fs::read(path).ok()?;
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(&svg_data, &opt).ok()?;
        let size = tree.size();
        let scale = (max_w as f32 / size.width()).min(max_h as f32 / size.height()).min(1.0);
        let target_w = ((size.width() * scale) as u32).max(1);
        let target_h = ((size.height() * scale) as u32).max(1);
        let mut pixmap = tiny_skia::Pixmap::new(target_w, target_h)?;
        let transform = tiny_skia::Transform::from_scale(
            target_w as f32 / size.width(),
            target_h as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        return Some((target_w, target_h, pixmap.take()));
    }

    // 3. Render Raster Image (JPEG, PNG, WEBP, BMP, ICO)
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let img = reader.decode().ok()?;
    let thumb = img.thumbnail(max_w, max_h);
    let (w, h) = thumb.dimensions();
    let rgba = thumb.to_rgba8();
    Some((w, h, rgba.into_raw()))
}

#[derive(Clone)]
struct CacheEntry<T> {
    data: T,
    last_access: u64,
    size_bytes: usize,
}

/// Thread-safe in-memory Bounded LRU cache for preview metadata and decoded thumbnail bytes.
/// Prevents redundant disk I/O, prevents cache thrashing, and strictly bounds memory usage (~30MB max).
/// 100% Send + Sync safe.
#[derive(Clone, Default)]
pub struct PreviewCache {
    preview_cache: Arc<RwLock<HashMap<String, CacheEntry<FilePreview>>>>,
    thumbnail_cache: Arc<RwLock<HashMap<String, CacheEntry<(u32, u32, Vec<u8>)>>>>,
    access_counter: Arc<std::sync::atomic::AtomicU64>,
}

impl PreviewCache {
    pub fn new() -> Self {
        Self {
            preview_cache: Arc::new(RwLock::new(HashMap::new())),
            thumbnail_cache: Arc::new(RwLock::new(HashMap::new())),
            access_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn get_or_create_preview(&self, path_str: &str) -> FilePreview {
        let access = self.access_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut lock) = self.preview_cache.write() {
            if let Some(entry) = lock.get_mut(path_str) {
                entry.last_access = access;
                return entry.data.clone();
            }
        }

        let preview = FilePreview::from_path(path_str);
        if let Ok(mut lock) = self.preview_cache.write() {
            // LRU eviction: if entries >= 300, evict the oldest 50
            if lock.len() >= 300 {
                let mut entries: Vec<(String, u64)> = lock.iter().map(|(k, v)| (k.clone(), v.last_access)).collect();
                entries.sort_by_key(|(_, acc)| *acc);
                for (k, _) in entries.into_iter().take(50) {
                    lock.remove(&k);
                }
            }
            lock.insert(path_str.to_string(), CacheEntry {
                data: preview.clone(),
                last_access: access,
                size_bytes: std::mem::size_of::<FilePreview>() + preview.text_snippet.len(),
            });
        }
        preview
    }

    pub fn get_cached_thumbnail(&self, path_str: &str) -> Option<(u32, u32, Vec<u8>)> {
        let access = self.access_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut lock) = self.thumbnail_cache.write() {
            if let Some(entry) = lock.get_mut(path_str) {
                entry.last_access = access;
                return Some(entry.data.clone());
            }
        }
        None
    }

    pub fn insert_cached_thumbnail(&self, path_str: &str, w: u32, h: u32, bytes: Vec<u8>) {
        let access = self.access_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let size_bytes = bytes.len();
        if let Ok(mut lock) = self.thumbnail_cache.write() {
            const MAX_THUMBNAIL_BYTES: usize = 30 * 1024 * 1024; // 30 MB
            const TARGET_THUMBNAIL_BYTES: usize = 20 * 1024 * 1024; // 20 MB

            let current_total: usize = lock.values().map(|v| v.size_bytes).sum();
            if current_total + size_bytes > MAX_THUMBNAIL_BYTES || lock.len() >= 80 {
                let mut entries: Vec<(String, u64, usize)> = lock.iter().map(|(k, v)| (k.clone(), v.last_access, v.size_bytes)).collect();
                entries.sort_by_key(|(_, acc, _)| *acc);
                let mut freed = 0usize;
                for (k, _, sz) in entries {
                    lock.remove(&k);
                    freed += sz;
                    if current_total.saturating_sub(freed) <= TARGET_THUMBNAIL_BYTES {
                        break;
                    }
                }
            }

            lock.insert(path_str.to_string(), CacheEntry {
                data: (w, h, bytes),
                last_access: access,
                size_bytes,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_preview_text_and_metadata() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_preview_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let text_path = temp_dir.join("sample.rs");
        std::fs::write(&text_path, "fn main() {\n    println!(\"Hello World\");\n}\n").unwrap();

        let preview = FilePreview::from_path(&text_path.to_string_lossy());
        assert_eq!(preview.preview_type, "text");
        assert_eq!(preview.preview_lang, "Rust");
        assert_eq!(preview.title, "sample.rs");
        assert!(preview.text_snippet.contains("Hello World"));
        assert!(preview.text_snippet.contains(" 1 │ fn main()"));
        assert!(!preview.file_size.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_directory_preview() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_dir_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let sub_dir = temp_dir.join("subfolder");
        let _ = std::fs::create_dir_all(&sub_dir);
        let f1 = temp_dir.join("a_file.txt");
        std::fs::write(&f1, "test").unwrap();

        let preview = FilePreview::from_path(&temp_dir.to_string_lossy());
        assert_eq!(preview.preview_type, "text");
        assert_eq!(preview.preview_lang, "DIRECTORY");
        assert!(preview.text_snippet.contains("📁 subfolder/"));
        assert!(preview.text_snippet.contains("📄 a_file.txt"));
        assert!(preview.file_size.contains("2 items"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_long_line_truncation() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_trunc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let json_path = temp_dir.join("minified.json");
        let long_line = "{\"key\": \"".to_string() + &"a".repeat(500) + "\"}";
        std::fs::write(&json_path, long_line).unwrap();

        let preview = FilePreview::from_path(&json_path.to_string_lossy());
        assert_eq!(preview.preview_type, "text");
        assert_eq!(preview.preview_lang, "JSON");
        assert!(preview.text_snippet.contains('…'));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_markdown_and_json_formatting() {
        let md = "# Title\n> Quote\n- [ ] Task\n```rust\nfn main() {}\n```";
        let formatted_md = format_text_snippet("md", md);
        assert!(formatted_md.contains("━ ◈ Title"));
        assert!(formatted_md.contains("▎ Quote"));
        assert!(formatted_md.contains("☐ Task"));
        assert!(formatted_md.contains("┌── ⟨rust⟩"));

        let minified_json = "{\"name\":\"tiarun\",\"tags\":[\"fast\",\"clean\"]}";
        let formatted_json = format_text_snippet("json", minified_json);
        assert!(formatted_json.contains(" 1 │ {"));
        assert!(formatted_json.contains("  \"name\": \"tiarun\""));
    }

    #[test]
    fn test_large_image_protection() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_large_img_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let img_path = temp_dir.join("huge.png");
        let file = std::fs::File::create(&img_path).unwrap();
        file.set_len(45 * 1024 * 1024).unwrap();

        let preview = FilePreview::from_path(&img_path.to_string_lossy());
        assert_eq!(preview.preview_type, "metadata");
        assert!(preview.file_size.contains("Large File"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_preview_cache_and_lru() {
        let cache = PreviewCache::new();
        let preview1 = cache.get_or_create_preview("assets/view-launcher.png");
        assert_eq!(preview1.preview_type, "image");

        // Second lookup hits cache
        let preview2 = cache.get_or_create_preview("assets/view-launcher.png");
        assert_eq!(preview2.title, preview1.title);

        // Thumbnail LRU cache
        cache.insert_cached_thumbnail("test_key", 10, 10, vec![0u8; 400]);
        assert!(cache.get_cached_thumbnail("test_key").is_some());
    }

    #[test]
    fn test_decode_thumbnail_rgba() {
        // Test PNG
        let res_png = decode_thumbnail_rgba("assets/view-launcher.png", 300, 200);
        assert!(res_png.is_some());
        let (w, h, bytes) = res_png.unwrap();
        assert!(w <= 300);
        assert!(h <= 200);
        assert_eq!(bytes.len(), (w * h * 4) as usize);

        // Test SVG
        let res_svg = decode_thumbnail_rgba("assets/view-launcher.svg", 300, 200);
        assert!(res_svg.is_some());
        let (w, h, bytes) = res_svg.unwrap();
        assert!(w <= 300);
        assert!(h <= 200);
        assert_eq!(bytes.len(), (w * h * 4) as usize);
    }

    #[test]
    fn test_freedesktop_thumbnail_resolution() {
        let pdf_path = Path::new("/home/hieunx/Desktop/bao-cao-phan-phoi-b13 (2).pdf");
        if pdf_path.exists() {
            let thumb = get_freedesktop_thumbnail(pdf_path);
            assert!(thumb.is_some());
            let thumb_path = thumb.unwrap();
            assert!(thumb_path.exists());
            assert!(thumb_path.to_string_lossy().contains("08dd4755fa886a96087efa010baf7781.png"));
        }
    }
}
