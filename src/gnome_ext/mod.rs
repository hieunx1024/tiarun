//! Auto-install and enable TiaRun's embedded GNOME Shell extension.
//!
//! GNOME Wayland (Mutter) does not expose a foreign-toplevel Wayland protocol,
//! so external apps cannot list or activate native Wayland windows directly.
//! Packing `metadata.json` + `extension.js` into a `.zip` and installing via
//! `gnome-extensions install --force` lets GNOME Shell load it immediately on
//! Wayland without requiring a logout/login.

#[cfg(target_os = "linux")]
const EXT_UUID: &str = "tiarun-shell@tiarun.app";
#[cfg(target_os = "linux")]
const METADATA_JSON: &str = include_str!("metadata.json");
#[cfg(target_os = "linux")]
const EXTENSION_JS: &str = include_str!("extension.js");

#[cfg(target_os = "linux")]
fn is_gnome_session() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    let is_sway = std::env::var_os("SWAYSOCK").is_some() || desktop.contains("sway");
    let is_hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() || desktop.contains("hyprland");
    !is_sway && !is_hyprland && (desktop.contains("gnome") || desktop.contains("ubuntu"))
}

#[cfg(target_os = "linux")]
fn extension_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("gnome-shell").join("extensions").join(EXT_UUID))
}

#[cfg(any(target_os = "linux", test))]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds an uncompressed (stored, method 0) ZIP archive in memory.
#[cfg(any(target_os = "linux", test))]
fn build_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central_dir = Vec::new();

    for &(name, data) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let size = data.len() as u32;
        let offset = out.len() as u32;

        // Local file header
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes());  // flags
        out.extend_from_slice(&0u16.to_le_bytes());  // method: stored
        out.extend_from_slice(&0u16.to_le_bytes());  // mod time
        out.extend_from_slice(&0u16.to_le_bytes());  // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());  // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // Central directory header
        central_dir.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central_dir.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central_dir.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // flags
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // method: stored
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // mod time
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // mod date
        central_dir.extend_from_slice(&crc.to_le_bytes());
        central_dir.extend_from_slice(&size.to_le_bytes());
        central_dir.extend_from_slice(&size.to_le_bytes());
        central_dir.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // extra len
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // comment len
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // disk start
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // internal attrs
        central_dir.extend_from_slice(&0u32.to_le_bytes());  // external attrs
        central_dir.extend_from_slice(&offset.to_le_bytes());
        central_dir.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    let cd_size = central_dir.len() as u32;
    let count = entries.len() as u16;
    out.extend_from_slice(&central_dir);

    // End of central directory
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(target_os = "linux")]
fn install_via_gnome_extensions_cli() -> bool {
    let zip_bytes = build_stored_zip(&[
        ("metadata.json", METADATA_JSON.as_bytes()),
        ("extension.js", EXTENSION_JS.as_bytes()),
    ]);
    let zip_path = std::env::temp_dir().join(format!("tiarun-shell-ext-{}.zip", std::process::id()));
    if std::fs::write(&zip_path, &zip_bytes).is_err() {
        return false;
    }
    let status = std::process::Command::new("gnome-extensions")
        .args(["install", "--force", &zip_path.to_string_lossy()])
        .status();
    let _ = std::fs::remove_file(&zip_path);
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "linux")]
fn enable_extension() {
    let ok = matches!(
        std::process::Command::new("gnome-extensions").args(["enable", EXT_UUID]).status(),
        Ok(s) if s.success()
    );
    if ok {
        return;
    }
    // Before a Wayland re-login, `gnome-extensions enable` rejects newly
    // extracted extensions; pre-register in `enabled-extensions` so GNOME Shell
    // activates it automatically on next login.
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
    {
        let current = String::from_utf8_lossy(&out.stdout);
        if !current.contains(EXT_UUID) {
            let mut list: Vec<String> = current
                .trim()
                .trim_start_matches('@')
                .trim_start_matches("as")
                .trim()
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "''")
                .collect();
            list.push(format!("'{}'", EXT_UUID));
            let new_val = format!("[{}]", list.join(", "));
            let _ = std::process::Command::new("gsettings")
                .args(["set", "org.gnome.shell", "enabled-extensions", &new_val])
                .status();
        }
    }
}

#[cfg(target_os = "linux")]
fn is_extension_enabled() -> bool {
    if let Ok(out) = std::process::Command::new("gnome-extensions")
        .args(["list", "--enabled"])
        .output()
    {
        if out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == EXT_UUID) {
            return true;
        }
    }
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
        .ok()
        .is_some_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).contains(EXT_UUID))
}

/// Ensures TiaRun's GNOME Shell extension is installed and enabled on GNOME sessions.
pub fn ensure_installed() {
    #[cfg(target_os = "linux")]
    {
        if !is_gnome_session() {
            return;
        }
        let Some(ext_dir) = extension_dir() else {
            return;
        };
        let meta_path = ext_dir.join("metadata.json");
        let js_path = ext_dir.join("extension.js");

        let up_to_date = meta_path.exists()
            && js_path.exists()
            && std::fs::read_to_string(&meta_path).ok().as_deref() == Some(METADATA_JSON)
            && std::fs::read_to_string(&js_path).ok().as_deref() == Some(EXTENSION_JS);

        if up_to_date {
            if !is_extension_enabled() {
                enable_extension();
            }
            return;
        }

        if install_via_gnome_extensions_cli() {
            enable_extension();
            return;
        }

        // Fallback: copy files directly if `gnome-extensions install` is unavailable
        if std::fs::create_dir_all(&ext_dir).is_ok()
            && std::fs::write(&meta_path, METADATA_JSON).is_ok()
            && std::fs::write(&js_path, EXTENSION_JS).is_ok()
        {
            enable_extension();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_stored_zip_structure() {
        let zip = build_stored_zip(&[
            ("metadata.json", br#"{"uuid":"test"}"#),
            ("extension.js", b"export default class {}"),
        ]);
        assert_eq!(&zip[0..4], b"PK\x03\x04");
        assert!(zip.windows(4).any(|w| w == b"PK\x01\x02"));
        assert!(zip.windows(4).any(|w| w == b"PK\x05\x06"));
    }
}
