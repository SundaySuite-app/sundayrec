//! Menubar tray + deep-link plumbing (PU-2 P2b) — **GUI-UNVERIFIED**, default-off `tray` feature.
//!
//! The impure half of the tray. The *model* — which localized items, their
//! actions, the tooltip, the icon — is the unit-tested [`sundayrec_core::tray`];
//! the inbound `sundayrec://` parse/dispatch is the unit-tested
//! [`sundayrec_core::link::parse_deep_link`]. This module is only the glue:
//!   - turn a [`TrayItem`] list into a `tauri::menu::Menu` ([`build_menu`]),
//!   - wire each [`TrayAction`] to an app event the renderer/commands handle
//!     ([`emit_action`]),
//!   - route an inbound deep link to the right side effect ([`dispatch_deep_link`]).
//!
//! ## Keeping the menu alive
//!
//! The menu used to be built ONCE at startup from `TrayState::default()` and
//! never touched again: a tray that permanently said "✅ Klar / Start opptak nå"
//! even mid-recording, with no next-recording line.
//! [`TrayController`] closes that loop. It is Tauri-managed state holding the
//! live [`TrayState`] + [`TrayLang`] and the `TrayIcon` handle, and every mutation
//! goes through [`update`], which re-renders the menu, the tooltip and the icon
//! — but only when the projected state actually CHANGED, so the high-frequency
//! `recording://state` stream can't thrash the menubar.
//!
//! State reaches it through [`wire_state_sources`], which SUBSCRIBES to the
//! events the recorder and scheduler already emit (`app.listen`) rather than
//! reaching into either engine. The recorder's capture/stop path is
//! hardware-verified; the tray observes it from the outside and can never
//! perturb it.
//!
//! ## ⚠️ GUI-UNVERIFIED
//!
//! The `TrayIconBuilder`, the menu rendering, the runtime-painted status badge
//! and the OS scheme delivery (`tauri-plugin-deep-link`) need a real desktop
//! session to verify — wired + compiling under `--features tray`, never run
//! headless. The badge compositing itself is pure + unit-tested in core.

use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter, Listener, Manager, Runtime};

use sundayrec_core::link::{parse_deep_link, DeepLinkAction};
use sundayrec_core::tray::{
    badge_rgb, build_menu as build_model, icon_for, tooltip, with_status_badge, TrayAction,
    TrayItem, TrayLang, TrayState,
};

/// The event the tray emits for an action the renderer/commands handle. The
/// payload is the action's stable string id (see [`action_id`]). Mirrors the
/// `tray-…` IPC sends in the Electron `tray.ts` click handlers.
pub const TRAY_ACTION_EVENT: &str = "tray://action";
/// The event emitted when an inbound deep link is an import hand-off.
pub const DEEP_LINK_IMPORT_EVENT: &str = "deeplink://import";
/// The event emitted after a captions hand-back (`sundayrec://captions`) has
/// been applied — the renderer can toast + refresh transcript search. The
/// payload is `{ ok, recording, transcriptPath?, error? }`.
pub const DEEP_LINK_CAPTIONS_EVENT: &str = "deeplink://captions";

/// The stable string id for a [`TrayAction`], used as the menu-item id and the
/// emitted event payload so the shell can route a click without re-deriving it.
pub fn action_id(action: TrayAction) -> &'static str {
    match action {
        TrayAction::ShowOnError => "show-on-error",
        TrayAction::None => "none",
        TrayAction::OpenWindow => "open-window",
        TrayAction::StartRecording => "start-recording",
        TrayAction::StopRecording => "stop-recording",
        TrayAction::OpenRecordingsFolder => "open-recordings-folder",
        TrayAction::RunPreflight => "run-preflight",
        TrayAction::RunDiagnostics => "run-diagnostics",
        TrayAction::Quit => "quit",
    }
}

/// Reverse of [`action_id`] — resolve a menu-item id back to its [`TrayAction`].
pub fn action_from_id(id: &str) -> Option<TrayAction> {
    Some(match id {
        "show-on-error" => TrayAction::ShowOnError,
        "none" => TrayAction::None,
        "open-window" => TrayAction::OpenWindow,
        "start-recording" => TrayAction::StartRecording,
        "stop-recording" => TrayAction::StopRecording,
        "open-recordings-folder" => TrayAction::OpenRecordingsFolder,
        "run-preflight" => TrayAction::RunPreflight,
        "run-diagnostics" => TrayAction::RunDiagnostics,
        "quit" => TrayAction::Quit,
        _ => return None,
    })
}

/// Build a `tauri::menu::Menu` from the core tray model for `state` + `lang`.
/// Each clickable item carries the [`action_id`] as its menu-item id so the
/// `on_menu_event` handler can resolve it via [`action_from_id`]. Disabled rows
/// (status / next-recording info) become disabled items. GUI-UNVERIFIED.
pub fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    state: &TrayState,
    lang: TrayLang,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for item in build_model(state, lang) {
        match item {
            TrayItem::Separator => {
                menu.append(&PredefinedMenuItem::separator(app)?)?;
            }
            TrayItem::Item {
                label,
                action,
                enabled,
            } => {
                let mi = MenuItem::with_id(app, action_id(action), &label, enabled, None::<&str>)?;
                menu.append(&mi)?;
            }
        }
    }
    Ok(menu)
}

/// Emit the [`TRAY_ACTION_EVENT`] for `action`, or perform it directly for the
/// ones the backend owns end-to-end:
///   - **show** (`OpenWindow`/`ShowOnError`) → bring the window forward;
///   - **stop** (`StopRecording`) → call the recorder engine's `stop()` directly
///     (the same path as the `stop_recording` command), so a tray "Stopp opptak"
///     works even with no window focused;
///   - **quit** → exit.
///
/// Everything else (start — which needs the renderer's device/settings context,
/// preflight, diagnostics) is emitted as [`TRAY_ACTION_EVENT`] for
/// the renderer to turn into the matching `invoke(...)`. This mirrors how the
/// Electron `tray.ts` mixed `app.quit()`/`win.show()` + a direct stop with
/// `webContents.send(...)` for the rest.
pub fn emit_action<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    match action {
        TrayAction::OpenWindow | TrayAction::ShowOnError => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
        TrayAction::StopRecording => {
            // Wire straight to the recorder command's effect — `RecorderEngine`
            // is managed state, and `stop()` is safe when nothing is running.
            app.state::<crate::recorder::engine::RecorderEngine>()
                .stop();
            // Still surface the event so the renderer can refresh its UI state.
            let _ = app.emit(TRAY_ACTION_EVENT, action_id(action));
        }
        TrayAction::Quit => app.exit(0),
        TrayAction::None => {}
        other => {
            let _ = app.emit(TRAY_ACTION_EVENT, action_id(other));
        }
    }
}

/// Handle an `on_menu_event` by id: resolve it to an action and run [`emit_action`].
pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, menu_id: &str) {
    if let Some(action) = action_from_id(menu_id) {
        emit_action(app, action);
    }
}

/// Route an inbound `sundayrec://…` deep link. Returns the parsed action so the
/// caller can log it; performs the side effect (show window + emit) for the ones
/// the backend owns. The OAuth-callback branch is surfaced for the cloud flow to
/// validate via its existing core path (no replay state lives here). GUI-UNVERIFIED.
///
/// ## Admission control (E1.1)
///
/// A deep link is NOT a user action — any web page that can trigger a
/// custom-scheme navigation reaches this function. Both arms therefore hand
/// their paths to [`crate::commands::deeplink`] first:
///   - **import** is validated (absolute, `..`-free, an existing media file,
///     nothing protected) and then, and only then, offered to the renderer. It
///     writes nothing, so validation is the whole gate.
///   - **captions** used to `fs::write` a `<recording>.transcript.json` for
///     whatever path the URL named. It is now VALIDATED (the recording must
///     resolve inside the configured save folder), PARKED under a single-use id,
///     and only written after the renderer confirms via
///     `deeplink_confirm_captions`. Nothing here touches the filesystem.
pub fn dispatch_deep_link<R: Runtime>(app: &AppHandle<R>, url: &str) -> Option<DeepLinkAction> {
    use crate::commands::deeplink;

    let action = parse_deep_link(url)?;
    match &action {
        DeepLinkAction::Import { path, return_to } => {
            // Bring the window forward and hand the import to the renderer —
            // but only once the path has passed the media guard.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
            let payload = match deeplink::validate_import_request(path) {
                Ok(path) => serde_json::json!({ "path": path, "returnTo": return_to }),
                Err(reject) => {
                    tracing::warn!(code = reject.code(), "deeplink: refused an import hand-off");
                    serde_json::json!({ "error": reject.code() })
                }
            };
            let _ = app.emit(DEEP_LINK_IMPORT_EVENT, payload);
        }
        DeepLinkAction::Captions { path, recording } => {
            // SundayEdit finished captioning and handed the SRT back. Resolving
            // the save folder is an async settings read, so the validate/park/
            // emit runs on the runtime; this handler stays non-blocking and the
            // OS scheme callback returns immediately.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
            let handle = app.clone();
            let (path, recording) = (path.clone(), recording.clone());
            tauri::async_runtime::spawn(async move {
                deeplink::offer_captions(&handle, DEEP_LINK_CAPTIONS_EVENT, recording, path).await;
            });
        }
        DeepLinkAction::OAuthCallback { .. } | DeepLinkAction::Unknown { .. } => {
            // OAuth-via-scheme is delivered to the cloud flow elsewhere; Unknown
            // is just logged by the caller.
        }
    }
    Some(action)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The live tray — managed state, rebuilt on every change
// ─────────────────────────────────────────────────────────────────────────────

/// The live tray: the `TrayIcon` handle plus the [`TrayState`]/[`TrayLang`] the
/// menu, tooltip and icon are projected from. Stored as Tauri-managed state, so
/// it lives for the whole process (which is also what keeps the tray on screen —
/// dropping the handle removes it).
pub struct TrayController<R: Runtime> {
    icon: tauri::tray::TrayIcon<R>,
    view: Mutex<TrayView>,
}

/// Everything the tray renders from. Compared before each rebuild so a
/// no-op update (the recorder emits `recording://state` on every transition,
/// several times a second while reconnecting) costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayView {
    state: TrayState,
    lang: TrayLang,
}

impl<R: Runtime> TrayController<R> {
    /// Re-render the menu + tooltip + icon for `view`. Best-effort: a failed
    /// menu build logs and leaves the previous menu in place rather than
    /// tearing the tray down.
    fn render(&self, app: &AppHandle<R>, view: &TrayView) {
        match build_menu(app, &view.state, view.lang) {
            Ok(menu) => {
                if let Err(e) = self.icon.set_menu(Some(menu)) {
                    tracing::warn!("tray: set_menu failed: {e}");
                }
            }
            Err(e) => tracing::warn!("tray: menu build failed: {e}"),
        }
        if let Err(e) = self
            .icon
            .set_tooltip(Some(tooltip(&view.state, view.lang).as_str()))
        {
            tracing::debug!("tray: set_tooltip unavailable: {e}");
        }
        if let Some(image) = status_icon(app, &view.state) {
            if let Err(e) = self.icon.set_icon(Some(image)) {
                tracing::warn!("tray: set_icon failed: {e}");
            }
        }
    }
}

/// The tray image for `state`: the app's own window icon, with a red (recording)
/// or amber (error) dot painted into its bottom-right corner.
///
/// Dedicated tray assets were never bundled (docs/NEEDS-RICHARD.md PU-2). Rather
/// than block "the icon reflects what the app is doing" on three PNGs, the badge
/// is composited at runtime from the icon's own RGBA buffer — the compositing is
/// the pure, unit-tested `sundayrec_core::tray::with_status_badge`. Idle returns
/// the untouched app icon, so the tray always ends a session looking like itself.
fn status_icon<R: Runtime>(
    app: &AppHandle<R>,
    state: &TrayState,
) -> Option<tauri::image::Image<'static>> {
    let base = app.default_window_icon()?;
    let (w, h) = (base.width(), base.height());
    // Always OWN the pixels: the borrowed `Image<'_>` is tied to the app handle,
    // and `set_icon` wants a `'static` one.
    let rgba = match badge_rgb(icon_for(state)) {
        Some(rgb) => with_status_badge(base.rgba(), w, h, rgb),
        None => base.rgba().to_vec(),
    };
    Some(tauri::image::Image::new_owned(rgba, w, h))
}

/// Mutate the tray state and re-render if anything actually changed. The single
/// write path — every source (recorder, scheduler, language) funnels through
/// here. A no-op mutation does no work.
pub fn update<R: Runtime>(app: &AppHandle<R>, mutate: impl FnOnce(&mut TrayState)) {
    let Some(ctl) = app.try_state::<TrayController<R>>() else {
        return; // tray not installed (headless / install failed)
    };
    let next = {
        let mut guard = match ctl.view.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = guard.clone();
        mutate(&mut guard.state);
        if *guard == before {
            return;
        }
        guard.clone()
    };
    ctl.render(app, &next);
}

/// Switch the tray's language and relabel every row. Called from the
/// `tray_set_language` command when the renderer's language changes (the UI
/// language lives in the renderer's own settings blob, not the backend's).
pub fn set_lang<R: Runtime>(app: &AppHandle<R>, lang: TrayLang) {
    let Some(ctl) = app.try_state::<TrayController<R>>() else {
        return;
    };
    let next = {
        let mut guard = match ctl.view.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.lang == lang {
            return;
        }
        guard.lang = lang;
        guard.clone()
    };
    ctl.render(app, &next);
}

/// Localized weekday abbreviations, Monday-first (chrono's
/// `Weekday::num_days_from_monday()` order). Same seven languages as the rest of
/// the tray model; kept here because it is wall-clock FORMATTING, which the core
/// model deliberately leaves to the shell.
const WEEKDAYS: [[&str; 7]; 7] = [
    // No
    ["man.", "tir.", "ons.", "tor.", "fre.", "lør.", "søn."],
    // En
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    // De
    ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."],
    // Sv
    ["mån.", "tis.", "ons.", "tors.", "fre.", "lör.", "sön."],
    // Da
    ["man.", "tir.", "ons.", "tor.", "fre.", "lør.", "søn."],
    // Pl
    ["pon.", "wt.", "śr.", "czw.", "pt.", "sob.", "niedz."],
    // Fr
    ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
];

fn lang_row(lang: TrayLang) -> usize {
    match lang {
        TrayLang::No => 0,
        TrayLang::En => 1,
        TrayLang::De => 2,
        TrayLang::Sv => 3,
        TrayLang::Da => 4,
        TrayLang::Pl => 5,
        TrayLang::Fr => 6,
    }
}

/// Format a `scheduler://next` payload (a zone-less local `YYYY-MM-DDTHH:MM:SS`)
/// into the short menu label the core model places — e.g. `søn. 11:00`. Returns
/// `None` for a null/unparseable payload so the info row simply disappears
/// rather than showing a raw timestamp. Pure → unit-tested.
pub fn format_next_label(iso: Option<&str>, lang: TrayLang) -> Option<String> {
    use chrono::{Datelike, NaiveDateTime, Timelike};
    let raw = iso?.trim();
    if raw.is_empty() {
        return None;
    }
    let dt = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
        .ok()?;
    let day = WEEKDAYS[lang_row(lang)][dt.weekday().num_days_from_monday() as usize];
    Some(format!("{day} {:02}:{:02}", dt.hour(), dt.minute()))
}

/// Whether a `recording://state` state string means a capture is live. Anything
/// from the spawn to the graceful finalise counts — the tray must not flip back
/// to "Start opptak nå" while ffmpeg is still closing the container. Pure.
pub fn state_is_live(state: &str) -> bool {
    matches!(
        state,
        "preparing" | "recording" | "reconnecting" | "stopping"
    )
}

/// Subscribe the tray to the events the recorder and scheduler ALREADY emit.
///
/// Deliberately observational: no engine is modified and no engine call is added
/// to a capture path. `app.listen` is the least invasive seam there is — the
/// recorder's hardware-verified start/stop code has no idea the tray exists.
pub fn wire_state_sources<R: Runtime>(app: &AppHandle<R>) {
    // Recorder lifecycle. `recording://state` fires on every transition and
    // carries `RecorderStatePayload { state, … }`.
    let h = app.clone();
    app.listen(crate::recorder::engine::STATE_EVENT, move |ev| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(ev.payload()) else {
            return;
        };
        let Some(st) = v.get("state").and_then(|s| s.as_str()) else {
            return;
        };
        let live = state_is_live(st);
        update(&h, |s| {
            s.is_recording = live;
            // A take that starts clears the previous session's error; a failed
            // one raises it. A clean stop leaves the flag as it is.
            if live {
                s.has_error = false;
            } else if st == "failed" {
                s.has_error = true;
            }
        });
    });

    // Terminal recorder error (the reconnect budget is gone / a fatal code).
    let h = app.clone();
    app.listen(crate::recorder::engine::ERROR_EVENT, move |_| {
        update(&h, |s| {
            s.is_recording = false;
            s.has_error = true;
        });
    });

    // The scheduler recomputed the nearest future start (payload: a zone-less
    // local ISO string, or null when nothing is scheduled).
    let h = app.clone();
    app.listen(crate::scheduler::NEXT_EVENT, move |ev| {
        let iso = serde_json::from_str::<Option<String>>(ev.payload()).unwrap_or(None);
        let lang = current_lang(&h);
        let label = format_next_label(iso.as_deref(), lang);
        update(&h, |s| s.next_recording_label = label);
    });
}

/// The tray's current language (Norwegian when the tray isn't installed).
fn current_lang<R: Runtime>(app: &AppHandle<R>) -> TrayLang {
    app.try_state::<TrayController<R>>()
        .map(|ctl| match ctl.view.lock() {
            Ok(g) => g.lang,
            Err(p) => p.into_inner().lang,
        })
        .unwrap_or(TrayLang::No)
}

/// Build + install the menubar tray icon with the current [`TrayState`] menu
/// and wire its `on_menu_event` to [`handle_menu_event`]. Called once from the
/// app `setup` under the `tray` feature.
///
/// The icon image is the app's default window icon, badged for the current state
/// ([`status_icon`]); the menu *shape* is the unit-tested [`build_model`]
/// projection. The resulting [`TrayController`] is stored as managed state — that
/// is what keeps the tray alive AND what later rebuilds go through.
/// GUI-UNVERIFIED.
pub fn install<R: Runtime>(
    app: &AppHandle<R>,
    state: &TrayState,
    lang: TrayLang,
) -> tauri::Result<()> {
    use tauri::tray::TrayIconBuilder;

    let menu = build_menu(app, state, lang)?;
    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip(tooltip(state, lang))
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()));

    if let Some(icon) = status_icon(app, state) {
        builder = builder.icon(icon);
    }

    let icon = builder.build(app)?;
    app.manage(TrayController {
        icon,
        view: Mutex::new(TrayView {
            state: state.clone(),
            lang,
        }),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_label_formats_weekday_and_time_per_language() {
        // 2026-08-09 is a Sunday.
        assert_eq!(
            format_next_label(Some("2026-08-09T11:00:00"), TrayLang::No).as_deref(),
            Some("søn. 11:00")
        );
        assert_eq!(
            format_next_label(Some("2026-08-09T11:00:00"), TrayLang::En).as_deref(),
            Some("Sun 11:00")
        );
        // A Wednesday evening, single-digit hour padded.
        assert_eq!(
            format_next_label(Some("2026-08-05T09:05:00"), TrayLang::Fr).as_deref(),
            Some("mer. 09:05")
        );
    }

    #[test]
    fn next_label_is_none_for_nothing_scheduled_or_garbage() {
        assert_eq!(format_next_label(None, TrayLang::No), None);
        assert_eq!(format_next_label(Some(""), TrayLang::No), None);
        assert_eq!(format_next_label(Some("   "), TrayLang::No), None);
        assert_eq!(format_next_label(Some("not-a-date"), TrayLang::No), None);
        // A raw timestamp in the menu would be worse than no row at all.
        assert_eq!(
            format_next_label(Some("2026-13-45T99:99"), TrayLang::No),
            None
        );
    }

    #[test]
    fn the_tray_stays_recording_until_the_container_is_closed() {
        for live in ["preparing", "recording", "reconnecting", "stopping"] {
            assert!(state_is_live(live), "{live} must read as a live session");
        }
        for done in ["idle", "stopped", "failed", "something-new"] {
            assert!(!state_is_live(done), "{done} must not read as live");
        }
    }

    #[test]
    fn action_id_round_trips_for_every_variant() {
        for action in [
            TrayAction::ShowOnError,
            TrayAction::None,
            TrayAction::OpenWindow,
            TrayAction::StartRecording,
            TrayAction::StopRecording,
            TrayAction::OpenRecordingsFolder,
            TrayAction::RunPreflight,
            TrayAction::RunDiagnostics,
            TrayAction::Quit,
        ] {
            assert_eq!(action_from_id(action_id(action)), Some(action));
        }
    }

    #[test]
    fn action_from_id_rejects_unknown() {
        assert_eq!(action_from_id("not-a-real-id"), None);
    }
}
