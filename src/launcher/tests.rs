use super::*;
#[cfg(not(target_os = "windows"))]
use std::fs;
#[cfg(not(target_os = "windows"))]
use std::path::Path;

#[test]
fn test_parse_command_line() {
    let cmd = r#"env GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus IBUS_ENABLE_SYNC_MODE=1 "/opt/idea-IU-262.8665.337/bin/idea.sh" %f"#;
    let tokens = parse_command_line(cmd);
    assert_eq!(
        tokens,
        vec![
            "env",
            "GTK_IM_MODULE=ibus",
            "XMODIFIERS=@im=ibus",
            "IBUS_ENABLE_SYNC_MODE=1",
            "/opt/idea-IU-262.8665.337/bin/idea.sh",
            "%f"
        ]
    );

    let cmd2 = r#""/path/with spaces/app" -v 'single quoted string' "unclosed"#;
    let tokens2 = parse_command_line(cmd2);
    assert_eq!(
        tokens2,
        vec![
            "/path/with spaces/app",
            "-v",
            "single quoted string",
            "unclosed"
        ]
    );
}

#[test]
fn test_quote_arg() {
    assert_eq!(quote_arg("simple"), "simple");
    assert_eq!(quote_arg("/path/with space"), "\"/path/with space\"");
    assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
}

#[test]
fn test_clean_app_exec() {
    assert_eq!(
        clean_app_exec(r#"env GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus IBUS_ENABLE_SYNC_MODE=1 "/opt/idea/bin/idea.sh" %f"#),
        "/opt/idea/bin/idea.sh"
    );
    assert_eq!(
        clean_app_exec("/usr/bin/google-chrome-stable %U"),
        "/usr/bin/google-chrome-stable"
    );
    assert_eq!(
        clean_app_exec("flatpak run com.spotify.Client"),
        "flatpak: com.spotify.Client"
    );
    assert_eq!(clean_app_exec("alacritty"), "alacritty");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn test_parse_idea_desktop() {
    let path = Path::new("/usr/share/applications/idea.desktop");
    if path.exists() {
        let config = Config::default();
        let engine = LauncherEngine::new(config);
        if let Some(item) = engine.parse_desktop_file(path) {
            let tokens = parse_command_line(&item.exec_or_path);
            assert!(!tokens.is_empty());
            assert!(!tokens.iter().any(|t| t.starts_with('"') || t.ends_with('"')));
            let bin_target = tokens.iter().find(|t| *t != "env" && !t.contains('='));
            assert!(bin_target.is_some());
            assert!(Path::new(bin_target.unwrap()).exists());
        }
    }
}

#[test]
fn test_remove_vietnamese_accents() {
    assert_eq!(remove_vietnamese_accents("Tải xuống"), "tai xuong");
    assert_eq!(remove_vietnamese_accents("Học tập"), "hoc tap");
    assert_eq!(remove_vietnamese_accents("Đường dẫn"), "duong dan");
    assert_eq!(remove_vietnamese_accents("Lập trình Rust"), "lap trinh rust");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn test_desktop_entry_precedence() {
    use std::io::Write;
    let temp_dir = std::env::temp_dir().join(format!("tiarun_test_prec_{}", std::process::id()));
    let user_dir = temp_dir.join("user");
    let sys_dir = temp_dir.join("sys");
    let _ = fs::create_dir_all(&user_dir);
    let _ = fs::create_dir_all(&sys_dir);

    let user_desktop = user_dir.join("testapp.desktop");
    let sys_desktop = sys_dir.join("testapp.desktop");

    let mut f1 = fs::File::create(&user_desktop).unwrap();
    writeln!(f1, "[Desktop Entry]\nType=Application\nName=TestApp\nExec=user_cmd").unwrap();

    let mut f2 = fs::File::create(&sys_desktop).unwrap();
    writeln!(f2, "[Desktop Entry]\nType=Application\nName=TestApp\nExec=sys_cmd").unwrap();

    let mut config = Config::default();
    config.apps.extra_desktop_paths = vec![
        user_dir.to_string_lossy().to_string(),
        sys_dir.to_string_lossy().to_string(),
    ];
    let engine = LauncherEngine::new(config);
    let found = engine.apps.iter().find(|a| a.name == "TestApp");
    assert!(found.is_some());
    assert_eq!(found.unwrap().exec_or_path, "user_cmd");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_resolve_path_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    assert!(engine.resolve_path_search("downloads").is_none());
    if let Some(home) = dirs::home_dir() {
        let mut home_str = home.to_string_lossy().to_string();
        if !home_str.ends_with('/') && !home_str.ends_with('\\') {
            home_str.push('/');
        }
        assert!(engine.resolve_path_search(&home_str).is_some());
        assert!(engine.resolve_path_search("~").is_some());
        assert!(engine.resolve_path_search("~/").is_some());
        assert!(engine.resolve_path_search(r"~\").is_some());
    }
}

#[test]
fn test_file_mode_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let results = engine.search("@f");
    assert!(results.iter().all(|(item, _)| item.item_type == ItemType::File || item.item_type == ItemType::Dir));
}

#[test]
fn test_default_search_excludes_shallow_files() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let hello_item = || {
        LauncherItem::new(
            "hello.txt".to_string(),
            "/home/user/hello.txt".to_string(),
            ItemType::File,
            Some("Test File".to_string()),
            false,
            None,
        )
    };
    {
        let mut lock = engine.shallow_files.write().unwrap();
        lock.push(hello_item());
    }

    let default_results = engine.search("hello");
    assert!(
        !default_results.iter().any(|(item, _)| item.name == "hello.txt"),
        "Default app search must not mix in shallow files"
    );

    // Background file indexing spawned by LauncherEngine::new() may finish and
    // overwrite shallow_files between the two searches; ensure hello.txt is present.
    {
        let mut lock = engine.shallow_files.write().unwrap();
        if !lock.iter().any(|item| item.name == "hello.txt") {
            lock.push(hello_item());
        }
    }

    let file_results = engine.search("@f hello");
    assert!(
        file_results.iter().any(|(item, _)| item.name == "hello.txt"),
        "@f file mode must find shallow files"
    );
}

#[test]
fn test_settings_save_and_load() {
    let mut config = Config::default();
    config.theme.show_icons = Some(false);
    config.search.max_results = 25;
    let serialized = toml::to_string_pretty(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.theme.show_icons, Some(false));
    assert_eq!(deserialized.search.max_results, 25);
}

#[test]
fn test_search_and_icon_performance() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let resolver = crate::icon_resolver::IconResolver::new();

    let start = std::time::Instant::now();
    let results = engine.search("int");
    let elapsed_search = start.elapsed();
    assert!(elapsed_search.as_millis() < 500, "Search took too long: {:?}", elapsed_search);

    // Cold icon resolution
    for (item, _) in results.iter().take(20) {
        let _ = resolver.resolve_icon(item.icon.as_deref(), &item.name, &item.exec_or_path);
    }
    let elapsed_cold = start.elapsed();
    assert!(elapsed_cold.as_millis() < 5000, "Cold search + icon took too long: {:?}", elapsed_cold);

    // Instant cached icon lookup test (O(1) memory cache)
    let cache_start = std::time::Instant::now();
    for (item, _) in results.iter().take(20) {
        let _ = resolver.resolve_icon(item.icon.as_deref(), &item.name, &item.exec_or_path);
    }
    let elapsed_cached = cache_start.elapsed();
    assert!(elapsed_cached.as_millis() < 50, "Cached icon lookup took too long: {:?}", elapsed_cached);
}

#[test]
fn test_format_file_size() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(500), "500 B");
    assert_eq!(format_file_size(1024), "1.0 KB");
    assert_eq!(format_file_size(2450), "2.4 KB");
    assert_eq!(format_file_size(1048576), "1.0 MB");
    assert_eq!(format_file_size(1073741824), "1.0 GB");
}

#[test]
fn test_directory_search_and_scan() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let current_dir = std::env::current_dir().unwrap();
    let current_dir_str = current_dir.to_string_lossy();

    let on_the_fly = engine.scan_dir_on_the_fly(&current_dir);
    assert!(!on_the_fly.is_empty(), "Current dir should not be empty");
    assert!(on_the_fly.iter().any(|item| item.name == "Cargo.toml" || item.name == "src"));

    let recursive = engine.scan_dir_recursive(&current_dir, 3);
    assert!(!recursive.is_empty(), "Recursive scan should find files");
    assert!(recursive.iter().any(|item| item.name == "main.rs" || item.name == "launcher.rs"));

    let results = engine.search(&format!("@f {}/", current_dir_str));
    assert!(!results.is_empty());
}

#[test]
fn test_web_search_fallback() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    // A normal query always gets a trailing "search the web" item, appended
    // after any real matches so it never outranks them.
    let results = engine.search("firefox");
    let last = results.last().expect("expected at least the web-search item");
    assert_eq!(last.0.item_type, ItemType::WebSearch);
    assert!(last.0.exec_or_path.starts_with("https://www.google.com/search?q="));

    // Query text is percent-encoded into the URL (spaces -> '+', not raw).
    let results = engine.search("rust async runtime");
    let last = &results.last().unwrap().0;
    assert_eq!(last.exec_or_path, "https://www.google.com/search?q=rust+async+runtime");

    // Empty query: no web-search item (nothing meaningful to search for).
    let empty_results = engine.search("");
    assert!(empty_results.iter().all(|(item, _)| item.item_type != ItemType::WebSearch));

    // Prefixed modes (e.g. "!") are dedicated modes and return before the
    // fallback is ever considered.
    let shell_results = engine.search("!ls");
    assert!(shell_results.iter().all(|(item, _)| item.item_type != ItemType::WebSearch));
}

#[test]
fn test_help_mode() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let results = engine.search("?");
    assert!(!results.is_empty());
    assert!(results.iter().all(|(item, _)| item.item_type == ItemType::Calc));
    // The global toggle shortcut, copy/scope keys, and "@" / "!" modes should all
    // be documented in the list.
    assert!(results.iter().any(|(item, _)| item.name.contains("Ctrl+Alt+Space")));
    assert!(results.iter().any(|(item, _)| item.name == "Ctrl+C"));
    assert!(results.iter().any(|(item, _)| item.name == "Alt+K"));
    assert!(results.iter().all(|(item, _)| item.name != "Ctrl+K"));
    assert!(results.iter().any(|(item, _)| item.name.contains("Shift+Tab")));
    assert!(results.iter().any(|(item, _)| item.name == "@"));
    assert!(results.iter().any(|(item, _)| item.name == "! <command>"));

    // "?" is an exact match only; a question mark elsewhere is just a query.
    let not_help = engine.search("what is 5?");
    assert!(not_help.iter().all(|(item, _)| !item.name.contains("Ctrl+Alt+Space")));
}

#[test]
fn test_shell_command_mode() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    // Bare "!" with nothing typed yet: no result, just waiting for input.
    let empty_results = engine.search("!");
    assert!(empty_results.is_empty());

    // "!<command>" produces exactly one Shell item carrying the raw command.
    let results = engine.search("!echo hello | wc -l");
    assert_eq!(results.len(), 1);
    let (item, _) = &results[0];
    assert_eq!(item.item_type, ItemType::Shell);
    assert_eq!(item.exec_or_path, "echo hello | wc -l");

    // A leading space after "!" is tolerated ("! ls" behaves like "!ls").
    let spaced_results = engine.search("! ls -la");
    assert_eq!(spaced_results.len(), 1);
    assert_eq!(spaced_results[0].0.exec_or_path, "ls -la");
}

#[test]
fn test_system_mode_and_keywords() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let sys_results = engine.search("@sys");
    assert!(!sys_results.is_empty());
    assert!(sys_results.iter().all(|(item, _)| item.item_type == ItemType::System));

    let lock_results = engine.search("lock");
    assert!(lock_results.iter().any(|(item, _)| item.item_type == ItemType::System && item.name.contains("Lock")));

    let reboot_results = engine.search("restart");
    assert!(reboot_results.iter().any(|(item, _)| item.item_type == ItemType::System && item.name.contains("Restart")));
}

#[cfg(target_os = "linux")]
#[test]
fn test_window_mode_excludes_background_processes() {
    let mut engine = LauncherEngine::new(Config::default());
    let mut process = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    engine.apps = vec![LauncherItem::new(
        "Window mode background fixture".to_string(),
        "sleep".to_string(),
        ItemType::App,
        None,
        false,
        None,
    )];

    let results = engine.search("@w");
    process.kill().unwrap();
    process.wait().unwrap();

    assert!(results.iter().all(|(item, _)| item.name != "Window mode background fixture"));
    assert!(results.iter().all(|(item, _)| !item.exec_or_path.starts_with("app:")));
}

#[test]
fn test_window_mode_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let win_results = engine.search("@w");
    assert!(win_results.iter().all(|(item, _)| item.item_type == ItemType::Window));
}

#[test]
fn test_clipboard_mode_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);
    let clip_results = engine.search("@c");
    assert!(clip_results.iter().all(|(item, _)| item.item_type == ItemType::Clipboard));
}

#[test]
fn test_unit_and_currency_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let conv1 = engine.search("100 usd in vnd");
    assert!(!conv1.is_empty());
    assert!(conv1.iter().any(|(item, _)| item.name.contains("VND")));

    let conv2 = engine.search("37 c to f");
    assert!(!conv2.is_empty());
    assert!(conv2.iter().any(|(item, _)| item.name.contains("98.6 °F")));
}

#[test]
fn test_custom_plugins_overview() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let modes = engine.search("@");
    assert!(!modes.is_empty());
    assert!(modes.iter().all(|(item, _)| item.item_type == ItemType::Mode));
    assert!(modes.iter().any(|(item, _)| item.exec_or_path == "@w"));
    assert!(modes.iter().any(|(item, _)| item.exec_or_path == "@c"));
    assert!(modes.iter().any(|(item, _)| item.exec_or_path == "@sys"));
    assert!(modes.iter().any(|(item, _)| item.exec_or_path == "@f"));
    assert!(modes.iter().any(|(item, _)| item.exec_or_path == "@theme"));

    let partial = engine.search("@sy");
    assert!(partial.iter().any(|(item, _)| item.item_type == ItemType::Mode && item.exec_or_path == "@sys"));
}

#[test]
fn test_prefix_and_tiered_ranking() {
    let mut config = Config::default();
    config.apps.custom.push(crate::config::CustomAppConfig {
        name: "Extension Manager".to_string(),
        exec: "extension-manager".to_string(),
        description: None,
        terminal: false,
        icon: None,
        category: None,
    });
    config.apps.custom.push(crate::config::CustomAppConfig {
        name: "Image Viewer".to_string(),
        exec: "image-viewer".to_string(),
        description: None,
        terminal: false,
        icon: None,
        category: None,
    });
    let engine = LauncherEngine::new(config);

    let results = engine.search("image");
    assert!(results.len() >= 2);
    // Image Viewer must rank at #0 above Extension Manager
    assert_eq!(results[0].0.name, "Image Viewer");
}

#[test]
fn test_edge_vs_registry() {
    let mut config = Config::default();
    config.apps.custom.push(crate::config::CustomAppConfig {
        name: "Registry Editor".to_string(),
        exec: "regedit.exe".to_string(),
        description: None,
        terminal: false,
        icon: None,
        category: None,
    });
    config.apps.custom.push(crate::config::CustomAppConfig {
        name: "Microsoft Edge".to_string(),
        exec: "msedge.exe".to_string(),
        description: None,
        terminal: false,
        icon: None,
        category: None,
    });
    let engine = LauncherEngine::new(config);
    let results = engine.search("ed");
    assert_eq!(results[0].0.name, "Microsoft Edge");
}

#[test]
fn test_theme_mode_search() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let results = engine.search("@theme");
    assert!(!results.is_empty());
    assert!(results.iter().any(|(item, _)| item.exec_or_path == "theme:dark"));
    assert!(results.iter().any(|(item, _)| item.exec_or_path == "theme:light"));
    assert!(results.iter().any(|(item, _)| item.exec_or_path == "theme:system"));
    assert!(results.iter().any(|(item, _)| item.exec_or_path == "theme:opacity:95"));

    let dark_match = engine.search("@theme dark");
    assert!(!dark_match.is_empty());
    assert_eq!(dark_match[0].0.exec_or_path, "theme:dark");

    let opacity_match = engine.search("@opacity 80");
    assert!(!opacity_match.is_empty());
    assert_eq!(opacity_match[0].0.exec_or_path, "theme:opacity:80");
}

#[test]
fn test_theme_config_and_is_dark() {
    let mut theme = crate::config::ThemeConfig::default();
    assert_eq!(theme.mode, "dark");
    assert_eq!(theme.opacity, 0.95);
    assert!(theme.is_dark());

    theme.mode = "light".to_string();
    assert!(!theme.is_dark());
    assert!(!theme.is_dark_cached());

    theme.mode = "dark".to_string();
    assert!(theme.is_dark());
    assert!(theme.is_dark_cached());
}

#[test]
fn test_system_dark_mode_live() {
    let is_dark = crate::config::is_system_dark_mode();
    assert_eq!(crate::config::cached_system_dark_mode(), is_dark);
    println!(">>> Current is_system_dark_mode(): {}", is_dark);
}

#[test]
fn test_smart_file_indexing_and_ignore_rules() {
    let temp_dir = std::env::temp_dir().join(format!("tiarun_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let docs_dir = temp_dir.join("Documents").join("Reports").join("2026");
    let node_modules = temp_dir.join("node_modules");
    let _ = std::fs::create_dir_all(&docs_dir);
    let _ = std::fs::create_dir_all(&node_modules);

    let valid_file = docs_dir.join("annual_report.docx");
    let ignored_ext_file = docs_dir.join("debug.log");
    let ignored_dir_file = node_modules.join("package.json");

    let _ = std::fs::write(&valid_file, "report content");
    let _ = std::fs::write(&ignored_ext_file, "some log");
    let _ = std::fs::write(&ignored_dir_file, "{}");

    let mut config = Config::default();
    config.search.paths = vec![crate::config::SearchPathConfig {
        path: temp_dir.to_string_lossy().to_string(),
        depth: 4,
        max_depth: Some(4),
        exclude: Vec::new(),
    }];

    let mut files = Vec::new();
    LauncherEngine::index_files_impl(&config, &mut files);

    assert!(files.iter().any(|f| f.name == "annual_report.docx"), "Should find nested annual_report.docx");
    assert!(!files.iter().any(|f| f.name == "debug.log"), "Should ignore .log extension by default");
    assert!(!files.iter().any(|f| f.name == "package.json"), "Should ignore files inside node_modules by default");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_smart_copy_item() {
    let config = Config::default();
    let engine = LauncherEngine::new(config);

    let calc_item = LauncherItem::new(
        "42".to_string(),
        "42".to_string(),
        ItemType::Calc,
        Some("42".to_string()),
        false,
        None,
    );
    engine.copy_item_smart(&calc_item);

    let file_item = LauncherItem::new(
        "tiarun.png".to_string(),
        "assets/tiarun.png".to_string(),
        ItemType::File,
        None,
        false,
        None,
    );
    engine.copy_item_smart(&file_item);

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            let out = std::process::Command::new("wl-paste").arg("--list-types").output().expect("wl-paste failed");
            let types = String::from_utf8_lossy(&out.stdout);
            assert!(types.contains("image/png"), "Expected image/png in clipboard types, got: {}", types);
        }
    }
}

#[test]
fn test_resolve_window_icon_matches_wm_class_desktop_id_and_fallback() {
    let temp_dir = std::env::temp_dir().join(format!("tiarun_win_icon_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let chrome_desktop = temp_dir.join("com.google.Chrome.desktop");
    std::fs::write(
        &chrome_desktop,
        "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nStartupWMClass=google-chrome\nIcon=google-chrome\n",
    )
    .unwrap();

    let claude_desktop = temp_dir.join("com.anthropic.Claude.desktop");
    std::fs::write(
        &claude_desktop,
        "[Desktop Entry]\nType=Application\nName=Claude\nExec=claude-desktop %U\nIcon=claude-desktop\n",
    )
    .unwrap();

    let firefox_desktop = temp_dir.join("firefox.desktop");
    std::fs::write(
        &firefox_desktop,
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\n",
    )
    .unwrap();

    let mut config = Config::default();
    config.apps.extra_desktop_paths = vec![temp_dir.to_string_lossy().to_string()];
    let engine = LauncherEngine::new(config);

    let chrome_parsed = engine.parse_desktop_file(&chrome_desktop).unwrap();
    assert_eq!(chrome_parsed.wm_class.as_deref(), Some("google-chrome"));
    assert_eq!(chrome_parsed.desktop_id.as_deref(), Some("com.google.Chrome"));

    let make_win = |title: &str, class_name: &str, exe: Option<&str>| crate::window_switcher::WindowItem {
        id: "sway:1".to_string(),
        title: title.to_string(),
        class_name: class_name.to_string(),
        pid: Some(1234),
        memory_bytes: None,
        cpu_percent: None,
        executable_path: exe.map(str::to_string),
    };

    assert_eq!(
        engine.resolve_window_icon(&make_win("Sheets - Google Chrome", "google-chrome", None)).as_deref(),
        Some("google-chrome")
    );
    assert_eq!(
        engine.resolve_window_icon(&make_win("Chat Thread", "com.anthropic.Claude", None)).as_deref(),
        Some("claude-desktop")
    );
    assert_eq!(
        engine.resolve_window_icon(&make_win("Mozilla Firefox", "Navigator.firefox", None)).as_deref(),
        Some("firefox")
    );
    assert_eq!(
        engine.resolve_window_icon(&make_win("Custom Tool", "custom-tool", Some("/usr/bin/custom-tool"))).as_deref(),
        Some("custom-tool")
    );
    assert_eq!(
        engine.resolve_window_icon(&make_win("Anon Script", "", Some("/usr/bin/python3"))),
        None
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}
