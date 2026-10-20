use tauri::{
    menu::{Menu, MenuEvent, MenuItem, MenuItemKind},
    AppHandle, Emitter, Manager, Runtime,
};

pub const CHECK_FOR_UPDATES_EVENT: &str = "halv://menu/check-for-updates";

const CHECK_FOR_UPDATES_MENU_ID: &str = "app.check-for-updates";
const QUIT_AND_STOP_AGENTS_MENU_ID: &str = "app.quit-and-stop-agents";
const QUIT_MENU_ID: &str = "app.quit";
const QUIT_ACCELERATOR: &str = "CmdOrCtrl+Q";
const CLOSE_WINDOW_MENU_ID: &str = "app.close-window";
// Cmd+W belongs to the page: it closes a pane, then a tab (ADR 0010). A menu
// accelerator takes a key before the webview gets it, so the window item
// moves to Shift+Cmd+W.
const CLOSE_WINDOW_ACCELERATOR: &str = "CmdOrCtrl+Shift+W";
const CLOSE_WINDOW_LABEL: &str = "Close Window";
const FILE_SUBMENU_LABEL: &str = "File";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMenuAction {
    CheckForUpdates,
    QuitAndStopAgents,
    Quit,
    CloseWindow,
}

/// The custom items of the app menu: id and accelerator. `build` reads the
/// accelerators from here, so the menu test checks the real values.
const CUSTOM_ITEMS: [(&str, Option<&str>); 4] = [
    (CHECK_FOR_UPDATES_MENU_ID, None),
    (QUIT_AND_STOP_AGENTS_MENU_ID, None),
    (QUIT_MENU_ID, Some(QUIT_ACCELERATOR)),
    (CLOSE_WINDOW_MENU_ID, Some(CLOSE_WINDOW_ACCELERATOR)),
];

fn accelerator_of(id: &str) -> Option<&'static str> {
    CUSTOM_ITEMS
        .iter()
        .find(|(item_id, _)| *item_id == id)
        .and_then(|(_, accelerator)| *accelerator)
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    let app_submenu = menu
        .items()?
        .into_iter()
        .next()
        .and_then(|item| match item {
            MenuItemKind::Submenu(submenu) => Some(submenu),
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other("default macOS application menu is missing"))?;
    let check_for_updates = MenuItem::with_id(
        app,
        CHECK_FOR_UPDATES_MENU_ID,
        "Check for Updates…",
        true,
        accelerator_of(CHECK_FOR_UPDATES_MENU_ID),
    )?;

    // The default macOS app submenu starts with About, followed by a separator.
    app_submenu.insert(&check_for_updates, 1)?;
    let quit_and_stop_agents = MenuItem::with_id(
        app,
        QUIT_AND_STOP_AGENTS_MENU_ID,
        "Quit and Stop Agents",
        true,
        accelerator_of(QUIT_AND_STOP_AGENTS_MENU_ID),
    )?;
    let app_items = app_submenu.items()?;
    let predefined_quit = app_items.iter().position(|item| {
        matches!(
            item,
            MenuItemKind::Predefined(item)
                if item.text().is_ok_and(|text| text.starts_with("Quit"))
        )
    });
    // The predefined Quit item ends the app without an exit request, so an
    // update cannot install at quit. The custom item exits through the app.
    let quit_index = match predefined_quit {
        Some(index) => {
            app_submenu.remove_at(index)?;
            index
        }
        None => app_items.len(),
    };
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ID,
        format!("Quit {}", app.package_info().name),
        true,
        accelerator_of(QUIT_MENU_ID),
    )?;
    app_submenu.insert(&quit_and_stop_agents, quit_index)?;
    app_submenu.insert(&quit, quit_index + 1)?;

    // The default File and Window submenus each hold a predefined "Close
    // Window" item on Cmd+W. Both go. The custom item takes the place of the
    // File item.
    let close_window = MenuItem::with_id(
        app,
        CLOSE_WINDOW_MENU_ID,
        CLOSE_WINDOW_LABEL,
        true,
        accelerator_of(CLOSE_WINDOW_MENU_ID),
    )?;
    let mut close_window_placed = false;
    for item in menu.items()? {
        let MenuItemKind::Submenu(submenu) = item else {
            continue;
        };
        let items = submenu.items()?;
        let Some(index) = items.iter().position(is_predefined_close_window) else {
            continue;
        };
        submenu.remove_at(index)?;
        if !close_window_placed && submenu.text()? == FILE_SUBMENU_LABEL {
            submenu.insert(&close_window, index)?;
            close_window_placed = true;
        }
    }
    if !close_window_placed {
        app_submenu.append(&close_window)?;
    }
    Ok(menu)
}

fn is_predefined_close_window<R: Runtime>(item: &MenuItemKind<R>) -> bool {
    matches!(
        item,
        MenuItemKind::Predefined(item)
            if item.text().is_ok_and(|text| is_close_window_text(&text))
    )
}

/// The predefined item reads "C&lose Window" with its mnemonic mark.
fn is_close_window_text(text: &str) -> bool {
    text.replace('&', "").trim() == CLOSE_WINDOW_LABEL
}

pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match action_for_menu_id(event.id().as_ref()) {
        Some(AppMenuAction::CheckForUpdates) => {
            if let Err(error) = crate::tray::show_main_window(app) {
                eprintln!("Failed to show Halv for an update check: {error}");
            }
            crate::app_update::check_from_menu(app);
            if let Err(error) = app.emit(CHECK_FOR_UPDATES_EVENT, ()) {
                eprintln!("Failed to request an update check from the app menu: {error}");
            }
        }
        Some(AppMenuAction::QuitAndStopAgents) => {
            let manager = app.state::<crate::terminal::TerminalManager>();
            if let Err(error) = manager.stop_all_supervised() {
                eprintln!("failed to stop supervised terminals: {error}");
            }
            app.exit(0);
        }
        Some(AppMenuAction::Quit) => app.exit(0),
        Some(AppMenuAction::CloseWindow) => {
            if let Err(error) = crate::main_window::hide_main_window(app) {
                eprintln!("failed to hide Halv window: {error}");
            }
        }
        None => {}
    }
}

fn action_for_menu_id(menu_id: &str) -> Option<AppMenuAction> {
    match menu_id {
        CHECK_FOR_UPDATES_MENU_ID => Some(AppMenuAction::CheckForUpdates),
        QUIT_AND_STOP_AGENTS_MENU_ID => Some(AppMenuAction::QuitAndStopAgents),
        QUIT_MENU_ID => Some(AppMenuAction::Quit),
        CLOSE_WINDOW_MENU_ID => Some(AppMenuAction::CloseWindow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_only_to_supported_app_actions() {
        assert_eq!(
            action_for_menu_id(CHECK_FOR_UPDATES_MENU_ID),
            Some(AppMenuAction::CheckForUpdates)
        );
        assert_eq!(
            action_for_menu_id(QUIT_AND_STOP_AGENTS_MENU_ID),
            Some(AppMenuAction::QuitAndStopAgents)
        );
        assert_eq!(action_for_menu_id(QUIT_MENU_ID), Some(AppMenuAction::Quit));
        assert_eq!(
            action_for_menu_id(CLOSE_WINDOW_MENU_ID),
            Some(AppMenuAction::CloseWindow)
        );
        assert_eq!(action_for_menu_id("app.unknown"), None);
    }

    #[test]
    fn the_quit_item_keeps_the_standard_shortcut() {
        // Tauri drops an accelerator that does not parse, so the item must
        // use a spelling that the menu library accepts.
        assert_eq!(QUIT_ACCELERATOR, "CmdOrCtrl+Q");
    }

    // The shared menu test: no item takes Cmd+W, "Close Window" moves to
    // Shift+Cmd+W, and both Quit items stay. The predefined items of
    // `Menu::default` that keep their place have these macOS accelerators
    // (muda): Copy, Cut, Paste, Undo, Redo, Select All, Minimize, Full
    // Screen, Hide, Hide Others. None of them is Cmd+W.
    #[test]
    fn no_menu_item_takes_cmd_w() {
        let kept_predefined = [
            "CmdOrCtrl+C",
            "CmdOrCtrl+X",
            "CmdOrCtrl+V",
            "CmdOrCtrl+Z",
            "CmdOrCtrl+Shift+Z",
            "CmdOrCtrl+A",
            "CmdOrCtrl+M",
            "Cmd+Ctrl+F",
            "CmdOrCtrl+H",
            "CmdOrCtrl+Alt+H",
        ];
        let custom = CUSTOM_ITEMS
            .iter()
            .filter_map(|(_, accelerator)| *accelerator)
            .collect::<Vec<_>>();
        for accelerator in kept_predefined.iter().chain(custom.iter()) {
            assert_ne!(*accelerator, "CmdOrCtrl+W", "an item takes Cmd+W");
        }
        assert_eq!(
            accelerator_of(CLOSE_WINDOW_MENU_ID),
            Some("CmdOrCtrl+Shift+W")
        );
        assert_eq!(accelerator_of(QUIT_MENU_ID), Some("CmdOrCtrl+Q"));
        assert!(CUSTOM_ITEMS
            .iter()
            .any(|(id, _)| *id == QUIT_AND_STOP_AGENTS_MENU_ID));
        assert_eq!(accelerator_of(QUIT_AND_STOP_AGENTS_MENU_ID), None);
    }

    #[test]
    fn the_predefined_close_window_items_are_found_and_nothing_else() {
        assert!(is_close_window_text("C&lose Window"));
        assert!(is_close_window_text("Close Window"));
        assert!(!is_close_window_text("Close"));
        assert!(!is_close_window_text("&Minimize"));
        assert!(!is_close_window_text("Quit Halv"));
    }
}
