#[cfg(not(target_os = "windows"))]
mod linux;
mod svg_icons;
#[cfg(target_os = "windows")]
mod windows;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use crate::launcher::{ItemType, LauncherItem};

use svg_icons::{
    SVG_BULB, SVG_ENTER, SVG_FOLDER, SVG_GEAR, SVG_NAVIGATE, SVG_SCOPE_APPS, SVG_SCOPE_FILES,
    SVG_SCOPE_WINDOWS, SVG_SEARCH, rasterize_svg, rgba_slice_to_slint_image, svg_for_category,
};

pub(super) fn is_supported_image(p: &Path) -> bool {
    p.extension().map_or(false, |ext| {
        let ext_str = ext.to_string_lossy();
        ext_str.eq_ignore_ascii_case("ico")
            || ext_str.eq_ignore_ascii_case("png")
            || ext_str.eq_ignore_ascii_case("svg")
            || ext_str.eq_ignore_ascii_case("jpg")
            || ext_str.eq_ignore_ascii_case("jpeg")
            || ext_str.eq_ignore_ascii_case("webp")
            || ext_str.eq_ignore_ascii_case("bmp")
            || ext_str.eq_ignore_ascii_case("gif")
    })
}

/// High-performance thread-safe icon resolver with O(1) in-memory index and pre-decoding cache.
pub struct IconResolver {
    #[allow(dead_code)]
    icon_path_index: Arc<RwLock<HashMap<String, PathBuf>>>,
    image_cache: Arc<RwLock<HashMap<String, Option<slint::Image>>>>,
    file_type_cache: Arc<RwLock<HashMap<&'static str, slint::Image>>>,
    indexing_done: Arc<AtomicBool>,
}

impl IconResolver {
    pub fn new() -> Self {
        let path_index = Arc::new(RwLock::new(HashMap::new()));
        let image_cache: Arc<RwLock<HashMap<String, Option<slint::Image>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let indexing_done = Arc::new(AtomicBool::new(false));

        #[cfg(not(target_os = "windows"))]
        linux::init_unix_icon_index(&path_index, &indexing_done);

        #[cfg(target_os = "windows")]
        {
            indexing_done.store(true, Ordering::SeqCst);
        }

        Self {
            icon_path_index: path_index,
            image_cache,
            file_type_cache: Arc::new(RwLock::new(HashMap::new())),
            indexing_done,
        }
    }

    /// Resolves file type icons (.rs, .png, .pdf, folder, etc.) with instant caching.
    pub fn resolve_file_type_icon(&self, path: &Path, item_type: ItemType) -> Option<slint::Image> {
        let category_key = match item_type {
            ItemType::Dir => "folder",
            ItemType::File => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                match ext.as_str() {
                    "txt" | "md" | "log" | "rtf" | "csv" | "tsv" | "doc" | "docx" | "odt" => "document",
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tiff" => "image",
                    "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "toml" | "json" | "c" | "cpp" | "h" | "hpp" | "go" | "html" | "css" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "sql" | "java" | "kt" | "lua" | "vue" | "svelte" => "code",
                    "pdf" => "pdf",
                    "zip" | "tar" | "gz" | "7z" | "rar" | "xz" | "bz2" | "zst" | "iso" => "archive",
                    "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" => "audio",
                    "mp4" | "mkv" | "mov" | "avi" | "webm" | "flv" | "wmv" => "video",
                    _ => "generic",
                }
            }
            _ => return None,
        };

        // 1. Instant Cache Hit
        if let Ok(guard) = self.file_type_cache.read() {
            if let Some(img) = guard.get(category_key) {
                return Some(img.clone());
            }
        }

        // 2. Render SVG for type
        let svg_str = svg_for_category(category_key);
        let slint_img = Self::rasterize_svg_bytes(svg_str.as_bytes(), 64)?;

        if let Ok(mut guard) = self.file_type_cache.write() {
            guard.insert(category_key, slint_img.clone());
        }

        Some(slint_img)
    }

    pub fn get_gear_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_GEAR.as_bytes(), 48).unwrap_or_default()
    }

    pub fn get_nav_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_NAVIGATE.as_bytes(), 36).unwrap_or_default()
    }

    pub fn get_enter_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_ENTER.as_bytes(), 36).unwrap_or_default()
    }

    pub fn get_search_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_SEARCH.as_bytes(), 18).unwrap_or_default()
    }

    pub fn get_bulb_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_BULB.as_bytes(), 36).unwrap_or_default()
    }

    pub fn get_folder_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_FOLDER.as_bytes(), 72).unwrap_or_default()
    }

    pub fn get_scope_apps_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_SCOPE_APPS.as_bytes(), 48).unwrap_or_default()
    }

    pub fn get_scope_files_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_SCOPE_FILES.as_bytes(), 48).unwrap_or_default()
    }

    pub fn get_scope_windows_icon(&self) -> slint::Image {
        Self::rasterize_svg_bytes(SVG_SCOPE_WINDOWS.as_bytes(), 48).unwrap_or_default()
    }

    pub fn rasterize_svg_bytes(svg_data: &[u8], size_px: u32) -> Option<slint::Image> {
        svg_icons::rasterize_svg_bytes(svg_data, size_px)
    }

    /// Preloads and decodes icons for all apps in the background.
    pub fn preload_icons(&self, apps: &[LauncherItem]) {
        for app in apps {
            let _ = self.resolve_icon(app.icon.as_deref(), &app.name, &app.exec_or_path);
        }
    }

    /// Resolves and loads the native system icon into a `slint::Image` instantly.
    pub fn resolve_icon(
        &self,
        icon_hint: Option<&str>,
        app_name: &str,
        exec_or_path: &str,
    ) -> Option<slint::Image> {
        let key = if let Some(hint) = icon_hint {
            if !hint.is_empty() {
                hint
            } else if !exec_or_path.is_empty() {
                exec_or_path
            } else {
                app_name
            }
        } else if !exec_or_path.is_empty() {
            exec_or_path
        } else {
            app_name
        };
        if key.is_empty() {
            return None;
        }

        // 1. Instant Cache Hit (only return cached None if indexing has completed)
        if let Ok(guard) = self.image_cache.read() {
            if let Some(cached) = guard.get(key) {
                if let Some(ref img) = *cached {
                    return Some(img.clone());
                } else if self.indexing_done.load(Ordering::Relaxed) {
                    return None;
                }
            }
        }

        // 2. Direct Image File Check
        let mut slint_img = None;

        if let Some(hint) = icon_hint {
            let p = Path::new(hint);
            if p.exists() && is_supported_image(p) {
                slint_img = Self::load_downscaled_icon(p);
            }
        }

        if slint_img.is_none() && !exec_or_path.is_empty() {
            let p = Path::new(exec_or_path);
            if p.exists() && is_supported_image(p) {
                slint_img = Self::load_downscaled_icon(p);
            }
        }

        // 3. Platform specific native resolution
        #[cfg(target_os = "windows")]
        if slint_img.is_none() {
            let candidate = icon_hint
                .filter(|hint| Path::new(hint).exists())
                .or_else(|| Path::new(exec_or_path).exists().then_some(exec_or_path));

            if let Some(target_path) = candidate {
                slint_img = Self::extract_windows_icon(target_path);
            }
        }

        #[cfg(not(target_os = "windows"))]
        if slint_img.is_none() {
            let resolved_path = self.find_icon_path(icon_hint, app_name, exec_or_path);
            slint_img = resolved_path.and_then(|p| Self::load_downscaled_icon(&p));
        }

        // 4. Store in cache (only store None if background indexing has completed)
        if let Ok(mut guard) = self.image_cache.write() {
            if slint_img.is_some() || self.indexing_done.load(Ordering::Relaxed) {
                guard.insert(key.to_string(), slint_img.clone());
            }
        }

        slint_img
    }

    pub fn is_indexing_done(&self) -> bool {
        self.indexing_done.load(Ordering::Relaxed)
    }

    fn load_downscaled_icon(path: &Path) -> Option<slint::Image> {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("svg") {
                if let Some(img) = rasterize_svg(path) {
                    return Some(img);
                }
            }
        }

        if let Ok(dyn_img) = image::open(path) {
            let (w, h) = (dyn_img.width(), dyn_img.height());
            let thumb = if w == 32 && h == 32 {
                dyn_img.to_rgba8()
            } else {
                dyn_img
                    .resize_exact(32, 32, image::imageops::FilterType::Triangle)
                    .to_rgba8()
            };
            let (w, h) = thumb.dimensions();
            return Some(rgba_slice_to_slint_image(w, h, &thumb.into_raw()));
        }

        None
    }

    #[allow(dead_code)]
    fn find_icon_path(
        &self,
        icon_hint: Option<&str>,
        #[allow(unused_variables)] app_name: &str,
        exec_or_path: &str,
    ) -> Option<PathBuf> {
        #[cfg(not(target_os = "windows"))]
        {
            linux::find_icon_path(&self.icon_path_index, icon_hint, app_name, exec_or_path)
        }
        #[cfg(target_os = "windows")]
        {
            windows::find_icon_path(icon_hint, exec_or_path)
        }
    }

    /// Extracts a high-quality icon directly from Windows Shell (.lnk, .exe, .url, .msc, .cpl, etc.)
    #[cfg(target_os = "windows")]
    pub fn extract_windows_icon(path_str: &str) -> Option<slint::Image> {
        windows::extract_windows_icon(path_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_resolver_basic() {
        let resolver = IconResolver::new();
        let start = std::time::Instant::now();
        while !resolver.indexing_done.load(Ordering::SeqCst) && start.elapsed().as_secs() < 3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // 1. Built-in file type icon resolution
        assert!(resolver.resolve_file_type_icon(Path::new("test.rs"), ItemType::File).is_some());
        assert!(resolver.resolve_file_type_icon(Path::new("photo.png"), ItemType::File).is_some());
        assert!(resolver.resolve_file_type_icon(Path::new("doc.pdf"), ItemType::File).is_some());
        assert!(resolver.resolve_file_type_icon(Path::new("archive.zip"), ItemType::File).is_some());
        assert!(resolver.resolve_file_type_icon(Path::new("folder"), ItemType::Dir).is_some());

        // 2. Built-in UI helper icons
        let _ = resolver.get_gear_icon();
        let _ = resolver.get_nav_icon();
        let _ = resolver.get_enter_icon();
        let _ = resolver.get_search_icon();
        let _ = resolver.get_bulb_icon();
        let _ = resolver.get_folder_icon();
        assert!(resolver.get_scope_apps_icon().size().width > 0);
        assert!(resolver.get_scope_files_icon().size().width > 0);
        assert!(resolver.get_scope_windows_icon().size().width > 0);

        // 3. Workspace asset icon resolution
        let asset_svg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("tiarun.svg");
        let asset_svg_fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("view-launcher.svg");
        let target_svg = if asset_svg.exists() { asset_svg } else { asset_svg_fallback };
        if target_svg.exists() {
            let svg_icon = resolver.resolve_icon(target_svg.to_str(), "TiaRun", "tiarun");
            assert!(svg_icon.is_some(), "Asset SVG should resolve when file exists");
        }

        let asset_png = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("tiarun.png");
        let asset_png_fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("view-launcher.png");
        let target_png = if asset_png.exists() { asset_png } else { asset_png_fallback };
        if target_png.exists() {
            let png_icon = resolver.resolve_icon(target_png.to_str(), "TiaRun", "tiarun");
            assert!(png_icon.is_some(), "Asset PNG should resolve when file exists");
        }

        // 4. Non-existent app icon gracefully handles None
        let _ = resolver.resolve_icon(Some("non.existent.random_app_xyz_987"), "RandomAppXYZ", "random-app-xyz");

        // 5. System app resolution if available on live system
        let p_ext = resolver.find_icon_path(Some("com.mattjakeman.ExtensionManager"), "Extension Manager", "extension-manager");
        println!("Extension Manager icon path: {:?}", p_ext);
        if p_ext.is_some() {
            let ext_icon = resolver.resolve_icon(Some("com.mattjakeman.ExtensionManager"), "Extension Manager", "extension-manager");
            assert!(ext_icon.is_some(), "Extension Manager icon should resolve when path is found");
        }

        let p_text = resolver.find_icon_path(Some("org.gnome.TextEditor"), "Text Editor", "gnome-text-editor");
        println!("Text Editor icon path: {:?}", p_text);
        if p_text.is_some() {
            let text_icon = resolver.resolve_icon(Some("org.gnome.TextEditor"), "Text Editor", "gnome-text-editor");
            assert!(text_icon.is_some(), "Text Editor icon should resolve when path is found");
        }
    }
}
