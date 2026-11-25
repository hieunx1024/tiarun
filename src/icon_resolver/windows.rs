use std::path::{Path, PathBuf};

use super::is_supported_image;

pub(super) fn find_icon_path(
    icon_hint: Option<&str>,
    exec_or_path: &str,
) -> Option<PathBuf> {
    if let Some(hint) = icon_hint {
        let p = PathBuf::from(hint);
        if p.exists() && is_supported_image(&p) {
            return Some(p);
        }
    }
    let p = PathBuf::from(exec_or_path);
    if p.exists() && is_supported_image(&p) {
        return Some(p);
    }
    None
}

/// Extracts a high-quality icon directly from Windows Shell (.lnk, .exe, .url, .msc, .cpl, etc.)
pub(super) fn extract_windows_icon(path_str: &str) -> Option<slint::Image> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows_sys::Win32::UI::Shell::{
        SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON, SHGetFileInfoW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{HICON, PrivateExtractIconsW};

    // Ensure COM is initialized on this thread so SHGetFileInfoW can resolve .lnk targets
    unsafe {
        let _ = CoInitializeEx(
            std::ptr::null_mut(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
    }

    let clean_path = path_str.trim().trim_matches('"');

    // Check if it is an Internet Shortcut (.url) which contains IconFile=...
    if clean_path.ends_with(".url") || clean_path.ends_with(".URL") {
        if let Ok(content) = std::fs::read_to_string(clean_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("IconFile=") {
                    let icon_file = rest.trim().trim_matches('"');
                    if !icon_file.is_empty() && Path::new(icon_file).exists() {
                        if let Some(img) = extract_windows_icon(icon_file) {
                            return Some(img);
                        }
                    }
                }
            }
        }
    }

    let wide_path: Vec<u16> = clean_path.encode_utf16().chain(std::iter::once(0)).collect();

    // 1. If it is an executable, DLL, or .ico, try PrivateExtractIconsW first (crisp 48x48 icon)
    let is_binary_or_ico = clean_path.ends_with(".exe")
        || clean_path.ends_with(".EXE")
        || clean_path.ends_with(".ico")
        || clean_path.ends_with(".ICO")
        || clean_path.ends_with(".dll")
        || clean_path.ends_with(".DLL");

    if is_binary_or_ico {
        let mut hicon: HICON = std::ptr::null_mut();
        let mut icon_id = 0;
        let count = unsafe {
            PrivateExtractIconsW(
                wide_path.as_ptr(),
                0,
                48,
                48,
                &mut hicon,
                &mut icon_id,
                1,
                0,
            )
        };
        if count > 0 && !hicon.is_null() {
            if let Some(img) = unsafe { hicon_to_slint_image(hicon) } {
                return Some(img);
            }
        }
    }

    // 2. Try SHGetFileInfoW with Large Icon (resolves .lnk shortcuts, documents, folders, etc.)
    let mut shfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let res = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };

    if res != 0 && !shfi.hIcon.is_null() {
        if let Some(img) = unsafe { hicon_to_slint_image(shfi.hIcon) } {
            return Some(img);
        }
    }

    // 3. Fallback: try SHGetFileInfoW with Small Icon if Large failed
    let mut shfi_small: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let res_small = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi_small,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        )
    };

    if res_small != 0 && !shfi_small.hIcon.is_null() {
        if let Some(img) = unsafe { hicon_to_slint_image(shfi_small.hIcon) } {
            return Some(img);
        }
    }

    None
}

unsafe fn hicon_to_slint_image(
    hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
) -> Option<slint::Image> {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    if hicon.is_null() {
        return None;
    }

    unsafe {
        let mut icon_info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut icon_info) == 0 {
            DestroyIcon(hicon);
            return None;
        }

        let hbm_color = icon_info.hbmColor;
        let hbm_mask = icon_info.hbmMask;

        let hbm_target = if !hbm_color.is_null() { hbm_color } else { hbm_mask };
        if hbm_target.is_null() {
            if !hbm_mask.is_null() {
                DeleteObject(hbm_mask as _);
            }
            DestroyIcon(hicon);
            return None;
        }

        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(
            hbm_target as _,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut _ as *mut _,
        ) == 0
        {
            if !hbm_color.is_null() {
                DeleteObject(hbm_color as _);
            }
            if !hbm_mask.is_null() {
                DeleteObject(hbm_mask as _);
            }
            DestroyIcon(hicon);
            return None;
        }

        let width = bm.bmWidth;
        let height = if !hbm_color.is_null() { bm.bmHeight } else { bm.bmHeight / 2 };

        if width <= 0 || height <= 0 {
            if !hbm_color.is_null() {
                DeleteObject(hbm_color as _);
            }
            if !hbm_mask.is_null() {
                DeleteObject(hbm_mask as _);
            }
            DestroyIcon(hicon);
            return None;
        }

        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() {
            if !hbm_color.is_null() {
                DeleteObject(hbm_color as _);
            }
            if !hbm_mask.is_null() {
                DeleteObject(hbm_mask as _);
            }
            DestroyIcon(hicon);
            return None;
        }

        let mut bi: BITMAPINFOHEADER = std::mem::zeroed();
        bi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.biWidth = width;
        bi.biHeight = -height; // Negative for top-down DIB
        bi.biPlanes = 1;
        bi.biBitCount = 32;
        bi.biCompression = BI_RGB;

        let mut bgra_buf = vec![0u8; (width * height * 4) as usize];

        if !hbm_color.is_null() {
            GetDIBits(
                hdc,
                hbm_color as _,
                0,
                height as u32,
                bgra_buf.as_mut_ptr() as *mut _,
                &mut bi as *mut _ as *mut BITMAPINFO,
                DIB_RGB_COLORS,
            );
        }

        // Check if 32-bit color buffer already has non-zero alpha channel
        let mut has_valid_alpha = false;
        if !hbm_color.is_null() {
            for chunk in bgra_buf.chunks_exact(4) {
                if chunk[3] != 0 {
                    has_valid_alpha = true;
                    break;
                }
            }
        }

        // If no valid alpha channel, use mask bitmap to compute transparency
        if !has_valid_alpha && !hbm_mask.is_null() {
            let mask_height_total = if !hbm_color.is_null() { height } else { height * 2 };
            let mut mask_bi = bi;
            mask_bi.biHeight = -mask_height_total;
            let mut mask_buf = vec![0u8; (width * mask_height_total * 4) as usize];

            GetDIBits(
                hdc,
                hbm_mask as _,
                0,
                mask_height_total as u32,
                mask_buf.as_mut_ptr() as *mut _,
                &mut mask_bi as *mut _ as *mut BITMAPINFO,
                DIB_RGB_COLORS,
            );

            if !hbm_color.is_null() {
                // Color icon with separate 1-bit mask
                for (i, mask_chunk) in mask_buf.chunks_exact(4).enumerate() {
                    let is_transparent = mask_chunk[0] != 0 || mask_chunk[1] != 0 || mask_chunk[2] != 0;
                    let dest_idx = i * 4;
                    bgra_buf[dest_idx + 3] = if is_transparent { 0 } else { 255 };
                }
            } else {
                // Monochrome icon: upper half is AND mask, lower half is XOR mask
                let pixels_per_half = (width * height) as usize;
                for i in 0..pixels_per_half {
                    let and_val = mask_buf[i * 4];
                    let xor_val = mask_buf[(pixels_per_half + i) * 4];
                    let is_transparent = and_val != 0;
                    let color = if xor_val != 0 { 255 } else { 0 };
                    let dest_idx = i * 4;
                    bgra_buf[dest_idx] = color;
                    bgra_buf[dest_idx + 1] = color;
                    bgra_buf[dest_idx + 2] = color;
                    bgra_buf[dest_idx + 3] = if is_transparent { 0 } else { 255 };
                }
            }
        }

        DeleteDC(hdc);
        if !hbm_color.is_null() {
            DeleteObject(hbm_color as _);
        }
        if !hbm_mask.is_null() {
            DeleteObject(hbm_mask as _);
        }
        DestroyIcon(hicon);

        // Convert BGRA to RGBA in Slint SharedPixelBuffer
        let mut pixel_buf =
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width as u32, height as u32);
        let dest = pixel_buf.make_mut_slice();
        for (src_chunk, dest_pixel) in bgra_buf.chunks_exact(4).zip(dest.iter_mut()) {
            *dest_pixel = slint::Rgba8Pixel {
                r: src_chunk[2], // BGRA -> RGBA
                g: src_chunk[1],
                b: src_chunk[0],
                a: src_chunk[3],
            };
        }

        Some(slint::Image::from_rgba8(pixel_buf))
    }
}
