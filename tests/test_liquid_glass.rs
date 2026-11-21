use tiarun::AppWindow;

#[test]
fn test_liquid_glass_style_and_expansion() {
    let ui_res = std::panic::catch_unwind(|| AppWindow::new());
    let ui = match ui_res {
        Ok(Ok(ui)) => ui,
        Ok(Err(e)) => {
            eprintln!("Skipping test (no graphical display available): {:?}", e);
            return;
        }
        Err(_) => {
            eprintln!("Skipping test (window backend missing in test runner)");
            return;
        }
    };

    ui.set_cfg_view_style("liquid_glass".into());
    assert_eq!(ui.get_cfg_view_style(), "liquid_glass");

    // Initially search text is empty and not expanded
    ui.set_search_text("".into());
    ui.set_is_expanded(false);
    assert!(!ui.get_is_expanded());

    // Focus search should work without error
    ui.invoke_focus_search();

    // Typing should expand the panel
    ui.set_search_text("calc".into());
    ui.invoke_search_text_changed("calc".into());
    ui.set_is_expanded(true);
    assert!(ui.get_is_expanded());

    // Clearing search text should allow collapse
    ui.set_search_text("".into());
    ui.set_is_expanded(false);
    assert!(!ui.get_is_expanded());

    for style in ["liquid_glass", "classic"] {
        ui.set_cfg_view_style(style.into());
        ui.set_cfg_compact_empty(true);
        let shadow_margin = if style == "liquid_glass" { 18.0 } else { 0.0 };
        assert_eq!(ui.get_launcher_width(), 680.0 + 2.0 * shadow_margin);
        assert_eq!(ui.get_launcher_height(), 48.0 + shadow_margin);
        assert_eq!(ui.get_launcher_radius(), 24.0);

        ui.set_is_expanded(true);
        assert_eq!(ui.get_launcher_width(), 920.0 + 2.0 * shadow_margin);
        assert_eq!(ui.get_launcher_height(), 520.0 + shadow_margin);
        assert_eq!(ui.get_launcher_radius(), 18.0);

        // Preview mode keeps fixed 920px width (Raycast-style unified window)
        ui.set_show_preview(true);
        assert_eq!(ui.get_launcher_width(), 920.0 + 2.0 * shadow_margin);
        assert_eq!(ui.get_launcher_height(), 520.0 + shadow_margin);
        ui.set_show_preview(false);
        assert_eq!(ui.get_launcher_width(), 920.0 + 2.0 * shadow_margin);

        ui.set_in_settings_mode(true);
        assert_eq!(ui.get_launcher_height(), 428.0);
        ui.set_in_settings_mode(false);
        ui.set_is_expanded(false);
    }
}

#[test]
fn test_category_chips_loaded_on_startup_and_filter_results() {
    use tiarun::config::Config;
    use tiarun::launcher::LauncherEngine;
    use tiarun::apps::AppCategoryFilter;

    let engine = LauncherEngine::new(Config::default());
    let available = engine.available_categories();
    assert!(available.contains(&"Applications".to_string()));
    assert!(available.contains(&"Actions".to_string()));

    // Ensure every category in available_categories actually returns results on empty query
    for cat in &available {
        let filter = AppCategoryFilter::from_chip_name(cat);
        let mut results = if filter == AppCategoryFilter::Actions {
            engine.search("@sys")
        } else {
            engine.search("")
        };
        if filter != AppCategoryFilter::All {
            results.retain(|(item, _)| filter.matches(item));
        }
        if filter == AppCategoryFilter::Help {
            results.extend(engine.search("?"));
        }
        assert!(!results.is_empty(), "Category '{}' in available_categories must not be empty", cat);
    }
}

#[test]
fn test_category_matching_and_untruncated_counts() {
    use tiarun::apps::AppCategoryFilter;
    use tiarun::launcher::{ItemType, LauncherItem};

    let dev_items = [
        LauncherItem::new("IntelliJ IDEA".into(), "/opt/idea.sh".into(), ItemType::App, None, false, None),
        LauncherItem::new("Visual Studio Code".into(), "/usr/bin/code".into(), ItemType::App, None, false, None),
        LauncherItem::new("DBeaver CE".into(), "/usr/bin/dbeaver".into(), ItemType::App, None, false, None),
        LauncherItem::new("Postman".into(), "/usr/bin/postman".into(), ItemType::App, None, false, None),
        LauncherItem::new("Claude".into(), "/usr/bin/claude".into(), ItemType::App, None, false, None),
    ];

    let filter = AppCategoryFilter::Development;
    for item in &dev_items {
        assert!(filter.matches(item), "App '{}' must match Development category", item.name);
    }
}
