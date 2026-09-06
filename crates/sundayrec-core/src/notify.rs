//! Notification ROUTING — pure, GUI-free, network-free.
//!
//! SundayRec has three ways to tell somebody that something went wrong:
//!
//!   1. a native OS notification (the volunteer is at the machine),
//!   2. an e-mail alert over the church's own SMTP server (the volunteer went
//!      home; [`crate::email`] renders it),
//!   3. the same e-mail over the SundaySuite relay ([`crate::relay`]), for the
//!      volunteer who has no SMTP server and no app password to type into one.
//!
//! (A fourth, a chat webhook POST, was removed with the sharing cluster.)
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
    /// A scheduled occurrence came and went with no recording at all.
    ///
    /// Not an *error* anybody saw happen — the absence of one. The machine was
    /// asleep, or the app was not running, and `check_missed` noticed afterwards
    /// that a slot had passed unrecorded. It routes through the same matrix as
    /// the other two because from the volunteer's side it is the same news
    /// ("Sunday was not recorded"), and because the alternative is what the app
    /// does today: `settings.rs` promises an e-mail that has never been sent.
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
//   The routing matrix
// ─────────────────────────────────────────────────────────────────────────────

/// Which channels a dispatch will actually use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyPlan {
    /// Fire a native OS notification.
    pub native: bool,
    /// Send the e-mail alert over the church's own SMTP server.
    pub email: bool,
    /// Queue the e-mail alert for the SundaySuite relay.
    ///
    /// Never `true` at the same time as [`Self::email`] — see [`plan_failure`].
    pub relay: bool,
}

/// The five facts the relay leg turns on, shared by [`FailureRouting`] and
/// [`ReceiptRouting`] so the two legs cannot drift apart. Private on purpose:
/// the public structs keep flat fields, because a call site that has to fill
/// `relay_suppressed` by name is a call site that had to think about it.
#[derive(Debug, Clone, Copy)]
struct RelayFacts {
    /// A subscription exists locally (`app_setting` `notify.relay`).
    enrolled: bool,
    /// …and the volunteer clicked the link in the confirmation mail.
    confirmed: bool,
    /// …and the endpoint has not since told us the address refuses mail
    /// (`410 recipient_suppressed`).
    suppressed: bool,
    /// The durable `notify_seen` table says this event was already relayed.
    throttled: bool,
    /// `SUNDAYREC_NOTIFY_URL` resolved to an endpoint in the RUNNING build.
    endpoint_built: bool,
}

impl RelayFacts {
    /// Whether the relay leg is usable at all, independent of *what* is being
    /// sent. All five, and the order they are written in is the order a
    /// volunteer meets them: sign up, confirm, keep working, not just told,
    /// have somewhere to send.
    fn open(self) -> bool {
        self.enrolled
            && self.confirmed
            && !self.suppressed
            && !self.throttled
            && self.endpoint_built
    }
}

/// Everything the failure matrix decides on. All of it is already-gathered fact:
/// the settings row, a compile-time flag, and yes/no answers the shell got from
/// the keychain, the throttle gate and the local subscription record.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailureRouting<'a> {
    /// `settings.email_on_error` — the user asked to be told by e-mail.
    ///
    /// ONE switch for both mail legs, deliberately. "Send me an e-mail when a
    /// recording fails" is the question the volunteer answered; which pipe the
    /// mail leaves through is not a second question they should have to answer.
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
    /// A relay subscription exists in `app_setting` `notify.relay`.
    pub relay_enrolled: bool,
    /// That subscription has been confirmed (the double opt-in link was
    /// clicked). An unconfirmed subscription may send its own subscribe row and
    /// nothing else — see [`crate::relay::relay_pump_decision`].
    pub relay_confirmed: bool,
    /// The endpoint answered `410 recipient_suppressed` for this address: it
    /// bounces or complained, and further sends would only hurt the domain's
    /// deliverability for every other church.
    pub relay_suppressed: bool,
    /// The durable `notify_seen` table says this exact event already went out —
    /// the relay's counterpart to [`Self::email_throttled`], and durable where
    /// [`crate::email::AlertGate`] is RAM (a restart must not re-send).
    pub relay_throttled: bool,
    /// An endpoint URL was compiled in / configured (`SUNDAYREC_NOTIFY_URL`).
    /// A build without one has nowhere to relay to, exactly as a build without
    /// the `email` feature has nothing to send with.
    pub relay_endpoint_built: bool,
}

impl FailureRouting<'_> {
    fn relay_facts(&self) -> RelayFacts {
        RelayFacts {
            enrolled: self.relay_enrolled,
            confirmed: self.relay_confirmed,
            suppressed: self.relay_suppressed,
            throttled: self.relay_throttled,
            endpoint_built: self.relay_endpoint_built,
        }
    }

    /// Whether a FULL SMTP transport exists for this alert — a recipient, a
    /// build that compiled the transport in, and a host+password in the
    /// keychain. Deliberately **excludes the throttle**: see [`plan_failure`].
    fn smtp_wins(&self) -> bool {
        !self.email_recipient.trim().is_empty()
            && self.email_feature_built
            && self.email_transport_ready
    }
}

/// Decide which channels a FAILURE goes out on.
///
/// The native notification is unconditional: the operator standing at the
/// machine is the one person who can still fix Sunday's recording, and no
/// setting has ever been able to silence that. E-mail needs all five of its
/// conditions (asked for it, somewhere to send, a build that can send, a
/// transport to send with, and not already told a minute ago) — unchanged, to
/// the letter, from before the relay existed.
///
/// ## The relay is a DERIVED leg, not a new setting
///
/// `email_on_error` keeps its meaning ("tell me by e-mail"). Which pipe carries
/// that mail is a consequence of what the machine has, not a third radio button
/// in a settings panel:
///
/// > **A configured SMTP server wins. The relay carries the rest.**
///
/// A church that already types its own mail server into SundayRec keeps sending
/// through it after this change, by construction rather than by migration —
/// there is no setting for them to lose, and `decideNotify`, the legacy
/// migration and the settings wire format are all untouched.
///
/// ## Why "wins" ignores the throttle
///
/// [`FailureRouting::smtp_wins`] is `recipient && feature_built &&
/// transport_ready` — everything the e-mail leg needs EXCEPT
/// [`FailureRouting::email_throttled`]. So a throttled SMTP alert does not fall
/// over to the relay: both legs stay shut for that failure.
///
/// That is the whole point of the throttle. [`crate::email::ALERT_THROTTLE_MS`]
/// protects an INBOX from a flapping recorder — ten identical device drop-outs
/// in ten minutes are one mail. If a suppressed send re-routed through the other
/// pipe, the same person would receive exactly the mail the gate had just
/// decided not to send them, and the gate would be decoration. The pipe is an
/// implementation detail of delivery; the throttle is a promise to a human.
///
/// The invariant that falls out — `!(email && relay)`, no failure ever produces
/// two e-mails — is pinned over the whole truth table in this module's tests.
///
/// A WARNING has no matrix: it always (and only) goes to the renderer's toast
/// via the `backend://warning` event.
pub fn plan_failure(r: &FailureRouting) -> NotifyPlan {
    NotifyPlan {
        native: true,
        email: r.email_on_error && r.smtp_wins() && !r.email_throttled,
        relay: r.email_on_error && !r.smtp_wins() && r.relay_facts().open(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The receipt matrix (relay only)
// ─────────────────────────────────────────────────────────────────────────────

/// Which channels a RECEIPT goes out on. One leg, and it is not a mistake that
/// the struct has a single field — see [`plan_receipt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptPlan {
    /// Queue the "the recording is finished" mail for the relay.
    pub relay: bool,
}

/// Everything the receipt matrix decides on: its own switch, plus the same five
/// relay facts [`FailureRouting`] carries.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiptRouting {
    /// `settings.email_receipt_enabled` — a SEPARATE switch from
    /// `email_on_error`, defaulting to off. "Tell me when something broke" and
    /// "tell me every single Sunday that nothing broke" are different appetites,
    /// and the second one is the shorter road to a filtered-away sender.
    pub receipt_enabled: bool,
    /// See [`FailureRouting::relay_enrolled`].
    pub relay_enrolled: bool,
    /// See [`FailureRouting::relay_confirmed`].
    pub relay_confirmed: bool,
    /// See [`FailureRouting::relay_suppressed`].
    pub relay_suppressed: bool,
    /// See [`FailureRouting::relay_throttled`] — for a receipt the durable
    /// `notify_seen` row is once-per-occurrence, full stop
    /// ([`crate::relay::seen_decision`]).
    pub relay_throttled: bool,
    /// See [`FailureRouting::relay_endpoint_built`].
    pub relay_endpoint_built: bool,
}

/// Decide whether the "your recording is finished" receipt is sent.
///
/// ## Why this is not a branch inside [`plan_failure`]
///
/// Two reasons, and both are about what the OTHER legs would do:
///
///   - **`native` is unconditional there.** That is right for a failure and
///     wrong for a receipt: a receipt is good news arriving at the exact moment
///     the volunteer is watching the app finish the recording — the app already
///     owns that surface (`notifyStop`). A second, OS-level "the recording is
///     finished" toast on top of it is noise about something the person just
///     watched happen. Routing a receipt through a table whose first row is
///     "always fire a native notification" could only produce that.
///   - **There is no SMTP leg, in v1.** A receipt is a *service* the relay
///     offers, not a promise the app has ever made over the user's own mail
///     server. Sending it through SMTP too would double the surface (a second
///     transport to debug, a second throttle to reason about) for a message
///     nobody has asked for yet. If it is ever wanted, it is one field here and
///     a test — not an unwind of a merged table.
///
/// So: separate function, separate fact struct, and the relay conditions
/// deliberately IDENTICAL to the failure matrix's, so an address that stopped
/// accepting mail stops receiving both kinds at once.
///
/// The caller narrows it further and this function cannot: receipts are for
/// SCHEDULED recordings only. A volunteer who pressed Start is standing there.
pub fn plan_receipt(r: &ReceiptRouting) -> ReceiptPlan {
    ReceiptPlan {
        relay: r.receipt_enabled
            && RelayFacts {
                enrolled: r.relay_enrolled,
                confirmed: r.relay_confirmed,
                suppressed: r.relay_suppressed,
                throttled: r.relay_throttled,
                endpoint_built: r.relay_endpoint_built,
            }
            .open(),
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

    /// The SMTP church: its own mail server configured, no relay subscription.
    /// This is the population the eight matrix tests below describe, and the
    /// state every existing user is in — which is why those eight are unchanged
    /// by the relay landing beside them. The relay fixture is [`relay_on`].
    fn all_on() -> FailureRouting<'static> {
        FailureRouting {
            email_on_error: true,
            email_recipient: "vakt@kirka.no",
            email_feature_built: true,
            email_transport_ready: true,
            email_throttled: false,
            ..FailureRouting::default()
        }
    }

    /// The volunteer with no mail server: nothing to send SMTP with, a confirmed
    /// relay subscription instead.
    fn relay_on() -> FailureRouting<'static> {
        FailureRouting {
            email_on_error: true,
            email_recipient: "",
            email_feature_built: true,
            email_transport_ready: false,
            email_throttled: false,
            relay_enrolled: true,
            relay_confirmed: true,
            relay_suppressed: false,
            relay_throttled: false,
            relay_endpoint_built: true,
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
                // …and NOT the relay: an SMTP church is not also a relay
                // church. `a_configured_smtp_server_wins_over_the_relay` is the
                // case where both are available at once.
                relay: false,
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
            relay_enrolled: false,
            relay_confirmed: false,
            relay_suppressed: true,
            relay_throttled: true,
            relay_endpoint_built: false,
        });
        assert_eq!(
            plan,
            NotifyPlan {
                native: true,
                email: false,
                relay: false,
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

    // ── The relay leg, next door to the eight above ──────────────────────────

    #[test]
    fn a_church_without_a_mail_server_gets_the_relay_instead() {
        // The whole point of the feature: no host, no app password, no
        // recipient typed anywhere — and the volunteer still gets the mail.
        assert_eq!(
            plan_failure(&relay_on()),
            NotifyPlan {
                native: true,
                email: false,
                relay: true,
            }
        );
    }

    #[test]
    fn a_configured_smtp_server_wins_over_the_relay() {
        // Both legs available at once. The church's own server carries it, so
        // an existing SMTP user's mail keeps arriving from their own domain —
        // by construction, with no migration and no setting to lose.
        let both = FailureRouting {
            relay_enrolled: true,
            relay_confirmed: true,
            relay_endpoint_built: true,
            ..all_on()
        };
        assert_eq!(
            plan_failure(&both),
            NotifyPlan {
                native: true,
                email: true,
                relay: false,
            }
        );
    }

    #[test]
    fn a_throttled_smtp_alert_does_not_fall_over_to_the_relay() {
        // THE decision this test exists to pin. `smtp_wins` deliberately
        // ignores `email_throttled`, so a flapping recorder cannot defeat
        // ALERT_THROTTLE_MS by leaving through the other pipe: the throttle is
        // a promise to an inbox, not a property of a transport. If it were part
        // of "wins", the tenth identical device drop-out in ten minutes would
        // arrive as a relay mail — exactly the mail the gate just decided not
        // to send.
        let plan = plan_failure(&FailureRouting {
            email_throttled: true,
            relay_enrolled: true,
            relay_confirmed: true,
            relay_endpoint_built: true,
            ..all_on()
        });
        assert!(!plan.email, "throttled");
        assert!(!plan.relay, "and NOT re-routed");
        assert!(plan.native);

        // The control, so the assertion above is not vacuous: the same routing
        // with the throttle lifted does send — over SMTP.
        let lifted = plan_failure(&FailureRouting {
            email_throttled: false,
            relay_enrolled: true,
            relay_confirmed: true,
            relay_endpoint_built: true,
            ..all_on()
        });
        assert!(lifted.email && !lifted.relay);
    }

    #[test]
    fn the_relay_needs_every_one_of_its_five_facts() {
        // Each fact alone is enough to shut the leg — and each of them is a
        // different real situation: never signed up, never clicked the link,
        // the address bounced, we already said this, no endpoint in this build.
        let cases: [(&str, FailureRouting); 5] = [
            (
                "not enrolled",
                FailureRouting {
                    relay_enrolled: false,
                    ..relay_on()
                },
            ),
            (
                "never confirmed",
                FailureRouting {
                    relay_confirmed: false,
                    ..relay_on()
                },
            ),
            (
                "address suppressed",
                FailureRouting {
                    relay_suppressed: true,
                    ..relay_on()
                },
            ),
            (
                "already relayed (notify_seen)",
                FailureRouting {
                    relay_throttled: true,
                    ..relay_on()
                },
            ),
            (
                "no endpoint compiled in",
                FailureRouting {
                    relay_endpoint_built: false,
                    ..relay_on()
                },
            ),
        ];
        for (why, r) in cases {
            let plan = plan_failure(&r);
            assert!(!plan.relay, "{why} must close the relay leg");
            assert!(plan.native, "{why} must not touch the native leg");
        }
    }

    #[test]
    fn the_relay_never_fires_for_a_user_who_did_not_ask_for_e_mail() {
        // `emailOnError` keeps its meaning across both pipes: it is the ONE
        // question the volunteer answered. A confirmed subscription is consent
        // to receive mail from us, not a request for alerts.
        let plan = plan_failure(&FailureRouting {
            email_on_error: false,
            ..relay_on()
        });
        assert!(!plan.relay && !plan.email && plan.native);
    }

    #[test]
    fn no_failure_ever_produces_two_e_mails() {
        // The invariant, over the WHOLE truth table rather than a handful of
        // cases: nine booleans × three recipient shapes = 1536 routings, and
        // not one of them lights both mail legs. This is what makes "SMTP wins"
        // a structural property instead of a comment.
        let mut both_seen = 0usize;
        let mut email_seen = 0usize;
        let mut relay_seen = 0usize;
        for bits in 0u32..(1 << 9) {
            let b = |i: u32| bits & (1 << i) != 0;
            for recipient in ["", "   ", "vakt@kirka.no"] {
                let r = FailureRouting {
                    email_on_error: b(0),
                    email_recipient: recipient,
                    email_feature_built: b(1),
                    email_transport_ready: b(2),
                    email_throttled: b(3),
                    relay_enrolled: b(4),
                    relay_confirmed: b(5),
                    relay_suppressed: b(6),
                    relay_throttled: b(7),
                    relay_endpoint_built: b(8),
                };
                let plan = plan_failure(&r);
                assert!(
                    !(plan.email && plan.relay),
                    "two e-mails for one failure: {r:?}"
                );
                assert!(plan.native, "the native leg survives {r:?}");
                both_seen += usize::from(plan.email || plan.relay);
                email_seen += usize::from(plan.email);
                relay_seen += usize::from(plan.relay);
            }
        }
        // Not vacuous: both legs do fire somewhere in that table.
        assert!(email_seen > 0 && relay_seen > 0);
        assert_eq!(
            both_seen,
            email_seen + relay_seen,
            "disjoint by construction"
        );
    }

    // ── The receipt matrix ───────────────────────────────────────────────────

    fn receipt_on() -> ReceiptRouting {
        ReceiptRouting {
            receipt_enabled: true,
            relay_enrolled: true,
            relay_confirmed: true,
            relay_suppressed: false,
            relay_throttled: false,
            relay_endpoint_built: true,
        }
    }

    #[test]
    fn a_receipt_is_a_relay_only_message() {
        // The type says it: `ReceiptPlan` has no `native` and no `email` field,
        // so "a receipt fires a desktop notification" is not a bug that can be
        // written here. `notifyStop` owns the on-screen half; a second OS toast
        // about something the volunteer just watched finish is noise.
        assert_eq!(plan_receipt(&receipt_on()), ReceiptPlan { relay: true });
    }

    #[test]
    fn the_receipt_switch_is_its_own() {
        // Default-off, and independent of `email_on_error`: "tell me when
        // something broke" is a different appetite from "tell me every Sunday
        // that nothing broke".
        assert!(!plan_receipt(&ReceiptRouting::default()).relay);
        assert!(
            !plan_receipt(&ReceiptRouting {
                receipt_enabled: false,
                ..receipt_on()
            })
            .relay
        );
    }

    #[test]
    fn both_matrices_read_the_relay_facts_the_same_way() {
        // An address that stopped accepting mail stops receiving BOTH kinds at
        // once, and a build with no endpoint sends neither. Proven over all 32
        // combinations of the five facts rather than asserted in prose.
        for bits in 0u32..(1 << 5) {
            let b = |i: u32| bits & (1 << i) != 0;
            let failure = plan_failure(&FailureRouting {
                relay_enrolled: b(0),
                relay_confirmed: b(1),
                relay_suppressed: b(2),
                relay_throttled: b(3),
                relay_endpoint_built: b(4),
                ..relay_on()
            });
            let receipt = plan_receipt(&ReceiptRouting {
                receipt_enabled: true,
                relay_enrolled: b(0),
                relay_confirmed: b(1),
                relay_suppressed: b(2),
                relay_throttled: b(3),
                relay_endpoint_built: b(4),
            });
            assert_eq!(
                failure.relay, receipt.relay,
                "the two legs disagree at bits {bits:05b}"
            );
        }
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
