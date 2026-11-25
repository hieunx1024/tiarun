mod actions;
mod indexer;
mod search;
mod text;
#[cfg(test)]
mod tests;

use std::sync::{Arc, RwLock};

use fuzzy_matcher::skim::SkimMatcherV2;

use crate::config::Config;
use crate::history::HistoryManager;

pub use actions::open_config_file;
pub(crate) use text::expand_tilde;
pub use text::{
    clean_app_exec, format_file_size, parse_command_line, quote_arg, remove_vietnamese_accents,
    strip_vendor_prefix,
};

#[derive(Debug, Clone, PartialEq)]
pub enum ItemType {
    App,
    File,
    Dir,
    Calc,
    System,
    Window,
    Clipboard,
    Dmenu,
    Theme,
    Shell,
    Mode,
    WebSearch,
}

#[derive(Debug, Clone)]
pub struct LauncherItem {
    pub name: String,
    pub normalized_name: String,
    pub exec_or_path: String,
    pub normalized_path: String,
    pub item_type: ItemType,
    pub description: Option<String>,
    pub terminal: bool,
    pub icon: Option<String>,
    pub desktop_categories: Option<std::sync::Arc<str>>,
    pub wm_class: Option<std::sync::Arc<str>>,
    pub desktop_id: Option<std::sync::Arc<str>>,
    pub file_size: Option<u64>,
    pub pid: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
}

impl LauncherItem {
    pub fn new(
        name: String,
        exec_or_path: String,
        item_type: ItemType,
        description: Option<String>,
        terminal: bool,
        icon: Option<String>,
    ) -> Self {
        let normalized_name = remove_vietnamese_accents(&name);
        let normalized_path = if matches!(item_type, ItemType::File | ItemType::Dir) {
            remove_vietnamese_accents(&exec_or_path)
        } else {
            String::new()
        };
        Self {
            name,
            normalized_name,
            exec_or_path,
            normalized_path,
            item_type,
            description,
            terminal,
            icon,
            desktop_categories: None,
            wm_class: None,
            desktop_id: None,
            file_size: None,
            pid: None,
            memory_bytes: None,
            cpu_percent: None,
        }
    }

    pub fn with_file_size(mut self, file_size: Option<u64>) -> Self {
        self.file_size = file_size;
        self
    }

    pub fn with_process_stats(
        mut self,
        pid: Option<u32>,
        memory_bytes: Option<u64>,
        cpu_percent: Option<f32>,
    ) -> Self {
        self.pid = pid;
        self.memory_bytes = memory_bytes;
        self.cpu_percent = cpu_percent;
        self
    }

    pub fn get_category_tag(&self) -> &'static str {
        match self.item_type {
            ItemType::Theme => "Theme",
            ItemType::Shell => "Shell",
            ItemType::Mode => "Mode",
            ItemType::WebSearch => "Web Search",
            ItemType::Calc => "Calc",
            ItemType::Dir => "Folder",
            ItemType::File => "File",
            ItemType::System => "System",
            ItemType::Window => "Window",
            ItemType::Clipboard => "Clipboard",
            ItemType::Dmenu => "Item",
            ItemType::App => {
                if let Some(category) = crate::apps::AppCategoryFilter::specific_label(self) {
                    return category;
                }
                let lower = format!(
                    "{} {} {}",
                    self.name.to_lowercase(),
                    self.exec_or_path.to_lowercase(),
                    self.icon.as_deref().unwrap_or("")
                );
                if lower.contains("idea")
                    || lower.contains("pycharm")
                    || lower.contains("clion")
                    || lower.contains("webstorm")
                    || lower.contains("code")
                    || lower.contains("studio")
                    || lower.contains("nvim")
                    || lower.contains("vim")
                {
                    "IDE"
                } else if lower.contains("dbeaver")
                    || lower.contains("datagrip")
                    || lower.contains("database")
                    || lower.contains("mysql")
                    || lower.contains("postgres")
                    || lower.contains("redis")
                    || lower.contains("mongo")
                {
                    "Database"
                } else if lower.contains("firefox")
                    || lower.contains("chrome")
                    || lower.contains("brave")
                    || lower.contains("edge")
                    || lower.contains("browser")
                {
                    "Browser"
                } else if lower.contains("viber")
                    || lower.contains("telegram")
                    || lower.contains("discord")
                    || lower.contains("slack")
                    || lower.contains("teams")
                    || lower.contains("chat")
                {
                    "Chat"
                } else if lower.contains("spotify")
                    || lower.contains("vlc")
                    || lower.contains("music")
                    || lower.contains("video")
                    || lower.contains("obs")
                    || lower.contains("gimp")
                    || lower.contains("blender")
                {
                    "Media"
                } else if lower.contains("writer")
                    || lower.contains("calc")
                    || lower.contains("office")
                    || lower.contains("word")
                    || lower.contains("excel")
                    || lower.contains("notes")
                    || lower.contains("pdf")
                {
                    "Office"
                } else if lower.contains("terminal")
                    || lower.contains("system")
                    || lower.contains("monitor")
                    || lower.contains("disk")
                    || lower.contains("driver")
                    || lower.contains("setting")
                    || lower.contains("network")
                {
                    "System"
                } else {
                    "App"
                }
            }
        }
    }
}

pub struct LauncherEngine {
    pub apps: Vec<LauncherItem>,
    pub custom_apps: Vec<LauncherItem>,
    pub pinned_apps: Vec<String>,
    pub hidden_apps: Vec<String>,
    pub shallow_files: Arc<RwLock<Vec<LauncherItem>>>,
    pub history: Arc<RwLock<HistoryManager>>,
    pub clipboard: Arc<crate::clipboard::ClipboardManager>,
    pub(super) matcher: SkimMatcherV2,
    pub config: Config,
    /// Filled in by a background thread once the shallow file list is loaded,
    /// so registering the watches never delays startup.
    pub file_watcher: Arc<std::sync::Mutex<Option<crate::file_watcher::FileWatcher>>>,
}

impl LauncherEngine {
    pub fn new(config: Config) -> Self {
        let history = Arc::new(RwLock::new(HistoryManager::load()));
        let clipboard = Arc::new(crate::clipboard::ClipboardManager::new());
        let shallow_files = Arc::new(RwLock::new(Vec::new()));
        let file_watcher = Arc::new(std::sync::Mutex::new(None));

        let mut engine = Self {
            apps: Vec::new(),
            custom_apps: Vec::new(),
            pinned_apps: config.apps.pinned.clone(),
            hidden_apps: config.apps.hidden.clone(),
            shallow_files,
            history,
            clipboard,
            matcher: SkimMatcherV2::default(),
            config: config.clone(),
            file_watcher,
        };

        // Load custom apps from config
        for custom in &config.apps.custom {
            let mut item = LauncherItem::new(
                custom.name.clone(),
                custom.exec.clone(),
                ItemType::App,
                custom.description.clone(),
                custom.terminal,
                custom.icon.clone(),
            );
            item.desktop_categories = custom.category.clone().map(Arc::from);
            engine.custom_apps.push(item);
        }

        engine.index_apps();

        let shallow_files_clone = engine.shallow_files.clone();
        let config_clone = config.clone();
        let watcher_slot = engine.file_watcher.clone();
        std::thread::spawn(move || {
            let mut files = Vec::new();
            Self::index_files_impl(&config_clone, &mut files);
            if let Ok(mut lock) = shallow_files_clone.write() {
                *lock = files;
            }
            // Started after the list is in place so its updates are not overwritten.
            let watcher = crate::file_watcher::FileWatcher::start(shallow_files_clone, &config_clone);
            *watcher_slot.lock().unwrap_or_else(|error| error.into_inner()) = watcher;
        });

        engine
    }

    pub fn available_categories(&self) -> Vec<String> {
        let all_apps: Vec<&LauncherItem> = self.apps.iter().chain(self.custom_apps.iter()).collect();
        crate::apps::AppCategoryFilter::get_available_categories(all_apps)
    }
}
