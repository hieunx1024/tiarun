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

pub struct DocumentExtractor;

impl DocumentExtractor {
    /// Extracts text and normalized text from a file based on its extension.
    pub fn extract(path: &Path) -> Option<ExtractedDocument> {
        let path_buf = path.to_path_buf();
        std::panic::catch_unwind(move || {
            Self::extract_inner(&path_buf)
        }).ok().flatten()
    }

    fn extract_inner(path: &Path) -> Option<ExtractedDocument> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

        let raw_text = match ext.as_str() {
            "docx" => Self::extract_docx(path)?,
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
        if ext == "pdf" || ext == "docx" {
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
}

