//! Notification decisions — pure, GUI-free, network-free.
//!
//! SundayRec tells people that something went wrong in exactly two places, and
//! both are on the machine itself:
//!
//!   1. a native OS notification — terminal failures (a recording died, a
//!      scheduled one could not start, a scheduled one never happened). The
//!      person standing at the machine is the one who can still save the
//!      service, so no setting silences these;
//!   2. an in-app banner — the non-fatal degradations ([`BackendWarning`]).
//!
//! (E-mail used to be a third: an SMTP alerter and the SundaySuite relay. Both
//! were removed — the setup was too heavy for a volunteer, and the app no
//! longer sends anything off the machine to tell someone about a failure. A
//! chat webhook before them went with the sharing cluster. All of it is in git.)
//!
//! Everything here is a decision over already-gathered facts: no clock, no
//! keychain, no socket. The `src-tauri` `notify` module gathers the facts and
//! performs the side effects.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─────────────────────────────────────────────────────────────────────────────
//   Stable warning codes
// ─────────────────────────────────────────────────────────────────────────────

/// The stable `code` values [`BackendWarning`] carries. The renderer localises
/// on these (a `notify.*` key per code) and falls back to
/// [`BackendWarning::msg`] when it doesn't recognise one, so adding a code here
/// degrades to the backend's own wording rather than to silence.
///
/// They live in the core (not in the emitting module) because the renderer's
/// key table and the Rust emitters must agree, and a constant both sides can be
/// tested against is the only way that agreement is checkable.
pub mod code {
    /// The pre-roll capture loop has been failing to open the device for long
    /// enough that the rolling buffer is effectively dead — the Home chip
    /// otherwise cannot tell "off" from "broken".
    pub const PREROLL_DEAD: &str = "preroll_dead";
    /// Crash recovery skipped a session/file instead of salvaging it.
    pub const RECOVERY_SKIPPED: &str = "recovery_skipped";
    /// The audio device named in settings was not among the enumerated inputs
    /// at preflight time.
    pub const DEVICE_MISSING: &str = "device_missing";
    /// Free space on the save volume fell below the GRADUATED warning threshold
    /// — well above the engine's terminal stop threshold, so this is a nudge
    /// while there is still time to act, not the emergency stop.
    pub const DISK_LOW: &str = "disk_low";
    /// The Papirkurv's `manifest.json` was there but could not be read, so it
    /// was renamed aside and the list rebuilt from empty. The FILES are
    /// untouched — they are still in the trash directory — but the app can no
    /// longer say where each one came from, which is exactly the thing a
    /// volunteer needs to hear before they conclude a recording is gone.
    pub const TRASH_MANIFEST_UNREADABLE: &str = "trash_manifest_unreadable";
    /// Windows only (F-W10): moving the database from Roaming to Local AppData
    /// failed, so this session runs on the old copy. Nothing is lost; the move
    /// is tried again at the next start. Raised once per install.
    pub const DATA_DIR_MOVE_FAILED: &str = "data_dir_move_failed";
    /// Windows only (F-W10): the database lives in Local AppData, but the old
    /// Roaming one was written to AFTER it — a downgraded version ran in
    /// between. What that version saved stays in the old folder. Raised once
    /// per such session.
    pub const DATA_LEFT_IN_OLD_DIR: &str = "data_left_in_old_dir";

    /// Every code above, in declaration order. The renderer's key table is
    /// checked against this list.
    pub const ALL: &[&str] = &[
        PREROLL_DEAD,
        RECOVERY_SKIPPED,
        DEVICE_MISSING,
        DISK_LOW,
        TRASH_MANIFEST_UNREADABLE,
        DATA_DIR_MOVE_FAILED,
        DATA_LEFT_IN_OLD_DIR,
    ];
}

// ─────────────────────────────────────────────────────────────────────────────
//   The live warning channel (backend → renderer)
// ─────────────────────────────────────────────────────────────────────────────

/// How loud a [`BackendWarning`] is. Serialised lowercase to match the
/// renderer's `'warn' | 'error'` union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WarnSeverity.ts")]
#[serde(rename_all = "lowercase")]
pub enum WarnSeverity {
    /// Something is degraded; the recording can still happen.
    Warn,
    /// Something is broken and needs attention.
    Error,
}

/// A non-fatal observation the backend wants on screen NOW.
///
/// The renderer localises on [`Self::code`] (a `notify.*` key) and interpolates
/// [`Self::params`]; [`Self::msg`] is the backend's own wording, used verbatim
/// when the code is unknown to this renderer build. That ordering matters: a
/// backend that learns a new warning before the renderer does still says
/// something true instead of nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "BackendWarning.ts")]
#[serde(rename_all = "camelCase")]
pub struct BackendWarning {
    /// Stable snake_case code — see [`code`].
    pub code: String,
    /// Human-readable fallback (Norwegian), or `None` to rely on the code alone.
    pub msg: Option<String>,
    /// Toast severity.
    pub severity: WarnSeverity,
    /// Interpolation values for the localized string (`{file}`, `{device}`, …).
    #[serde(default)]
    pub params: HashMap<String, String>,
}

impl BackendWarning {
    /// A `warn`-severity warning with no params.
    pub fn warn(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            msg: None,
            severity: WarnSeverity::Warn,
            params: HashMap::new(),
        }
    }

    /// An `error`-severity warning with no params.
    pub fn error(code: impl Into<String>) -> Self {
        Self {
            severity: WarnSeverity::Error,
            ..Self::warn(code)
        }
    }

    /// Attach the backend's own wording (the renderer's fallback).
    pub fn msg(mut self, msg: impl Into<String>) -> Self {
        self.msg = Some(msg.into());
        self
    }

    /// Attach one interpolation value.
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Where a failure came from
// ─────────────────────────────────────────────────────────────────────────────

/// Which part of the app produced a failure. Carried on the dispatch context so
/// the log says where to look, and so future routing can differ per
/// source without changing the call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureSource {
    /// The recorder engine's terminal error (`recording://error`).
    Recording,
    /// The scheduler could not start / prepare a scheduled recording.
    Scheduler,
    /// A scheduled occurrence came and went with no recording at all.
    ///
    /// Not an *error* anybody saw happen — the absence of one. The machine was
    /// asleep, or the app was not running, and `check_missed` noticed afterwards
    /// that a slot had passed unrecorded. It takes the same native path as the
    /// other two because from the volunteer's side it is the same news
    /// ("Sunday was not recorded").
    Missed,
}

impl FailureSource {
    /// Stable lowercase label used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureSource::Recording => "recording",
            FailureSource::Scheduler => "scheduler",
            FailureSource::Missed => "missed",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Once-semantics for the repeating observers
// ─────────────────────────────────────────────────────────────────────────────

/// How many consecutive pre-roll failures (no device / spawn error) count as
/// "the rolling buffer is dead" rather than "a device blipped".
///
/// The loop's own back-off ([`crate::preroll::preroll_restart_delay`]) already
/// ramps, so by the third consecutive failure we are seconds in with nothing
/// captured — early enough to matter before Sunday, late enough that unplugging
/// a USB mixer for a moment does not raise an alarm.
pub const PREROLL_DEAD_AFTER_ATTEMPTS: u32 = 3;

/// Whether THIS pre-roll back-off should raise [`code::PREROLL_DEAD`].
///
/// `attempt` is the failure counter the loop keeps (0 on the first failure of a
/// streak, reset to 0 by a successful spawn); `already_warned` is whether this
/// streak has already spoken. One warning per give-up streak — a loop that
/// retries every few seconds for an hour must not produce an hour of toasts.
pub fn should_warn_preroll_dead(attempt: u32, already_warned: bool) -> bool {
    !already_warned && attempt + 1 >= PREROLL_DEAD_AFTER_ATTEMPTS
}

/// Graduated low-disk warning for an AUDIO recording: 2 GB.
///
/// Deliberately far above the engine's terminal threshold
/// ([`crate::preflight::MIN_DISK_AUDIO_BYTES`], 500 MB) at which it stops the
/// take to finalise a playable file. This one is a nudge with time left to
/// clear space; that one is the emergency brake. They are separate numbers on
/// purpose and the engine's is not touched here.
pub const DISK_WARN_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Graduated low-disk warning for a VIDEO recording: 8 GB (the engine's
/// terminal threshold is 4 GB — see [`DISK_WARN_AUDIO_BYTES`]).
pub const DISK_WARN_VIDEO_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The graduated warning threshold for the capture mode in play.
pub fn disk_warn_threshold_bytes(video_active: bool) -> u64 {
    if video_active {
        DISK_WARN_VIDEO_BYTES
    } else {
        DISK_WARN_AUDIO_BYTES
    }
}

/// Whether the disk observer should raise [`code::DISK_LOW`] now. Once per
/// recording session: `already_warned` is reset when a take starts, not when
/// free space recovers, so a disk hovering at the threshold cannot produce a
/// toast every 60 s for an hour.
pub fn should_warn_low_disk(free_bytes: u64, video_active: bool, already_warned: bool) -> bool {
    !already_warned && free_bytes < disk_warn_threshold_bytes(video_active)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Native notifications during a take
// ─────────────────────────────────────────────────────────────────────────────

/// The degradations during a recording that also reach the OS notification
/// centre — not only the in-app banner — when nobody is looking at the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TakeAlert {
    /// The silence watcher tripped (`recording://silence`).
    Silence,
    /// The quality alarm: the take has far less audio than it should
    /// (`recording://quality`).
    Quality,
    /// The input dropped out and the engine is reconnecting
    /// (`recording://reconnecting`).
    Reconnecting,
    /// Free space fell below the graduated warning threshold.
    DiskLow,
}

/// Whether THIS degradation should also raise a native notification.
///
/// Two conditions, both deliberate:
///
///   - **Not while the window has focus.** An operator who is looking at
///     SundayRec already sees the banner; a toast on top of it is the same
///     news twice. Hidden, minimised or behind another app is exactly when the
///     banner is invisible and the notification is the only way to hear it.
///   - **Once per take per kind.** A device that flaps, or a silence that comes
///     and goes during a long prayer, must not produce a stream of toasts.
///     `already_sent` is reset when a new take starts, not when the condition
///     clears.
pub fn should_native_during_take(window_focused: bool, already_sent: bool) -> bool {
    !window_focused && !already_sent
}

// ─────────────────────────────────────────────────────────────────────────────
//   "Have we already said this?"
// ─────────────────────────────────────────────────────────────────────────────

/// Which once-policy applies to an event — the `scope` column of the durable
/// `notify_seen` table.
///
/// One scope today. The table was built with three (`failure`, `receipt` and
/// this one) for the e-mail relay; only the missed-recording notice outlived
/// it, and its rows are keyed `missed`, so that label must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenScope {
    /// A scheduled occurrence that was never recorded. Names a moment in the
    /// past that cannot happen twice.
    Missed,
}

impl SeenScope {
    /// Stable lowercase label — the `scope` column of `notify_seen`.
    pub fn as_str(self) -> &'static str {
        match self {
            SeenScope::Missed => "missed",
        }
    }
}

/// Whether this event should be SUPPRESSED because it has already been reported.
///
/// `true` means "do not tell anyone again". [`SeenScope::Missed`] keys a single
/// occurrence in time, so it is ONCE, full stop: any recorded sighting
/// suppresses. `check_missed` runs at startup and after every wake, so a durable
/// row is the only thing standing between one unrecorded Sunday and a fresh
/// native notification about it on every launch.
pub fn seen_decision(scope: SeenScope, last_seen_ms: Option<i64>, _now_ms: i64) -> bool {
    match (scope, last_seen_ms) {
        (_, None) => false,
        (SeenScope::Missed, Some(_)) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Once-semantics ───────────────────────────────────────────────────────

    #[test]
    fn preroll_stays_quiet_for_the_first_couple_of_retries() {
        assert!(!should_warn_preroll_dead(0, false));
        assert!(!should_warn_preroll_dead(1, false));
    }

    #[test]
    fn preroll_speaks_once_when_the_streak_is_real() {
        assert!(should_warn_preroll_dead(2, false));
        // …and then never again for the same streak, however long it runs.
        assert!(!should_warn_preroll_dead(2, true));
        assert!(!should_warn_preroll_dead(99, true));
    }

    #[test]
    fn the_graduated_disk_thresholds_sit_above_the_engines_terminal_ones() {
        use crate::preflight::{MIN_DISK_AUDIO_BYTES, MIN_DISK_VIDEO_BYTES};
        // If these ever crossed, the "you are running low" nudge would arrive
        // after the recording had already been stopped for being out of space.
        // Const-block asserts: this is a relationship between four constants, so
        // the compiler — not the test runner — is the right thing to enforce it.
        const {
            assert!(DISK_WARN_AUDIO_BYTES > MIN_DISK_AUDIO_BYTES);
            assert!(DISK_WARN_VIDEO_BYTES > MIN_DISK_VIDEO_BYTES);
        }
        assert_eq!(disk_warn_threshold_bytes(false), DISK_WARN_AUDIO_BYTES);
        assert_eq!(disk_warn_threshold_bytes(true), DISK_WARN_VIDEO_BYTES);
    }

    #[test]
    fn disk_warns_once_per_session_below_the_threshold() {
        let gb = 1024 * 1024 * 1024;
        assert!(should_warn_low_disk(gb, false, false));
        assert!(!should_warn_low_disk(gb, false, true));
        // 3 GB is fine for audio but not for video.
        assert!(!should_warn_low_disk(3 * gb, false, false));
        assert!(should_warn_low_disk(3 * gb, true, false));
    }

    // ── Native during a take ─────────────────────────────────────────────────

    #[test]
    fn a_take_alert_reaches_the_os_only_when_nobody_is_looking() {
        assert!(should_native_during_take(false, false));
        assert!(
            !should_native_during_take(true, false),
            "the banner is on screen — no second copy"
        );
    }

    #[test]
    fn a_take_alert_is_said_once_per_take() {
        assert!(!should_native_during_take(false, true));
        assert!(!should_native_during_take(true, true));
    }

    // ── Seen ledger ──────────────────────────────────────────────────────────

    #[test]
    fn an_unseen_missed_occurrence_is_never_suppressed() {
        assert!(!seen_decision(SeenScope::Missed, None, 1_800_000_000_000));
    }

    #[test]
    fn a_missed_occurrence_is_reported_once_and_for_all() {
        let seen = 1_800_000_000_000;
        for now in [seen, seen + 60_000, seen + 30 * 86_400_000, 0] {
            assert!(
                seen_decision(SeenScope::Missed, Some(seen), now),
                "a sighting suppresses at any distance ({now})"
            );
        }
    }

    #[test]
    fn the_missed_scope_label_matches_the_rows_already_on_disk() {
        // Rows written by v0.20 and earlier carry `missed`; renaming the label
        // would re-announce every missed Sunday still in the look-back window.
        assert_eq!(SeenScope::Missed.as_str(), "missed");
    }

    // ── The wire shapes ──────────────────────────────────────────────────────

    #[test]
    fn a_warning_serialises_to_the_camel_case_shape_the_renderer_reads() {
        let w = BackendWarning::warn(code::DISK_LOW)
            .msg("Lite plass igjen")
            .param("freeBytes", "1073741824");
        let json = serde_json::to_string(&w).expect("serialise");
        assert!(json.contains("\"code\":\"disk_low\""));
        assert!(json.contains("\"severity\":\"warn\""));
        assert!(json.contains("\"msg\":\"Lite plass igjen\""));
        assert!(json.contains("\"freeBytes\":\"1073741824\""));
        let back: BackendWarning = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back, w);
    }

    #[test]
    fn every_code_is_snake_case_and_listed_exactly_once() {
        // `code::ALL` is what the renderer's key table is checked against; a code
        // that exists but isn't listed would be a warning nobody can localise.
        let mut seen = std::collections::HashSet::new();
        for c in code::ALL {
            assert!(seen.insert(*c), "{c} listed twice");
            assert!(
                c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
                "{c} is not snake_case"
            );
        }
        assert_eq!(code::ALL.len(), 7);
    }

    #[test]
    fn failure_sources_have_stable_labels() {
        assert_eq!(FailureSource::Recording.as_str(), "recording");
        assert_eq!(FailureSource::Scheduler.as_str(), "scheduler");
        assert_eq!(FailureSource::Missed.as_str(), "missed");
        // The label and the serialised form are the same word, so a log line
        // and a stored context cannot describe the same failure differently.
        for source in [
            FailureSource::Recording,
            FailureSource::Scheduler,
            FailureSource::Missed,
        ] {
            let json = serde_json::to_string(&source).expect("serialise");
            assert_eq!(json, format!("\"{}\"", source.as_str()));
            let back: FailureSource = serde_json::from_str(&json).expect("round-trip");
            assert_eq!(back, source);
        }
    }
}
