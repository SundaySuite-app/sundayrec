//! The telemetry outbox state machine — pure, no persistence and no timers.
//!
//! Same shape as [`crate::cloud::queue`], which this is modelled on: the
//! transitions over an in-memory `Vec<TelemetryEntry>` live here and carry the
//! tests, while the `src-tauri` shell owns the sqlx table, the clock and the
//! socket. Every function takes `now_ms` rather than reading a clock.
//!
//! It differs from the cloud queue in three ways, and each difference is a
//! consequence of what telemetry IS:
//!
//!   - **A blocked send is not the user's problem.** A cloud backup that will
//!     not upload is a recording the church might lose, so that queue retries
//!     ten times over 24 hours and then sits in a `failed` state a person is
//!     expected to look at. A telemetry payload that will not send is worth
//!     nobody's attention: [`MAX_ATTEMPTS`] is 6, the ladder is shorter, and a
//!     row that exhausts it is `Failed` purely so the settings panel can be
//!     honest about it — the bound below reclaims it soon enough.
//!   - **The queue is bounded, and the bound drops the OLDEST.** The cloud queue
//!     is unbounded because every entry names a file the user wants preserved.
//!     Here, a laptop that spends a winter offline must not accumulate a
//!     megabyte of stale reports about a version nobody runs any more.
//!   - **There is no `reauth-required`.** Telemetry is anonymous; there is
//!     nothing to authenticate and therefore nothing to re-authenticate.
//!
//! ## The consent gate lives here too
//!
//! [`pump_decision`] is the only function a sender loop calls to find out what
//! to do, and its FIRST argument is whether consent is active. With consent off
//! it returns [`PumpDecision::Blocked`] without so much as reading the queue —
//! so "no consent means no network activity, not even DNS" is a property of a
//! pure function with a test, rather than a discipline someone has to remember
//! at every call site.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Give up on a payload after this many attempts. Six, not the cloud queue's
/// ten: a report that six spread-out attempts could not deliver is describing a
/// version and a machine state that have moved on.
pub const MAX_ATTEMPTS: u32 = 6;

/// Per-attempt backoff after a failure, indexed by `attempts - 1` and clamped to
/// the last step: 1 min → 5 → 15 → 1 h → 6 h → 24 h.
///
/// Front-loaded like the cloud ladder (a flaky minute of wifi resolves on the
/// second try) but it reaches a day quickly, because there is no urgency: the
/// data describes something that already happened and will read the same
/// tomorrow.
pub const BACKOFF_STEPS_MS: [i64; 6] = [
    60_000,
    5 * 60_000,
    15 * 60_000,
    60 * 60_000,
    6 * 60 * 60_000,
    24 * 60 * 60_000,
];

/// How many payloads the outbox may hold before the oldest are dropped.
///
/// Fifty. The reasoning, so the number can be re-argued rather than guessed at
/// again: a payload is bounded by the caps in [`super`] — 20 crash summaries
/// (≤ ~350 bytes each), 20 quality records (~300 bytes each), 40 finding codes,
/// 20 wake failures, 24 counters — so a FULL one is roughly 15 kB and a typical
/// one is under 2 kB. Fifty full payloads is therefore ~750 kB worst case, the
/// same order as the crash ring's 200 kB ceiling and small beside a single
/// minute of recorded audio.
///
/// Fifty is also far more than any real backlog. Payloads are enqueued when
/// something happened, not on a timer, so a church recording once a week fills
/// fifty rows in a year of being offline; a machine crashing constantly fills
/// them in a few days, and by then the fifty NEWEST are a better description of
/// the problem than the fifty oldest would be. Dropping from the front is what
/// makes that true.
pub const QUEUE_MAX: usize = 50;

/// Where a queued payload is in its lifecycle. Serialised lowercase, matching
/// the `status` CHECK constraint in migration 0005.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TelemetryStatus.ts")]
#[serde(rename_all = "lowercase")]
pub enum TelemetryStatus {
    /// Waiting for its `next_attempt` time to pass.
    Pending,
    /// Currently being sent.
    Sending,
    /// Out of attempts. Kept only so the settings panel can say so; the
    /// [`QUEUE_MAX`] bound reclaims it.
    Failed,
}

/// One queued payload.
///
/// `payload_json` is the serialised [`super::TelemetryPayload`] — already
/// sanitised, already capped. Storing the rendered JSON rather than the struct
/// means a row queued by version N is sent unchanged by version N+1: the payload
/// the user could inspect in the preview is byte-for-byte the payload that
/// leaves, even across an update that changed the builder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEntry {
    pub id: String,
    /// Unix ms (UTC) the payload was built and queued.
    pub created_at: i64,
    /// The [`super::TELEMETRY_SCHEMA`] the payload was built at.
    pub schema_ver: u32,
    /// What this payload is about (`"crash:<ts>"`, `"quality:<ts>"`,
    /// `"counters:<ts>"`). Unique in storage, so the same batch cannot be queued
    /// twice by two racing drains.
    pub dedup_key: String,
    pub payload_json: String,
    pub attempts: u32,
    /// Unix ms — earliest the sender may try this entry.
    pub next_attempt: i64,
    pub last_error: Option<String>,
    pub status: TelemetryStatus,
}

/// What a sender loop should do right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpDecision {
    /// Consent is not active. Do nothing — do not read the queue, do not resolve
    /// a hostname, do not open a socket. The ONLY correct behaviour, and the one
    /// this enum exists to make impossible to skip.
    Blocked,
    /// Consent is active but nothing is due.
    Idle,
    /// Send this entry.
    Send(String),
}

/// THE gate. Consent first, everything else after.
///
/// Written as one function returning a three-way decision, rather than a
/// `should_send` boolean beside a `select_next`, because the boolean version has
/// a failure mode: a caller can select the next entry, then check consent, then
/// send — and the DNS lookup for the endpoint has already happened by the time
/// the check runs. Here there is nothing to select until consent has been
/// confirmed.
pub fn pump_decision(
    consent_active: bool,
    entries: &[TelemetryEntry],
    now_ms: i64,
) -> PumpDecision {
    if !consent_active {
        return PumpDecision::Blocked;
    }
    match select_next(entries, now_ms) {
        Some(id) => PumpDecision::Send(id),
        None => PumpDecision::Idle,
    }
}

/// The id of the next entry to send: the `pending` entry whose `next_attempt`
/// has passed, earliest first. `None` when nothing is runnable.
///
/// Deliberately NOT public-facing on its own — [`pump_decision`] is the entry
/// point, so the consent check cannot be forgotten.
fn select_next(entries: &[TelemetryEntry], now_ms: i64) -> Option<String> {
    entries
        .iter()
        .filter(|e| e.status == TelemetryStatus::Pending && e.next_attempt <= now_ms)
        .min_by_key(|e| (e.next_attempt, e.created_at))
        .map(|e| e.id.clone())
}

/// Reset any entry left in `Sending` back to `Pending`, returning how many.
/// Call ONCE at startup: an entry only reaches `Sending` while a send is in
/// flight, so at boot every one of them is stale by definition. Without this a
/// force-quit mid-send strands the row forever, exactly as it did for the cloud
/// queue before `reset_stale_uploading`.
pub fn reset_stale_sending(entries: &mut [TelemetryEntry]) -> usize {
    let mut reset = 0;
    for e in entries.iter_mut() {
        if e.status == TelemetryStatus::Sending {
            e.status = TelemetryStatus::Pending;
            reset += 1;
        }
    }
    reset
}

/// Transition an entry to `Sending` and count the attempt, just before the send.
pub fn mark_sending(entries: &mut [TelemetryEntry], id: &str) {
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.status = TelemetryStatus::Sending;
        e.attempts += 1;
    }
}

/// A delivered payload leaves the queue. Returns `true` if one was removed.
pub fn on_success(entries: &mut Vec<TelemetryEntry>, id: &str) -> bool {
    let before = entries.len();
    entries.retain(|e| e.id != id);
    entries.len() != before
}

/// Apply a failed attempt: back off, or give up at [`MAX_ATTEMPTS`].
///
/// `error` is stored for the settings panel. Attempts are incremented by
/// [`mark_sending`] BEFORE the attempt, so by the time this runs `attempts`
/// already counts this try — the same convention as the cloud queue.
pub fn on_failure(entries: &mut [TelemetryEntry], id: &str, error: impl Into<String>, now_ms: i64) {
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.last_error = Some(error.into());
        if e.attempts >= MAX_ATTEMPTS {
            e.status = TelemetryStatus::Failed;
        } else {
            e.status = TelemetryStatus::Pending;
            let idx = (e.attempts.saturating_sub(1) as usize).min(BACKOFF_STEPS_MS.len() - 1);
            e.next_attempt = now_ms + BACKOFF_STEPS_MS[idx];
        }
    }
}

/// Drop an entry the endpoint will never accept. Returns `true` if one went.
///
/// The counterpart to [`on_failure`], and the distinction is the whole reason
/// this function exists rather than being a flag on that one. The backoff ladder
/// answers "the church wifi is down": wait, try again, the same bytes will
/// succeed later. It is exactly wrong for "this payload is malformed", because a
/// malformed payload is malformed the same way all six times — six spread-out
/// attempts over 24 hours, six identical rejections, and a row that then sits in
/// `Failed` until the [`QUEUE_MAX`] bound reclaims it.
///
/// So a permanent rejection removes the row immediately. The endpoint's half of
/// this contract is in `sunday-telemetry/src/ingest.ts`: 4xx means "will never be
/// accepted" and carries `"retryable": false`, with 429 called out as the one
/// transient 4xx. `src-tauri`'s `telemetry::http_sender::classify` is where a
/// status becomes one or the other.
///
/// Nothing is kept for the settings panel here, unlike an exhausted retry
/// ladder. A row that ran out of attempts describes something the user might
/// recognise — a network that has been down for a day. A schema rejection
/// describes a disagreement between this build and the endpoint, which the user
/// can neither cause nor fix; it belongs in the log (and in the endpoint's own
/// rejection log, which records WHICH field), not in a badge they cannot act on.
pub fn on_permanent_failure(entries: &mut Vec<TelemetryEntry>, id: &str) -> bool {
    let before = entries.len();
    entries.retain(|e| e.id != id);
    entries.len() != before
}

/// Which entry ids must go so at most `cap` remain, oldest first.
///
/// Ordered by `created_at` (then `id`, so the answer is deterministic when two
/// payloads were built in the same millisecond). Failed rows are not preferred
/// over pending ones: a `Failed` row that is NEWER describes a more current
/// problem than a `Pending` row from last winter, and the age order is the one
/// that keeps the queue's contents relevant.
pub fn overflow_victims(entries: &[TelemetryEntry], cap: usize) -> Vec<String> {
    if entries.len() <= cap {
        return Vec::new();
    }
    let mut by_age: Vec<&TelemetryEntry> = entries.iter().collect();
    by_age.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    by_age[..entries.len() - cap]
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

/// A compact status line for the settings panel: how many are waiting, how old
/// the oldest is, and what went wrong last.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TelemetryQueueStatus.ts")]
#[serde(rename_all = "camelCase")]
pub struct TelemetryQueueStatus {
    /// Rows not yet delivered (pending + sending).
    pub pending: u32,
    /// Rows that ran out of attempts.
    pub failed: u32,
    /// Unix ms (UTC) the oldest undelivered payload was built.
    #[ts(type = "number | null")]
    pub oldest_at: Option<i64>,
    /// The most recent failure message, if any.
    pub last_error: Option<String>,
}

/// Summarise the queue for the UI.
pub fn queue_status(entries: &[TelemetryEntry]) -> TelemetryQueueStatus {
    TelemetryQueueStatus {
        pending: entries
            .iter()
            .filter(|e| e.status != TelemetryStatus::Failed)
            .count() as u32,
        failed: entries
            .iter()
            .filter(|e| e.status == TelemetryStatus::Failed)
            .count() as u32,
        oldest_at: entries
            .iter()
            .filter(|e| e.status != TelemetryStatus::Failed)
            .map(|e| e.created_at)
            .min(),
        // The newest failure is the useful one — an error from three weeks ago
        // describes a network that has since come back.
        last_error: entries
            .iter()
            .filter(|e| e.last_error.is_some())
            .max_by_key(|e| e.created_at)
            .and_then(|e| e.last_error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, created_at: i64) -> TelemetryEntry {
        TelemetryEntry {
            id: id.to_string(),
            created_at,
            schema_ver: 1,
            dedup_key: format!("quality:{created_at}"),
            payload_json: "{}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: TelemetryStatus::Pending,
        }
    }

    // ── The consent gate ─────────────────────────────────────────────────────

    #[test]
    fn without_consent_the_pump_never_selects_anything() {
        // THE guarantee, as a pure property: whatever the queue holds and
        // whatever the clock says, consent-off yields Blocked. There is no
        // combination of inputs that produces a Send.
        let queues: Vec<Vec<TelemetryEntry>> = vec![
            vec![],
            vec![entry("a", 0)],
            vec![entry("a", 0), entry("b", 1)],
            {
                let mut e = entry("c", 0);
                e.status = TelemetryStatus::Sending;
                vec![e]
            },
            {
                let mut e = entry("d", 0);
                e.status = TelemetryStatus::Failed;
                vec![e]
            },
        ];
        for q in &queues {
            for now in [i64::MIN, -1, 0, 1, 1_800_000_000_000, i64::MAX] {
                assert_eq!(
                    pump_decision(false, q, now),
                    PumpDecision::Blocked,
                    "consent off must block regardless of queue {q:?} / now {now}"
                );
            }
        }
        // …and the positive control, so the assertion above is not vacuous: the
        // SAME queue with consent on does select an entry.
        assert_eq!(
            pump_decision(true, &queues[1], 1_800_000_000_000),
            PumpDecision::Send("a".into())
        );
    }

    #[test]
    fn with_consent_the_pump_is_idle_until_something_is_due() {
        let q = vec![entry("a", 10_000)];
        assert_eq!(pump_decision(true, &q, 9_999), PumpDecision::Idle);
        assert_eq!(
            pump_decision(true, &q, 10_000),
            PumpDecision::Send("a".into())
        );
        assert_eq!(pump_decision(true, &[], 10_000), PumpDecision::Idle);
    }

    #[test]
    fn the_pump_picks_the_earliest_due_entry() {
        let mut later = entry("later", 5_000);
        later.next_attempt = 5_000;
        let mut earlier = entry("earlier", 9_000);
        earlier.next_attempt = 1_000;
        let q = vec![later, earlier];
        assert_eq!(
            pump_decision(true, &q, 10_000),
            PumpDecision::Send("earlier".into()),
            "next_attempt orders the queue, not insertion order"
        );
    }

    #[test]
    fn only_pending_entries_are_selected() {
        for status in [TelemetryStatus::Sending, TelemetryStatus::Failed] {
            let mut e = entry("a", 0);
            e.status = status;
            assert_eq!(
                pump_decision(true, &[e], 1_000_000),
                PumpDecision::Idle,
                "{status:?} must not be picked up"
            );
        }
    }

    // ── The lifecycle ────────────────────────────────────────────────────────

    #[test]
    fn a_send_counts_its_attempt_before_it_happens() {
        let mut q = vec![entry("a", 0)];
        mark_sending(&mut q, "a");
        assert_eq!(q[0].status, TelemetryStatus::Sending);
        assert_eq!(q[0].attempts, 1);
        // An unknown id is a no-op, not a panic.
        mark_sending(&mut q, "ghost");
        assert_eq!(q[0].attempts, 1);
    }

    #[test]
    fn success_removes_the_entry() {
        let mut q = vec![entry("a", 0), entry("b", 1)];
        assert!(on_success(&mut q, "a"));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].id, "b");
        assert!(!on_success(&mut q, "ghost"));
    }

    #[test]
    fn failures_walk_the_backoff_ladder_then_give_up() {
        let mut q = vec![entry("a", 0)];
        let now = 1_000_000i64;
        for attempt in 1..=MAX_ATTEMPTS {
            mark_sending(&mut q, "a");
            on_failure(&mut q, "a", format!("boom {attempt}"), now);
            if attempt < MAX_ATTEMPTS {
                assert_eq!(q[0].status, TelemetryStatus::Pending, "attempt {attempt}");
                assert_eq!(
                    q[0].next_attempt,
                    now + BACKOFF_STEPS_MS[(attempt - 1) as usize],
                    "attempt {attempt} must use the matching ladder step"
                );
            }
        }
        assert_eq!(q[0].status, TelemetryStatus::Failed);
        assert_eq!(q[0].attempts, MAX_ATTEMPTS);
        assert_eq!(q[0].last_error.as_deref(), Some("boom 6"));
        // A failed entry is never selected again, even far in the future.
        assert_eq!(pump_decision(true, &q, i64::MAX), PumpDecision::Idle);
    }

    #[test]
    fn the_ladder_is_clamped_rather_than_indexed_out_of_bounds() {
        let mut q = vec![entry("a", 0)];
        // A row that somehow carries more attempts than the ladder has steps
        // (a hand-edited database, a changed MAX_ATTEMPTS) must not panic.
        q[0].attempts = 3;
        on_failure(&mut q, "a", "x", 0);
        assert_eq!(q[0].next_attempt, BACKOFF_STEPS_MS[2]);
        assert_eq!(BACKOFF_STEPS_MS.len(), MAX_ATTEMPTS as usize);
    }

    #[test]
    fn a_force_quit_mid_send_is_recovered_at_startup() {
        let mut q = vec![entry("a", 0), entry("b", 1)];
        q[0].status = TelemetryStatus::Sending;
        assert_eq!(reset_stale_sending(&mut q), 1);
        assert_eq!(q[0].status, TelemetryStatus::Pending);
        // Idempotent.
        assert_eq!(reset_stale_sending(&mut q), 0);
    }

    // ── The bound ────────────────────────────────────────────────────────────

    #[test]
    fn the_queue_bound_drops_the_oldest() {
        let q: Vec<TelemetryEntry> = (0..QUEUE_MAX + 7)
            .map(|i| entry(&format!("id-{i:03}"), i as i64))
            .collect();
        let victims = overflow_victims(&q, QUEUE_MAX);
        assert_eq!(victims.len(), 7);
        assert_eq!(victims[0], "id-000", "oldest goes first");
        assert_eq!(victims[6], "id-006");
        // Under the cap nothing is dropped.
        assert!(overflow_victims(&q[..QUEUE_MAX], QUEUE_MAX).is_empty());
        assert!(overflow_victims(&[], QUEUE_MAX).is_empty());
    }

    #[test]
    fn the_bound_is_deterministic_when_payloads_share_a_millisecond() {
        let q = vec![entry("b", 5), entry("a", 5), entry("c", 9)];
        assert_eq!(
            overflow_victims(&q, 1),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn a_newer_failed_row_outlives_an_older_pending_one() {
        // Relevance beats status: a failed report about today's version is
        // better company than a pending one about last winter's.
        let mut old_pending = entry("old", 1);
        old_pending.status = TelemetryStatus::Pending;
        let mut new_failed = entry("new", 100);
        new_failed.status = TelemetryStatus::Failed;
        let victims = overflow_victims(&[old_pending, new_failed], 1);
        assert_eq!(victims, vec!["old".to_string()]);
    }

    // ── The status line ──────────────────────────────────────────────────────

    #[test]
    fn the_status_counts_and_reports_the_newest_error() {
        let mut a = entry("a", 100);
        a.last_error = Some("old failure".into());
        let mut b = entry("b", 200);
        b.last_error = Some("recent failure".into());
        let mut c = entry("c", 300);
        c.status = TelemetryStatus::Failed;

        let s = queue_status(&[a, b, c]);
        assert_eq!(s.pending, 2);
        assert_eq!(s.failed, 1);
        assert_eq!(s.oldest_at, Some(100));
        assert_eq!(
            s.last_error.as_deref(),
            Some("recent failure"),
            "the newest error is the one that still describes reality"
        );
    }

    #[test]
    fn an_empty_queue_has_an_honest_status() {
        let s = queue_status(&[]);
        assert_eq!(s, TelemetryQueueStatus::default());
        assert_eq!(s.pending, 0);
        assert_eq!(s.oldest_at, None);
        assert_eq!(s.last_error, None);
    }

    #[test]
    fn a_permanent_failure_drops_the_row_instead_of_backing_off() {
        // The failure this prevents: a payload the endpoint will refuse every
        // time, retried on a ladder that reaches 24 hours, six times, and then
        // parked in `Failed` until the bound reclaims it.
        let mut q = vec![entry("a", 0), entry("b", 1)];
        assert!(on_permanent_failure(&mut q, "a"));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].id, "b");
        // Idempotent: a second permanent failure for the same id is a no-op, not
        // a panic and not a second removal.
        assert!(!on_permanent_failure(&mut q, "a"));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn a_permanent_failure_leaves_no_attempt_state_behind() {
        // Contrast with `on_failure`, which is the whole point of having both.
        let now = 1_800_000_000_000i64;
        let mut transient = vec![entry("a", 0)];
        mark_sending(&mut transient, "a");
        on_failure(&mut transient, "a", "connection reset", now);
        assert_eq!(transient.len(), 1, "a transient failure keeps the payload");
        assert_eq!(transient[0].attempts, 1);
        assert_eq!(transient[0].next_attempt, now + BACKOFF_STEPS_MS[0]);

        let mut permanent = vec![entry("a", 0)];
        mark_sending(&mut permanent, "a");
        on_permanent_failure(&mut permanent, "a");
        assert!(permanent.is_empty(), "a permanent failure keeps nothing");
    }

    #[test]
    fn a_dropped_payload_does_not_block_the_next_one() {
        // A permanently-rejected entry must not wedge the queue behind it: the
        // pump has to find the next due payload on the very next iteration.
        let mut q = vec![entry("bad", 0), entry("good", 1)];
        mark_sending(&mut q, "bad");
        on_permanent_failure(&mut q, "bad");
        assert_eq!(
            pump_decision(true, &q, 1_800_000_000_000),
            PumpDecision::Send("good".to_string())
        );
    }
}
