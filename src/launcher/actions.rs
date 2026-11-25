use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(not(target_os = "windows"))]
use super::text::parse_command_line;
use super::{ItemType, LauncherEngine, LauncherItem};

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
}

pub fn open_config_file() {
    let config_path = crate::config::Config::get_config_path();
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("xdg-open")
            .arg(&config_path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        #[cfg(windows)]
        use std::os::windows::process::CommandExt;
        #[allow(unused_mut)]
        let mut cmd = Command::new("cmd");
        cmd.args(&["/C", "start", "", &config_path.to_string_lossy()]);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let _ = cmd.spawn();
    }
}

#[cfg(not(target_os = "windows"))]
fn spawn_detached(cmd: &mut Command) {
    unsafe {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .pre_exec(|| {
                setsid();
                Ok(())
            })
            .spawn()
            .ok();
    }
}

/// Most terminal emulators (xterm, kitty, alacritty, foot, konsole, wezterm, ghostty,
/// xfce4-terminal) run a command via `-e prog arg1 arg2 ...` as separate argv entries.
/// gnome-terminal deprecated that form: passing more than one token after `-e` now
/// fails with "Unknown option", and it asks for `-- prog arg1 arg2 ...` instead.
#[cfg(not(target_os = "windows"))]
fn terminal_exec_flag(term: &str) -> &'static str {
    if term.contains("gnome-terminal") { "--" } else { "-e" }
}

/// Runs an arbitrary shell command line in a terminal, via `sh -c` (Linux) or
/// `cmd /K` (Windows) so the user's command supports pipes, redirects, `&&`, etc.
/// The terminal is left open after the command finishes so output/errors stay visible.
fn run_shell_command_in_terminal(command: &str) {
    #[cfg(not(target_os = "windows"))]
    {
        let term = find_terminal_emulator();
        let wrapped = format!(
            "{}; printf '\\n[Done. Press Enter to close]'; read _",
            command
        );
        unsafe {
            Command::new(&term)
                .arg(terminal_exec_flag(&term))
                .arg("sh")
                .arg("-c")
                .arg(&wrapped)
                .pre_exec(|| {
                    setsid();
                    Ok(())
                })
                .spawn()
                .ok();
        }
    }

    #[cfg(target_os = "windows")]
    {
        #[cfg(windows)]
        use std::os::windows::process::CommandExt;
        #[allow(unused_mut)]
        let mut cmd = Command::new("cmd");
        cmd.args(&["/C", "start", "cmd", "/K", command]);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW (applies to the outer `start` launcher only)
        let _ = cmd.spawn();
    }
}

#[cfg(not(target_os = "windows"))]
fn find_terminal_emulator() -> String {
    if let Ok(term) = std::env::var("TERMINAL") {
        if !term.trim().is_empty() {
            return term;
        }
    }

    let common_terminals = [
        "kitty",
        "alacritty",
        "wezterm",
        "ghostty",
        "foot",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "xterm",
    ];

    for term in common_terminals {
        if which_binary(term) {
            return term.to_string();
        }
    }

    "xterm".to_string()
}

#[cfg(not(target_os = "windows"))]
fn which_binary(name: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let p = Path::new(dir).join(name);
            if p.exists() && p.is_file() {
                return true;
            }
        }
    }
    false
}

impl LauncherEngine {
    /// Spawns the application or executes item action safely.
    pub fn launch(&self, item: &LauncherItem) {
        if item.item_type == ItemType::Calc || item.item_type == ItemType::Clipboard {
            self.clipboard.copy_to_clipboard(&item.exec_or_path);
            return;
        }

        if item.item_type == ItemType::Window {
            crate::window_switcher::focus_window(&item.exec_or_path);
            return;
        }

        if item.item_type == ItemType::Mode || item.item_type == ItemType::Theme {
            return;
        }

        if item.item_type == ItemType::System {
            crate::system_actions::execute_system_action(&item.exec_or_path);
            return;
        }

        if item.item_type == ItemType::Dmenu {
            println!("{}", item.name);
            std::process::exit(0);
        }

        if item.item_type == ItemType::Shell {
            if let Ok(mut h) = self.history.write() {
                h.record_shell_command(&item.exec_or_path);
            }
            run_shell_command_in_terminal(&item.exec_or_path);
            return;
        }

        // Record history
        let key = if item.item_type == ItemType::App { &item.name } else { &item.exec_or_path };
        if let Ok(mut h) = self.history.write() {
            h.record_launch(key);
        }

        #[cfg(not(target_os = "windows"))]
        {
            match item.item_type {
                ItemType::App => {
                    let tokens = parse_command_line(&item.exec_or_path);
                    if tokens.is_empty() { return; }

                    if item.terminal {
                        let term = find_terminal_emulator();
                        let mut cmd = Command::new(&term);
                        cmd.arg(terminal_exec_flag(&term)).args(&tokens);
                        spawn_detached(&mut cmd);
                    } else {
                        let mut cmd = Command::new(&tokens[0]);
                        if tokens.len() > 1 {
                            cmd.args(&tokens[1..]);
                        }
                        spawn_detached(&mut cmd);
                    }
                }
                ItemType::File | ItemType::Dir | ItemType::WebSearch => {
                    let mut cmd = Command::new("xdg-open");
                    cmd.arg(&item.exec_or_path);
                    spawn_detached(&mut cmd);
                }
                ItemType::Calc | ItemType::Clipboard | ItemType::Window | ItemType::System | ItemType::Dmenu | ItemType::Theme | ItemType::Shell | ItemType::Mode => {}
            }
        }

        #[cfg(target_os = "windows")]
        {
            #[cfg(windows)]
            use std::os::windows::process::CommandExt;
            #[allow(unused_mut)]
            let mut cmd = Command::new("cmd");
            cmd.args(&["/C", "start", "", &item.exec_or_path])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            let _ = cmd.spawn();
        }
    }

    /// Opens the containing directory of the item in terminal.
    pub fn open_in_terminal(&self, item: &LauncherItem) {
        let dir = match item.item_type {
            ItemType::Dir => PathBuf::from(&item.exec_or_path),
            ItemType::File => Path::new(&item.exec_or_path).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")),
            ItemType::App | ItemType::Calc | ItemType::Clipboard | ItemType::Window | ItemType::System | ItemType::Dmenu | ItemType::Theme | ItemType::Shell | ItemType::Mode | ItemType::WebSearch => return,
        };

        #[cfg(not(target_os = "windows"))]
        {
            let term = find_terminal_emulator();
            let mut cmd = Command::new(&term);
            cmd.current_dir(&dir);
            spawn_detached(&mut cmd);
        }

        #[cfg(target_os = "windows")]
        {
            #[cfg(windows)]
            use std::os::windows::process::CommandExt;
            #[allow(unused_mut)]
            let dir_str = dir.to_string_lossy().to_string();
            let mut cmd = Command::new("cmd");
            cmd.args(&[
                "/C",
                "start",
                "",
                "cmd",
                "/C",
                &format!("where wt.exe >nul 2>&1 && start wt.exe -d \"{0}\" || start powershell.exe -NoExit -Command Set-Location -LiteralPath '{0}'", dir_str),
            ]);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            let _ = cmd.spawn();
        }
    }

    /// Copies path or result string to clipboard.
    pub fn copy_to_clipboard(&self, text: &str) {
        self.clipboard.copy_to_clipboard(text);
    }

    /// Copies an image's raw pixels, or file URI/path to clipboard.
    pub fn copy_file_or_image(&self, path_str: &str) {
        let path = Path::new(path_str);
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let canonical_str = canonical.to_string_lossy().to_string();
        let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "ico");

        if is_image && canonical.exists() {
            // On Linux (Wayland / X11), native clipboard tools like wl-copy and xclip
            // keep the image alive persistently in the desktop clipboard.
            // Arboard on Linux drops its X11 window almost immediately when dropped,
            // and has no native Wayland clipboard support.
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                use std::io::Write;
                let png_bytes: Option<Vec<u8>> = if ext == "png" {
                    fs::read(&canonical).ok()
                } else if let Ok(img) = image::open(&canonical) {
                    let mut buf = Vec::new();
                    if img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).is_ok() {
                        Some(buf)
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(bytes) = png_bytes {
                    let mut copied = false;
                    // 1. Wayland wl-copy
                    if let Ok(mut child) = Command::new("wl-copy")
                        .args(&["-t", "image/png"])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                    {
                        if let Some(mut stdin) = child.stdin.take() {
                            let _ = stdin.write_all(&bytes);
                        }
                        if let Ok(status) = child.wait() {
                            if status.success() {
                                copied = true;
                            }
                        }
                    }

                    // 2. X11 xclip fallback
                    if !copied {
                        if let Ok(mut child) = Command::new("xclip")
                            .args(&["-selection", "clipboard", "-t", "image/png"])
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                        {
                            if let Some(mut stdin) = child.stdin.take() {
                                let _ = stdin.write_all(&bytes);
                            }
                            if let Ok(status) = child.wait() {
                                if status.success() {
                                    copied = true;
                                }
                            }
                        }
                    }

                    if copied {
                        return;
                    }
                }
            }

            // Cross-platform / Windows / macOS / fallback: use arboard
            if let Ok(img) = image::open(&canonical) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let img_data = arboard::ImageData {
                    width: w as usize,
                    height: h as usize,
                    bytes: std::borrow::Cow::Borrowed(rgba.as_raw()),
                };

                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if cb.set_image(img_data).is_ok() {
                        return;
                    }
                }
            }
        }

        // 2. If it's a file or directory, copy file URI on Linux Wayland so File Managers can paste file
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::io::Write;
            let uri = format!("file://{}\r\n", canonical_str);
            if let Ok(mut child) = Command::new("wl-copy")
                .args(&["-t", "text/uri-list"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(uri.as_bytes());
                }
                if let Ok(status) = child.wait() {
                    if status.success() {
                        return;
                    }
                }
            }
        }

        // 3. Fallback: Copy path string to clipboard
        self.copy_to_clipboard(&canonical_str);
    }

    /// Smart copy: copies image pixels for images, file URI for files, result for calculator, etc.
    pub fn copy_item_smart(&self, item: &LauncherItem) {
        match item.item_type {
            ItemType::File | ItemType::Dir => {
                self.copy_file_or_image(&item.exec_or_path);
            }
            ItemType::Calc => {
                if let Some(res) = item.description.as_ref() {
                    self.copy_to_clipboard(res);
                } else {
                    self.copy_to_clipboard(&item.name);
                }
            }
            ItemType::Clipboard => {
                self.copy_to_clipboard(&item.exec_or_path);
            }
            _ => {
                if !item.exec_or_path.is_empty() {
                    self.copy_to_clipboard(&item.exec_or_path);
                } else {
                    self.copy_to_clipboard(&item.name);
                }
            }
        }
    }
}
