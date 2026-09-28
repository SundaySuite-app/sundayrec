//! The notification DISPATCH seam — one place a failure or a warning is told to
//! everybody who should hear it.
//!
//! ## Why this module exists
//!
//! Before it, each source of trouble picked its own audience by hand, and the
//! picks were wrong in ways nobody could see from any single file: the
//! recorder's terminal `recording://error` reached the tray badge and the
//! renderer, and never the OS notification centre.
//!
//! Everything now goes through [`dispatch_failure`] (terminal failures → a
//! native OS notification, always) or [`warn`] (degradations → an in-app
//! banner). There is no third leg: the SMTP alerter and the SundaySuite e-mail
//! relay were removed — the setup was too heavy for a volunteer, and SundayRec
//! no longer sends anything off the machine to report a failure. The person
//! standing at the machine is the one who can still save the service, so the
//! native notification is the channel, and no setting silences it. (A chat
//! webhook lived here before the e-mail legs; all of it is in git.)
//!
//! ## Observational, never invasive
//!
//! Nothing here is called from a capture path. Recorder failures arrive on the
//! events the engine ALREADY emits (`app.listen`, exactly like the tray does);
//! the warning sources are back-off branches and post-hoc skips. The engine's
//! hardware-verified start/stop code has no idea this module exists.

use tauri::{AppHandle, Emitter, Listener};

use sundayrec_core::notify::{BackendWarning, FailureSource};

pub mod disk;
/// Whether the OS actually shows SundayRec's notifications.
pub mod permission;
/// The durable "already said this" ledger (`notify_seen`) behind the
/// missed-recording notice.
pub mod seen;
/// Native notifications during a take, when nobody is looking at the app.
pub mod take;

pub use sundayrec_core::notify::code;

/// The event the renderer's `backend-warning` channel is mapped to. Follows the
/// `scheme://name` convention every other Rust-emitted event uses
/// (`recording://…`, `scheduler://…`, `tray://…`).
pub const WARNING_EVENT: &str = "backend://warning";

/// The stable code a [`FailureSource::Missed`] dispatch carries. Not one of
/// [`code`]'s renderer-facing warning codes: those name a live degradation the
/// banner localises, and this names the absence of a recording, which reaches
/// the operator as a native notification.
pub const CODE_SCHEDULED_MISSED: &str = "scheduled_missed";

/// One scheduled occurrence the missed-recording sweep found unrecorded.
#[derive(Debug, Clone)]
pub struct MissedSlot {
    /// ISO-like local start (`YYYY-MM-DDTHH:MM:SS`) — the machine's clock, and
    /// half of the durable `notify_seen` key.
    pub at: String,
    /// The schedule's own name for the slot ("Ukentlig opptak (11:00–13:00)").
    pub label: String,
}

impl MissedSlot {
    /// This occurrence's row in the `notify_seen` ledger, under
    /// [`sundayrec_core::notify::SeenScope::Missed`].
    ///
    /// The label is HASHED rather than spelled out: a special recording's name
    /// is something a person typed ("Bryllup Kari og Ola"), and a ledger that
    /// keeps names is a second place a name lives for no gain — the key only
    /// ever has to be compared with itself. `at` stays legible because a
    /// timestamp is the one part somebody debugging this actually needs to read.
    ///
    /// The format is load-bearing: rows written by earlier versions carry it,
    /// and a changed key would re-announce every missed Sunday still in the
    /// look-back window.
    pub fn seen_key(&self) -> String {
        format!("{}:{}", self.at, short_hash(&self.label))
    }
}

/// Everything [`dispatch_failure`] needs to know about a failure.
#[derive(Debug, Clone)]
pub struct FailureCtx {
    /// Stable machine code (the recorder's `RecordingEvent::code`, or one of the
    /// scheduler's). Logged with the dispatch so a device drop-out can be told
    /// from a disk stop at a glance.
    pub code: String,
    /// The human sentence — the native notification body. The scheduler's
    /// existing wording is passed through verbatim rather than re-derived here.
    pub message: String,
    /// Which half of the app failed.
    pub source: FailureSource,
}

impl FailureCtx {
    /// A failure that just happened.
    pub fn now(code: impl Into<String>, message: impl Into<String>, source: FailureSource) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            source,
        }
    }
}

/// Sixteen hex digits of SHA-256 — enough that two different labels colliding
/// is not a thing that happens, short enough that a `notify_seen` key stays
/// readable in a `sqlite3` session.
fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(s.as_bytes())
        .iter()
        .take(8)
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The title a native notification carries when it has nothing more specific
/// to say. NOT localized, and deliberately so: it is the product's name.
///
/// Stated once here because five call sites spelled it as a literal, and a
/// gate that hunts for Norwegian in string literals should not have to reason
/// about which `"SundayRec"` is a brand and which is the start of a sentence.
pub const APP_TITLE: &str = "SundayRec";

/// Fire a native OS notification. The one channel no setting can silence: the
/// person standing at the machine is the only one who can still save the
/// service. Previously private to the scheduler — the recorder had no way to
/// reach it at all.
///
/// Generic over the runtime because the tray and the quit path are too: the
/// tray's «Avslutt» and the app menu's Quit both reach the notification through
/// `crate::window`, and a concrete `AppHandle` there would force the runtime
/// parameter out of those call sites for no gain.
pub fn native<R: tauri::Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notify: native notification failed: {e}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Failures
// ─────────────────────────────────────────────────────────────────────────────

/// Subscribe the dispatcher to the failure events the app ALREADY emits.
///
/// Exactly the seam the tray uses (`tray::wire_state_sources` listens to this
/// same [`ERROR_EVENT`](crate::recorder::engine::ERROR_EVENT)): `app.listen` is
/// observational, so the recorder's hardware-verified capture and stop code is
/// not touched, not even by one line. That matters more here than anywhere —
/// every regression this app has shipped in the recorder came from editing the
/// capture path for a reason that turned out to be a reporting reason.
///
/// The scheduler's failures do NOT arrive here: they are not events, they are
/// return values, so those call sites invoke [`dispatch_failure`] directly.
///
/// Call once, from `setup`.
pub fn wire_failure_sources(app: &AppHandle) {
    use crate::recorder::engine::{RecordingEvent, ERROR_EVENT};

    let handle = app.clone();
    app.listen(ERROR_EVENT, move |ev| {
        let Ok(e) = serde_json::from_str::<RecordingEvent>(ev.payload()) else {
            tracing::warn!("notify: unparseable {ERROR_EVENT} payload — no alert sent");
            return;
        };
        // The listener callback runs on the event loop; the notification is
        // shown from a task, as it always has been, so a slow notification
        // centre can never stall the loop that is delivering recorder events.
        let app = handle.clone();
        tauri::async_runtime::spawn(async move {
            dispatch_failure(
                &app,
                FailureCtx::now(e.code, e.message, FailureSource::Recording),
            );
        });
    });

    // The graduated low-disk observer rides on the same observational seam.
    disk::wire(app);

    // Silence, missing sound and a dropped input — natively, when the window
    // is not in view (see `take`).
    take::wire(app);

    tracing::info!("notify: failure dispatch wired to {ERROR_EVENT}");
}

/// Tell the operator about a terminal failure: a native OS notification, always.
///
/// Never returns an error: an alert path that can itself fail loudly is a second
/// failure on top of the first. [`native`] logs a failed show and moves on.
pub fn dispatch_failure<R: tauri::Runtime>(app: &AppHandle<R>, ctx: FailureCtx) {
    tracing::warn!(
        code = %ctx.code,
        source = ctx.source.as_str(),
        "notify: failure — alerting natively"
    );
    native(app, APP_TITLE, &ctx.message);
}

// ─────────────────────────────────────────────────────────────────────────────
//   Warnings
// ─────────────────────────────────────────────────────────────────────────────

/// Raise a live backend warning: log it and emit it to the renderer (which
/// localises on [`BackendWarning::code`] and toasts it).
///
/// Synchronous by design so it can be called from anywhere — including the
/// `&mut`-heavy back-off branches of the pre-roll loop. The event goes out
/// immediately, because a warning must never make the thing it is warning
/// about slower.
pub fn warn(app: &AppHandle, w: BackendWarning) {
    tracing::warn!(code = %w.code, msg = ?w.msg, "notify: backend warning");
    if let Err(e) = app.emit(WARNING_EVENT, &w) {
        tracing::warn!("notify: could not emit {WARNING_EVENT}: {e}");
    }
}

/// The handle [`warn_detached`] emits through. Armed once from `setup`.
static DETACHED: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// Arm [`warn_detached`]. Called once from `setup`; a second call is ignored.
pub fn arm_detached(app: AppHandle) {
    let _ = DETACHED.set(app);
}

/// Raise a warning from a seam that has no [`AppHandle`] to raise it with.
///
/// Every other warning source in the app is a background task that was HANDED
/// a handle when it was spawned. The Papirkurv seam is not: it is plain
/// filesystem code, called from four commands, the sweep and the retention
/// pass, and threading a handle through all six to reach one `if` deep inside a
/// manifest read would make the seam's signature about notifications rather
/// than about the trash.
///
/// Before `setup` — and in every unit test, which is the point — this logs and
/// returns. A warning nobody is listening for is not an error; a seam that
/// cannot be unit-tested because it insists on a GUI is.
pub fn warn_detached(w: BackendWarning) {
    match DETACHED.get() {
        Some(app) => warn(app, w),
        None => tracing::warn!(
            code = %w.code,
            msg = ?w.msg,
            "notify: backend warning raised before the app was armed"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warning_event_follows_the_projects_scheme_convention() {
        // The renderer's EVENT_MAP maps its legacy `backend-warning` channel to
        // exactly this string; a rename here without one there re-creates the
        // very "live consumer, no emitter" gap this phase closed.
        assert_eq!(WARNING_EVENT, "backend://warning");
        assert!(WARNING_EVENT.contains("://"));
    }

    #[test]
    fn a_failure_context_carries_what_it_was_given() {
        let ctx = FailureCtx::now(
            "device_disconnected",
            "Enheten forsvant",
            FailureSource::Recording,
        );
        assert_eq!(ctx.code, "device_disconnected");
        assert_eq!(ctx.message, "Enheten forsvant");
        assert_eq!(ctx.source, FailureSource::Recording);
    }

    /// The listener registered by [`wire_failure_sources`] sees JSON, not the
    /// struct the engine emitted. A serde rename, an added `#[serde(rename_all)]`
    /// or a wrapper on either side would turn every recorder failure back into
    /// no alert at all — silently, because a failed parse is exactly what "no
    /// recorder failures happened" looks like. Pin the round-trip.
    #[test]
    fn the_listener_parses_exactly_what_the_engine_emits() {
        use crate::recorder::engine::{RecordingEvent, ERROR_EVENT};

        // What `engine::emit_error` puts on the wire, verbatim.
        let emitted = serde_json::to_string(&RecordingEvent {
            code: "device_disconnected".into(),
            message: "Lydenheten forsvant under opptak.".into(),
        })
        .expect("the engine's payload must serialise");

        // What the listener does with it.
        let parsed: RecordingEvent =
            serde_json::from_str(&emitted).expect("the listener must be able to parse it");
        let ctx = FailureCtx::now(parsed.code, parsed.message, FailureSource::Recording);

        assert_eq!(ctx.code, "device_disconnected");
        assert_eq!(ctx.message, "Lydenheten forsvant under opptak.");
        assert_eq!(ctx.source, FailureSource::Recording);
        // And the event we subscribe to is the terminal one, not the warning.
        assert_eq!(ERROR_EVENT, "recording://error");
    }

    /// The ledger key keeps the machine's timestamp legible and hashes the
    /// name somebody typed — and it is the SAME key earlier versions wrote, or
    /// every missed Sunday still in the window would be announced again.
    #[test]
    fn a_missed_slot_is_keyed_on_its_timestamp_and_a_hashed_label() {
        let slot = MissedSlot {
            at: "2026-09-06T11:00:00".into(),
            label: "Bryllup Kari og Ola".into(),
        };
        let key = slot.seen_key();
        assert!(
            key.starts_with("2026-09-06T11:00:00:"),
            "the machine's timestamp stays legible: {key}"
        );
        assert!(
            !key.contains("Kari") && !key.contains("Bryllup"),
            "a name somebody typed is hashed, not stored: {key}"
        );
        // 16 hex digits after the timestamp and its separator.
        let hash = &key["2026-09-06T11:00:00:".len()..];
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        // Same slot, same key; a different label, a different key.
        assert_eq!(key, slot.clone().seen_key());
        let other = MissedSlot {
            label: "Kveldsmesse".into(),
            ..slot
        };
        assert_ne!(key, other.seen_key());
    }
}
