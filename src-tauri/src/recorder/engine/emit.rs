//! The recorder's error/warning/failure events to the renderer, mirrored to
//! `last-error.json` for Lydhjelp. Split out of `engine.rs`; see the parent
//! module docs.

use sundayrec_core::alerts::AlertText;
use tauri::{AppHandle, Emitter};

use super::payloads::RecordingEvent;
use super::{ERROR_EVENT, WARNING_EVENT};

/// The `recording://reconnecting` message for this attempt.
///
/// Used to read `"… ({attempt}/20)"`, which was a promise the recorder could
/// keep only because it gave up after twenty tries. Under the 2026-08-10
/// time-budget policy there is no denominator — so the message says the true
/// thing instead: how many tries so far, and (once the streak passes
/// `RECONNECT_GRACE_MS`) how long the device has been gone. That second half is
/// the whole reason `degraded_for_ms` exists: a recorder that retries for an
/// hour while the UI shows an unchanging cheerful "reconnecting" is the silent
/// forever-loop the policy is required not to be.
pub(crate) fn reconnecting_message(attempt: u32, degraded_for_ms: Option<u64>) -> String {
    match degraded_for_ms {
        None => format!("Losing contact — trying to reconnect (attempt {attempt})"),
        Some(gone_ms) => {
            // Whole minutes: the operator needs "a while now", not precision.
            let minutes = gone_ms / 60_000;
            format!(
                "The audio device has been gone for {minutes} min — the recording keeps \
                 trying (attempt {attempt}). Check the cable and power to the audio gear."
            )
        }
    }
}

/// Emit a classified TERMINAL error to the renderer (the UI tears the recording
/// overlay down on this event — see [`ERROR_EVENT`]).
pub(crate) fn emit_error(app: &AppHandle, code: &str, message: &str) {
    let _ = app.emit(
        ERROR_EVENT,
        RecordingEvent {
            code: code.to_string(),
            message: message.to_string(),
        },
    );
    // Companion for the standalone "SundayRec Lydhjelp" diagnostic: persist the
    // last classified error to disk so that tool can explain, in plain Norwegian,
    // what stopped the recording last time (it can't see our in-process events).
    skriv_siste_feil_til_disk(app, code, message);
}

/// A terminal failure whose only words are diagnostics — the last ffmpeg
/// stderr line, a Rust `io::Error`, a camera classifier's tag.
///
/// Those words used to be the event's `message`, and the native notification
/// shows the message verbatim: a volunteer in Polish got «Input/output error»
/// on the desktop. The event now carries the SENTENCE for the code, in the
/// volunteer's language ([`AlertText::for_recording_code`], the renderer's own
/// wording); the diagnostics go where diagnostics are read — the log, and
/// Lydhjelp's `last-error.json`, which wants "the code + a stderr snippet".
///
/// Use [`emit_error`] when the message already IS a localized sentence.
pub(crate) fn emit_failure(app: &AppHandle, code: &str, detail: &str) {
    tracing::error!(code, %detail, "recorder: terminal failure");
    let _ = app.emit(
        ERROR_EVENT,
        RecordingEvent {
            code: code.to_string(),
            message: AlertText::for_recording_code(code).text(crate::ui_lang::current()),
        },
    );
    skriv_siste_feil_til_disk(app, code, detail);
}

/// Emit a classified NON-terminal error (the session continues — the reconnect
/// policy will retry). Still mirrored to `last-error.json` so the diagnostics
/// surface sees transient hiccups too.
pub(crate) fn emit_warning(app: &AppHandle, code: &str, message: &str) {
    let _ = app.emit(
        WARNING_EVENT,
        RecordingEvent {
            code: code.to_string(),
            message: message.to_string(),
        },
    );
    skriv_siste_feil_til_disk(app, code, message);
}

/// Best-effort write of the most recent classified error to
/// `<app_data_dir>/last-error.json` (atomic temp+rename). Never fails the
/// recorder — any I/O error is logged and swallowed.
fn skriv_siste_feil_til_disk(app: &AppHandle, code: &str, message: &str) {
    let Ok(dir) = crate::appdata::dir(app) else {
        return;
    };
    // Keep the file small — the diagnostic only needs the code + a stderr snippet.
    let msg: String = message.chars().take(2000).collect();
    let body = serde_json::json!({
        "code": code,
        "message": msg,
        "timestamp": chrono::Local::now().to_rfc3339(),
    });
    // Blocking fs I/O OFF the async caller: emit_error/emit_warning run on the
    // supervisor task — the drainer of the reader channel. A slow disk here used
    // to stall the drain (part of the 2026-07-31 back-pressure chain).
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("last-error.json");
        let tmp = dir.join("last-error.json.tmp");
        if std::fs::write(&tmp, body.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            tracing::info!(path = %path.display(), "Lydhjelp: siste feil skrevet til disk");
        } else {
            tracing::warn!("Lydhjelp: klarte ikke skrive last-error.json");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnecting_message_promises_no_denominator() {
        // The old text was "(1/20)" — a countdown to giving up. The time-budget
        // policy has no such number, and printing one would be a lie.
        let m = reconnecting_message(1, None);
        assert!(m.contains("attempt 1"), "{m}");
        assert!(
            !m.contains("/20"),
            "the retired attempt cap must not reappear: {m}"
        );
        assert!(!m.contains('/'), "no denominator at all: {m}");
    }

    #[test]
    fn reconnecting_message_reports_how_long_the_device_has_been_gone() {
        // Past the grace window the operator must be told the DURATION — this is
        // what makes "keep retrying for the whole session" honest rather than a
        // silent forever-loop.
        let m = reconnecting_message(41, Some(23 * 60_000 + 30_000));
        assert!(m.contains("23 min"), "whole minutes of absence: {m}");
        assert!(m.contains("attempt 41"), "{m}");
        assert_ne!(
            m,
            reconnecting_message(41, None),
            "a degraded streak must not read like an ordinary retry"
        );
    }
}
