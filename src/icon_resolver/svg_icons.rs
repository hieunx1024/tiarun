use std::path::Path;

pub(super) const SVG_FOLDER: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" fill="#EBCB8B" stroke="#D08770" stroke-width="1.2"/></svg>"##;
pub(super) const SVG_CODE: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="18" height="18" rx="4" fill="#A78BFA22" stroke="#A78BFA" stroke-width="1.5"/><path d="M9 8.5L5.5 12 9 15.5M15 8.5l3.5 3.5L15 15.5M13 7l-2 10" stroke="#A78BFA" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_DOCUMENT: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6 3h8l5 5v13a1 1 0 01-1 1H6a1 1 0 01-1-1V4a1 1 0 011-1z" fill="#7AA2F722" stroke="#7AA2F7" stroke-width="1.5"/><path d="M14 3v5h5M9 12h6M9 16h4" stroke="#7AA2F7" stroke-width="1.5" stroke-linecap="round"/></svg>"##;
pub(super) const SVG_IMAGE: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="18" height="18" rx="4" fill="#2AC3DE22" stroke="#2AC3DE" stroke-width="1.5"/><circle cx="8.5" cy="8.5" r="1.5" fill="#2AC3DE"/><path d="M21 15l-5-5L5 21" stroke="#2AC3DE" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_PDF: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6 3h8l5 5v13a1 1 0 01-1 1H6a1 1 0 01-1-1V4a1 1 0 011-1z" fill="#F7768E22" stroke="#F7768E" stroke-width="1.5"/><path d="M14 3v5h5" stroke="#F7768E" stroke-width="1.5"/><text x="6.5" y="16" fill="#F7768E" font-size="6" font-weight="bold" font-family="sans-serif">PDF</text></svg>"##;
pub(super) const SVG_ARCHIVE: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><rect x="4" y="4" width="16" height="16" rx="3" fill="#FBBF2422" stroke="#FBBF24" stroke-width="1.5"/><path d="M10 4v16M14 4v16M10 7h4M10 10h4M10 13h4M10 16h4" stroke="#FBBF24" stroke-width="1.2"/></svg>"##;
pub(super) const SVG_AUDIO: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="9" fill="#BB9AF722" stroke="#BB9AF7" stroke-width="1.5"/><path d="M10 15a2 2 0 102-2V7h4" stroke="#BB9AF7" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_VIDEO: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="4" width="18" height="16" rx="3" fill="#7DCFFF22" stroke="#7DCFFF" stroke-width="1.5"/><path d="M10 9l5 3-5 3V9z" fill="#7DCFFF"/></svg>"##;
pub(super) const SVG_GENERIC: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6 3h8l5 5v13a1 1 0 01-1 1H6a1 1 0 01-1-1V4a1 1 0 011-1z" fill="#9AA5CE22" stroke="#9AA5CE" stroke-width="1.5"/><path d="M14 3v5h5" stroke="#9AA5CE" stroke-width="1.5"/></svg>"##;
pub(super) const SVG_GEAR: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="3" stroke="#98989D" stroke-width="1.8"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06a1.65 1.65 0 001.82.33H9a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" stroke="#98989D" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_NAVIGATE: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M12 4L6 10H18L12 4Z" fill="#98989D"/><path d="M12 20L6 14H18L12 20Z" fill="#98989D"/></svg>"##;
pub(super) const SVG_ENTER: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M19 6V13C19 14.1 18.1 15 17 15H6M6 15L10 11M6 15L10 19" stroke="#98989D" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_SEARCH: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="11" cy="11" r="7" stroke="#dedcd1" stroke-width="2.2"/><path d="M20 20L16 16" stroke="#dedcd1" stroke-width="2.2" stroke-linecap="round"/></svg>"##;
pub(super) const SVG_BULB: &str = r##"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M9 21h6M10 17h4M12 3a6 6 0 00-6 6c0 2.2 1.2 4.2 3 5.2V16a1 1 0 001 1h4a1 1 0 001-1v-1.8c1.8-1 3-3 3-5.2a6 6 0 00-6-6z" stroke="#FBBF24" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(super) const SVG_SCOPE_APPS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#FFFFFF" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/></svg>"##;
pub(super) const SVG_SCOPE_FILES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#FFFFFF" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z"/></svg>"##;
pub(super) const SVG_SCOPE_WINDOWS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#FFFFFF" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><line x1="2" y1="10" x2="22" y2="10"/><circle cx="6" cy="7" r="1" fill="#FFFFFF"/><circle cx="9.5" cy="7" r="1" fill="#FFFFFF"/></svg>"##;

pub(super) fn svg_for_category(category_key: &str) -> &'static str {
    match category_key {
        "folder" => SVG_FOLDER,
        "document" => SVG_DOCUMENT,
        "image" => SVG_IMAGE,
        "code" => SVG_CODE,
        "pdf" => SVG_PDF,
        "archive" => SVG_ARCHIVE,
        "audio" => SVG_AUDIO,
        "video" => SVG_VIDEO,
        _ => SVG_GENERIC,
    }
}

pub(super) fn rgba_slice_to_slint_image(width: u32, height: u32, raw_slice: &[u8]) -> slint::Image {
    let mut pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    let dest = pixel_buf.make_mut_slice();
    for (src_chunk, dest_pixel) in raw_slice.chunks_exact(4).zip(dest.iter_mut()) {
        *dest_pixel = slint::Rgba8Pixel {
            r: src_chunk[0],
            g: src_chunk[1],
            b: src_chunk[2],
            a: src_chunk[3],
        };
    }
    slint::Image::from_rgba8(pixel_buf)
}

pub fn rasterize_svg_bytes(svg_data: &[u8], size_px: u32) -> Option<slint::Image> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_data, &opt).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size_px, size_px)?;
    let size = tree.size();
    let sx = size_px as f32 / size.width();
    let sy = size_px as f32 / size.height();
    let scale = sx.min(sy);
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Some(rgba_slice_to_slint_image(size_px, size_px, pixmap.data()))
}

pub(super) fn rasterize_svg(path: &Path) -> Option<slint::Image> {
    let svg_data = std::fs::read(path).ok()?;
    rasterize_svg_bytes(&svg_data, 32)
}
