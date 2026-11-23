use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use rayon::prelude::*;
use walkdir::WalkDir;
use crate::config::Config;
use crate::launcher::{remove_vietnamese_accents, ItemType, LauncherItem};
use crate::search_index::{scope::SearchScope, search_words};

#[derive(Debug, Clone)]
pub struct ContentMatch {
    pub file_path: String,
    pub file_name: String,
    pub snippet: String,
    pub line_number: usize,
    pub is_docx: bool,
}

pub struct ContentSearchEngine;

impl ContentSearchEngine {
    /// Formats a clean snippet around the primary matching word.
    fn format_snippet(line: &str, primary_word: &str) -> String {
        let clean = line.trim().replace('\t', " ");
        let chars: Vec<char> = clean.chars().collect();
        if chars.len() <= 90 {
            return clean;
        }

        let clean_lower = clean.to_lowercase();
        let word_lower = primary_word.to_lowercase();
        if let Some(byte_idx) = clean_lower.find(&word_lower) {
            let char_idx = clean[..byte_idx].chars().count();
            let start = char_idx.saturating_sub(25);
            let end = (start + 85).min(chars.len());
            let prefix = if start > 0 { "..." } else { "" };
            let suffix = if end < chars.len() { "..." } else { "" };
            let slice: String = chars[start..end].iter().collect();
            format!("{}{}{}", prefix, slice, suffix)
        } else {
            let slice: String = chars.iter().take(85).collect();
            format!("{}...", slice)
        }
    }

    /// Scans a text file for all query words (AND matching + Vietnamese accent insensitive).
    fn scan_file(
        path: &Path,
        query_words: &[&str],
        query_words_norm: &[&str],
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Option<ContentMatch> {
        let mut reader = BufReader::new(File::open(path).ok()?);
        let head = reader.fill_buf().ok()?;
        if head[..head.len().min(512)].contains(&0) {
            return None;
        }

        let mut line = Vec::new();
        let mut ascii_lowercase = Vec::new();
        for idx in 0..=3000 {
            if idx % 64 == 0 && cancelled() {
                return None;
            }
            line.clear();
            if reader.read_until(b'\n', &mut line).ok()? == 0 {
                break;
            }
            let Ok(text) = std::str::from_utf8(&line) else { continue };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }

            let all_match = if text.is_ascii() {
                ascii_lowercase.clear();
                ascii_lowercase.extend_from_slice(text.as_bytes());
                ascii_lowercase.make_ascii_lowercase();
                let lower = std::str::from_utf8(&ascii_lowercase).ok()?;
                query_words.iter().zip(query_words_norm).all(|(word, normalized)| {
                    lower.contains(word) || (word != normalized && lower.contains(normalized))
                })
            } else {
                let lower = text.to_lowercase();
                let normalized = remove_vietnamese_accents(text).to_lowercase();
                query_words.iter().zip(query_words_norm).all(|(word, normalized_word)| {
                    lower.contains(word) || normalized.contains(normalized_word)
                })
            };

            if all_match {
                return Some(ContentMatch {
                    file_path: path.to_string_lossy().into_owned(),
                    file_name: path.file_name()?.to_string_lossy().into_owned(),
                    snippet: Self::format_snippet(text, query_words[0]),
                    line_number: idx + 1,
                    is_docx: false,
                });
            }
        }

        None
    }

    /// Scans a Word .docx document by reading paragraphs from `word/document.xml`.
    fn scan_docx_file(path: &Path, query_words: &[&str], query_words_norm: &[&str]) -> Option<ContentMatch> {
        let file = File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut doc_file = archive.by_name("word/document.xml").ok()?;
        let mut xml = String::new();
        doc_file.read_to_string(&mut xml).ok()?;

        let mut p_idx = 0;
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
                p_idx += 1;
                let line_norm = remove_vietnamese_accents(trimmed);
                let line_lower = trimmed.to_lowercase();
                let line_norm_lower = line_norm.to_lowercase();

                let all_match = query_words.iter().zip(query_words_norm.iter()).all(|(w, wn)| {
                    line_lower.contains(w) || line_norm_lower.contains(wn)
                });

                if all_match {
                    let snippet = Self::format_snippet(trimmed, query_words[0]);
                    let file_name = path.file_name()?.to_string_lossy().to_string();
                    let file_path = path.to_string_lossy().to_string();

                    return Some(ContentMatch {
                        file_path,
                        file_name,
                        snippet,
                        line_number: p_idx,
                        is_docx: true,
                    });
                }
            }
        }

        None
    }

    /// Generates contextual preview lines around the matching line for the Preview Pane.
    pub fn get_contextual_preview(path_str: &str, target_line: usize) -> Option<String> {
        let path = Path::new(path_str);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

        if ext == "docx" {
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

            let start = target_line.saturating_sub(3);
            let end = (target_line + 5).min(paragraphs.len());
            let mut out = Vec::new();
            for (idx, p) in paragraphs.iter().enumerate().take(end).skip(start) {
                let p_num = idx + 1;
                let marker = if p_num == target_line { "▶" } else { " " };
                let trunc: String = p.chars().take(200).collect();
                out.push(format!("{}{:>2} │ {}", marker, p_num, trunc));
            }
            return if out.is_empty() { None } else { Some(out.join("\n")) };
        }

        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);
        let start = target_line.saturating_sub(5).max(1);
        let end = target_line + 15;

        let mut lines_out = Vec::new();
        for (idx, line_res) in reader.lines().enumerate() {
            let line_num = idx + 1;
            if line_num > end {
                break;
            }
            if line_num >= start {
                if let Ok(line) = line_res {
                    let marker = if line_num == target_line { "▶" } else { " " };
                    let truncated: String = line.chars().take(200).collect();
                    lines_out.push(format!("{}{:>3} │ {}", marker, line_num, truncated));
                }
            }
        }

        if lines_out.is_empty() {
            None
        } else {
            Some(lines_out.join("\n"))
        }
    }

    /// Searches file contents across target directories in parallel with Rayon.
    pub fn search(query: &str, config: &Config, max_results: usize) -> Vec<ContentMatch> {
        Self::search_until_cancelled(query, config, max_results, &|| false)
    }

    pub fn search_until_cancelled(
        query: &str,
        config: &Config,
        max_results: usize,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<ContentMatch> {
        Self::search_in_scope(query, &SearchScope::new(&config.search), max_results, cancelled)
    }

    pub(crate) fn search_in_scope(
        query: &str,
        scope: &SearchScope,
        max_results: usize,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> Vec<ContentMatch> {
        let words = search_words(query);
        if words.is_empty() || max_results == 0 || cancelled() {
            return Vec::new();
        }
        let words_norm: Vec<String> = words.iter().map(|w| remove_vietnamese_accents(w).to_lowercase()).collect();
        let words_ref: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
        let words_norm_ref: Vec<&str> = words_norm.iter().map(|s| s.as_str()).collect();

        // Collect candidate files (ignoring build folders like node_modules, target, etc.)
        let mut candidate_files: Vec<PathBuf> = Vec::new();
        'roots: for (dir, depth) in scope.fallback_roots() {
            for entry in WalkDir::new(dir)
                .max_depth(*depth)
                .into_iter()
                .filter_entry(|entry| scope.allows_name(entry.path()))
                .flatten()
            {
                if cancelled() {
                    return Vec::new();
                }
                if candidate_files.len() >= 10000 {
                    break 'roots;
                }
                if entry.file_type().is_file() && scope.allows_extension(entry.path()) {
                    candidate_files.push(entry.into_path());
                }
            }
        }

        // Parallel scan using Rayon (supports both text files and .docx documents)
        let matches: Vec<ContentMatch> = candidate_files
            .par_iter()
            .filter_map(|path| {
                if cancelled() {
                    return None;
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if ext == "docx" {
                    Self::scan_docx_file(path, &words_ref, &words_norm_ref)
                } else {
                    Self::scan_file(path, &words_ref, &words_norm_ref, cancelled)
                }
            })
            .take_any(max_results)
            .collect();

        matches
    }

    /// Converts a `ContentMatch` into a `LauncherItem` displayable in the UI.
    pub fn match_to_launcher_item(m: &ContentMatch) -> LauncherItem {
        let desc = format!("Line {}: \"{}\"", m.line_number, m.snippet);
        LauncherItem::new(
            m.file_name.clone(),
            m.file_path.clone(),
            ItemType::File,
            Some(desc),
            false,
            Some(if m.is_docx { "application-msword".to_string() } else { "text-x-generic".to_string() }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_search_detects_snippet() {
        let temp_dir = std::env::temp_dir().join(format!("view_launcher_content_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let test_file = temp_dir.join("sample_contract.txt");
        let content = "CỘNG HÒA XÃ HỘI CHỦ NGHĨA VIỆT NAM\nĐiều 1: Hợp đồng nguyên tắc cung cấp dịch vụ 2026\nĐiều 2: Giá trị thanh toán";
        std::fs::write(&test_file, content).unwrap();

        let mut config = Config::default();
        config.search.paths = vec![crate::config::SearchPathConfig {
            path: temp_dir.to_string_lossy().to_string(),
            depth: 2,
            max_depth: Some(2),
            exclude: Vec::new(),
        }];

        // Search with Vietnamese diacritics
        let results = ContentSearchEngine::search("hợp đồng nguyên tắc", &config, 10);
        assert!(!results.is_empty(), "Should find file matching full query");
        assert_eq!(results[0].line_number, 2);
        assert!(results[0].snippet.contains("Hợp đồng nguyên tắc"));

        // Search without Vietnamese diacritics
        let results_unaccented = ContentSearchEngine::search("hop dong nguyen tac", &config, 10);
        assert!(!results_unaccented.is_empty(), "Should find file with accent-insensitive search");

        // Multi-word non-contiguous search
        let results_multi = ContentSearchEngine::search("hop dong 2026", &config, 10);
        assert!(!results_multi.is_empty(), "Should find file with non-contiguous words");

        // Contextual preview test
        let preview = ContentSearchEngine::get_contextual_preview(&test_file.to_string_lossy(), 2);
        assert!(preview.is_some());
        let preview_text = preview.unwrap();
        assert!(preview_text.contains("▶"));
        assert!(preview_text.contains("Hợp đồng"));

        // DOCX search test
        let docx_path = temp_dir.join("test_document.docx");
        let file = std::fs::File::create(&docx_path).unwrap();
        let mut zip_archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip_archive.start_file("word/document.xml", options).unwrap();
        use std::io::Write;
        let xml_content = r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Báo cáo tài chính quý 4 năm 2026</w:t></w:r></w:p></w:body></w:document>"#;
        zip_archive.write_all(xml_content.as_bytes()).unwrap();
        zip_archive.finish().unwrap();

        let docx_results = ContentSearchEngine::search("tai chinh 2026", &config, 10);
        assert!(!docx_results.is_empty(), "Should find docx file by content");
        assert_eq!(docx_results[0].file_name, "test_document.docx");
        assert!(docx_results[0].snippet.contains("tài chính"));

        let docx_preview = ContentSearchEngine::get_contextual_preview(&docx_path.to_string_lossy(), 1);
        assert!(docx_preview.is_some());
        assert!(docx_preview.unwrap().contains("Báo cáo tài chính"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
