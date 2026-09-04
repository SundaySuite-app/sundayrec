//! Notification ROUTING — pure, GUI-free, network-free.
//!
//! SundayRec has two ways to tell somebody that something went wrong:
//!
//!   1. a native OS notification (the volunteer is at the machine),
//!   2. an e-mail alert (the volunteer went home; [`crate::email`] renders it).
//!
//! (A third, a chat webhook POST, was removed with the sharing cluster.)
//!
//! Until now each *source* of trouble picked its own subset by hand — the
//! scheduler fired a native notification and nothing else, the recorder's
//! terminal error fired nothing at all, and the e-mail path had no callers
//! whatsoever. This module holds the ONE decision table those channels are
//! chosen from, so "who hears about a failure" is a unit-tested function of the
//! settings rather than an accident of which file the failure happened in.
//!
//! Everything here is a decision over already-gathered facts: no clock, no
//! keychain, no socket. The `src-tauri` `notify` module gathers the facts
//! (settings row, `cfg!(feature = "email")`, the [`crate::email::AlertGate`]
//! verdict, whether a transport could be built) and performs the side effects.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::email::MailLang;

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

    /// Every code above, in declaration order. The renderer's key table is
    /// checked against this list.
    pub const ALL: &[&str] = &[
        PREROLL_DEAD,
        RECOVERY_SKIPPED,
        DEVICE_MISSING,
        DISK_LOW,
        TRASH_MANIFEST_UNREADABLE,
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
}

impl FailureSource {
    /// Stable lowercase label used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureSource::Recording => "recording",
            FailureSource::Scheduler => "scheduler",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The routing matrix
// ─────────────────────────────────────────────────────────────────────────────

/// Which channels a dispatch will actually use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyPlan {
    /// Fire a native OS notification.
    pub native: bool,
    /// Send the e-mail alert.
    pub email: bool,
}

/// Everything the failure matrix decides on. All of it is already-gathered fact:
/// the settings row, a compile-time flag, and two yes/no answers the shell got
/// from the keychain and the throttle gate.
#[derive(Debug, Clone, Copy)]
pub struct FailureRouting<'a> {
    /// `settings.email_on_error` — the user asked to be told by e-mail.
    pub email_on_error: bool,
    /// `settings.email_address` — where to. Blank means nowhere.
    pub email_recipient: &'a str,
    /// `cfg!(feature = "email")` in the RUNNING build. A `--no-default-features`
    /// build has no transport compiled in, and must not pretend otherwise.
    pub email_feature_built: bool,
    /// The shell could assemble a transport (an SMTP host + a password). Without
    /// one there is nothing to send *with*, however willing the settings are.
    pub email_transport_ready: bool,
    /// The [`crate::email::AlertGate`] says this (recipient, error) pair was
    /// already mailed inside the throttle window.
    pub email_throttled: bool,
}

/// Decide which channels a FAILURE goes out on.
///
/// The native notification is unconditional: the operator standing at the
/// machine is the one person who can still fix Sunday's recording, and no
/// setting has ever been able to silence that. E-mail needs all five of its
/// conditions (asked for it, somewhere to send, a build that can send, a
/// transport to send with, and not already told a minute ago).
///
/// A WARNING has no matrix: it always (and only) goes to the renderer's toast
/// via the `backend://warning` event.
pub fn plan_failure(r: &FailureRouting) -> NotifyPlan {
    NotifyPlan {
        native: true,
        email: r.email_on_error
            && !r.email_recipient.trim().is_empty()
            && r.email_feature_built
            && r.email_transport_ready
            && !r.email_throttled,
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
//   Small shaping helpers for the alert bodies
// ─────────────────────────────────────────────────────────────────────────────

/// `chrono` format string for the human date in an alert e-mail, per language.
///
/// [`MailLang::locale`] gives the BCP-47 tag the Electron build handed to
/// `toLocaleDateString`; Rust has no CLDR, so this is the honest minimum: the
/// day-first forms Norwegian/Danish/Swedish/German/Polish readers expect with
/// dots, and the slashed variant for English/French. Never the US order — the
/// one arrangement that would be actively misread by every target audience.
pub fn alert_date_format(lang: MailLang) -> &'static str {
    match lang {
        MailLang::En | MailLang::Fr => "%d/%m/%Y %H:%M",
        _ => "%d.%m.%Y %H:%M",
    }
}

/// Who the alert greets. `settings.responsible_person` when set, else the
/// recipient address's local part (`vakt@kirka.no` → `vakt`), else the app name
/// — so the greeting is never the bare "Hei ," an empty profile used to produce.
pub fn alert_person(responsible: &str, recipient: &str) -> String {
    let responsible = responsible.trim();
    if !responsible.is_empty() {
        return responsible.to_string();
    }
    let local = recipient.trim().split('@').next().unwrap_or("").trim();
    if local.is_empty() {
        "SundayRec".to_string()
    } else {
        local.to_string()
    }
}

/// The church name for an alert subject/body, falling back to the app name when
/// the profile was never filled in (the Electron build's webhook had this
/// `untitled` guard and the mailer did not).
pub fn alert_church(church: &str) -> &str {
    let trimmed = church.trim();
    if trimmed.is_empty() {
        "SundayRec"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything on, nothing in the way: the maximal failure fan-out.
    fn all_on() -> FailureRouting<'static> {
        FailureRouting {
            email_on_error: true,
            email_recipient: "vakt@kirka.no",
            email_feature_built: true,
            email_transport_ready: true,
            email_throttled: false,
        }
    }

    // ── The failure matrix ───────────────────────────────────────────────────

    #[test]
    fn everything_configured_uses_every_channel() {
        assert_eq!(
            plan_failure(&all_on()),
            NotifyPlan {
                native: true,
                email: true,
            }
        );
    }

    #[test]
    fn the_native_notification_can_never_be_switched_off() {
        // The person at the machine is the only one who can still save the
        // service. Every other channel off, native still fires.
        let plan = plan_failure(&FailureRouting {
            email_on_error: false,
            email_recipient: "",
            email_feature_built: false,
            email_transport_ready: false,
            email_throttled: true,
        });
        assert_eq!(
            plan,
            NotifyPlan {
                native: true,
                email: false,
            }
        );
    }

    #[test]
    fn email_off_in_settings_suppresses_only_the_email() {
        let plan = plan_failure(&FailureRouting {
            email_on_error: false,
            ..all_on()
        });
        assert!(!plan.email);
        assert!(plan.native);
    }

    #[test]
    fn a_blank_recipient_suppresses_the_email() {
        assert!(
            !plan_failure(&FailureRouting {
                email_recipient: "",
                ..all_on()
            })
            .email
        );
        assert!(
            !plan_failure(&FailureRouting {
                email_recipient: "   ",
                ..all_on()
            })
            .email
        );
    }

    #[test]
    fn a_throttled_pair_suppresses_the_email_but_not_the_rest() {
        // Ten identical device drop-outs in ten minutes are one e-mail and ten
        // native notifications — the gate is about the inbox, not the screen.
        let plan = plan_failure(&FailureRouting {
            email_throttled: true,
            ..all_on()
        });
        assert!(!plan.email);
        assert!(plan.native);
    }

    #[test]
    fn a_build_without_the_email_feature_degrades_to_native() {
        // `--no-default-features`: the transport isn't compiled in. The plan
        // must say so rather than let the shell attempt a send that cannot exist.
        let plan = plan_failure(&FailureRouting {
            email_feature_built: false,
            ..all_on()
        });
        assert!(!plan.email);
        assert!(plan.native);
    }

    #[test]
    fn no_transport_means_no_email_however_willing_the_settings() {
        // Email switched on, recipient set, feature built — but no SMTP host.
        // Nothing to send with.
        assert!(
            !plan_failure(&FailureRouting {
                email_transport_ready: false,
                ..all_on()
            })
            .email
        );
    }

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

    // ── Body shaping ─────────────────────────────────────────────────────────

    #[test]
    fn the_date_format_is_never_the_us_order() {
        for lang in [
            MailLang::No,
            MailLang::En,
            MailLang::De,
            MailLang::Sv,
            MailLang::Da,
            MailLang::Pl,
            MailLang::Fr,
        ] {
            assert!(
                alert_date_format(lang).starts_with("%d"),
                "{lang:?} must be day-first"
            );
        }
        assert_eq!(alert_date_format(MailLang::En), "%d/%m/%Y %H:%M");
        assert_eq!(alert_date_format(MailLang::No), "%d.%m.%Y %H:%M");
    }

    #[test]
    fn the_greeting_falls_back_from_profile_to_address_to_app_name() {
        assert_eq!(alert_person("Ola", "vakt@kirka.no"), "Ola");
        assert_eq!(alert_person("  ", "vakt@kirka.no"), "vakt");
        assert_eq!(alert_person("", ""), "SundayRec");
        assert_eq!(alert_person("", "@kirka.no"), "SundayRec");
    }

    #[test]
    fn a_blank_church_never_leaves_a_dangling_subject() {
        assert_eq!(alert_church("Domkirken"), "Domkirken");
        assert_eq!(alert_church("  Domkirken "), "Domkirken");
        assert_eq!(alert_church(""), "SundayRec");
        assert_eq!(alert_church("   "), "SundayRec");
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
        assert_eq!(code::ALL.len(), 5);
    }

    #[test]
    fn failure_sources_have_stable_labels() {
        assert_eq!(FailureSource::Recording.as_str(), "recording");
        assert_eq!(FailureSource::Scheduler.as_str(), "scheduler");
    }
}
