//! Human-review queue state machine — pure, GUI-free, store-free (PU-6 P2a).
//!
//! Ported from the Electron `src/main/review-queue.ts` (the behavioural spec).
//! That module persisted `EpisodePrep`s awaiting review in `electron-store`,
//! computed each entry's age on read, and ran an hourly reminder timeline
//! (24 h → 48 h → 7 d → auto-discard at 14 d). The persistence + the
//! notification side effects are I/O; here we keep ONLY the deterministic
//! transitions:
//!   - enqueue (with dedup by id), age-on-read, remove,
//!   - the immutable-field-guarded patch (`update_entry`), mark-published,
//!     mark-discarded,
//!   - [`process_reminders`] — the pure decision of *which* notifications to
//!     fire + which entries to drop, given the current queue snapshot + `now`.
//!
//! The `src-tauri` shell owns the actual sqlx/keyring persistence and turns each
//! returned [`ReminderAction`] into a notification/email/webhook/renderer event,
//! so this state machine is fully unit-testable without a store or a clock.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::prep::{EpisodePrep, EpisodePrepStatus};

// ── Reminder thresholds (ms) — port review-queue.ts ────────────────────────

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// First nudge.
pub const REMIND_24H_MS: i64 = 24 * HOUR_MS;
/// Second nudge.
pub const REMIND_48H_MS: i64 = 48 * HOUR_MS;
/// Third nudge.
pub const REMIND_7D_MS: i64 = 7 * DAY_MS;
/// Auto-discard with a history note.
pub const AUTO_DISCARD_MS: i64 = 14 * DAY_MS;

// ── Entry ──────────────────────────────────────────────────────────────────

/// One queue entry: an [`EpisodePrep`] plus bookkeeping. Mirrors the renderer
/// `ReviewQueueEntry` (camelCase). `age_in_days` is derived on read, not stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ReviewQueueEntry.ts")]
#[serde(rename_all = "camelCase")]
pub struct ReviewQueueEntry {
    pub id: String,
    pub prep: EpisodePrep,
    #[ts(type = "number")]
    pub added_at: i64,
    /// Reminders sent so far: 0=none, 1=24h, 2=48h, 3=7d.
    pub reminded: u8,
    /// Days since `added_at` — computed on read (0 when persisted).
    pub age_in_days: f64,
}

/// Days since `added_at` at `now`, never negative. Ports `ageInDays`.
pub fn age_in_days(added_at: i64, now: i64) -> f64 {
    ((now - added_at) as f64 / DAY_MS as f64).max(0.0)
}

/// Enqueue a prep, deduping by id (an existing entry with the same id is
/// replaced). Ports `addToQueue`. `entries` is the current snapshot.
pub fn enqueue(
    mut entries: Vec<ReviewQueueEntry>,
    prep: EpisodePrep,
    now: i64,
) -> Vec<ReviewQueueEntry> {
    entries.retain(|e| e.id != prep.id);
    entries.push(ReviewQueueEntry {
        id: prep.id.clone(),
        prep,
        added_at: now,
        reminded: 0,
        age_in_days: 0.0,
    });
    entries
}

/// The entry already holding `recording_path`, if the queue has one.
///
/// [`enqueue`] dedups by `prep.id`, which is a fresh uuid minted per call — so
/// it cannot recognise the same RECORDING arriving twice, only the same prep
/// object. Automatic population (E8: every fresh content-analysis pass offers
/// its recording to the queue) needs the other key, or re-running detection on
/// a file would stack a second entry on top of the first and the operator would
/// review the same service twice.
///
/// Deliberately ignores `status`, so a **published or discarded** entry blocks
/// re-entry just as firmly as a waiting one. Those two statuses are the
/// operator's decision on this recording; re-queueing it would overrule a human
/// with a re-analysis. (`mark_published`/`mark_discarded` leave the entry in the
/// queue for exactly this reason — see [`crate::prep::EpisodePrepStatus`].)
///
/// Path comparison is exact. A recording MOVED to another folder is genuinely
/// not findable by the path the old entry recorded, and inventing a looser
/// match — same basename, say — would let two different services on two volumes
/// collide, which is the worse failure of the two.
pub fn find_by_recording_path<'a>(
    entries: &'a [ReviewQueueEntry],
    recording_path: &str,
) -> Option<&'a ReviewQueueEntry> {
    entries
        .iter()
        .find(|e| e.prep.recording_path == recording_path)
}

/// Return the queue with `age_in_days` filled in and sorted newest-first. Ports
/// `getQueue`.
pub fn read_with_age(entries: &[ReviewQueueEntry], now: i64) -> Vec<ReviewQueueEntry> {
    let mut out: Vec<ReviewQueueEntry> = entries
        .iter()
        .cloned()
        .map(|mut e| {
            e.age_in_days = age_in_days(e.added_at, now);
            e
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.added_at));
    out
}

/// Apply a partial patch to the prep inside an entry, guarding the immutable
/// `id`/`created_at` and bumping `updated_at`. Ports `updateEntry`. The patch is
/// a closure over the prep so we don't need a partial-struct shape; returns
/// `true` if the id was found.
pub fn update_entry(
    entries: &mut [ReviewQueueEntry],
    id: &str,
    now: i64,
    patch: impl FnOnce(&mut EpisodePrep),
) -> bool {
    let Some(e) = entries.iter_mut().find(|e| e.id == id) else {
        return false;
    };
    let immutable_id = e.prep.id.clone();
    let immutable_created = e.prep.created_at;
    patch(&mut e.prep);
    // Restore the immutable fields in case the closure touched them.
    e.prep.id = immutable_id;
    e.prep.created_at = immutable_created;
    e.prep.updated_at = now;
    true
}

/// Set an entry's prep status to `Published`. Ports `markPublished` (the entry is
/// kept briefly so the UI can toast; the shell removes it later). Returns `true`
/// if found.
pub fn mark_published(entries: &mut [ReviewQueueEntry], id: &str, now: i64) -> bool {
    set_status(entries, id, EpisodePrepStatus::Published, now)
}

/// Set an entry's prep status to `Discarded`. Ports `markDiscarded`.
pub fn mark_discarded(entries: &mut [ReviewQueueEntry], id: &str, now: i64) -> bool {
    set_status(entries, id, EpisodePrepStatus::Discarded, now)
}

fn set_status(
    entries: &mut [ReviewQueueEntry],
    id: &str,
    status: EpisodePrepStatus,
    now: i64,
) -> bool {
    let Some(e) = entries.iter_mut().find(|e| e.id == id) else {
        return false;
    };
    e.prep.status = status;
    e.prep.updated_at = now;
    true
}

// ── Reminder processing (port processReminders) ─────────────────────────────

/// A notification the shell should fire as a result of [`process_reminders`].
/// Mirrors the channels the Electron `processReminders` fired (tray notify,
/// email, webhook, in-app warning) — but as a *decision*, so the shell does the
/// I/O. `body` is the localized message; the shell holds the labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderChannel {
    /// Tray/native notification only.
    Notify,
    /// Notify + email.
    NotifyEmail,
    /// Notify + email + webhook.
    NotifyEmailWebhook,
    /// Notify + email + webhook + an in-app backend-warning toast.
    NotifyEmailWebhookWarning,
    /// Auto-discard: a notify + a history note (the shell writes the note).
    AutoDiscard,
}

impl ReminderChannel {
    /// Whether this rung wants an e-mail. The NATIVE notification is not asked
    /// about: every rung fires one, exactly as a failure does — the person at
    /// the machine is never the one we skip.
    pub fn wants_email(self) -> bool {
        matches!(
            self,
            Self::NotifyEmail | Self::NotifyEmailWebhook | Self::NotifyEmailWebhookWarning
        )
    }

    /// Whether this rung wants the chat/webhook POST (48 h and later — a
    /// channel that pings on day one for every recording gets muted).
    pub fn wants_webhook(self) -> bool {
        matches!(
            self,
            Self::NotifyEmailWebhook | Self::NotifyEmailWebhookWarning
        )
    }

    /// Whether this rung also wants the in-app warning toast (7 d only: by then
    /// "somebody should look at this" has become "something is wrong here").
    pub fn wants_warning(self) -> bool {
        matches!(self, Self::NotifyEmailWebhookWarning)
    }

    /// Whether this action is the 14-day auto-discard rather than a reminder.
    /// The entry is already dropped from [`ReminderOutcome::survivors`]; the
    /// shell only has to say so.
    pub fn is_auto_discard(self) -> bool {
        matches!(self, Self::AutoDiscard)
    }
}

/// The auto-discard threshold in whole days — the number the shell puts in the
/// "discarded automatically after N days" notice, derived from
/// [`AUTO_DISCARD_MS`] so the sentence cannot drift from the behaviour.
pub const AUTO_DISCARD_DAYS: i64 = AUTO_DISCARD_MS / DAY_MS;

/// Which localized message a reminder action wants. The shell maps this to the
/// per-language string (`body24`/`body48`/`body7d`/`bodyDiscard`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderMessage {
    Day1,
    Day2,
    Day7,
    Discard,
}

/// One reminder action targeting one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderAction {
    pub id: String,
    pub channel: ReminderChannel,
    pub message: ReminderMessage,
}

/// The result of processing reminders: the new queue snapshot (survivors with
/// bumped `reminded` counters, dropped entries removed) + the actions to fire.
#[derive(Debug, Clone, PartialEq)]
pub struct ReminderOutcome {
    pub survivors: Vec<ReviewQueueEntry>,
    pub actions: Vec<ReminderAction>,
    /// True when the snapshot changed (the shell persists + emits a refresh).
    pub changed: bool,
}

/// Pure port of `processReminders`. For each entry, decide whether to clean up a
/// published/discarded entry (older than a day), auto-discard at 14 d, or fire
/// the next reminder threshold it has crossed. Idempotent: re-running within the
/// same threshold window returns no new actions because `reminded` only bumps on
/// crossing a NEW threshold.
pub fn process_reminders(entries: &[ReviewQueueEntry], now: i64) -> ReminderOutcome {
    let mut survivors: Vec<ReviewQueueEntry> = Vec::with_capacity(entries.len());
    let mut actions: Vec<ReminderAction> = Vec::new();
    let mut changed = false;

    for entry in entries {
        // Terminal states: keep ~1 day for the UI toast, then drop.
        if matches!(
            entry.prep.status,
            EpisodePrepStatus::Published | EpisodePrepStatus::Discarded
        ) {
            if now - entry.added_at > DAY_MS {
                changed = true; // dropped
            } else {
                survivors.push(entry.clone());
            }
            continue;
        }

        let age = now - entry.added_at;

        // Auto-discard at 14 days.
        if age > AUTO_DISCARD_MS {
            actions.push(ReminderAction {
                id: entry.id.clone(),
                channel: ReminderChannel::AutoDiscard,
                message: ReminderMessage::Discard,
            });
            changed = true; // dropped
            continue;
        }

        let mut new_reminded = entry.reminded;
        if new_reminded < 1 && age >= REMIND_24H_MS {
            new_reminded = 1;
            actions.push(ReminderAction {
                id: entry.id.clone(),
                channel: ReminderChannel::NotifyEmail,
                message: ReminderMessage::Day1,
            });
        } else if new_reminded < 2 && age >= REMIND_48H_MS {
            new_reminded = 2;
            actions.push(ReminderAction {
                id: entry.id.clone(),
                channel: ReminderChannel::NotifyEmailWebhook,
                message: ReminderMessage::Day2,
            });
        } else if new_reminded < 3 && age >= REMIND_7D_MS {
            new_reminded = 3;
            actions.push(ReminderAction {
                id: entry.id.clone(),
                channel: ReminderChannel::NotifyEmailWebhookWarning,
                message: ReminderMessage::Day7,
            });
        }

        if new_reminded != entry.reminded {
            changed = true;
            let mut e = entry.clone();
            e.reminded = new_reminded;
            survivors.push(e);
        } else {
            survivors.push(entry.clone());
        }
    }

    ReminderOutcome {
        survivors,
        actions,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{PrepAnalysisSegment, SegmentType};
    use crate::prep::{build_episode_prep, PrepDefaults};

    fn prep(id: &str) -> EpisodePrep {
        let segs = vec![PrepAnalysisSegment {
            start_sec: 0.0,
            end_sec: 600.0,
            duration_sec: 600.0,
            kind: SegmentType::Speech,
            confidence: 0.9,
            avg_rms_db: -20.0,
            label: String::new(),
        }];
        build_episode_prep(
            id.into(),
            "/r.mp4".into(),
            segs,
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
            0,
        )
    }

    fn entry(id: &str, added_at: i64, reminded: u8) -> ReviewQueueEntry {
        ReviewQueueEntry {
            id: id.into(),
            prep: prep(id),
            added_at,
            reminded,
            age_in_days: 0.0,
        }
    }

    fn prep_at(id: &str, recording_path: &str) -> EpisodePrep {
        let mut p = prep(id);
        p.recording_path = recording_path.into();
        p
    }

    #[test]
    fn enqueue_dedups_by_id() {
        let q = enqueue(vec![], prep("a"), 1000);
        let q = enqueue(q, prep("a"), 2000); // same id replaces
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].added_at, 2000);
    }

    // ── Path-keyed idempotency (E8 automatic population) ────────────────────

    #[test]
    fn enqueue_alone_cannot_recognise_the_same_recording_twice() {
        // The gap `find_by_recording_path` exists to close: two preps built
        // from the same file carry different ids, so `enqueue` stacks them.
        let q = enqueue(vec![], prep_at("first", "/rec/service.mp4"), 1000);
        let q = enqueue(q, prep_at("second", "/rec/service.mp4"), 2000);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn find_by_recording_path_spots_the_duplicate() {
        let q = enqueue(vec![], prep_at("a", "/rec/service.mp4"), 1000);
        assert_eq!(
            find_by_recording_path(&q, "/rec/service.mp4").map(|e| e.id.as_str()),
            Some("a"),
        );
        assert!(find_by_recording_path(&q, "/rec/other.mp4").is_none());
        assert!(find_by_recording_path(&[], "/rec/service.mp4").is_none());
    }

    #[test]
    fn a_published_recording_is_still_found_so_it_cannot_come_back() {
        let mut q = enqueue(vec![], prep_at("a", "/rec/service.mp4"), 1000);
        assert!(mark_published(&mut q, "a", 2000));
        assert!(find_by_recording_path(&q, "/rec/service.mp4").is_some());
    }

    #[test]
    fn a_discarded_recording_is_still_found_so_it_cannot_come_back() {
        let mut q = enqueue(vec![], prep_at("a", "/rec/service.mp4"), 1000);
        assert!(mark_discarded(&mut q, "a", 2000));
        assert!(find_by_recording_path(&q, "/rec/service.mp4").is_some());
    }

    #[test]
    fn read_with_age_sorts_newest_first_and_fills_age() {
        let entries = vec![entry("old", 0, 0), entry("new", DAY_MS, 0)];
        let read = read_with_age(&entries, 2 * DAY_MS);
        assert_eq!(read[0].id, "new");
        assert_eq!(read[1].id, "old");
        assert!((read[1].age_in_days - 2.0).abs() < 1e-9);
    }

    #[test]
    fn update_entry_guards_immutable_fields() {
        let mut entries = vec![entry("a", 0, 0)];
        let ok = update_entry(&mut entries, "a", 5000, |p| {
            p.master_preset = "music-rich".into();
            p.id = "HACKED".into(); // attempt to mutate immutable
            p.created_at = 999;
        });
        assert!(ok);
        assert_eq!(entries[0].prep.master_preset, "music-rich");
        assert_eq!(entries[0].prep.id, "a"); // restored
        assert_eq!(entries[0].prep.created_at, 0); // restored
        assert_eq!(entries[0].prep.updated_at, 5000);
        assert!(!update_entry(&mut entries, "missing", 1, |_| {}));
    }

    #[test]
    fn mark_published_and_discarded() {
        let mut entries = vec![entry("a", 0, 0)];
        assert!(mark_published(&mut entries, "a", 1));
        assert_eq!(entries[0].prep.status, EpisodePrepStatus::Published);
        assert!(mark_discarded(&mut entries, "a", 2));
        assert_eq!(entries[0].prep.status, EpisodePrepStatus::Discarded);
        assert!(!mark_published(&mut entries, "z", 1));
    }

    #[test]
    fn reminder_fires_24h_then_is_idempotent() {
        let entries = vec![entry("a", 0, 0)];
        let out = process_reminders(&entries, REMIND_24H_MS);
        assert_eq!(out.actions.len(), 1);
        assert_eq!(out.actions[0].channel, ReminderChannel::NotifyEmail);
        assert_eq!(out.actions[0].message, ReminderMessage::Day1);
        assert!(out.changed);
        assert_eq!(out.survivors[0].reminded, 1);

        // Re-run with the bumped state at the same age → no new action.
        let again = process_reminders(&out.survivors, REMIND_24H_MS + HOUR_MS);
        assert!(again.actions.is_empty());
        assert!(!again.changed);
    }

    #[test]
    fn reminder_48h_and_7d_escalate_channels() {
        let out48 = process_reminders(&[entry("a", 0, 1)], REMIND_48H_MS);
        assert_eq!(
            out48.actions[0].channel,
            ReminderChannel::NotifyEmailWebhook
        );
        let out7d = process_reminders(&[entry("a", 0, 2)], REMIND_7D_MS);
        assert_eq!(
            out7d.actions[0].channel,
            ReminderChannel::NotifyEmailWebhookWarning
        );
        assert_eq!(out7d.actions[0].message, ReminderMessage::Day7);
    }

    #[test]
    fn entry_past_multiple_thresholds_advances_only_one_rung_per_call() {
        // An entry that has aged past ALL reminder thresholds in a single poll
        // (e.g. the app was closed over a long weekend) must advance only ONE step
        // (Day1), not fire Day1+Day2+Day7 at once — the else-if chain guarantees
        // one rung per call. 8 days is past REMIND_7D_MS but under AUTO_DISCARD_MS.
        let out = process_reminders(&[entry("a", 0, 0)], 8 * DAY_MS);
        assert_eq!(out.actions.len(), 1);
        assert_eq!(out.actions[0].channel, ReminderChannel::NotifyEmail);
        assert_eq!(out.actions[0].message, ReminderMessage::Day1);
        assert_eq!(out.survivors[0].reminded, 1);
    }

    #[test]
    fn auto_discard_at_14_days_drops_and_emits_history() {
        let out = process_reminders(&[entry("a", 0, 3)], AUTO_DISCARD_MS + 1);
        assert_eq!(out.actions[0].channel, ReminderChannel::AutoDiscard);
        assert_eq!(out.actions[0].message, ReminderMessage::Discard);
        assert!(out.survivors.is_empty()); // dropped
        assert!(out.changed);
    }

    #[test]
    fn published_entries_are_cleaned_up_after_a_day() {
        let mut e = entry("a", 0, 0);
        e.prep.status = EpisodePrepStatus::Published;
        // younger than a day → kept
        let kept = process_reminders(std::slice::from_ref(&e), HOUR_MS);
        assert_eq!(kept.survivors.len(), 1);
        // older than a day → dropped
        let dropped = process_reminders(&[e], DAY_MS + 1);
        assert!(dropped.survivors.is_empty());
        assert!(dropped.changed);
        assert!(dropped.actions.is_empty());
    }

    #[test]
    fn the_channel_ladder_only_ever_widens() {
        // Each rung must be a superset of the one below it — a day-7 reminder
        // that dropped the e-mail leg would be a quieter escalation than day 2.
        let rungs = [
            ReminderChannel::NotifyEmail,
            ReminderChannel::NotifyEmailWebhook,
            ReminderChannel::NotifyEmailWebhookWarning,
        ];
        for w in rungs.windows(2) {
            assert!(!w[0].wants_email() || w[1].wants_email());
            assert!(!w[0].wants_webhook() || w[1].wants_webhook());
            assert!(!w[0].wants_warning() || w[1].wants_warning());
        }
        // And the shape of each rung, pinned.
        assert!(ReminderChannel::NotifyEmail.wants_email());
        assert!(!ReminderChannel::NotifyEmail.wants_webhook());
        assert!(ReminderChannel::NotifyEmailWebhook.wants_webhook());
        assert!(!ReminderChannel::NotifyEmailWebhook.wants_warning());
        assert!(ReminderChannel::NotifyEmailWebhookWarning.wants_warning());
        // Notify-only asks for nothing beyond the native leg every rung fires.
        assert!(!ReminderChannel::Notify.wants_email());
        assert!(!ReminderChannel::Notify.wants_webhook());
        // Auto-discard is not a reminder: no mail, no webhook, just the notice.
        assert!(ReminderChannel::AutoDiscard.is_auto_discard());
        assert!(!ReminderChannel::AutoDiscard.wants_email());
        assert!(!ReminderChannel::NotifyEmail.is_auto_discard());
    }

    #[test]
    fn the_auto_discard_day_count_matches_the_threshold() {
        // The shell prints this number in a sentence; deriving it from the same
        // constant the decision uses is what keeps the sentence true.
        assert_eq!(AUTO_DISCARD_DAYS, 14);
        assert_eq!(AUTO_DISCARD_DAYS * DAY_MS, AUTO_DISCARD_MS);
    }

    #[test]
    fn empty_queue_is_a_noop() {
        let out = process_reminders(&[], 99_999_999);
        assert!(out.survivors.is_empty() && out.actions.is_empty() && !out.changed);
    }
}
