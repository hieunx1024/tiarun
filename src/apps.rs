use crate::launcher::{ItemType, LauncherItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategoryFilter {
    All,
    Applications,
    Development,
    Browser,
    Messages,
    Media,
    Office,
    Settings,
    System,
    Actions,
    Calendar,
    Mail,
    Contacts,
    Help,
}

impl AppCategoryFilter {
    pub fn from_chip_name(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "applications" | "apps" | "all" => Self::Applications,
            "development" | "dev" | "ide" => Self::Development,
            "browser" | "browsers" | "web" | "internet" => Self::Browser,
            "messages" | "chat" | "messaging" => Self::Messages,
            "media" | "multimedia" | "audio" | "video" | "graphics" => Self::Media,
            "office" | "documents" | "productivity" => Self::Office,
            "settings" | "preferences" | "config" => Self::Settings,
            "system" | "utilities" | "utility" | "tools" => Self::System,
            "actions" => Self::Actions,
            "calendar" => Self::Calendar,
            "mail" | "email" => Self::Mail,
            "contacts" | "people" => Self::Contacts,
            "help" => Self::Help,
            _ => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All | Self::Applications => "Applications",
            Self::Development => "Development",
            Self::Browser => "Browser",
            Self::Messages => "Messages",
            Self::Media => "Media",
            Self::Office => "Office",
            Self::Settings => "Settings",
            Self::System => "System",
            Self::Actions => "Actions",
            Self::Calendar => "Calendar",
            Self::Mail => "Mail",
            Self::Contacts => "Contacts",
            Self::Help => "Help",
        }
    }

    pub fn matches(self, item: &LauncherItem) -> bool {
        match self {
            Self::All => true,
            Self::Applications => item.item_type == ItemType::App,
            Self::Actions => matches!(item.item_type, ItemType::System | ItemType::Shell | ItemType::Theme),
            _ => item.item_type == ItemType::App && self.matches_app(item),
        }
    }

    fn desktop_tags(self) -> &'static [&'static str] {
        match self {
            Self::Development => &[
                "Development", "IDE", "Programming", "Debugger", "Building",
                "GUIDesigner", "Profiling", "RevisionControl", "Translation",
                "Database", "WebDevelopment",
            ],
            Self::Browser => &["WebBrowser"],
            Self::Messages => &["InstantMessaging", "Chat", "IRCClient", "VideoConference", "Telephony"],
            Self::Media => &[
                "AudioVideo", "Audio", "Video", "Graphics", "2DGraphics",
                "RasterGraphics", "VectorGraphics", "Photography", "Music", "Player", "Recorder",
                "Midi", "Sequencer", "Mixer",
            ],
            Self::Office => &["Office", "WordProcessor", "Spreadsheet", "Presentation", "Publishing", "Viewer"],
            Self::Settings => &["Settings", "DesktopSettings", "HardwareSettings", "PackageManager"],
            Self::System => &[
                "System", "Utility", "FileManager", "FileTools", "Filesystem",
                "Monitor", "Core", "TerminalEmulator", "Settings", "DesktopSettings",
                "HardwareSettings", "PackageManager",
            ],
            Self::Calendar => &["Calendar"],
            Self::Mail => &["Email"],
            Self::Contacts => &["ContactManagement"],
            Self::Help => &["Documentation"],
            _ => &[],
        }
    }

    fn matches_app(self, item: &LauncherItem) -> bool {
        if item.desktop_categories.as_deref().is_some_and(|categories| {
            categories.split(';').any(|tag| self.desktop_tags().contains(&tag))
        }) {
            return true;
        }

        // Match against item name, exec, and icon hint (with accent-insensitivity)
        let name = item.name.to_lowercase();
        let name_norm = crate::launcher::remove_vietnamese_accents(&name);
        let exec = item.exec_or_path.to_lowercase();
        let icon = item.icon.as_deref().unwrap_or("").to_lowercase();

        let contains_kw = |keywords: &[&str]| {
            let check_target = |s: &str| {
                if s.is_empty() {
                    return false;
                }
                let tokens: Vec<&str> = s
                    .split(|ch: char| !ch.is_alphanumeric())
                    .filter(|t| !t.is_empty())
                    .collect();
                keywords.iter().any(|&kw| {
                    if kw.contains(' ') {
                        s.contains(kw)
                    } else {
                        tokens.iter().any(|&tok| tok == kw)
                    }
                })
            };
            check_target(&name)
                || check_target(&name_norm)
                || check_target(&exec)
                || check_target(&icon)
        };

        match self {
            Self::Development => contains_kw(&[
                "code", "idea", "intellij", "intel", "pycharm", "clion", "webstorm", "phpstorm", "goland",
                "rider", "rubymine", "rustrover", "datagrip", "fleet", "studio", "nvim", "vim",
                "neovim", "sublime", "git", "dev", "compiler", "dbeaver", "postman", "insomnia",
                "docker", "podman", "kubernetes", "kubectl", "wireshark", "tableplus", "navicat",
                "workbench", "pgadmin", "sqlite", "mongodb", "compass", "redis",
                "python", "node", "rust", "cargo", "java", "javac", "gcc", "clang", "dotnet",
                "eclipse", "netbeans", "android studio", "xcode", "qtcreator", "cursor", "zed",
                "vscodium", "vscode", "sysprof", "antigravity", "claude", "chatgpt",
            ]),
            Self::Browser => contains_kw(&[
                "firefox", "chrome", "chromium", "brave", "edge", "browser", "opera",
                "vivaldi", "tor", "safari", "coc coc", "coccoc", "waterfox", "librewolf",
                "epiphany", "midori", "zen", "trình duyệt", "web",
            ]),
            Self::Messages => contains_kw(&[
                "messages", "chat", "slack", "discord", "telegram", "teams", "viber",
                "zalo", "whatsapp", "messenger", "zoom", "signal", "element", "tin nhắn",
                "skype", "mattermost", "matrix", "wechat", "line", "revolt", "voip", "meet",
            ]),
            Self::Media => contains_kw(&[
                "spotify", "vlc", "music", "video", "obs", "gimp", "blender", "inkscape",
                "kdenlive", "audacity", "media", "mpv", "photo", "image", "player", "shotcut",
                "handbrake", "davinci", "lightroom", "photoshop", "rhythmbox", "totem",
                "smplayer", "clementine", "deadbeef", "netflix", "youtube", "sound", "audio",
                "graphics", "picture", "camera", "âm nhạc", "hình ảnh", "phim", "mp3", "mp4",
            ]),
            Self::Office => contains_kw(&[
                "writer", "calc", "office", "word", "excel", "notes", "pdf", "libreoffice",
                "document", "powerpoint", "onlyoffice", "wps", "notion", "obsidian", "evernote",
                "onenote", "evince", "okular", "acrobat", "foxit", "spreadsheet", "presentation",
                "văn phòng", "tài liệu", "ghi chú",
            ]),
            Self::Settings => contains_kw(&[
                "settings", "preferences", "systemsettings", "control center", "cài đặt",
                "config", "tweaks", "setup", "configuration", "cấu hình", "bluetooth",
                "network", "display", "sound", "keyboard", "mouse", "driver", "software updater",
            ]),
            Self::System => contains_kw(&[
                "terminal", "system", "monitor", "disk", "driver", "files", "file manager",
                "thunar", "nautilus", "htop", "btop", "konsole", "alacritty", "kitty",
                "wezterm", "gnome-terminal", "foot", "task manager", "sysprof", "logs",
                "hardware", "process", "memory", "storage", "hệ thống",
                "settings", "preferences", "systemsettings", "control center", "cài đặt",
                "config", "tweaks", "setup", "configuration", "cấu hình", "bluetooth",
                "network", "display", "sound", "keyboard", "mouse", "software updater",
            ]),
            Self::Calendar => contains_kw(&["calendar", "kalender", "korganizer", "lịch"]),
            Self::Mail => contains_kw(&["mail", "email", "thunderbird", "outlook", "geary", "kmail", "thư điện tử"]),
            Self::Contacts => contains_kw(&["contacts", "kaddressbook", "address book", "danh bạ", "people"]),
            Self::Help => contains_kw(&["help", "yelp", "khelpcenter", "hướng dẫn", "manual", "documentation"]),
            _ => false,
        }
    }

    pub(crate) fn specific_label(item: &LauncherItem) -> Option<&'static str> {
        let categories = [
            Self::Development,
            Self::Browser,
            Self::Media,
            Self::System,
            Self::Office,
            Self::Messages,
            Self::Settings,
            Self::Calendar,
            Self::Mail,
            Self::Contacts,
            Self::Help,
        ];
        // A single label represents the first matching category, while the
        // filters can expose the app in every category it belongs to.
        categories.into_iter()
            .find_map(|category| category.matches(item).then_some(category.label()))
    }

    pub fn get_available_categories<'a>(apps: impl IntoIterator<Item = &'a LauncherItem>) -> Vec<String> {
        let app_vec: Vec<&'a LauncherItem> = apps.into_iter().collect();
        let mut list = Vec::new();
        list.push("Applications".to_string());

        let candidates = [
            Self::Development,
            Self::Browser,
            Self::Media,
            Self::Office,
            Self::System,
            Self::Actions,
        ];

        for cat in candidates {
            if cat == Self::Actions {
                list.push("Actions".to_string());
            } else if app_vec.iter().any(|app| cat.matches(app)) {
                list.push(cat.label().to_string());
            }
        }

        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_uses_desktop_metadata_and_matches_whole_words() {
        let mut app = LauncherItem::new(
            "Evolution".into(), "evolution".into(), ItemType::App, None, false, None,
        );
        app.desktop_categories = Some("Office;Email;Calendar;ContactManagement;".into());
        assert!(AppCategoryFilter::Mail.matches(&app));
        assert!(AppCategoryFilter::Calendar.matches(&app));
        assert!(AppCategoryFilter::Contacts.matches(&app));
        assert!(!AppCategoryFilter::Messages.matches(&app));

        app.name = "Mailman".into();
        app.desktop_categories = None;
        assert!(!AppCategoryFilter::Mail.matches(&app));

        app.name = "Help".into();
        app.item_type = ItemType::System;
        assert!(AppCategoryFilter::Actions.matches(&app));
        assert!(!AppCategoryFilter::Help.matches(&app));
    }

    #[test]
    fn test_category_filter_from_chip() {
        assert_eq!(AppCategoryFilter::from_chip_name("Calendar"), AppCategoryFilter::Calendar);
        assert_eq!(AppCategoryFilter::from_chip_name("Applications"), AppCategoryFilter::Applications);
        assert_eq!(AppCategoryFilter::from_chip_name("Actions"), AppCategoryFilter::Actions);
        assert_eq!(AppCategoryFilter::from_chip_name("Messages"), AppCategoryFilter::Messages);
        assert_eq!(AppCategoryFilter::from_chip_name("Help"), AppCategoryFilter::Help);
        assert_eq!(AppCategoryFilter::from_chip_name("Mail"), AppCategoryFilter::Mail);
        assert_eq!(AppCategoryFilter::from_chip_name("Settings"), AppCategoryFilter::Settings);
        assert_eq!(AppCategoryFilter::from_chip_name("Contacts"), AppCategoryFilter::Contacts);
        assert_eq!(AppCategoryFilter::from_chip_name("Development"), AppCategoryFilter::Development);
        assert_eq!(AppCategoryFilter::from_chip_name("Browser"), AppCategoryFilter::Browser);
        assert_eq!(AppCategoryFilter::from_chip_name("Media"), AppCategoryFilter::Media);
        assert_eq!(AppCategoryFilter::from_chip_name("Office"), AppCategoryFilter::Office);
        assert_eq!(AppCategoryFilter::from_chip_name("System"), AppCategoryFilter::System);
        assert_eq!(AppCategoryFilter::from_chip_name("Unknown"), AppCategoryFilter::All);
    }

    #[test]
    fn test_new_categories_and_available_list() {
        let mut vscode = LauncherItem::new("Visual Studio Code".into(), "code".into(), ItemType::App, None, false, None);
        vscode.desktop_categories = Some("Development;IDE;".into());

        let mut chrome = LauncherItem::new("Google Chrome".into(), "google-chrome".into(), ItemType::App, None, false, None);
        chrome.desktop_categories = Some("Network;WebBrowser;".into());

        let spotify = LauncherItem::new("Spotify".into(), "spotify".into(), ItemType::App, None, false, None);

        assert!(AppCategoryFilter::Development.matches(&vscode));
        assert!(AppCategoryFilter::Browser.matches(&chrome));
        assert!(AppCategoryFilter::Media.matches(&spotify));

        assert_eq!(AppCategoryFilter::specific_label(&vscode), Some("Development"));
        assert_eq!(AppCategoryFilter::specific_label(&chrome), Some("Browser"));
        assert_eq!(AppCategoryFilter::specific_label(&spotify), Some("Media"));

        let apps = vec![vscode, chrome, spotify];
        let available = AppCategoryFilter::get_available_categories(&apps);
        assert!(available.contains(&"Applications".to_string()));
        assert!(available.contains(&"Development".to_string()));
        assert!(available.contains(&"Browser".to_string()));
        assert!(available.contains(&"Media".to_string()));
        assert!(available.contains(&"Actions".to_string()));
        // Categories without any matching apps should not be present
        assert!(!available.contains(&"Calendar".to_string()));
        assert!(!available.contains(&"Mail".to_string()));
        assert!(!available.contains(&"Contacts".to_string()));
    }
}
