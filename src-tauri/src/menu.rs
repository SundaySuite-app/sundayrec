//! The macOS application menu — rebuilt for exactly ONE reason: Cmd+Q.
//!
//! ## The trap
//!
//! Tauri installs [`Menu::default`] on macOS when the app sets no menu of its
//! own (`AppBuilder::build`: `if self.menu.is_none() && self.enable_macos_default_menu`).
//! That default's Quit item is `PredefinedMenuItem::quit`, and on macOS muda
//! wires a predefined Quit to the `terminate:` selector
//! (`muda/src/platform_impl/macos/mod.rs`, `PredefinedMenuItemType::selector`).
//! `terminate:` asks the app delegate's `applicationShouldTerminate:` — which
//! tao does **not** implement (`tao/src/platform_impl/macos/app_delegate.rs`
//! registers `applicationWillTerminate:` and no `ShouldTerminate`) — so the
//! answer is the default `NSTerminateNow` and the process is already dying by
//! the time `applicationWillTerminate:` reaches tao's `AppState::exit()`, which
//! raises `LoopDestroyed`, not `ExitRequested`.
//!
//! In other words: on macOS, **Cmd+Q and the app menu's Quit never produce
//! `RunEvent::ExitRequested` at all**. They cannot be prevented, and today they
//! do not even reach the handler that stops the capture — the process simply
//! vanishes mid-service and the recording's only rescue is the recovery scan on
//! the next launch. Every `prevent_exit`-based confirmation is dead code until
//! that item is replaced.
//!
//! ## The fix, and its deliberate smallness
//!
//! This module rebuilds `Menu::default` **item for item** from the tauri 2.11
//! source, changing one thing: the App submenu's Quit is a custom
//! [`MenuItem`] with the same label and the same `Cmd+Q` accelerator, whose menu
//! event reaches [`crate::window::request_quit`]. Undo/Redo/Cut/Copy/Paste/
//! Select All keep their predefined selectors — a hand-written Edit menu is how
//! a webview loses Cmd+C on macOS — and Window/Help keep the ids
//! ([`WINDOW_SUBMENU_ID`]/[`HELP_SUBMENU_ID`]) tauri looks for.
//!
//! Nothing here is INSTALLED off macOS: Windows and Linux get no default menu
//! from tauri, so there is no Cmd+Q to intercept and adding a menubar would be a
//! visible regression. Their quits arrive as `ExitRequested` (last window
//! destroyed) or through the tray, both already covered. The module itself
//! compiles everywhere on purpose — every predefined item used here exists on
//! every platform, and CI's Rust lanes are Linux and Windows, so a
//! `#[cfg(target_os = "macos")]` module would be code no gate ever reads.
//!
//! ## ⚠️ What is still NOT interceptable
//!
//! Anything that sends `terminate:` without going through this menu: the Dock
//! icon's "Quit", Force Quit, and a log-out / restart / shut-down. Those remain
//! exactly as they were before this change — the recovery scan is their safety
//! net. Closing that hole means implementing `applicationShouldTerminate:`,
//! which is a tao/tauri-level change, not an app-level one.
//!
//! ## ⚠️ GUI-UNVERIFIED
//!
//! A menu cannot be built in `cargo test` (every constructor needs a live
//! `AppHandle` on the main thread), so this file is compiled and reasoned about,
//! never clicked headless. The decision it routes to is unit-tested in
//! `sundayrec_core::window`; the menu itself is on the rig list in
//! `docs/APP-SHELL.md`.

use tauri::menu::{
    AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Runtime};

/// The id of the Quit item this module owns. Matched in [`handle_event`], which
/// is the app-level `on_menu_event`; the tray has its own handler and its own
/// ids, and the two never meet.
pub const QUIT_ITEM_ID: &str = "sundayrec:quit";

/// Build the macOS application menu.
///
/// A faithful copy of `tauri::menu::Menu::default` with the Quit item swapped —
/// see the module docs for why the copy is necessary and why it must stay
/// faithful.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg_info = app.package_info();
    let config = app.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    // The same label muda's predefined Quit renders ("Quit <app>"), so the menu
    // looks byte-identical to the one that shipped — only the wiring differs.
    let quit = MenuItem::with_id(
        app,
        QUIT_ITEM_ID,
        format!("Quit {}", pkg_info.name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;

    let window_menu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &[])?;

    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    // ⚠️ THE one changed line in this whole file.
                    &quit,
                ],
            )?,
            &Submenu::with_items(
                app,
                "File",
                true,
                &[&PredefinedMenuItem::close_window(app, None)?],
            )?,
            // Predefined on purpose: these carry the AppKit selectors that make
            // Cmd+C/Cmd+V work inside the webview. Hand-rolling them is how a
            // Tauri app loses copy and paste on macOS.
            &Submenu::with_items(
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
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

/// The app-level `on_menu_event`. Only [`QUIT_ITEM_ID`] is ours; every other id
/// belongs to a predefined item AppKit handles itself.
pub fn handle_event<R: Runtime>(app: &AppHandle<R>, menu_id: &str) {
    if menu_id != QUIT_ITEM_ID {
        return;
    }
    // Same seam as the tray's «Avslutt»: ask, and only exit if the answer is
    // "nothing to protect". A refusal or an armed wait returns `Handled`, and
    // the app stays alive on purpose.
    if crate::window::request_quit(app) == crate::window::QuitVerdict::Proceed {
        app.exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quit_id_is_namespaced_away_from_the_trays_ids() {
        // The two menu-event handlers are separate, but a shared id would still
        // be a trap for the next person: the tray's ids are bare action names.
        assert!(QUIT_ITEM_ID.starts_with("sundayrec:"));
        assert_ne!(QUIT_ITEM_ID, "quit");
    }
}
