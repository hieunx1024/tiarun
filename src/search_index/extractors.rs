use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use crate::launcher::remove_vietnamese_accents;

pub struct ExtractedDocument {
    pub content: String,
    pub content_norm: String,
}

/// Supported text file extensions
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "rs", "py", "js", "ts", "jsx", "tsx",
    "c", "cpp", "h", "hpp", "cs", "go", "java", "kt", "scala", "rb", "php",
    "sh", "bash", "zsh", "fish", "json", "yaml", "yml", "toml", "xml", "html",
    "css", "scss", "sass", "less", "sql", "env", "ini", "cfg", "conf", "log",
    "csv", "tsv", "tex", "diff", "patch",
];

/// Known binary extensions that should never be opened as text
pub const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg", "tif", "tiff",
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v",
    "mp3", "wav", "flac", "aac", "ogg", "wma", "m4a",
    "zip", "tar", "gz", "bz2", "xz", "7z", "rar",
    "exe", "msi", "dll", "so", "dylib", "bin", "dat", "iso", "img",
    "class", "jar", "pyc", "o", "a", "lib",
];

/// Files whose text is indexed for `@t` content search. Everything else (source
/// code, configs, binaries, caches) is indexed by name only: searching inside
/// code bloated the index ~10x without helping find documents.
pub const DOCUMENT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "tex", "csv", "tsv", "rtf",
    "pdf", "docx", "xlsx", "pptx", "odt", "ods", "odp",
];

/// Documents stored as plain lines of text: their line numbers and previews can be
/// read straight from the file. Everything else needs a parser.
pub const PLAIN_TEXT_DOCUMENTS: &[&str] = &["txt", "md", "markdown", "rst", "org", "tex", "csv", "tsv"];

pub struct DocumentExtractor;

impl DocumentExtractor {
    pub fn is_binary_extension(ext: &str) -> bool {
        let clean = ext.strip_prefix('.').unwrap_or(ext);
        BINARY_EXTENSIONS.iter().any(|b| b.eq_ignore_ascii_case(clean))
    }

    pub fn is_document(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| DOCUMENT_EXTENSIONS.iter().any(|doc| doc.eq_ignore_ascii_case(ext)))
    }

    pub fn is_plain_text_document(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| PLAIN_TEXT_DOCUMENTS.iter().any(|doc| doc.eq_ignore_ascii_case(ext)))
    }

    /// True for files whose data lives only in the cloud (OneDrive / Dropbox /
    /// iCloud "Files On-Demand" placeholders). Reading such a file makes Windows
    /// download it, so the indexer must never open them; metadata is safe.
    #[cfg(windows)]
    pub fn is_cloud_placeholder(path: &Path) -> bool {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
        const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
        const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
        std::fs::metadata(path).is_ok_and(|metadata| {
            metadata.file_attributes()
                & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
                != 0
        })
    }

    #[cfg(not(windows))]
    pub fn is_cloud_placeholder(_path: &Path) -> bool {
        false
    }

    /// What the search index stores for `path`: the text of documents, and empty
    /// content for every other file so it can still be found by name.
    pub fn extract_for_index(path: &Path) -> ExtractedDocument {
        let readable = Self::is_document(path) && !Self::is_cloud_placeholder(path);
        readable.then(|| Self::extract(path)).flatten().unwrap_or(ExtractedDocument {
            content: String::new(),
            content_norm: String::new(),
        })
    }

    /// Extracts text and normalized text from a file based on its extension.
    pub fn extract(path: &Path) -> Option<ExtractedDocument> {
        let path_buf = path.to_path_buf();
        std::panic::catch_unwind(move || {
            Self::extract_inner(&path_buf)
        }).ok().flatten()
    }

    fn extract_inner(path: &Path) -> Option<ExtractedDocument> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if Self::is_binary_extension(&ext) {
            return None;
        }

        let raw_text = match ext.as_str() {
            "docx" => Self::extract_docx(path)?,
            "xlsx" => Self::extract_xlsx(path)?,
            "pptx" => Self::extract_pptx(path)?,
            "odt" | "ods" | "odp" => Self::extract_opendocument(path)?,
            "rtf" => Self::extract_rtf(path)?,
            "pdf" => Self::extract_pdf(path)?,
            _ if TEXT_EXTENSIONS.contains(&ext.as_str()) => Self::extract_plain_text(path)?,
            _ => {
                // If extension is not explicitly listed, check if it looks like a text file
                Self::extract_plain_text_checked(path)?
            }
        };

        let trimmed = raw_text.trim();
        if trimmed.is_empty() {
            return None;
        }

        let content_norm = remove_vietnamese_accents(trimmed);
        Some(ExtractedDocument {
            content: trimmed.to_string(),
            content_norm,
        })
    }

    /// Extracts plain text from a text file, reading up to 50,000 lines (or 10MB).
    fn extract_plain_text(path: &Path) -> Option<String> {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);

        // Quick binary check on first 512 bytes
        let head = reader.fill_buf().ok()?;
        if head[..head.len().min(512)].contains(&0) {
            return None;
        }

        let mut lines = Vec::new();
        let mut total_bytes = 0usize;
        let mut line_buf = Vec::new();

        for _ in 0..50_000 {
            line_buf.clear();
            if reader.read_until(b'\n', &mut line_buf).ok()? == 0 {
                break;
            }
            total_bytes += line_buf.len();
            if total_bytes > 10 * 1024 * 1024 {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&line_buf) {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Checks if a file is plain text before extracting
    fn extract_plain_text_checked(path: &Path) -> Option<String> {
        // Limit file size to 10MB
        if let Ok(meta) = path.metadata() {
            if meta.len() > 10 * 1024 * 1024 {
                return None;
            }
        }
        Self::extract_plain_text(path)
    }

    /// Extracts text from Word .docx file (XML inside zip archive).
    fn extract_docx(path: &Path) -> Option<String> {
        let file = File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut doc_file = archive.by_name("word/document.xml").ok()?;

        let mut xml = String::new();
        doc_file.read_to_string(&mut xml).ok()?;

        let mut paragraphs = Vec::new();
        for p_chunk in xml.split("<w:p") {
            let mut p_text = String::new();
            for t_chunk in p_chunk.split("<w:t") {
                if let Some((_, rest)) = t_chunk.split_once('>') {
                    if let Some((text, _)) = rest.split_once("</w:t>") {
                        p_text.push_str(text);
                    }
                }
            }
            let trimmed = p_text.trim();
            if !trimmed.is_empty() {
                paragraphs.push(trimmed.to_string());
            }
        }

        if paragraphs.is_empty() {
            None
        } else {
            Some(paragraphs.join("\n"))
        }
    }

    /// Extracts the cell text of an Excel .xlsx workbook. Text cells live in the
    /// shared string table; inline strings and numbers in the sheets are skipped.
    fn extract_xlsx(path: &Path) -> Option<String> {
        if path.metadata().ok()?.len() > 30 * 1024 * 1024 {
            return None;
        }
        let file = File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut xml = String::new();
        archive.by_name("xl/sharedStrings.xml").ok()?.read_to_string(&mut xml).ok()?;

        let mut cells = Vec::new();
        for item in xml.split("<si>").skip(1) {
            let mut text = String::new();
            for chunk in item.split("<t").skip(1) {
                if let Some((_, rest)) = chunk.split_once('>') {
                    if let Some((value, _)) = rest.split_once("</t>") {
                        text.push_str(value);
                    }
                }
            }
            let text = Self::unescape_xml(text.trim());
            if !text.is_empty() {
                cells.push(text);
            }
        }
        if cells.is_empty() { None } else { Some(cells.join("\n")) }
    }

    /// Extracts slide text from a PowerPoint .pptx deck, in slide order.
    fn extract_pptx(path: &Path) -> Option<String> {
        if path.metadata().ok()?.len() > 100 * 1024 * 1024 {
            return None;
        }
        let mut archive = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
        let mut slides: Vec<(u32, String)> = archive
            .file_names()
            .filter_map(|name| {
                let name = name.ok()?.into_owned();
                let number = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                Some((number, name))
            })
            .collect();
        slides.sort();

        let mut text = String::new();
        for (_, name) in slides {
            let mut xml = String::new();
            if archive.by_name(&name).ok()?.read_to_string(&mut xml).is_ok() {
                let slide = Self::xml_text(&xml, &["a:p"]);
                let slide = slide.trim();
                if !slide.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(slide);
                }
            }
        }
        let text = text.trim();
        if text.is_empty() { None } else { Some(text.to_string()) }
    }

    /// Extracts text from OpenDocument files (LibreOffice .odt/.ods/.odp), which
    /// all keep their body in `content.xml`.
    fn extract_opendocument(path: &Path) -> Option<String> {
        if path.metadata().ok()?.len() > 50 * 1024 * 1024 {
            return None;
        }
        let mut archive = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
        let mut xml = String::new();
        archive.by_name("content.xml").ok()?.read_to_string(&mut xml).ok()?;
        let text = Self::xml_text(&xml, &["text:p", "text:h"]);
        let text = text.trim();
        if text.is_empty() { None } else { Some(text.to_string()) }
    }

    /// Collects the character data of an XML document, ending a line after each
    /// closing tag in `line_tags`. Self-closing spacing tags (`<text:s/>`,
    /// `<text:tab/>`, `<a:br/>`...) become a space.
    fn xml_text(xml: &str, line_tags: &[&str]) -> String {
        let mut out = String::with_capacity(xml.len() / 4);
        let mut line = String::new();
        let mut rest = xml;
        while let Some(open) = rest.find('<') {
            line.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('>') else { break };
            let tag = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];

            if let Some(name) = tag.strip_prefix('/') {
                if line_tags.contains(&name.trim()) {
                    let text = Self::unescape_xml(line.trim());
                    if !text.is_empty() {
                        out.push_str(&text);
                        out.push('\n');
                    }
                    line.clear();
                }
            } else if tag.ends_with('/') && !line.ends_with(' ') {
                line.push(' ');
            }
        }
        let text = Self::unescape_xml(line.trim());
        out.push_str(&text);
        out
    }

    /// Extracts the visible text of an RTF document: drops control words, groups
    /// that hold no document text (fonts, styles, pictures...) and decodes
    /// `\uN` (Unicode, used for Vietnamese) and `\'hh` (code page) characters.
    fn extract_rtf(path: &Path) -> Option<String> {
        if path.metadata().ok()?.len() > 20 * 1024 * 1024 {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if !bytes.starts_with(b"{\\rtf") {
            return None;
        }
        let text = Self::rtf_text(&bytes);
        let text = text.trim();
        if text.is_empty() { None } else { Some(text.to_string()) }
    }

    fn rtf_text(bytes: &[u8]) -> String {
        const SKIPPED: &[&str] = &[
            "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer",
            "headerl", "headerr", "footerl", "footerr", "listtable", "listoverridetable",
            "generator", "xmlnstbl", "themedata", "colorschememapping", "datastore", "latentstyles",
            "rsidtbl", "mmathPr", "fldinst",
        ];
        let mut out = String::new();
        // Per group: (skipped, characters to drop after a \uN escape)
        let mut stack: Vec<(bool, usize)> = vec![(false, 1)];
        let mut skip_fallback = 0usize;
        let mut i = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            let (skipped, uc) = *stack.last().unwrap_or(&(false, 1));
            match byte {
                b'{' => {
                    stack.push((skipped, uc));
                    i += 1;
                }
                b'}' => {
                    stack.pop();
                    if stack.is_empty() {
                        break;
                    }
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    let Some(&next) = bytes.get(i) else { break };
                    if next == b'\'' {
                        // \'hh: one byte in the document code page (Windows-1252 assumed).
                        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
                        if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                            if skip_fallback > 0 {
                                skip_fallback -= 1;
                            } else if !skipped {
                                out.push(value as char);
                            }
                        }
                        i += 3;
                    } else if next.is_ascii_alphabetic() {
                        let start = i;
                        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                        let num_start = i;
                        if i < bytes.len() && bytes[i] == b'-' {
                            i += 1;
                        }
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        let number: Option<i32> = std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|n| n.parse().ok());
                        if i < bytes.len() && bytes[i] == b' ' {
                            i += 1;
                        }
                        let top = stack.last_mut();
                        match word {
                            "u" => {
                                if let Some(code) = number {
                                    let code = if code < 0 { code + 65536 } else { code };
                                    if !skipped {
                                        if let Some(character) = char::from_u32(code as u32) {
                                            out.push(character);
                                        }
                                    }
                                    skip_fallback = uc;
                                }
                            }
                            "uc" => {
                                if let (Some(top), Some(count)) = (top, number) {
                                    top.1 = usize::try_from(count).unwrap_or(1);
                                }
                            }
                            "par" | "line" | "row" | "sect" | "page" if !skipped => out.push('\n'),
                            "tab" | "cell" if !skipped => out.push(' '),
                            _ if SKIPPED.contains(&word) => {
                                if let Some(top) = top {
                                    top.0 = true;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // \*, \~, \-, \{, \} ...
                        if next == b'*' {
                            if let Some(top) = stack.last_mut() {
                                top.0 = true;
                            }
                        } else if !skipped && matches!(next, b'{' | b'}' | b'\\') {
                            out.push(next as char);
                        } else if !skipped && next == b'~' {
                            out.push(' ');
                        }
                        i += 1;
                    }
                }
                b'\r' | b'\n' => i += 1,
                _ => {
                    if skip_fallback > 0 {
                        skip_fallback -= 1;
                    } else if !skipped {
                        out.push(byte as char);
                    }
                    i += 1;
                }
            }
        }
        out
    }

    fn unescape_xml(text: &str) -> String {
        if !text.contains('&') {
            return text.to_string();
        }
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    /// Extracts text from PDF document using pdf-extract safely with panic isolation.
    fn extract_pdf(path: &Path) -> Option<String> {
        // Protect against massive PDFs (>30MB)
        if let Ok(meta) = path.metadata() {
            if meta.len() > 30 * 1024 * 1024 {
                return None;
            }
        }

        let path_buf = path.to_path_buf();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let res = std::panic::catch_unwind(move || {
            pdf_extract::extract_text(&path_buf)
        });
        std::panic::set_hook(prev_hook);

        match res {
            Ok(Ok(text)) => {
                let cleaned = text
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if cleaned.is_empty() {
                    None
                } else {
                    Some(cleaned)
                }
            }
            _ => None,
        }
    }

    /// Finds the exact 1-based line number in a file where the query matches.
    /// Fast: does NOT re-parse heavy binary formats (PDF, DOCX) at query time.
    pub fn find_matching_line_number(path: &Path, query: &str) -> usize {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        // Parsed formats (PDF, Office, OpenDocument, RTF) have no meaningful line numbers.
        if DOCUMENT_EXTENSIONS.contains(&ext.as_str()) && !PLAIN_TEXT_DOCUMENTS.contains(&ext.as_str()) {
            return 1;
        }
        if Self::is_cloud_placeholder(path) {
            return 1;
        }

        // Protect query latency: only scan small text files (<500KB)
        if let Ok(meta) = path.metadata() {
            if meta.len() > 500 * 1024 {
                return 1;
            }
        }

        let words = super::search_words(query);
        if words.is_empty() {
            return 1;
        }
        let words_norm: Vec<String> = words.iter().map(|w| remove_vietnamese_accents(w)).collect();

        // Plain text scan (fast buffer, capped at 1,000 lines)
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            for (idx, line_res) in reader.lines().take(1000).enumerate() {
                if let Ok(line) = line_res {
                    let line_norm = remove_vietnamese_accents(&line).to_lowercase();
                    let line_lower = line.to_lowercase();
                    if words.iter().zip(&words_norm).all(|(w, wn)| {
                        line_lower.contains(w) || line_norm.contains(wn)
                    }) {
                        return idx + 1;
                    }
                }
            }
        }

        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_extract_plain_text_and_vietnamese_line() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_ext_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("ke_hoach_2026.txt");

        let mut file = File::create(&test_file).expect("create test file");
        writeln!(file, "Dòng 1: Giới thiệu dự án").unwrap();
        writeln!(file, "Dòng 2: Mục tiêu quý 1 năm 2026").unwrap();
        writeln!(file, "Dòng 3: Kế hoạch mở rộng thị trường và thanh toán tiền mặt").unwrap();
        drop(file);

        let doc = DocumentExtractor::extract(&test_file).expect("extract document");
        assert!(doc.content.contains("Kế hoạch mở rộng"));
        assert!(doc.content_norm.contains("ke hoach mo rong"));

        // Match with accented query
        let line_accented = DocumentExtractor::find_matching_line_number(&test_file, "Kế hoạch");
        assert_eq!(line_accented, 3);

        // Match with unaccented query
        let line_unaccented = DocumentExtractor::find_matching_line_number(&test_file, "ke hoach");
        assert_eq!(line_unaccented, 3);

        // Test C/C++ function signature with parentheses like "int main()"
        let c_file = temp_dir.join("main.c");
        let mut c_f = File::create(&c_file).expect("create c file");
        writeln!(c_f, "#include <stdio.h>").unwrap();
        writeln!(c_f, "").unwrap();
        writeln!(c_f, "int main(int argc, char *argv[]) {{").unwrap();
        writeln!(c_f, "    printf(\"Hello World\\n\");").unwrap();
        writeln!(c_f, "    return 0;").unwrap();
        writeln!(c_f, "}}").unwrap();
        drop(c_f);

        let main_line = DocumentExtractor::find_matching_line_number(&c_file, "int main()");
        assert_eq!(main_line, 3, "Should match 'int main()' on line 3 despite arguments inside parentheses");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn indexes_document_text_and_only_names_of_other_files() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_ext_policy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let xlsx = temp_dir.join("danh_muc.xlsx");
        let mut zip = zip::ZipWriter::new(File::create(&xlsx).unwrap());
        zip.start_file("xl/sharedStrings.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(
            r#"<sst><si><t>Bưu cục Thái Nguyên</t></si><si><r><t>Doanh </t></r><r><t>thu &amp; chi</t></r></si></sst>"#
                .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
        let doc = DocumentExtractor::extract_for_index(&xlsx);
        assert!(doc.content.contains("Bưu cục Thái Nguyên"));
        assert!(doc.content.contains("Doanh thu & chi"));
        assert!(doc.content_norm.contains("buu cuc thai nguyen"));

        let code = temp_dir.join("main.rs");
        std::fs::write(&code, "fn main() { println!(\"hello\"); }").unwrap();
        let image = temp_dir.join("photo.png");
        std::fs::write(&image, [0u8, 1, 2, 3]).unwrap();
        for path in [&code, &image] {
            assert!(!DocumentExtractor::is_document(path));
            assert!(DocumentExtractor::extract_for_index(path).content.is_empty());
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_pptx_opendocument_and_rtf_text() {
        let temp_dir = std::env::temp_dir().join(format!("tiarun_ext_formats_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Slides are read in numeric order (slide10 after slide2).
        let pptx = temp_dir.join("bao_cao.pptx");
        write_zip(&pptx, &[
            ("ppt/slides/slide10.xml", r#"<p:sld><a:p><a:r><a:t>Kết luận</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/slide2.xml", r#"<p:sld><a:p><a:r><a:t>Doanh thu </a:t></a:r><a:r><a:t>quý 3 &amp; 4</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slideLayouts/slideLayout1.xml", r#"<a:p><a:t>LAYOUT_PLACEHOLDER</a:t></a:p>"#),
        ]);
        let text = DocumentExtractor::extract_for_index(&pptx).content;
        assert_eq!(text, "Doanh thu quý 3 & 4\nKết luận");

        let odt = temp_dir.join("hop_dong.odt");
        write_zip(&odt, &[(
            "content.xml",
            r#"<office:document-content><office:body><office:text><text:h>Điều 1</text:h><text:p>Bên<text:s/>A thanh toán</text:p></office:text></office:body></office:document-content>"#,
        )]);
        let doc = DocumentExtractor::extract_for_index(&odt);
        assert_eq!(doc.content, "Điều 1\nBên A thanh toán");
        assert!(doc.content_norm.contains("ben a thanh toan"));

        let ods = temp_dir.join("bang_luong.ods");
        write_zip(&ods, &[(
            "content.xml",
            r#"<table:table><table:table-row><table:table-cell><text:p>Nguyễn Văn A</text:p></table:table-cell><table:table-cell><text:p>5000000</text:p></table:table-cell></table:table-row></table:table>"#,
        )]);
        assert!(DocumentExtractor::extract_for_index(&ods).content.contains("Nguyễn Văn A"));

        // Vietnamese RTF as Word writes it: \uN escapes with '?' fallbacks, plus
        // font/colour tables and an ignorable destination that must not leak.
        let rtf = temp_dir.join("cong_van.rtf");
        std::fs::write(
            &rtf,
            r"{\rtf1\ansi\uc1{\fonttbl{\f0 Times New Roman;}}{\colortbl;\red0\green0\blue0;}{\*\generator Msftedit;}\f0 C\u244?ng v\u259?n s\u7889? 12\par Ng\'e0y {\b k\u253?}\par}",
        )
        .unwrap();
        let doc = DocumentExtractor::extract_for_index(&rtf);
        assert_eq!(doc.content, "Công văn số 12\nNgày ký");
        assert!(doc.content_norm.contains("cong van so 12"));

        for path in [&pptx, &odt, &ods, &rtf] {
            assert!(DocumentExtractor::is_document(path));
            assert!(!DocumentExtractor::is_plain_text_document(path));
            assert_eq!(DocumentExtractor::find_matching_line_number(path, "anything"), 1);
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
