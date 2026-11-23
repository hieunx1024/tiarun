use std::sync::Arc;
use slint::ComponentHandle;
use tiarun::config::Config;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher::{self, LauncherEngine, LauncherItem};
use tiarun::AppWindow;

#[test]
fn test_tab_directory_and_scope_navigation() {
    let ui_res = std::panic::catch_unwind(|| AppWindow::new());
    let ui = match ui_res {
        Ok(Ok(ui)) => ui,
        _ => {
            eprintln!("Skipping test (no display available)");
            return;
        }
    };

    let config = Config::default();
    let _engine = Arc::new(LauncherEngine::with_temporary_index(config));
    let icon_resolver = Arc::new(IconResolver::new());
    let current_results: Arc<std::sync::RwLock<Vec<LauncherItem>>> = Arc::new(std::sync::RwLock::new(Vec::new()));

    // Verify Scope Icons are properly rasterized and set
    ui.set_scope_apps_icon(icon_resolver.get_scope_apps_icon());
    ui.set_scope_files_icon(icon_resolver.get_scope_files_icon());
    ui.set_scope_text_icon(icon_resolver.get_scope_text_icon());
    ui.set_scope_windows_icon(icon_resolver.get_scope_windows_icon());

    assert!(icon_resolver.get_scope_apps_icon().size().width > 0, "scope_apps_icon must not be empty");
    assert!(icon_resolver.get_scope_files_icon().size().width > 0, "scope_files_icon must not be empty");
    assert!(icon_resolver.get_scope_text_icon().size().width > 0, "scope_text_icon must not be empty");
    assert!(icon_resolver.get_scope_windows_icon().size().width > 0, "scope_windows_icon must not be empty");

    // Wire Tab Callback with the scope cycling loop (Apps -> Files -> Text -> Windows -> Apps)
    {
        let ui_weak = ui.as_weak();

        ui.on_tab_pressed(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let cur = ui.get_active_scope().to_string();
                let next = match cur.as_str() {
                    "apps" => "files",
                    "files" => "text",
                    "text" => "windows",
                    _ => "apps",
                };
                ui.set_active_scope(next.into());

                if next != "files" {
                    let raw = ui.get_search_text().to_string();
                    let trimmed = raw.trim();
                    if trimmed.ends_with('/') || trimmed.starts_with('~') || trimmed.starts_with('/') {
                        ui.set_search_text("".into());
                    }
                }
            }
        });
    }

    // Wire Right Arrow Callback to step into directory
    {
        let current_results = current_results.clone();
        let ui_weak = ui.as_weak();

        ui.on_handle_right_arrow(move || -> bool {
            if let Some(ui) = ui_weak.upgrade() {
                let sel_idx = ui.get_selected_index() as usize;
                let item_opt = {
                    if let Ok(lock) = current_results.read() {
                        lock.get(sel_idx).cloned()
                    } else {
                        None
                    }
                };

                if let Some(item) = item_opt {
                    if item.item_type == launcher::ItemType::Dir {
                        let mut path = item.exec_or_path;
                        if !path.ends_with('/') {
                            path.push('/');
                        }
                        ui.set_search_text(path.into());
                        ui.set_selected_index(0);
                        return true;
                    }
                }
            }
            false
        });
    }

    // Test 1: Typing search text (e.g. "Des") and pressing Tab cycles scope without replacing query
    ui.set_search_text("Des".into());
    ui.set_active_scope("apps".into());

    let test_app_item = LauncherItem::new(
        "DBeaver".to_string(),
        "/usr/bin/dbeaver".to_string(),
        launcher::ItemType::App,
        None,
        false,
        None,
    );
    if let Ok(mut lock) = current_results.write() {
        *lock = vec![test_app_item];
    }
    ui.set_selected_index(0);

    // Tab 1: apps -> files, query remains "Des"
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "files", "Tab cycles from apps to files");
    assert_eq!(ui.get_search_text().as_str(), "Des", "Query must not be overwritten by top item name");

    // Tab 2: files -> text, query remains "Des"
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "text", "Tab cycles from files to text");
    assert_eq!(ui.get_search_text().as_str(), "Des", "Query must not be overwritten by top item name");

    // Tab 3: text -> windows, query remains "Des"
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "windows", "Tab cycles from text to windows");
    assert_eq!(ui.get_search_text().as_str(), "Des", "Query must not be overwritten by top item name");

    // Tab 4: windows -> apps loop back, query remains "Des"
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "apps", "Tab loops back from windows to apps");
    assert_eq!(ui.get_search_text().as_str(), "Des", "Query must not be overwritten by top item name");

    // Test 2: Pressing Tab when search_text is empty also cycles scope chips
    ui.set_search_text("".into());
    ui.set_active_scope("apps".into());
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "files", "Tab on empty query cycles from apps to files");

    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "text", "Tab on empty query cycles from files to text");

    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "windows", "Tab on empty query cycles from text to windows");

    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "apps", "Tab on empty query wraps back to apps");

    // Test 3: Right Arrow drills into directory
    let test_dir_item = LauncherItem::new(
        "test_folder".to_string(),
        "/tmp/test_folder".to_string(),
        launcher::ItemType::Dir,
        None,
        false,
        None,
    );
    if let Ok(mut lock) = current_results.write() {
        *lock = vec![test_dir_item];
    }
    ui.set_selected_index(0);
    assert!(ui.invoke_handle_right_arrow(), "Right arrow should step into directory");
    assert_eq!(ui.get_search_text().as_str(), "/tmp/test_folder/", "Search text should be directory path with trailing slash");

    // Test 4: When browsed into a directory (e.g. "~/Desktop/"), pressing Tab to switch away from files clears the directory path
    ui.set_search_text("~/Desktop/".into());
    ui.set_active_scope("files".into());
    ui.invoke_tab_pressed();
    assert_eq!(ui.get_active_scope().as_str(), "text", "Tab switches from files to text");
    assert_eq!(ui.get_search_text().as_str(), "", "Directory path must be cleared when switching away from files scope");
}
