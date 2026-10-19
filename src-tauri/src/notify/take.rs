//! Native notifications DURING a take — the degradations the banner shows,
//! also told to an operator who is not looking at SundayRec.
//!
//! Silence, missing sound, a dropped input device and a filling disk already
//! raise a banner on the Record page. That banner is invisible when the window
//! is hidden, minimised or behind the slides — which is exactly when a
//! volunteer at the mixer most needs to hear it. So each of the four also
//! raises a native notification, but only:
//!
//!   - when the main window is NOT in view (the banner is enough when it is),
//!   - once per take per kind (reset when the next take starts).
//!
//! The decision is `sundayrec_core::notify::should_native_during_take`. Like
//! `wire_failure_sources`, this listens to events the engine ALREADY emits;
//! no capture code is touched.

use std::collections::HashSet;
use std::sync::Mutex;

use tauri::{AppHandle, Listener, Runtime};

use sundayrec_core::alerts::AlertText;
use sundayrec_core::notify::{should_native_during_take, CameraOutageWatch, TakeAlert};

use crate::util::lock_recover;

/// The kinds already told natively during the current take.
static SENT: Mutex<Option<HashSet<TakeAlert>>> = Mutex::new(None);

/// The camera outage in progress, if any (C5b). Its own state and not part of
/// [`SENT`]: that one is once per TAKE, this is once per OUTAGE, and it is
/// re-armed by the camera coming back, not by a new fragment starting.
static CAMERA: Mutex<CameraOutageWatch> = Mutex::new(CameraOutageWatch::new());

/// A new take started: every kind may be said once again.
fn reset() {
    *lock_recover(&SENT) = None;
}

/// Whether `alert` was already told natively during this take.
fn already_sent(alert: TakeAlert) -> bool {
    lock_recover(&SENT)
        .as_ref()
        .is_some_and(|s| s.contains(&alert))
}

fn mark_sent(alert: TakeAlert) {
    lock_recover(&SENT)
        .get_or_insert_with(HashSet::new)
        .insert(alert);
}

/// The sentence for `alert`, in the volunteer's language.
fn text(alert: TakeAlert, free_gb: Option<f64>) -> String {
    let lang = crate::ui_lang::current();
    match alert {
        TakeAlert::Silence => AlertText::TakeSilence.text(lang),
        TakeAlert::Quality => AlertText::TakeQuality.text(lang),
        TakeAlert::Reconnecting => AlertText::TakeReconnecting.text(lang),
        TakeAlert::CameraLost => AlertText::TakeCameraLost.text(lang),
        TakeAlert::DiskLow => {
            AlertText::TakeDiskLow.fill(lang, &[("gb", &format!("{:.1}", free_gb.unwrap_or(0.0)))])
        }
    }
}

/// Tell the operator natively about `alert`, if nobody is looking at the app
/// and it has not been said yet this take. Marked as sent only when it WAS
/// sent: a silence noticed while the window had focus is still news if it
/// comes back after the operator switched to the slides.
pub fn raise<R: Runtime>(app: &AppHandle<R>, alert: TakeAlert, free_gb: Option<f64>) {
    let in_view = crate::window::main_window_in_view(app);
    if !should_native_during_take(in_view, already_sent(alert)) {
        return;
    }
    mark_sent(alert);
    tracing::info!(
        ?alert,
        "notify: take degradation told natively (window not in view)"
    );
    super::native(app, super::APP_TITLE, &text(alert, free_gb));
}

/// One `recording://progress` heartbeat: is the camera-lost notification due,
/// and has the operator not got the window in front of them?
///
/// `true` means "raise it now". The outage is marked as told only then, so an
/// outage that passes 60 s while the window is in view is still told when the
/// window goes away (same rule as [`raise`]).
fn camera_step(watch: &mut CameraOutageWatch, lost_ms: Option<u64>, window_in_view: bool) -> bool {
    let due = watch.observe(lost_ms.map(std::time::Duration::from_millis));
    if due && should_native_during_take(window_in_view, false) {
        watch.mark_alerted();
        return true;
    }
    false
}

/// Subscribe to the engine's own events. Call once, from `setup` (through
/// `wire_failure_sources`).
pub fn wire(app: &AppHandle) {
    use crate::recorder::engine::{
        RecordingEvent, RecordingProgress, PROGRESS_EVENT, QUALITY_EVENT, RECONNECTING_EVENT,
        SILENCE_EVENT, STARTED_EVENT,
    };

    app.listen(STARTED_EVENT, |_| reset());

    // C5b: the camera is gone for over a minute while the audio goes on. The
    // heartbeat carries how long; the decision is `CameraOutageWatch`.
    let handle = app.clone();
    app.listen(PROGRESS_EVENT, move |ev| {
        let Ok(p) = serde_json::from_str::<RecordingProgress>(ev.payload()) else {
            return;
        };
        let in_view = crate::window::main_window_in_view(&handle);
        let raise_now = camera_step(&mut lock_recover(&CAMERA), p.video_lost_ms, in_view);
        if raise_now {
            let app = handle.clone();
            tracing::info!("notify: camera has been gone for over a minute — told natively");
            tauri::async_runtime::spawn(async move {
                super::native(&app, super::APP_TITLE, &text(TakeAlert::CameraLost, None));
            });
        }
    });

    let handle = app.clone();
    app.listen(SILENCE_EVENT, move |_| {
        let app = handle.clone();
        tauri::async_runtime::spawn(async move { raise(&app, TakeAlert::Silence, None) });
    });

    let handle = app.clone();
    app.listen(QUALITY_EVENT, move |_| {
        let app = handle.clone();
        tauri::async_runtime::spawn(async move { raise(&app, TakeAlert::Quality, None) });
    });

    let handle = app.clone();
    app.listen(RECONNECTING_EVENT, move |ev| {
        // Only `reconnecting` is a dropout on this channel. (Until C3b it also
        // carried the two-process fallback's `two_process_fallback`.)
        let Ok(e) = serde_json::from_str::<RecordingEvent>(ev.payload()) else {
            return;
        };
        if !is_dropout(&e.code) {
            return;
        }
        let app = handle.clone();
        tauri::async_runtime::spawn(async move { raise(&app, TakeAlert::Reconnecting, None) });
    });
}

/// Whether a `recording://reconnecting` code means the input dropped out.
fn is_dropout(code: &str) -> bool {
    code == "reconnecting"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_real_dropout_counts_as_reconnecting() {
        assert!(is_dropout("reconnecting"));
        assert!(!is_dropout("reconnected"));
    }

    #[test]
    fn a_new_take_lets_every_kind_be_said_again() {
        reset();
        assert!(!already_sent(TakeAlert::Silence));
        mark_sent(TakeAlert::Silence);
        assert!(already_sent(TakeAlert::Silence));
        assert!(
            !already_sent(TakeAlert::DiskLow),
            "each kind has its own once"
        );
        reset();
        assert!(!already_sent(TakeAlert::Silence));
    }

    fn ms(s: u64) -> Option<u64> {
        Some(s * 1000)
    }

    #[test]
    fn the_camera_notification_waits_for_60_s_and_comes_once_per_outage() {
        let mut w = CameraOutageWatch::default();
        // Window out of view throughout.
        assert!(!camera_step(&mut w, None, false));
        assert!(!camera_step(&mut w, ms(1), false));
        assert!(!camera_step(&mut w, Some(59_999), false), "not at 59.999 s");
        assert!(camera_step(&mut w, ms(60), false), "at 60 s");
        for s in 61..90 {
            assert!(!camera_step(&mut w, ms(s), false), "not again at {s} s");
        }
        // The camera comes back; a second outage is told again at its 60 s.
        assert!(!camera_step(&mut w, None, false));
        assert!(!camera_step(&mut w, ms(10), false));
        assert!(camera_step(&mut w, ms(60), false));
    }

    #[test]
    fn an_outage_seen_with_the_window_in_view_is_told_when_it_leaves() {
        let mut w = CameraOutageWatch::default();
        assert!(!camera_step(&mut w, ms(60), true), "the banner is enough");
        assert!(!camera_step(&mut w, ms(75), true));
        assert!(camera_step(&mut w, ms(76), false), "now nobody is looking");
        assert!(!camera_step(&mut w, ms(77), false));
    }

    #[test]
    fn the_disk_sentence_carries_the_free_space() {
        let s = text(TakeAlert::DiskLow, Some(1.84));
        assert!(s.contains("1.8"), "{s}");
        assert!(!s.contains("{gb}"), "{s}");
    }
}
