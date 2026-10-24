//! Native menu bar. Custom items forward an action string to the focused
//! window as a `menu` event; standard edit items use the OS implementations
//! (on macOS copy/paste in the webview only work through these).

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};

fn item<R: Runtime>(app: &AppHandle<R>, id: &str, label: &str, accel: Option<&str>) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(app, id, label, true, accel)
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item(app, "new-window", "New Window", Some("CmdOrCtrl+Shift+N"))?,
            &item(app, "open-folder", "Open Folder...", Some("CmdOrCtrl+O"))?,
            // No accelerator: Cmd/Ctrl+N rejects an AI diff in the editor.
            &item(app, "new-file", "New File", None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "save", "Save", Some("CmdOrCtrl+S"))?,
            &item(app, "close-tab", "Close Tab", Some("CmdOrCtrl+W"))?,
        ],
    )?;
    // On macOS Settings and Quit live in the app menu; elsewhere in File.
    #[cfg(not(target_os = "macos"))]
    file.append_items(&[
        &PredefinedMenuItem::separator(app)?,
        &item(app, "settings", "Settings", Some("Ctrl+,"))?,
        &PredefinedMenuItem::quit(app, Some("Exit"))?,
    ])?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item(app, "command-palette", "Command Palette", Some("CmdOrCtrl+K"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "toggle-sidebar", "Toggle Sidebar", Some("CmdOrCtrl+B"))?,
            &item(app, "toggle-ai-panel", "Toggle AI Panel", Some("CmdOrCtrl+J"))?,
            &item(app, "toggle-terminal", "Toggle Terminal", Some("Ctrl+`"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    let account = Submenu::with_items(app, "Account", true, &[&item(app, "account", "BlazeAPI Account", Some("CmdOrCtrl+Shift+A"))?])?;
    let window = Submenu::with_items(app, "Window", true, &[&PredefinedMenuItem::minimize(app, None)?, &PredefinedMenuItem::maximize(app, None)?])?;

    #[cfg(target_os = "macos")]
    {
        let about = AboutMetadata { name: Some("Blaze IDE".into()), website: Some("https://blazeapi.org".into()), ..Default::default() };
        let app_menu = Submenu::with_items(
            app,
            "Blaze IDE",
            true,
            &[
                &PredefinedMenuItem::about(app, None, Some(about))?,
                &PredefinedMenuItem::separator(app)?,
                &item(app, "settings", "Settings...", Some("Cmd+,"))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        Menu::with_items(app, &[&app_menu, &file, &edit, &view, &account, &window])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = AboutMetadata::default();
        Menu::with_items(app, &[&file, &edit, &view, &account, &window])
    }
}

const ACTIONS: &[&str] = &[
    "open-folder", "new-file", "new-window", "save", "close-tab", "toggle-sidebar",
    "toggle-ai-panel", "toggle-terminal", "command-palette", "settings", "account",
];

pub fn on_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if id == "new-window" {
        let _ = crate::windows::open(app, None);
        return;
    }
    if !ACTIONS.contains(&id) {
        return;
    }
    let target = app
        .webview_windows()
        .into_values()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| app.get_webview_window("main"));
    if let Some(w) = target {
        let _ = w.emit_to(w.label(), "menu", id);
    }
}
