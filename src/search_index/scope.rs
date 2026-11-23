use std::path::{Path, PathBuf};

use crate::config::SearchConfig;
use crate::launcher::expand_tilde;

const HARD_IGNORED_NAMES: &[&str] = &[
    "node_modules", ".git", "target", "build", "dist", ".cache", "venv", ".venv",
    "__pycache__", ".cargo", ".rustup", ".local", "AppData", ".Trash", ".Trash-1000",
    ".idea", ".vscode", "vendor", "bin", "obj", ".next", ".nuxt", ".gradle", ".m2",
    ".npm", ".nvm", "proc", "sys", "dev",
];

pub(crate) struct SearchScope {
    roots: Vec<(PathBuf, usize)>,
    fallback_roots: Vec<(PathBuf, usize)>,
    home: Option<PathBuf>,
    ignored_dirs: Vec<String>,
    ignored_extensions: Vec<String>,
}

impl SearchScope {
    pub(crate) fn new(config: &SearchConfig) -> Self {
        let mut roots = Vec::new();
        let mut fallback_roots = Vec::new();
        let home = if config.paths.is_empty() {
            let home = dirs::home_dir();
            for (dir, depth, fallback_depth) in [
                (home.clone(), 1, Some(1)),
                (dirs::desktop_dir(), 12, Some(6)),
                (dirs::document_dir(), 12, Some(6)),
                (dirs::download_dir(), 4, Some(3)),
                (dirs::picture_dir(), 4, None),
                (dirs::video_dir(), 2, None),
            ] {
                if let Some(dir) = dir {
                    if let Some(depth) = fallback_depth {
                        fallback_roots.push((dir.clone(), depth));
                    }
                    roots.push((dir, depth));
                }
            }
            home
        } else {
            for entry in &config.paths {
                let path = expand_tilde(&entry.path);
                if let Ok(root) = std::path::absolute(path) {
                    roots.push((root, entry.max_depth.unwrap_or(config.max_depth)));
                }
            }
            fallback_roots.clone_from(&roots);
            None
        };

        Self {
            roots,
            fallback_roots,
            home,
            ignored_dirs: config.ignored_dirs.clone(),
            ignored_extensions: config.ignored_extensions.clone(),
        }
    }

    pub(crate) fn crawl_roots(&self) -> Vec<(PathBuf, usize)> {
        let mut roots = self.roots.clone();
        if let Some(entries) = self.home.as_ref().and_then(|home| std::fs::read_dir(home).ok()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if self.is_default_subdirectory(&path) && path.is_dir() {
                    roots.push((path, 10));
                }
            }
        }
        roots
    }

    pub(crate) fn fallback_roots(&self) -> &[(PathBuf, usize)] {
        &self.fallback_roots
    }

    fn is_default_subdirectory(&self, path: &Path) -> bool {
        self.allows_name(path) && !self.roots.iter().any(|(root, _)| root == path)
    }

    pub(crate) fn allows_name(&self, path: &Path) -> bool {
        path.file_name().is_none_or(|name| {
            name.to_str().is_some_and(|name| {
                !name.starts_with('.')
                    && !HARD_IGNORED_NAMES.contains(&name)
                    && !self.ignored_dirs.iter().any(|ignored| ignored == name)
            })
        })
    }

    pub(crate) fn allows_extension(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        !self.ignored_extensions.iter().any(|ignored| {
            ignored.strip_prefix('.').unwrap_or(ignored).eq_ignore_ascii_case(ext)
        })
    }

    fn contains_under_root(&self, root: &Path, depth: usize, path: &Path) -> bool {
        path.strip_prefix(root).is_ok_and(|relative| {
            self.allows_name(root)
                && relative.components().enumerate().all(|(index, component)| {
                    index < depth && matches!(
                        component,
                        std::path::Component::Normal(name) if self.allows_name(Path::new(name))
                    )
                })
        })
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        if !self.allows_extension(path) {
            return false;
        }
        if self.roots.iter().any(|(root, depth)| self.contains_under_root(root, *depth, path)) {
            return true;
        }
        self.home.as_ref().is_some_and(|home| {
            let Ok(relative) = path.strip_prefix(home) else {
                return false;
            };
            let mut components = relative.components();
            let Some(std::path::Component::Normal(name)) = components.next() else {
                return false;
            };
            let directory = home.join(name);
            components.next().is_some()
                && self.is_default_subdirectory(&directory)
                && self.contains_under_root(&directory, 10, path)
        })
    }
}
