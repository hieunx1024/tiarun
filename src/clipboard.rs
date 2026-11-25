use std::collections::VecDeque;
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use arboard::Clipboard;

#[derive(Clone)]
pub struct ClipboardManager {
    history: Arc<RwLock<VecDeque<String>>>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        let manager = Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
        };
        manager.start_monitoring();
        manager
    }

    /// Records a copied text entry at the front of history (deduplicated, max 100).
    pub fn record_text(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Ok(mut lock) = self.history.write() {
            lock.retain(|item| item != text);
            lock.push_front(text.to_string());
            if lock.len() > 100 {
                lock.pop_back();
            }
        }
    }

    fn start_monitoring(&self) {
        let manager = self.clone();
        thread::spawn(move || {
            let mut last_text = String::new();
            let mut cached_cb: Option<Clipboard> = None;

            loop {
                if let Some(text) = read_system_clipboard_text(&mut cached_cb) {
                    if !text.trim().is_empty() && text != last_text {
                        last_text = text.clone();
                        manager.record_text(&text);
                    }
                }
                thread::sleep(Duration::from_millis(600));
            }
        });
    }

    pub fn get_history(&self) -> Vec<String> {
        self.history.read().map(|h| h.iter().cloned().collect()).unwrap_or_default()
    }

    /// Reads the current system clipboard immediately (useful on GNOME Wayland when the
    /// launcher just gained focus and can read clipboard entries unavailable in background).
    pub fn refresh_now(&self) {
        let mut cb = None;
        if let Some(text) = read_system_clipboard_text(&mut cb) {
            self.record_text(&text);
        }
    }

    pub fn copy_to_clipboard(&self, text: &str) {
        copy_text_to_system_clipboard(text);
        self.record_text(text);
    }
}

fn read_system_clipboard_text(cached_cb: &mut Option<Clipboard>) -> Option<String> {
    if cached_cb.is_none() {
        *cached_cb = Clipboard::new().ok();
    }
    if let Some(cb) = cached_cb.as_mut() {
        match cb.get_text() {
            Ok(text) => return Some(text),
            Err(arboard::Error::ContentNotAvailable) => return None,
            Err(_) => {
                *cached_cb = None;
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Fallback for Wayland compositors without wlr-data-control or X11 fallback tools
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Ok(out) = Command::new("wl-paste").args(&["--no-newline"]).output() {
                if out.status.success() {
                    if let Ok(text) = String::from_utf8(out.stdout) {
                        return Some(text);
                    }
                }
            }
        }
        if let Ok(out) = Command::new("xclip").args(&["-o", "-selection", "clipboard"]).output() {
            if out.status.success() {
                if let Ok(text) = String::from_utf8(out.stdout) {
                    return Some(text);
                }
            }
        }
        if let Ok(out) = Command::new("xsel").args(&["--clipboard", "--output"]).output() {
            if out.status.success() {
                if let Ok(text) = String::from_utf8(out.stdout) {
                    return Some(text);
                }
            }
        }
    }

    None
}

fn copy_text_to_system_clipboard(text: &str) {
    #[cfg(not(target_os = "windows"))]
    {
        use std::io::Write;
        // Native clipboard utilities keep the selection alive after launcher closes/returns
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Ok(mut child) = Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().is_ok_and(|s| s.success()) {
                    return;
                }
            }
        }
        if let Ok(mut child) = Command::new("xclip").args(&["-selection", "clipboard"]).stdin(std::process::Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return;
            }
        }
        if let Ok(mut child) = Command::new("xsel").args(&["--clipboard", "--input"]).stdin(std::process::Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return;
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use arboard::SetExtLinux;
            let text_owned = text.to_string();
            // Keep the selection alive natively without requiring xclip/xsel/wl-copy.
            thread::spawn(move || {
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set().wait().text(text_owned);
                }
            });
            return;
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        if let Ok(mut cb) = Clipboard::new() {
            let _ = cb.set_text(text);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(text).is_ok() {
                return;
            }
        }

        use std::io::Write;
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("clip");
        cmd.stdin(std::process::Stdio::piped());
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        if let Ok(mut child) = cmd.spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_text_preserves_whitespace_dedups_and_caps_history() {
        let mgr = ClipboardManager {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
        };
        mgr.record_text("   \n\t  ");
        assert!(mgr.get_history().is_empty());

        mgr.record_text("  indented line\nsecond");
        mgr.record_text("alpha");
        mgr.record_text("  indented line\nsecond");

        assert_eq!(
            mgr.get_history(),
            vec![
                "  indented line\nsecond".to_string(),
                "alpha".to_string(),
            ]
        );

        for i in 0..110 {
            mgr.record_text(&format!("item-{}", i));
        }
        let hist = mgr.get_history();
        assert_eq!(hist.len(), 100);
        assert_eq!(hist[0], "item-109");
    }
}
