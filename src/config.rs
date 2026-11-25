use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static HAS_CACHED_SYSTEM_DARK: AtomicBool = AtomicBool::new(false);
static CACHED_SYSTEM_DARK: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct GeneralConfig {
    #[serde(default)]
    pub autostart: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_mode")]
    pub mode: String,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_query_color")]
    pub query_color: String,
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,
    #[serde(default = "default_selection_fg")]
    pub selection_fg: String,
    #[serde(default = "default_border_color")]
    pub border_color: String,
    #[serde(default = "default_highlight_color")]
    pub highlight_color: String,
    #[serde(default = "default_true")]
    pub show_icons: Option<bool>,
    #[serde(default = "default_false")]
    pub show_status_bar: Option<bool>,
    #[serde(default = "default_true")]
    pub compact_empty_view: Option<bool>,
}

fn default_theme_mode() -> String { "dark".to_string() }
fn default_opacity() -> f32 { 0.95 }
fn default_query_color() -> String { "#dedcd1".to_string() }
fn default_selection_bg() -> String { "#2d2d2d".to_string() }
fn default_selection_fg() -> String { "#faf9f5".to_string() }
fn default_border_color() -> String { "#2d2d2d".to_string() }
fn default_highlight_color() -> String { "#dedcd1".to_string() }
fn default_true() -> Option<bool> { Some(true) }
fn default_false() -> Option<bool> { Some(false) }

impl ThemeConfig {
    pub fn is_dark(&self) -> bool {
        match self.mode.to_lowercase().as_str() {
            "light" => false,
            "system" => is_system_dark_mode(),
            _ => true,
        }
    }

    /// Returns dark-mode state using the cached system theme when `mode == "system"`,
    /// avoiding synchronous subprocess spawns on hot UI toggle paths.
    pub fn is_dark_cached(&self) -> bool {
        match self.mode.to_lowercase().as_str() {
            "light" => false,
            "system" => cached_system_dark_mode(),
            _ => true,
        }
    }
}

/// Returns the most recently observed system dark-mode state without spawning
/// external processes once initialized by [`is_system_dark_mode`].
pub fn cached_system_dark_mode() -> bool {
    if HAS_CACHED_SYSTEM_DARK.load(Ordering::Acquire) {
        CACHED_SYSTEM_DARK.load(Ordering::Relaxed)
    } else {
        is_system_dark_mode()
    }
}

pub fn is_system_dark_mode() -> bool {
    let dark = detect_system_dark_mode();
    CACHED_SYSTEM_DARK.store(dark, Ordering::Relaxed);
    HAS_CACHED_SYSTEM_DARK.store(true, Ordering::Release);
    dark
}

fn detect_system_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Registry::{
            RegOpenKeyExW, RegQueryValueExW, RegCloseKey, HKEY_CURRENT_USER, KEY_READ,
        };

        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
            .encode_utf16()
            .collect();
        let value_name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();

        let mut hkey = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            )
        };

        if status == 0 && !hkey.is_null() {
            let mut data: u32 = 0;
            let mut data_size = std::mem::size_of::<u32>() as u32;
            let mut data_type: u32 = 0;

            let query_status = unsafe {
                RegQueryValueExW(
                    hkey,
                    value_name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut data_type,
                    &mut data as *mut u32 as *mut u8,
                    &mut data_size,
                )
            };

            unsafe { RegCloseKey(hkey) };

            if query_status == 0 {
                // AppsUseLightTheme == 0 means Dark mode is enabled
                return data == 0;
            }
        }
        true
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("defaults")
            .args(&["read", "-g", "AppleInterfaceStyle"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            return text.contains("dark");
        }
        false
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        // 1. Check Freedesktop Portal Settings (Standard for GNOME 42+, KDE Plasma 5.24+, Sway, Hyprland, etc.)
        if let Ok(output) = std::process::Command::new("dbus-send")
            .args(&[
                "--session",
                "--dest=org.freedesktop.portal.Desktop",
                "--type=method_call",
                "--print-reply=literal",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings.Read",
                "string:org.freedesktop.appearance",
                "string:color-scheme",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains("uint32 1") {
                return true;
            } else if text.contains("uint32 2") || text.contains("uint32 0") {
                // uint32 1 = prefer dark, uint32 2 = prefer light, uint32 0 = no preference (light)
                return false;
            }
        }

        // 2. Check GNOME gsettings color-scheme
        if let Ok(output) = std::process::Command::new("gsettings")
            .args(&["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            if text.contains("prefer-dark") || text.contains("dark") {
                return true;
            } else if text.contains("prefer-light") || text.contains("default") {
                return false;
            }
        }

        // 3. Check GNOME gsettings gtk-theme
        if let Ok(theme_out) = std::process::Command::new("gsettings")
            .args(&["get", "org.gnome.desktop.interface", "gtk-theme"])
            .output()
        {
            let theme_text = String::from_utf8_lossy(&theme_out.stdout).to_lowercase();
            if theme_text.contains("dark") {
                return true;
            } else if !theme_text.is_empty() && theme_out.status.success() {
                return false;
            }
        }

        // 4. Check KDE Plasma kreadconfig
        if let Ok(kde_out) = std::process::Command::new("kreadconfig5")
            .args(&["--group", "General", "--key", "ColorScheme"])
            .output()
        {
            let kde_text = String::from_utf8_lossy(&kde_out.stdout).to_lowercase();
            if kde_text.contains("dark") {
                return true;
            } else if !kde_text.is_empty() && kde_out.status.success() {
                return false;
            }
        }

        true
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            mode: default_theme_mode(),
            opacity: default_opacity(),
            query_color: default_query_color(),
            selection_bg: default_selection_bg(),
            selection_fg: default_selection_fg(),
            border_color: default_border_color(),
            highlight_color: default_highlight_color(),
            show_icons: Some(true),
            show_status_bar: Some(false),
            compact_empty_view: Some(true),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SearchPathConfig {
    pub path: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
    pub max_depth: Option<usize>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn default_depth() -> usize { 2 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SearchConfig {
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_depth")]
    pub max_depth: usize,
    #[serde(default)]
    pub paths: Vec<SearchPathConfig>,
    #[serde(default = "default_true")]
    pub enable_path_matching: Option<bool>,
    #[serde(default = "default_ignored_dirs")]
    pub ignored_dirs: Vec<String>,
    #[serde(default = "default_ignored_extensions")]
    pub ignored_extensions: Vec<String>,
    /// URL template for the "Search the web" fallback result; `{query}` is replaced
    /// with the (percent-encoded) search text. Change this to use a different search
    /// engine, e.g. "https://duckduckgo.com/?q={query}".
    #[serde(default = "default_web_search_url")]
    pub web_search_url: String,
}

fn default_max_results() -> usize { 50 }

fn default_web_search_url() -> String {
    "https://www.google.com/search?q={query}".to_string()
}

fn default_ignored_dirs() -> Vec<String> {
    vec![
        "node_modules", ".git", "target", "venv", ".venv",
        "__pycache__", ".cache", ".cargo", ".rustup", ".local",
        "build", "dist", ".gradle", ".m2", ".npm", ".nvm",
        "AppData", ".Trash", ".Trash-1000",
    ].into_iter().map(String::from).collect()
}

fn default_ignored_extensions() -> Vec<String> {
    vec![
        ".tmp", ".log", ".bak", ".swp", ".lock", ".iso",
        ".o", ".so", ".dylib", ".class", ".pyc",
    ].into_iter().map(String::from).collect()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: default_max_results(),
            max_depth: default_depth(),
            paths: Vec::new(),
            enable_path_matching: Some(true),
            ignored_dirs: default_ignored_dirs(),
            ignored_extensions: default_ignored_extensions(),
            web_search_url: default_web_search_url(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CustomAppConfig {
    pub name: String,
    pub exec: String,
    pub description: Option<String>,
    #[serde(default)]
    pub terminal: bool,
    pub icon: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AppsConfig {
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
    #[serde(default)]
    pub extra_desktop_paths: Vec<String>,
    #[serde(default)]
    pub custom: Vec<CustomAppConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub apps: AppsConfig,
}

impl Config {
    pub fn get_config_path() -> PathBuf {
        if let Some(config_path) = dirs::config_dir() {
            let tiarun_dir = config_path.join("tiarun");
            let legacy_dir = config_path.join("view-launcher");
            if !tiarun_dir.exists() && legacy_dir.exists() {
                let _ = fs::rename(&legacy_dir, &tiarun_dir);
            }
            tiarun_dir.join("config.toml")
        } else {
            PathBuf::from("config.toml")
        }
    }

    /// Loads `config.toml` without probing platform autostart files/directories.
    /// Suitable for hot UI toggle paths where autostart state is not needed.
    pub fn load_fast() -> Self {
        let mut config = Self::default();
        let config_path = Self::get_config_path();

        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(parsed) = toml::from_str(&content) {
                    config = parsed;
                }
            }
        } else {
            // Write default config to disk if it doesn't exist
            if let Some(parent) = config_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(toml_str) = toml::to_string_pretty(&config) {
                let _ = fs::write(&config_path, toml_str);
            }
        }

        config
    }

    pub fn load() -> Self {
        let mut config = Self::load_fast();
        config.general.autostart = is_autostart_enabled();
        config
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::get_config_path();
        if let Some(parent) = config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let toml_str = toml::to_string_pretty(self)?;
        fs::write(config_path, toml_str)?;
        let _ = set_autostart(self.general.autostart);
        Ok(())
    }
}

/// Checks if autostart entry exists for current platform
pub fn is_autostart_enabled() -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        // 1. Standard XDG autostart desktop entry
        if let Some(mut path) = dirs::config_dir() {
            path.push("autostart");
            let tiarun_entry = path.join("tiarun.desktop");
            let legacy_entry = path.join("view-launcher.desktop");
            if tiarun_entry.exists() || legacy_entry.exists() {
                return true;
            }
        }
        // 2. Sway check: check if sway config has exec tiarun --daemon
        if let Some(mut sway_dir) = dirs::config_dir() {
            sway_dir.push("sway");
            for entry in walkdir::WalkDir::new(&sway_dir).max_depth(3).into_iter().flatten() {
                if entry.file_type().is_file() {
                    if let Ok(c) = fs::read_to_string(entry.path()) {
                        if c.lines().any(|line| {
                            let l = line.trim();
                            (l.starts_with("exec ") || l.starts_with("exec_always "))
                                && (l.contains("tiarun") || l.contains("view-launcher"))
                                && l.contains("--daemon")
                        }) {
                            return true;
                        }
                    }
                }
            }
        }
        // 3. Hyprland check
        if let Some(mut hypr_cfg) = dirs::config_dir() {
            hypr_cfg.push("hypr");
            hypr_cfg.push("hyprland.conf");
            if hypr_cfg.exists() {
                if let Ok(c) = fs::read_to_string(&hypr_cfg) {
                    if c.lines().any(|line| {
                        let l = line.trim();
                        (l.starts_with("exec-once") || l.starts_with("exec =") || l.starts_with("exec="))
                            && (l.contains("tiarun") || l.contains("view-launcher"))
                            && l.contains("--daemon")
                    }) {
                        return true;
                    }
                }
            }
        }
        false
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Registry::{
            RegOpenKeyExW, RegQueryValueExW, RegCloseKey, HKEY_CURRENT_USER, KEY_READ,
        };

        // 1. Check Registry HKCU\Software\Microsoft\Windows\CurrentVersion\Run
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
            .encode_utf16()
            .collect();
        let value_name: Vec<u16> = "tiarun\0".encode_utf16().collect();

        let mut hkey = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            )
        };

        if status == 0 && !hkey.is_null() {
            let mut data_type: u32 = 0;
            let mut data_size: u32 = 0;
            let query_status = unsafe {
                RegQueryValueExW(
                    hkey,
                    value_name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut data_type,
                    std::ptr::null_mut(),
                    &mut data_size,
                )
            };
            unsafe { RegCloseKey(hkey) };
            if query_status == 0 {
                return true;
            }
        }

        // 2. Legacy fallback check: Startup folder .bat
        if let Some(mut path) = dirs::config_dir() {
            path.push("Microsoft");
            path.push("Windows");
            path.push("Start Menu");
            path.push("Programs");
            path.push("Startup");
            path.push("tiarun.bat");
            if path.exists() {
                return true;
            }
        }
        false
    }
}

/// Sets or removes system autostart entry for current platform
pub fn set_autostart(enabled: bool) -> Result<(), std::io::Error> {
    #[cfg(not(target_os = "windows"))]
    {
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "tiarun".to_string());

        // 1. Standard XDG autostart entry for Desktop Environments (GNOME, KDE, XFCE, etc.)
        if let Some(mut path) = dirs::config_dir() {
            path.push("autostart");
            if enabled {
                fs::create_dir_all(&path)?;
                path.push("tiarun.desktop");
                let desktop_entry = format!(
                    "[Desktop Entry]\nType=Application\nName=TiaRun\nComment=Think. Search. Run. - High-performance desktop launcher in Rust\nExec=\"{}\" --daemon\nIcon=tiarun\nTerminal=false\nHidden=false\nNoDisplay=true\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n",
                    exe_path
                );
                fs::write(&path, desktop_entry)?;
            } else {
                let tiarun_entry = path.join("tiarun.desktop");
                if tiarun_entry.exists() {
                    let _ = fs::remove_file(&tiarun_entry);
                }
                let legacy_entry = path.join("view-launcher.desktop");
                if legacy_entry.exists() {
                    let _ = fs::remove_file(&legacy_entry);
                }
            }
        }

        // 2. Sway integration if Sway config exists or running in Sway
        let is_sway = std::env::var("SWAYSOCK").is_ok()
            || std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase().contains("sway");
        if let Some(mut sway_dir) = dirs::config_dir() {
            sway_dir.push("sway");
            if sway_dir.exists() || is_sway {
                let session_conf = sway_dir.join("conf.d").join("50-session.conf");
                let target_file = if session_conf.exists() {
                    session_conf
                } else {
                    sway_dir.join("config")
                };

                if target_file.exists() {
                    if let Ok(content) = fs::read_to_string(&target_file) {
                        let daemon_line = "exec tiarun --daemon";
                        let has_daemon = content.lines().any(|l| {
                            let trimmed = l.trim();
                            (trimmed.starts_with("exec ") || trimmed.starts_with("exec_always "))
                                && (trimmed.contains("tiarun") || trimmed.contains("view-launcher"))
                                && trimmed.contains("--daemon")
                        });

                        if enabled && !has_daemon {
                            let mut new_content = content;
                            if !new_content.ends_with('\n') {
                                new_content.push('\n');
                            }
                            new_content.push_str(&format!("{}\n", daemon_line));
                            let _ = fs::write(&target_file, new_content);
                        } else if !enabled && has_daemon {
                            let filtered: Vec<&str> = content.lines()
                                .filter(|l| {
                                    let trimmed = l.trim();
                                    !((trimmed.starts_with("exec ") || trimmed.starts_with("exec_always "))
                                        && (trimmed.contains("tiarun") || trimmed.contains("view-launcher"))
                                        && trimmed.contains("--daemon"))
                                })
                                .collect();
                            let mut new_content = filtered.join("\n");
                            if !new_content.is_empty() {
                                new_content.push('\n');
                            }
                            let _ = fs::write(&target_file, new_content);
                        }
                    }
                }
            }
        }

        // 3. Hyprland integration if Hyprland config exists or running in Hyprland
        let is_hyprland = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok()
            || std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase().contains("hyprland");
        if let Some(mut hypr_cfg) = dirs::config_dir() {
            hypr_cfg.push("hypr");
            hypr_cfg.push("hyprland.conf");
            if hypr_cfg.exists() || is_hyprland {
                if let Ok(content) = fs::read_to_string(&hypr_cfg) {
                    let daemon_line = "exec-once = tiarun --daemon";
                    let has_daemon = content.lines().any(|l| {
                        let trimmed = l.trim();
                        trimmed.starts_with("exec-once")
                            && (trimmed.contains("tiarun") || trimmed.contains("view-launcher"))
                            && trimmed.contains("--daemon")
                    });

                    if enabled && !has_daemon {
                        let mut new_content = content;
                        if !new_content.ends_with('\n') {
                            new_content.push('\n');
                        }
                        new_content.push_str(&format!("{}\n", daemon_line));
                        let _ = fs::write(&hypr_cfg, new_content);
                    } else if !enabled && has_daemon {
                        let filtered: Vec<&str> = content.lines()
                            .filter(|l| {
                                let trimmed = l.trim();
                                !(trimmed.starts_with("exec-once")
                                    && (trimmed.contains("tiarun") || trimmed.contains("view-launcher"))
                                    && trimmed.contains("--daemon"))
                            })
                            .collect();
                        let mut new_content = filtered.join("\n");
                        if !new_content.is_empty() {
                            new_content.push('\n');
                        }
                        let _ = fs::write(&hypr_cfg, new_content);
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Registry::{
            RegOpenKeyExW, RegCreateKeyW, RegSetValueExW, RegDeleteValueW, RegCloseKey,
            HKEY_CURRENT_USER, KEY_WRITE, REG_SZ,
        };

        // 1. Clean up legacy .bat file if it exists
        if let Some(mut path) = dirs::config_dir() {
            path.push("Microsoft");
            path.push("Windows");
            path.push("Start Menu");
            path.push("Programs");
            path.push("Startup");
            let _ = fs::remove_file(path.join("tiarun.bat"));
            let _ = fs::remove_file(path.join("view-launcher.bat"));
        }

        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
            .encode_utf16()
            .collect();
        let value_name: Vec<u16> = "tiarun\0".encode_utf16().collect();

        if enabled {
            let exe_path = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "tiarun.exe".to_string());
            let cmd = format!("\"{}\" --daemon", exe_path);
            let cmd_utf16: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();

            let mut hkey = std::ptr::null_mut();
            let status = unsafe {
                RegCreateKeyW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    &mut hkey,
                )
            };
            if status == 0 && !hkey.is_null() {
                let bytes = cmd_utf16.len() * std::mem::size_of::<u16>();
                let set_status = unsafe {
                    RegSetValueExW(
                        hkey,
                        value_name.as_ptr(),
                        0,
                        REG_SZ,
                        cmd_utf16.as_ptr() as *const u8,
                        bytes as u32,
                    )
                };
                unsafe { RegCloseKey(hkey) };
                if set_status != 0 {
                    return Err(std::io::Error::from_raw_os_error(set_status as i32));
                }
            } else {
                return Err(std::io::Error::from_raw_os_error(status as i32));
            }
        } else {
            let mut hkey = std::ptr::null_mut();
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    0,
                    KEY_WRITE,
                    &mut hkey,
                )
            };
            if status == 0 && !hkey.is_null() {
                unsafe {
                    RegDeleteValueW(hkey, value_name.as_ptr());
                    RegCloseKey(hkey);
                }
            }
        }
    }
    Ok(())
}

/// Automatically registers global shortcut on supported desktop environments (GNOME, etc.)
pub fn setup_global_shortcut() {
    #[cfg(not(target_os = "windows"))]
    {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let is_gnome = desktop.contains("gnome") || desktop.contains("ubuntu");
        let is_hyprland = desktop.contains("hyprland");

        let is_sway = desktop.contains("sway") || std::env::var("SWAYSOCK").is_ok();

        if is_gnome {
            // Ensure GNOME automatically centers new windows in the middle of the screen
            let _ = std::process::Command::new("gsettings")
                .args(&["set", "org.gnome.mutter", "center-new-windows", "true"])
                .status();

            let output = std::process::Command::new("gsettings")
                .args(&["get", "org.gnome.settings-daemon.plugins.media-keys", "custom-keybindings"])
                .output();

            if let Ok(out) = output {
                let current = String::from_utf8_lossy(&out.stdout);
                let path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/tiarun/";
                if !current.contains("tiarun") && !current.contains("view-launcher") {
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
                    list.push(format!("'{}'", path));
                    let new_val = format!("[{}]", list.join(", "));
                    let _ = std::process::Command::new("gsettings")
                        .args(&["set", "org.gnome.settings-daemon.plugins.media-keys", "custom-keybindings", &new_val])
                        .status();
                    let _ = std::process::Command::new("gsettings")
                        .args(&["set", &format!("org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{}", path), "name", "TiaRun"])
                        .status();
                    let _ = std::process::Command::new("gsettings")
                        .args(&["set", &format!("org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{}", path), "command", "tiarun"])
                        .status();
                }
                // Ensure binding is set to Ctrl+Alt+Space
                let _ = std::process::Command::new("gsettings")
                    .args(&["set", &format!("org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{}", path), "binding", "<Primary><Alt>space"])
                    .status();
            }
        } else if is_hyprland {
            if let Some(mut hypr_cfg) = dirs::config_dir() {
                hypr_cfg.push("hypr");
                hypr_cfg.push("hyprland.conf");
                if hypr_cfg.exists() {
                    if let Ok(content) = fs::read_to_string(&hypr_cfg) {
                        let mut new_content = content.clone();
                        let mut modified = false;
                        if new_content.contains("bind = CTRL ALT, SPACE, exec, view-launcher")
                            || new_content.contains("bind = CTRL ALT, space, exec, view-launcher")
                        {
                            new_content = new_content
                                .replace("bind = CTRL ALT, SPACE, exec, view-launcher", "bind = CTRL ALT, space, exec, tiarun")
                                .replace("bind = CTRL ALT, space, exec, view-launcher", "bind = CTRL ALT, space, exec, tiarun");
                            modified = true;
                        } else if !new_content.contains("exec, tiarun") && !new_content.contains("exec, view-launcher") {
                            new_content.push_str("\nbind = CTRL ALT, space, exec, tiarun\n");
                            modified = true;
                        }
                        if modified {
                            let _ = fs::write(&hypr_cfg, new_content);
                            let _ = std::process::Command::new("hyprctl").arg("reload").status();
                        }
                    }
                }
            }
        } else if is_sway {
            if let Some(mut sway_dir) = dirs::config_dir() {
                sway_dir.push("sway");
                // Check if already configured anywhere in ~/.config/sway/ (including conf.d/)
                let mut already_configured = false;
                for entry in walkdir::WalkDir::new(&sway_dir).max_depth(3).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Ok(c) = fs::read_to_string(entry.path()) {
                            if c.contains("exec tiarun") || c.contains("exec view-launcher") {
                                already_configured = true;
                                break;
                            }
                        }
                    }
                }
                if !already_configured {
                    let sway_cfg = sway_dir.join("config");
                    if sway_cfg.exists() {
                        if let Ok(content) = fs::read_to_string(&sway_cfg) {
                            let mut new_content = content.clone();
                            new_content.push_str("\nbindsym Ctrl+Alt+space exec tiarun\n");
                            let _ = fs::write(&sway_cfg, new_content);
                            let _ = std::process::Command::new("swaymsg").arg("reload").status();
                        }
                    }
                }
            }
        }
    }
}
