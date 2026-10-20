//! The main window actions of the page and the menu (ADR 0010).
//!
//! The app menu has no Cmd+W item, so WKWebView gives Cmd+W to the page. The
//! page closes a pane, then a tab, and with nothing left to close it hides
//! the window through `app_hide_main_window`. The window close request and
//! the "Close Window" menu item (Shift+Cmd+W) hide the window the same way.

use tauri::{AppHandle, Manager, Runtime};

pub const MAIN_WINDOW_LABEL: &str = "main";

/// Hides the main window. Halv keeps running in the tray, as after a close
/// request.
pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window.hide()?;
    }
    Ok(())
}

/// The last link of the Cmd+W chain of the page.
#[tauri::command]
pub fn app_hide_main_window(app: AppHandle) -> Result<(), String> {
    hide_main_window(&app).map_err(|error| format!("failed to hide the Halv window: {error}"))
}
