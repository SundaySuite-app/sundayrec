//! The main window's close button — the thin shell over
//! [`sundayrec_core::window`].
//!
//! ## What changed
//!
//! There was no `on_window_event` handler at all. Closing the last window
//! produced `RunEvent::ExitRequested`, and that handler stops the recorder — so
//! a volunteer who closed the window mid-sermon lost the rest of the service.
//! Now a close request during a live (or finalising) session hides the window
//! instead: `api.prevent_close()` + `window.hide()`, with the capture untouched
//! and the app still in the menubar / system tray.
//!
//! Outside a session nothing changed: the close goes through, the last window
//! closing still raises `ExitRequested`, and that handler still stops the
//! sidecars exactly as before. Cmd+Q / the tray's "Avslutt" are likewise
//! untouched — they are `ExitRequested`, not `CloseRequested`, and this module
//! never sees them.
//!
//! ## Why the decision is not in the closure
//!
//! A `tauri::Window` cannot be constructed in `cargo test`, so anything written
//! inline in the handler is unreachable by every gate the repo has. The rule
//! therefore lives in the pure, exhaustively-matched
//! [`sundayrec_core::window::close_action`] and this file is deliberately dumb:
//! ask, then hide or stand aside.
//!
//! ## ⚠️ GUI-UNVERIFIED
//!
//! The `prevent_close`/`hide` pair and the Dock-reopen path need a real desktop
//! session; they are compiled and reasoned about, never clicked headless. The
//! decision and the notification wording above them are unit-tested.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Manager, Window, WindowEvent};

use sundayrec_core::tray::TrayLang;
use sundayrec_core::window::{close_action, hidden_notice, CloseAction, HideReason, TraySpot};

/// The label of the one window `tauri.conf.json` defines. A close request on any
/// other window (none exist today) is left alone.
pub const MAIN_LABEL: &str = "main";

/// One "the window is only hidden" notification per hide. Set while the window
/// is hidden and cleared by [`note_window_shown`] when it comes back, so a
/// volunteer who hides, reopens and hides again is told again — and a platform
/// that delivers `CloseRequested` twice for one click is not.
static NOTICE_PENDING: AtomicBool = AtomicBool::new(false);

/// The `on_window_event` handler. Registered on the builder in `lib.rs`.
pub fn on_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN_LABEL {
        return;
    }
    let app = window.app_handle();
    let state = app
        .state::<crate::recorder::engine::RecorderEngine>()
        .current_state();
    let reason = match close_action(state) {
        // Nothing is running: close means quit, exactly as it always did.
        CloseAction::Exit => return,
        CloseAction::Hide(reason) => reason,
    };

    // Hide FIRST, and only claim the close if the hide actually worked. The
    // other order can leave a window that refuses to close AND refuses to
    // disappear — a wedged app is worse than the bug being fixed.
    if let Err(e) = window.hide() {
        tracing::error!(
            ?state,
            "close during a session: hiding the window failed ({e}) — letting the close through"
        );
        return;
    }
    api.prevent_close();
    tracing::info!(
        ?state,
        "close during a session: window hidden, the recording keeps running"
    );

    if !NOTICE_PENDING.swap(true, Ordering::SeqCst) {
        notify_hidden(app.clone(), reason);
    }
}

/// Re-arm the hide notification. Called from every path that brings the window
/// back (the tray's "Åpne SundayRec", a second launch, the macOS Dock icon), so
/// the next hide explains itself again.
pub fn note_window_shown() {
    NOTICE_PENDING.store(false, Ordering::SeqCst);
}

/// Show + focus the main window. THE one way back from a hidden window, so every
/// caller also re-arms the notification.
pub fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window(MAIN_LABEL) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        note_window_shown();
    }
}

/// Fire the OS notification that says the window is hidden, not closed.
///
/// Deliberately NOT gated by the `notifyStart`/`notifyStop` comfort toggles —
/// see [`sundayrec_core::window::hidden_notice`]. Async because the UI language
/// lives in the settings row and the event-loop callback must not block on the
/// database.
fn notify_hidden(app: tauri::AppHandle, reason: HideReason) {
    tauri::async_runtime::spawn(async move {
        let lang = match app.try_state::<crate::db::Db>() {
            Some(db) => TrayLang::from_code(
                crate::settings::load(&db.pool)
                    .await
                    .unwrap_or_default()
                    .language
                    .as_deref(),
            ),
            None => TrayLang::No,
        };
        let (title, body) = hidden_notice(reason, lang, tray_spot());
        crate::notify::native(&app, &title, &body);
    });
}

/// Where this platform parks a background app's icon. macOS says menu bar,
/// Windows/Linux say notification area.
///
/// A build without the `tray` feature has no icon anywhere — but `tray` is in
/// `default` AND in both release feature lists (docs/SMOKE-TEST.md §"Features"),
/// so that is a CI compile lane, never something a volunteer runs. Such a build
/// still hides rather than killing the capture (the recording matters more than
/// the sentence being exact), and the window is still reachable by launching
/// SundayRec again — the single-instance plugin focuses the running one.
fn tray_spot() -> TraySpot {
    if cfg!(target_os = "macos") {
        TraySpot::Menubar
    } else {
        TraySpot::SystemTray
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_flag_fires_once_per_hide_and_re_arms_on_show() {
        note_window_shown();
        // First hide claims the notification…
        assert!(!NOTICE_PENDING.swap(true, Ordering::SeqCst));
        // …a repeated CloseRequested for the same hidden window does not.
        assert!(NOTICE_PENDING.swap(true, Ordering::SeqCst));
        // Coming back re-arms it, so the NEXT hide explains itself again.
        note_window_shown();
        assert!(!NOTICE_PENDING.swap(true, Ordering::SeqCst));
        note_window_shown();
    }

    #[test]
    fn the_tray_spot_matches_the_platform_that_has_that_word() {
        let spot = tray_spot();
        if cfg!(target_os = "macos") {
            assert_eq!(spot, TraySpot::Menubar);
        } else {
            assert_eq!(spot, TraySpot::SystemTray);
        }
    }
}
