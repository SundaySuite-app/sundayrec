//! The telemetry consent state machine — pure, and default-closed.
//!
//! ## Why "off" is not one state but two
//!
//! The obvious model is a boolean, and it is wrong. A boolean cannot tell
//! "this user was asked and said no" apart from "this user has never been
//! asked", and those two demand opposite behaviour from the UI: the first must
//! be left alone forever, the second must be asked exactly once. SundayRec has
//! shipped since v0.1, so most installs already have `onboardingDone: true` and
//! will never see an onboarding step again — a boolean would silently mean
//! "no" for every one of them, and the feature would only ever reach people who
//! install the app fresh.
//!
//! So the persisted record is `Option<ConsentRecord>` and the derived state is a
//! three-value [`ConsentStatus`]:
//!
//! ```text
//!                     ┌──────────────┐
//!    no record  ────►  │  NeverAsked  │  needsPrompt = true   active = false
//!                     └──────┬───────┘
//!                            │  telemetry_consent_set(true|false)
//!               ┌────────────┴────────────┐
//!               ▼                         ▼
//!        ┌─────────────┐           ┌────────────┐
//!        │  Granted(v) │           │  Denied(v) │
//!        └─────────────┘           └────────────┘
//!    v == CURRENT:                 v == CURRENT:
//!      needsPrompt = false           needsPrompt = false
//!      active      = true            active      = false
//!
//!    v <  CURRENT  (the scope was widened since they answered):
//!      needsPrompt = true            needsPrompt = true
//!      active      = FALSE           active      = false
//! ```
//!
//! ## The two rules that make it safe
//!
//! 1. **Absent means no.** A missing record, an unparseable record, a record
//!    from a version we do not understand — all of them are "not granted".
//!    Nothing about telemetry is ever the default-on branch of a condition.
//! 2. **A stale grant is not a grant.** If [`CONSENT_VERSION`] is bumped because
//!    the payload started carrying something new, a user who consented to the
//!    OLD scope has consented to something that no longer exists. Sending stops
//!    (`active = false`) and the prompt returns. That is the direction the
//!    failure has to fall: a paused report costs a datapoint, a widened one
//!    costs the promise the privacy text made.
//!
//! Everything here is pure: the shell reads/writes the record, mints the random
//! install id, and calls these functions to decide what any of it means.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The version of the consent SCOPE — bumped when the payload starts carrying a
/// category of data the user was not asked about.
///
/// Deliberately separate from [`super::TELEMETRY_SCHEMA`]: adding a field within
/// a category the user already agreed to (one more counter, one more number on a
/// quality record) is a schema change, not a scope change, and must not re-prompt
/// anyone. Sending a NEW CATEGORY is a scope change and must.
///
/// Bumping this is never a lone edit. Three things move together or the version
/// is a lie: this constant, the scope described in `PRIVACY.md`, and the
/// re-prompt copy (`onboarding.rePrompt*`) that has to name what was added —
/// otherwise a user who already said yes is asked a question they cannot tell
/// apart from the one they already answered.
///
/// - **v1** — crashes, quality verdicts, feature counters, technical settings.
/// - **v2** — quality data may additionally carry COARSE SIZE BANDS for editor
///   corrections ("the sermon start was moved 30–60 seconds earlier"), not only
///   a count of them. A band is a magnitude, and v1 promised counts only, so it
///   is a new category no matter how anonymous it is.
pub const CONSENT_VERSION: u32 = 2;

/// The persisted consent record. Absent from storage = never asked.
///
/// Versioned so a future scope expansion can re-prompt rather than assume; and
/// timestamped so the privacy text can honestly say when the choice was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRecord {
    /// What the user answered.
    pub granted: bool,
    /// The [`CONSENT_VERSION`] they answered FOR.
    pub version: u32,
    /// Unix ms (UTC) they answered.
    pub decided_at: i64,
}

impl ConsentRecord {
    /// Record an answer at the CURRENT scope version.
    pub fn decide(granted: bool, now_ms: i64) -> Self {
        Self {
            granted,
            version: CONSENT_VERSION,
            decided_at: now_ms,
        }
    }
}

/// The three states the UI has to tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ConsentStatus.ts")]
#[serde(rename_all = "kebab-case")]
pub enum ConsentStatus {
    /// No answer has ever been recorded. NOT the same as "no" — see the module
    /// docs. This is the state every pre-E3 install starts in, including the
    /// ones that finished onboarding years ago.
    NeverAsked,
    /// The user said yes.
    Granted,
    /// The user said no. Left alone; never re-asked unless the SCOPE changes.
    Denied,
}

/// The full consent picture, as the settings UI and the onboarding step need it.
///
/// `status` + `version` are the facts; `needs_prompt` and `active` are the two
/// derived questions callers actually ask, computed here so no caller has to
/// re-implement the "a stale grant is not a grant" rule and get it subtly wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TelemetryConsent.ts")]
#[serde(rename_all = "camelCase")]
pub struct TelemetryConsent {
    pub status: ConsentStatus,
    /// The scope version the recorded answer was given for. 0 when never asked.
    pub version: u32,
    /// Unix ms (UTC) the answer was recorded, when there is one.
    #[ts(type = "number | null")]
    pub decided_at: Option<i64>,
    /// [`CONSENT_VERSION`] — what an answer given NOW would be recorded as.
    pub current_version: u32,
    /// Whether the UI should ask. True when never asked, and true again when the
    /// scope has widened since the recorded answer — for a `Denied` user too,
    /// because "no" was an answer to a different question.
    pub needs_prompt: bool,
    /// Whether telemetry may be collected and sent RIGHT NOW. The only flag the
    /// backend consults; false in every state except a current-version grant.
    pub active: bool,
}

/// Derive the full picture from the persisted record (or its absence).
///
/// The single place the state machine lives. `None` in, `NeverAsked` out — an
/// unreadable or absent record is never a grant.
pub fn evaluate(record: Option<ConsentRecord>) -> TelemetryConsent {
    let Some(r) = record else {
        return TelemetryConsent {
            status: ConsentStatus::NeverAsked,
            version: 0,
            decided_at: None,
            current_version: CONSENT_VERSION,
            needs_prompt: true,
            active: false,
        };
    };
    // A record from a FUTURE version (a downgrade after the user answered a
    // wider question) is also stale: we cannot know that the answer covers what
    // this build sends, so it is not current and does not grant.
    let current_scope = r.version == CONSENT_VERSION;
    TelemetryConsent {
        status: if r.granted {
            ConsentStatus::Granted
        } else {
            ConsentStatus::Denied
        },
        version: r.version,
        decided_at: Some(r.decided_at),
        current_version: CONSENT_VERSION,
        needs_prompt: !current_scope,
        active: r.granted && current_scope,
    }
}

/// Parse a persisted record from its stored JSON. Any failure — absent,
/// malformed, truncated, hand-edited — is [`None`], i.e. never asked, i.e. off.
pub fn parse_record(raw: Option<&str>) -> Option<ConsentRecord> {
    serde_json::from_str::<ConsentRecord>(raw?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absence_is_never_asked_and_never_active() {
        let c = evaluate(None);
        assert_eq!(c.status, ConsentStatus::NeverAsked);
        assert!(
            c.needs_prompt,
            "an install that was never asked must be asked"
        );
        assert!(!c.active, "the default is OFF");
        assert_eq!(c.version, 0);
        assert_eq!(c.decided_at, None);
        assert_eq!(c.current_version, CONSENT_VERSION);
    }

    #[test]
    fn a_current_grant_is_the_only_active_state() {
        let c = evaluate(Some(ConsentRecord::decide(true, 1_800_000_000_000)));
        assert_eq!(c.status, ConsentStatus::Granted);
        assert!(c.active);
        assert!(!c.needs_prompt);
        assert_eq!(c.decided_at, Some(1_800_000_000_000));
        assert_eq!(c.version, CONSENT_VERSION);
    }

    #[test]
    fn a_denial_is_remembered_and_not_re_asked() {
        let c = evaluate(Some(ConsentRecord::decide(false, 1_800_000_000_000)));
        assert_eq!(c.status, ConsentStatus::Denied);
        assert!(!c.active);
        assert!(
            !c.needs_prompt,
            "'no' at the current scope means leave the user alone"
        );
    }

    #[test]
    fn a_stale_grant_stops_sending_and_re_asks() {
        // The rule that makes a scope expansion safe: consent to the OLD scope
        // is not consent to the new one, so sending stops immediately.
        let stale = ConsentRecord {
            granted: true,
            version: CONSENT_VERSION - 1,
            decided_at: 1,
        };
        let c = evaluate(Some(stale));
        assert_eq!(c.status, ConsentStatus::Granted, "the answer is remembered");
        assert!(!c.active, "but it no longer permits sending");
        assert!(c.needs_prompt, "and the user is asked the new question");
    }

    #[test]
    fn a_stale_denial_is_also_re_asked() {
        let stale = ConsentRecord {
            granted: false,
            version: CONSENT_VERSION - 1,
            decided_at: 1,
        };
        let c = evaluate(Some(stale));
        assert!(c.needs_prompt, "'no' answered a different question");
        assert!(!c.active);
    }

    // ── The v1 → v2 widening (E8.D1) ─────────────────────────────────────────
    //
    // The tests above are written relative to CONSENT_VERSION, so they keep
    // testing the RULE after any future bump. These are written against the
    // literal version 1, because version 1 is what is on real disks right now:
    // they are about the migration that actually happens, not about the rule.

    #[test]
    fn version_two_is_the_current_scope() {
        // A tripwire, not a tautology. Changing this number changes what every
        // installed copy is allowed to send, so it must not be possible to do
        // it as an incidental edit — see CONSENT_VERSION's docs for the two
        // other things that have to move with it.
        assert_eq!(CONSENT_VERSION, 2);
    }

    /// CONSENT_VERSION's own docs say three things move together or the version
    /// is a lie: this constant, the scope described in `PRIVACY.md`, and the
    /// re-prompt copy. Nothing enforced the middle one, and it is the one a
    /// church actually reads — so a bump that left the document behind was a
    /// silent, and entirely plausible, way to publish a false promise.
    ///
    /// This does not check that the PROSE is right; no test can. It checks that
    /// the document states which scope it describes, and that the number it
    /// states is this one. A bump now fails here until somebody opens the file,
    /// which is exactly the moment to notice the categories are stale too.
    #[test]
    fn the_privacy_document_states_the_scope_it_describes() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../PRIVACY.md");
        let doc = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("PRIVACY.md must be readable at {path}: {e}"));
        let expected = format!("**versjon {CONSENT_VERSION}**");
        assert!(
            doc.contains(&expected),
            "PRIVACY.md must name the consent scope it describes as {expected}. \
             CONSENT_VERSION is {CONSENT_VERSION}; either the document was not \
             updated with the bump, or the wording that carries the number moved."
        );
    }

    #[test]
    fn a_version_one_grant_stops_sending_and_is_asked_again() {
        let v1_yes = ConsentRecord {
            granted: true,
            version: 1,
            decided_at: 1_800_000_000_000,
        };
        let c = evaluate(Some(v1_yes));
        assert!(
            !c.active,
            "yes-to-counts is not yes-to-bands: sending must stop the moment \
             this build starts up, before anything wider is collected"
        );
        assert!(c.needs_prompt, "and the wider question must be put to them");
        assert_eq!(c.version, 1, "what they answered is still on the record");
        assert_eq!(c.current_version, 2, "what they would answer now");
    }

    #[test]
    fn a_version_one_denial_never_becomes_a_version_two_grant() {
        // The failure that would matter most, and the quietest one: a bump that
        // re-derived "answered" as "agreed" would turn every recorded NO into a
        // YES for a wider scope than the first question even had.
        let v1_no = ConsentRecord {
            granted: false,
            version: 1,
            decided_at: 1_800_000_000_000,
        };
        let c = evaluate(Some(v1_no));
        assert_eq!(c.status, ConsentStatus::Denied, "'no' is still 'no'");
        assert!(!c.active, "a denial can never be upgraded into permission");
        assert!(
            c.needs_prompt,
            "they are asked once more only because the question changed"
        );
    }

    #[test]
    fn no_recorded_denial_is_active_at_any_version() {
        // The sweep the two tests above are instances of: `granted: false` has
        // no version at which it permits sending, so no future bump can find a
        // seam where a denial leaks through.
        for version in 0..=CONSENT_VERSION + 2 {
            let c = evaluate(Some(ConsentRecord {
                granted: false,
                version,
                decided_at: 1,
            }));
            assert!(!c.active, "denial at version {version} permitted sending");
            assert_eq!(c.status, ConsentStatus::Denied, "version {version}");
        }
    }

    #[test]
    fn answering_the_v2_question_records_v2_and_resumes() {
        // The far side of the re-prompt: the new answer has to actually clear
        // the prompt, or the card returns every launch and reads as a bug.
        let yes = evaluate(Some(ConsentRecord::decide(true, 1_800_000_000_001)));
        assert_eq!(yes.version, 2);
        assert!(yes.active);
        assert!(!yes.needs_prompt);

        let no = evaluate(Some(ConsentRecord::decide(false, 1_800_000_000_001)));
        assert_eq!(no.version, 2);
        assert!(!no.active);
        assert!(!no.needs_prompt, "a fresh 'no' is not re-litigated either");
    }

    #[test]
    fn a_record_from_the_future_does_not_grant() {
        // A downgrade after answering a wider question. We cannot know the
        // answer covers what THIS build sends.
        let future = ConsentRecord {
            granted: true,
            version: CONSENT_VERSION + 1,
            decided_at: 1,
        };
        let c = evaluate(Some(future));
        assert!(!c.active);
        assert!(c.needs_prompt);
    }

    #[test]
    fn a_malformed_record_reads_as_never_asked() {
        for raw in [
            None,
            Some(""),
            Some("null"),
            Some("{"),
            Some("{\"granted\":true}"), // missing version/decidedAt
            Some("\"granted\""),
            Some("{\"granted\":\"yes\",\"version\":1,\"decidedAt\":0}"),
        ] {
            let parsed = parse_record(raw);
            assert_eq!(parsed, None, "{raw:?}");
            assert!(!evaluate(parsed).active, "{raw:?}");
        }
    }

    #[test]
    fn a_well_formed_record_round_trips() {
        let r = ConsentRecord::decide(true, 1_800_000_000_000);
        let json = serde_json::to_string(&r).expect("serialise");
        assert_eq!(parse_record(Some(&json)), Some(r));
        // camelCase on the wire, so a hand-inspected settings row reads like the
        // rest of the bag.
        assert!(json.contains("\"decidedAt\""), "{json}");
    }

    #[test]
    fn every_state_is_reachable_and_exactly_one_is_active() {
        let states = [
            evaluate(None),
            evaluate(Some(ConsentRecord::decide(true, 1))),
            evaluate(Some(ConsentRecord::decide(false, 1))),
            evaluate(Some(ConsentRecord {
                granted: true,
                version: CONSENT_VERSION + 1,
                decided_at: 1,
            })),
        ];
        assert_eq!(
            states.iter().filter(|c| c.active).count(),
            1,
            "exactly one of the four states may permit sending"
        );
    }
}
