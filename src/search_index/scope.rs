use std::path::{Path, PathBuf};

use crate::config::SearchConfig;
use crate::launcher::expand_tilde;

const HARD_IGNORED_NAMES: &[&str] = &[
    "node_modules", ".git", "target", "build", "dist", ".cache", "venv", ".venv",
    "__pycache__", ".cargo", ".rustup", ".local", "AppData", ".Trash", ".Trash-1000",
    ".idea", ".vscode", "vendor", "bin", "obj", ".next", ".nuxt", ".gradle", ".m2",
    ".npm", ".nvm", "proc", "sys", "dev",
];

/// Package-manager caches under the home directory that are not named like the
/// entries of `HARD_IGNORED_NAMES`. Go's module cache (`~/go/pkg/mod`) alone can
/// hold 100k+ third-party source files and dwarf the user's own documents.
fn default_ignored_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(cache) = std::env::var_os("GOMODCACHE").filter(|value| !value.is_empty()) {
        paths.push(PathBuf::from(cache));
    }
    let gopaths = std::env::var_os("GOPATH").filter(|value| !value.is_empty());
    let gopaths: Vec<PathBuf> = match gopaths {
        Some(value) => std::env::split_paths(&value).collect(),
        None => vec![home.join("go")],
    };
    for gopath in gopaths {
        paths.push(gopath.join("pkg").join("mod"));
        paths.push(gopath.join("pkg").join("sumdb"));
    }
    paths.dedup();
    paths
}

pub(crate) struct SearchScope {
    roots: Vec<(PathBuf, usize)>,
    fallback_roots: Vec<(PathBuf, usize)>,
    home: Option<PathBuf>,
    /// Absolute directories excluded from the default scope.
    ignored_paths: Vec<PathBuf>,
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
        let ignored_paths = home.as_deref().map(default_ignored_paths).unwrap_or_default();

        Self {
            roots,
            fallback_roots,
            home,
            ignored_paths,
            ignored_dirs: config.ignored_dirs.clone(),
            ignored_extensions: config.ignored_extensions.clone(),
        }
    }

    /// Identifies the configuration this scope was built from, so a completed
    /// index pass can be trusted again after a restart with the same settings.
    pub(crate) fn signature(&self) -> String {
        format!(
            "v1|{:?}|{:?}|{:?}|{:?}|{:?}",
            self.roots, self.home, self.ignored_paths, self.ignored_dirs, self.ignored_extensions
        )
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
        if self.is_ignored_path(path) {
            return false;
        }
        path.file_name().is_none_or(|name| {
            name.to_str().is_some_and(|name| {
                !name.starts_with('.')
                    && !HARD_IGNORED_NAMES.iter().any(|ignored| ignored.eq_ignore_ascii_case(name))
                    && !self.ignored_dirs.iter().any(|ignored| ignored.eq_ignore_ascii_case(name))
            })
        })
    }

    fn is_ignored_path(&self, path: &Path) -> bool {
        !self.ignored_paths.is_empty()
            && path.is_absolute()
            && self.ignored_paths.iter().any(|ignored| strip_prefix_case_insensitive(path, ignored).is_some())
    }

    pub(crate) fn allows_extension(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        !self.ignored_extensions.iter().any(|ignored| {
            ignored.strip_prefix('.').unwrap_or(ignored).eq_ignore_ascii_case(ext)
        })
    }

    fn contains_under_root(&self, root: &Path, depth: usize, path: &Path) -> bool {
        strip_prefix_case_insensitive(path, root).is_some_and(|relative| {
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
        let clean = clean_path_prefix(path);
        if !self.allows_extension(clean) || self.is_ignored_path(clean) {
            return false;
        }
        if self.roots.iter().any(|(root, depth)| self.contains_under_root(root, *depth, clean)) {
            return true;
        }
        self.home.as_ref().is_some_and(|home| {
            let Some(relative) = strip_prefix_case_insensitive(clean, home) else {
                return false;
            };
            let mut components = relative.components();
            let Some(std::path::Component::Normal(name)) = components.next() else {
                return false;
            };
            let directory = home.join(name);
            components.next().is_some()
                && self.is_default_subdirectory(&directory)
                && self.contains_under_root(&directory, 10, clean)
        })
    }
}

pub(crate) fn clean_path_prefix(path: &Path) -> &Path {
    let s = path.to_str().unwrap_or("");
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        Path::new(stripped)
    } else {
        path
    }
}

pub(crate) fn strip_prefix_case_insensitive<'a>(path: &'a Path, prefix: &Path) -> Option<PathBuf> {
    let clean_path = clean_path_prefix(path);
    let clean_prefix = clean_path_prefix(prefix);

    #[cfg(target_os = "windows")]
    {
        use std::path::Component;
        let mut path_comps = clean_path.components();
        let mut prefix_comps = clean_prefix.components();

        loop {
            match (prefix_comps.next(), path_comps.next()) {
                (None, Some(rem)) => {
                    let mut result = PathBuf::from(rem.as_os_str());
                    for comp in path_comps {
                        result.push(comp.as_os_str());
                    }
                    return Some(result);
                }
                (None, None) => return Some(PathBuf::new()),
                (Some(p_comp), Some(c_comp)) => {
                    let match_comp = match (p_comp, c_comp) {
                        (Component::Prefix(p1), Component::Prefix(p2)) => {
                            p1.as_os_str().to_string_lossy().eq_ignore_ascii_case(&p2.as_os_str().to_string_lossy())
                        }
                        (Component::RootDir, Component::RootDir) => true,
                        (Component::CurDir, Component::CurDir) => true,
                        (Component::ParentDir, Component::ParentDir) => true,
                        (Component::Normal(n1), Component::Normal(n2)) => {
                            n1.to_string_lossy().eq_ignore_ascii_case(&n2.to_string_lossy())
                        }
                        _ => false,
                    };
                    if !match_comp {
                        return None;
                    }
                }
                (Some(_), None) => return None,
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        clean_path.strip_prefix(clean_prefix).ok().map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_path_prefix() {
        assert_eq!(clean_path_prefix(Path::new(r"\\?\C:\Users\test.txt")), Path::new(r"C:\Users\test.txt"));
        assert_eq!(clean_path_prefix(Path::new(r"C:\Users\test.txt")), Path::new(r"C:\Users\test.txt"));
        assert_eq!(clean_path_prefix(Path::new("/home/user/test.txt")), Path::new("/home/user/test.txt"));
    }

    #[test]
    fn excludes_the_go_module_cache_from_the_default_scope() {
        let scope = SearchScope::new(&SearchConfig::default());
        let Some(home) = scope.home.clone() else { return };
        if std::env::var_os("GOMODCACHE").is_some() || std::env::var_os("GOPATH").is_some() {
            return;
        }
        let cached = home.join("go/pkg/mod/github.com/acme/lib@v1.0.0/lib.go");
        let own = home.join("go/src/myproject/main.go");
        assert!(!scope.contains(&cached));
        assert!(!scope.allows_name(&home.join("go/pkg/mod")));
        assert!(scope.contains(&own));
    }

    #[test]
    fn test_allows_name_case_insensitive() {
        let config = SearchConfig::default();
        let scope = SearchScope::new(&config);
        assert!(!scope.allows_name(Path::new("node_modules")));
        assert!(!scope.allows_name(Path::new("NODE_MODULES")));
        assert!(!scope.allows_name(Path::new("AppData")));
        assert!(!scope.allows_name(Path::new("appdata")));
        assert!(scope.allows_name(Path::new("Documents")));
        assert!(scope.allows_name(Path::new("New Text Document.txt")));
    }
}
