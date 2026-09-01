//! The e-mail relay's outbox decisions — pure, no persistence, no timers.
//!
//! The relay is the answer to a question a volunteer asked: is there a lighter
//! way to get an e-mail when a recording fails than typing an SMTP host, a user
//! name and an app password into a settings panel? There is — SundayRec posts
//! the already-rendered mail to the SundaySuite endpoint, which sends it.
//!
//! This module is the client half of that, minus every side effect: which queued
//! row may leave, when a row is too old to be worth sending, which rows go when
//! the queue is full, and whether an event has already been reported. The
//! `src-tauri` shell owns the sqlite table, the clock and the socket, exactly as
//! it does for [`crate::telemetry::queue`] — which this is modelled on, and
//! which it re-uses rather than re-states wherever the two agree.
//!
//! ## What it borrows from the telemetry queue, and what it does not
//!
//! Borrowed verbatim, by import: [`BACKOFF_STEPS_MS`] and [`MAX_ATTEMPTS`]. The
//! ladder is right here for the same reason it is right there — a flaky minute
//! of church wifi resolves on the second try, and beyond that there is no
//! urgency worth a tighter retry. Re-typing the numbers would create two
//! ladders that agree today and drift later.
//!
//! Borrowed in shape, not in value: the bound ([`RELAY_QUEUE_MAX`], 20 against
//! the telemetry queue's 50) and the three-way pump decision.
//!
//! NOT borrowed — the gate. Telemetry has ONE global permission (consent), so
//! [`crate::telemetry::queue::pump_decision`] can answer `Blocked` before it
//! reads the queue. The relay's permission is per-row, because the row that
//! ASKS for permission has to be allowed out before the permission can exist:
//!
//!   - `subscribe` and `unsubscribe` need only a local subscription record.
//!     A subscribe row that waited for confirmation would wait forever — the
//!     confirmation mail is what that row sends.
//!   - `send` rows (failure, missed, receipt, test) need a CONFIRMED, unsuppressed
//!     subscription. Double opt-in means exactly this: nothing but the
//!     confirmation request reaches an address that has not said yes.
//!
//! ## The freshness caps, and the one kind that has none
//!
//! A queued row describes something that was true when it was queued. An alert
//! about a recording that failed six hours ago is still news; one about last
//! month is archaeology, and arrives without context on a Sunday morning. So
//! events expire ([`RELAY_EVENT_MAX_AGE_MS`]) and so do subscribe requests
//! ([`RELAY_SUBSCRIBE_MAX_AGE_MS`], matching the endpoint's own retention of
//! unconfirmed rows) — a week-old sign-up would put a surprise mail in a
//! stranger's inbox.
//!
//! An `unsubscribe` row never expires. "Stop sending me mail" is not news that
//! goes stale, and dropping it silently would leave the endpoint sending to
//! somebody who asked us to stop. It is bounded by [`MAX_ATTEMPTS`] like
//! everything else, and by the footer link in every mail, which reaches the same
//! endpoint without this app's help at all.

use serde::{Deserialize, Serialize};

use crate::email::{RelayMessageKind, ALERT_THROTTLE_MS};

/// The retry ladder, and the attempt count that ends it — the telemetry
/// outbox's, imported rather than copied. See this module's header.
pub use crate::telemetry::queue::{BACKOFF_STEPS_MS, MAX_ATTEMPTS};

// ─────────────────────────────────────────────────────────────────────────────
//   The bounds
// ─────────────────────────────────────────────────────────────────────────────

/// How many rows the relay outbox may hold before the oldest are dropped.
///
/// Twenty, and the arithmetic so it can be re-argued rather than re-guessed: a
/// row stores the RENDERED message, which the endpoint caps at 200 + 8 000 +
/// 24 000 characters ([`crate::email::SUBJECT_MAX_CHARS`] and friends). A
/// realistic alert is nowhere near those — a failure mail is ~1 kB of text and
/// ~2 kB of HTML — but the cap is what a bound has to reason about, and at
/// UTF-8's worst case those 32 200 characters are ~32 kB. Twenty rows is
/// therefore ~640 kB worst case, the same order as the telemetry queue's ~750 kB
/// and invisible beside a minute of recorded audio.
///
/// Twenty is also far more than any real backlog, and for a sharper reason than
/// the telemetry queue's: the freshness caps below mean a stale row is deleted
/// long before it can be crowded out. A queue that reaches twenty live rows is
/// describing a machine that failed twenty times in six hours — at which point
/// the twenty NEWEST are the better report, which is what dropping from the
/// front gives.
pub const RELAY_QUEUE_MAX: usize = 20;

/// How long a confirmation link lives, in days.
///
/// The endpoint mints and enforces it; this constant exists so the seven
/// hand-written confirmation mails cannot promise a different number from the
/// one the link honours (pinned by
/// `email::tests::the_confirm_mail_promises_the_ttl_the_endpoint_enforces`), and
/// so [`RELAY_SUBSCRIBE_MAX_AGE_MS`] is derived from it rather than agreeing
/// with it by coincidence.
pub const CONFIRM_LINK_TTL_DAYS: i64 = 7;

/// How old an EVENT row may be when it is sent: six hours.
///
/// Long enough to survive a service (a machine that lost wifi at 11:00 still
/// reports before evening), short enough that nothing arrives out of its own
/// context. The alternative — an alert about a fortnight-old failure landing
/// while the volunteer sets up for today — is a message that costs attention and
/// buys nothing, because whatever it describes is either long fixed or long
/// forgotten.
pub const RELAY_EVENT_MAX_AGE_MS: i64 = 6 * 60 * 60 * 1_000;

/// How old a SUBSCRIBE row may be when it is sent: seven days, the same
/// [`CONFIRM_LINK_TTL_DAYS`] the endpoint keeps an unconfirmed row for.
///
/// The number is not really about the token. It is about the third party: a
/// sign-up queued a week ago, on a machine that has been offline since, is an
/// intention somebody expressed and has had a week to forget — and the mail it
/// produces lands in an inbox that did not ask for it that morning.
pub const RELAY_SUBSCRIBE_MAX_AGE_MS: i64 = CONFIRM_LINK_TTL_DAYS * 24 * 60 * 60 * 1_000;

// ─────────────────────────────────────────────────────────────────────────────
//   A queued row
// ─────────────────────────────────────────────────────────────────────────────

/// What a queued row DOES at the endpoint. Serialised lowercase, matching the
/// `kind` CHECK constraint in the outbox migration.
///
/// Distinct from [`RelayMessageKind`], which says what the MAIL is: a
/// `Subscribe` row causes a `confirm` mail, a `Send` row carries one of the
/// other four, and `Unsubscribe` causes no mail at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayKind {
    /// Register the address and ask the endpoint to send the confirmation mail.
    Subscribe,
    /// Deliver one already-rendered notification.
    Send,
    /// Delete the subscription. Idempotent at the endpoint.
    Unsubscribe,
}

impl RelayKind {
    /// Stable lowercase label for logs and the `kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            RelayKind::Subscribe => "subscribe",
            RelayKind::Send => "send",
            RelayKind::Unsubscribe => "unsubscribe",
        }
    }
}

/// Where a queued row is in its lifecycle. The same three states as
/// [`crate::telemetry::queue::TelemetryStatus`], as its own type because the two
/// tables carry their own CHECK constraints and neither should be able to widen
/// the other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayStatus {
    /// Waiting for its `next_attempt` time to pass.
    Pending,
    /// Currently being sent.
    Sending,
    /// Out of attempts.
    Failed,
}

/// One queued relay operation.
///
/// `payload_json` is the RENDERED request body — subject, text and HTML as they
/// will be sent, not the facts they were built from. Same reasoning as
/// [`crate::telemetry::queue::TelemetryEntry`], and it matters more here: a row
/// queued by version N is sent unchanged by version N+1, so an update that
/// rewords a template cannot silently reword an alert that was already written,
/// and the mail the volunteer receives is the mail this build composed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayEntry {
    pub id: String,
    /// Unix ms (UTC) the row was built and queued.
    pub created_at: i64,
    /// What this row does at the endpoint.
    pub kind: RelayKind,
    /// Which mail a [`RelayKind::Send`] row carries. `Some` exactly for `Send`
    /// rows: it names the `notify_seen` scope and appears in the request body.
    pub event: Option<RelayMessageKind>,
    /// Unique in storage, so two racing drains cannot queue the same operation
    /// twice (`"failure:<code>:<ts>"`, `"missed:<occurrence-ms>"`, …).
    pub dedup_key: String,
    pub payload_json: String,
    pub attempts: u32,
    /// Unix ms — earliest the sender may try this row.
    pub next_attempt: i64,
    pub last_error: Option<String>,
    pub status: RelayStatus,
}

// ─────────────────────────────────────────────────────────────────────────────
//   The gate
// ─────────────────────────────────────────────────────────────────────────────

/// What the local subscription record says, and the whole of what the pump is
/// allowed to decide on.
///
/// ⚠️ **`enrolled` means "a subscription record exists locally", in any state —
/// including one being unsubscribed.** The record is what the pump is gated on,
/// so it has to outlive the `unsubscribe` row it queued: a store that deleted
/// the record the moment the user clicked "unsubscribe" would strand that row
/// behind a shut gate, and the endpoint would go on sending mail to somebody who
/// asked it to stop. The record is cleared when the row LEAVES, not when it is
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayGate {
    /// A subscription record exists in `app_setting` `notify.relay`.
    pub enrolled: bool,
    /// The confirmation link has been clicked.
    pub confirmed: bool,
    /// The endpoint answered `410 recipient_suppressed`: the address bounces or
    /// complained. Nothing but an unsubscribe should reach it again.
    pub suppressed: bool,
}

/// What a relay sender loop should do right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayPumpDecision {
    /// No subscription exists on this machine. Do nothing — do not read the
    /// queue, do not resolve a hostname, do not open a socket. A machine that
    /// never signed up must be indistinguishable, on the network, from one that
    /// never heard of the relay.
    Blocked,
    /// A subscription exists, but nothing is both eligible and due.
    Idle,
    /// Send this row.
    Send(String),
}

/// THE gate. The subscription record first, everything else after.
///
/// One function returning a three-way decision, for the same reason
/// [`crate::telemetry::queue::pump_decision`] is one: a `should_send` boolean
/// beside a `select_next` lets a caller select a row, resolve the endpoint's
/// hostname, and only then check whether it was allowed to — by which time the
/// DNS lookup has already happened. Here there is nothing to select until the
/// gate has been passed.
///
/// It differs from its telemetry twin in exactly one way, and the difference is
/// forced: the per-row admission rules ([`row_allowed`]) live INSIDE the
/// selection, because "may this leave?" has a different answer for a subscribe
/// row than for a failure alert, and a pump that had to remember which was which
/// would eventually forget.
pub fn relay_pump_decision(
    gate: RelayGate,
    entries: &[RelayEntry],
    now_ms: i64,
) -> RelayPumpDecision {
    if !gate.enrolled {
        return RelayPumpDecision::Blocked;
    }
    match select_next(gate, entries, now_ms) {
        Some(id) => RelayPumpDecision::Send(id),
        None => RelayPumpDecision::Idle,
    }
}

/// Whether THIS row may leave, given the subscription's state.
///
/// Subscribe and unsubscribe need only the record. A send row needs a confirmed,
/// unsuppressed subscription — the double opt-in promise stated as a predicate,
/// so an unconfirmed address cannot receive an alert by any path through this
/// module.
pub fn row_allowed(gate: RelayGate, kind: RelayKind) -> bool {
    match kind {
        RelayKind::Subscribe | RelayKind::Unsubscribe => true,
        RelayKind::Send => gate.confirmed && !gate.suppressed,
    }
}

/// The id of the next row to send: eligible, `pending`, fresh, and due —
/// earliest `next_attempt` first. `None` when nothing qualifies.
///
/// Deliberately private: [`relay_pump_decision`] is the entry point, so the
/// enrolment check cannot be skipped.
fn select_next(gate: RelayGate, entries: &[RelayEntry], now_ms: i64) -> Option<String> {
    entries
        .iter()
        .filter(|e| e.status == RelayStatus::Pending)
        .filter(|e| row_allowed(gate, e.kind))
        .filter(|e| !is_stale(e, now_ms))
        .filter(|e| e.next_attempt <= now_ms)
        .min_by_key(|e| (e.next_attempt, e.created_at))
        .map(|e| e.id.clone())
}

// ─────────────────────────────────────────────────────────────────────────────
//   Freshness and the bound
// ─────────────────────────────────────────────────────────────────────────────

/// How old a row of this kind may be, or `None` for the kind that never goes
/// stale. See this module's header for why `unsubscribe` is that kind.
pub fn max_age_ms(kind: RelayKind) -> Option<i64> {
    match kind {
        RelayKind::Subscribe => Some(RELAY_SUBSCRIBE_MAX_AGE_MS),
        RelayKind::Send => Some(RELAY_EVENT_MAX_AGE_MS),
        RelayKind::Unsubscribe => None,
    }
}

/// Whether this row has outlived its usefulness and must be dropped unsent.
///
/// A clock that jumped BACKWARDS (a laptop correcting its time after a long
/// sleep) makes `now` earlier than `created_at`; the saturating subtraction
/// treats that as age zero, so a time correction can never age a row out.
pub fn is_stale(entry: &RelayEntry, now_ms: i64) -> bool {
    match max_age_ms(entry.kind) {
        Some(max) => now_ms.saturating_sub(entry.created_at) > max,
        None => false,
    }
}

/// Which rows the freshness sweep should delete, oldest first.
///
/// A pure list rather than a mutation so the shell can log what it dropped —
/// a queue that silently empties itself is indistinguishable from one that
/// delivered everything.
pub fn stale_victims(entries: &[RelayEntry], now_ms: i64) -> Vec<String> {
    let mut victims: Vec<&RelayEntry> = entries.iter().filter(|e| is_stale(e, now_ms)).collect();
    victims.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    victims.into_iter().map(|e| e.id.clone()).collect()
}

/// Which row ids must go so at most `cap` remain, oldest first.
///
/// Ordered by `created_at`, then `id` so the answer is deterministic when two
/// rows were queued in the same millisecond — the shape of
/// [`crate::telemetry::queue::overflow_victims`], and for the same reason: when
/// the queue is full, the newest rows are the better description of what is
/// happening to this machine.
pub fn overflow_victims(entries: &[RelayEntry], cap: usize) -> Vec<String> {
    if entries.len() <= cap {
        return Vec::new();
    }
    let mut by_age: Vec<&RelayEntry> = entries.iter().collect();
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

// ─────────────────────────────────────────────────────────────────────────────
//   The transitions
// ─────────────────────────────────────────────────────────────────────────────
//
// Four functions, and every one of them is [`crate::telemetry::queue`]'s with
// [`RelayEntry`] substituted for `TelemetryEntry`. They are re-stated rather
// than shared because Rust has no way to write them once over two structs
// without a trait whose only implementors are these two — and the ladder they
// step through, which is the part with a judgement in it, IS shared (see the
// module header). Their tests below assert against the imported
// [`BACKOFF_STEPS_MS`], so a change to the ladder shows up here rather than
// producing two queues that retry differently.

/// Reset any row left in `Sending` back to `Pending`, returning how many.
///
/// Call ONCE at startup: a row only reaches `Sending` while a request is in
/// flight, so at boot every one of them was stranded by a force-quit. Without
/// this, a machine killed mid-send never sends that row again — and for an
/// `unsubscribe` row that means the endpoint goes on mailing somebody who asked
/// it to stop.
pub fn reset_stale_sending(entries: &mut [RelayEntry]) -> usize {
    let mut reset = 0;
    for e in entries.iter_mut() {
        if e.status == RelayStatus::Sending {
            e.status = RelayStatus::Pending;
            reset += 1;
        }
    }
    reset
}

/// Transition a row to `Sending` and count the attempt, just BEFORE the request.
///
/// Counting first is what makes a crash mid-request cost an attempt rather than
/// being free: a row that could be tried, killed, and tried again without the
/// count moving would retry forever.
pub fn mark_sending(entries: &mut [RelayEntry], id: &str) {
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.status = RelayStatus::Sending;
        e.attempts += 1;
    }
}

/// A delivered row leaves the queue. Returns `true` if one was removed.
pub fn on_success(entries: &mut Vec<RelayEntry>, id: &str) -> bool {
    let before = entries.len();
    entries.retain(|e| e.id != id);
    entries.len() != before
}

/// Apply a failed attempt: back off, or give up at [`MAX_ATTEMPTS`].
///
/// `error` is stored so the panel can say why nothing has arrived. Attempts are
/// incremented by [`mark_sending`] before the attempt, so by the time this runs
/// `attempts` already counts this try.
pub fn on_failure(entries: &mut [RelayEntry], id: &str, error: impl Into<String>, now_ms: i64) {
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.last_error = Some(error.into());
        if e.attempts >= MAX_ATTEMPTS {
            e.status = RelayStatus::Failed;
        } else {
            e.status = RelayStatus::Pending;
            let idx = (e.attempts.saturating_sub(1) as usize).min(BACKOFF_STEPS_MS.len() - 1);
            e.next_attempt = now_ms + BACKOFF_STEPS_MS[idx];
        }
    }
}

/// Drop a row the endpoint will never accept. Returns `true` if one went.
///
/// The counterpart to [`on_failure`]. The ladder answers "the church wifi is
/// down"; it is exactly wrong for "this request is malformed", which is
/// malformed the same way all six times. The endpoint's half of the contract is
/// `sunday-telemetry/src/notify.ts`: a 400 names the offending field and is not
/// retryable, and 429 is the one transient 4xx.
///
/// Nothing is kept for the panel, unlike an exhausted ladder. A row that ran out
/// of attempts describes something the user might recognise — a network that has
/// been down all day. A schema rejection describes a disagreement between this
/// build and the endpoint, which they can neither cause nor fix.
pub fn on_permanent_failure(entries: &mut Vec<RelayEntry>, id: &str) -> bool {
    let before = entries.len();
    entries.retain(|e| e.id != id);
    entries.len() != before
}

// ─────────────────────────────────────────────────────────────────────────────
//   "Have we already said this?"
// ─────────────────────────────────────────────────────────────────────────────

/// Which once-policy applies to an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenScope {
    /// A recording error. Repeatable news: the same device can drop out again
    /// next Sunday, and that is worth hearing about again.
    Failure,
    /// A scheduled occurrence that was never recorded. Names a moment in the
    /// past that cannot happen twice.
    Missed,
    /// A finished recording. Likewise.
    Receipt,
}

impl SeenScope {
    /// Stable lowercase label — the `scope` column of `notify_seen`.
    pub fn as_str(self) -> &'static str {
        match self {
            SeenScope::Failure => "failure",
            SeenScope::Missed => "missed",
            SeenScope::Receipt => "receipt",
        }
    }
}

/// Whether this event should be SUPPRESSED because it has already been reported.
///
/// `true` means "do not send". Two policies, and the difference is not a tuning
/// choice — it follows from what the key means:
///
///   - [`SeenScope::Failure`] keys a repeatable event (recipient + error), so it
///     is a WINDOW: suppress inside [`ALERT_THROTTLE_MS`], the same ten minutes
///     [`crate::email::AlertGate`] applies to SMTP, imported so the two cannot
///     drift. Ten identical device drop-outs in ten minutes are one mail; the
///     same drop-out next Sunday is a new one.
///   - [`SeenScope::Missed`] and [`SeenScope::Receipt`] key a single occurrence
///     in time, so it is ONCE, full stop: any recorded sighting suppresses.
///     `check_missed` runs at startup and after every wake, so a durable row is
///     the only thing standing between one Sunday and five identical e-mails
///     about it — which is precisely why this lives in a table and not, like
///     `AlertGate`, in RAM that a restart empties.
///
/// A clock that jumped backwards yields a zero-length gap, which suppresses.
/// That direction is deliberate: too quiet for ten minutes after a time
/// correction, never a duplicate.
pub fn seen_decision(scope: SeenScope, last_seen_ms: Option<i64>, now_ms: i64) -> bool {
    match (scope, last_seen_ms) {
        (_, None) => false,
        (SeenScope::Failure, Some(seen)) => now_ms.saturating_sub(seen) < ALERT_THROTTLE_MS,
        (SeenScope::Missed | SeenScope::Receipt, Some(_)) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: RelayKind, created_at: i64) -> RelayEntry {
        RelayEntry {
            id: id.to_string(),
            created_at,
            kind,
            event: match kind {
                RelayKind::Send => Some(RelayMessageKind::Failure),
                _ => None,
            },
            dedup_key: format!("{}:{created_at}", kind.as_str()),
            payload_json: "{}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: RelayStatus::Pending,
        }
    }

    fn confirmed() -> RelayGate {
        RelayGate {
            enrolled: true,
            confirmed: true,
            suppressed: false,
        }
    }

    fn pending_confirmation() -> RelayGate {
        RelayGate {
            enrolled: true,
            confirmed: false,
            suppressed: false,
        }
    }

    // ── The gate ─────────────────────────────────────────────────────────────

    #[test]
    fn without_a_subscription_the_pump_never_selects_anything() {
        // The guarantee as a pure property: whatever the queue holds and
        // whatever the clock says, no local record yields Blocked. There is no
        // combination of inputs that produces a Send.
        let queues: Vec<Vec<RelayEntry>> = vec![
            vec![],
            vec![entry("a", RelayKind::Subscribe, 0)],
            vec![entry("a", RelayKind::Send, 0)],
            vec![entry("a", RelayKind::Unsubscribe, 0)],
            vec![
                entry("a", RelayKind::Subscribe, 0),
                entry("b", RelayKind::Send, 1),
            ],
        ];
        for gate in [
            RelayGate::default(),
            RelayGate {
                enrolled: false,
                confirmed: true,
                suppressed: false,
            },
        ] {
            for q in &queues {
                for now in [i64::MIN, -1, 0, 1, 1_800_000_000_000, i64::MAX] {
                    assert_eq!(
                        relay_pump_decision(gate, q, now),
                        RelayPumpDecision::Blocked,
                        "not enrolled must block regardless of queue {q:?} / now {now}"
                    );
                }
            }
        }
        // The positive control, so the assertion above is not vacuous.
        assert_eq!(
            relay_pump_decision(confirmed(), &queues[2], 1_000),
            RelayPumpDecision::Send("a".into())
        );
    }

    #[test]
    fn an_unconfirmed_subscription_may_ask_but_not_tell() {
        // The double opt-in promise, as a property of the queue rather than a
        // discipline at the call sites: the row that REQUESTS confirmation goes
        // out, and nothing else does.
        let gate = pending_confirmation();
        for kind in [RelayKind::Subscribe, RelayKind::Unsubscribe] {
            assert_eq!(
                relay_pump_decision(gate, &[entry("a", kind, 0)], 1_000),
                RelayPumpDecision::Send("a".into()),
                "{} must leave before confirmation",
                kind.as_str()
            );
        }
        assert_eq!(
            relay_pump_decision(gate, &[entry("a", RelayKind::Send, 0)], 1_000),
            RelayPumpDecision::Idle,
            "no notification may reach an address that has not said yes"
        );
        // …and the same row does leave once the link has been clicked.
        assert_eq!(
            relay_pump_decision(confirmed(), &[entry("a", RelayKind::Send, 0)], 1_000),
            RelayPumpDecision::Send("a".into())
        );
    }

    #[test]
    fn a_suppressed_address_still_gets_to_be_unsubscribed() {
        // 410 recipient_suppressed: the address bounces. Notifications stop —
        // but the row that tells the endpoint to forget the address entirely
        // must still leave, or a suppressed subscriber is stuck being a
        // subscriber.
        let gate = RelayGate {
            enrolled: true,
            confirmed: true,
            suppressed: true,
        };
        assert_eq!(
            relay_pump_decision(gate, &[entry("a", RelayKind::Send, 0)], 1_000),
            RelayPumpDecision::Idle
        );
        assert_eq!(
            relay_pump_decision(gate, &[entry("a", RelayKind::Unsubscribe, 0)], 1_000),
            RelayPumpDecision::Send("a".into())
        );
    }

    #[test]
    fn an_unsubscribe_leaves_while_the_local_record_survives() {
        // The trap this pins, for the store that A2 writes: `enrolled` is the
        // gate, so clearing the local record at the moment the user clicks
        // "unsubscribe" would strand the very row that carries the request —
        // and the endpoint would keep sending. The record is cleared when the
        // row LEAVES.
        let q = [entry("bye", RelayKind::Unsubscribe, 0)];
        assert_eq!(
            relay_pump_decision(confirmed(), &q, 1_000),
            RelayPumpDecision::Send("bye".into())
        );
        assert_eq!(
            relay_pump_decision(RelayGate::default(), &q, 1_000),
            RelayPumpDecision::Blocked,
            "cleared too early, and the request never leaves"
        );
    }

    #[test]
    fn the_pump_is_idle_until_something_is_due_and_picks_the_earliest() {
        let q = vec![entry("a", RelayKind::Send, 10_000)];
        assert_eq!(
            relay_pump_decision(confirmed(), &q, 9_999),
            RelayPumpDecision::Idle
        );
        assert_eq!(
            relay_pump_decision(confirmed(), &q, 10_000),
            RelayPumpDecision::Send("a".into())
        );
        assert_eq!(
            relay_pump_decision(confirmed(), &[], 10_000),
            RelayPumpDecision::Idle
        );

        let mut later = entry("later", RelayKind::Send, 5_000);
        later.next_attempt = 5_000;
        let mut earlier = entry("earlier", RelayKind::Send, 9_000);
        earlier.next_attempt = 1_000;
        assert_eq!(
            relay_pump_decision(confirmed(), &[later, earlier], 10_000),
            RelayPumpDecision::Send("earlier".into()),
            "next_attempt orders the queue, not insertion order"
        );
    }

    #[test]
    fn only_pending_rows_are_selected() {
        for status in [RelayStatus::Sending, RelayStatus::Failed] {
            let mut e = entry("a", RelayKind::Send, 0);
            e.status = status;
            assert_eq!(
                relay_pump_decision(confirmed(), &[e], 1_000_000),
                RelayPumpDecision::Idle,
                "{status:?} must not be picked up"
            );
        }
    }

    #[test]
    fn an_ineligible_row_does_not_wedge_the_queue_behind_it() {
        // An unconfirmed subscription with a failure alert queued first: the
        // subscribe row behind it must still be found on this very iteration.
        let mut blocked = entry("blocked", RelayKind::Send, 0);
        blocked.next_attempt = 0;
        let mut ask = entry("ask", RelayKind::Subscribe, 1);
        ask.next_attempt = 1;
        assert_eq!(
            relay_pump_decision(pending_confirmation(), &[blocked, ask], 1_000),
            RelayPumpDecision::Send("ask".into())
        );
    }

    // ── Freshness ────────────────────────────────────────────────────────────

    #[test]
    fn an_event_older_than_six_hours_is_not_worth_sending() {
        let e = entry("a", RelayKind::Send, 0);
        assert!(!is_stale(&e, RELAY_EVENT_MAX_AGE_MS));
        assert!(is_stale(&e, RELAY_EVENT_MAX_AGE_MS + 1));
        // …and a stale row is never selected, whether or not the sweeper has
        // run yet. Belt and braces on purpose: the alternative is a window
        // between "expired" and "deleted" in which it can still leave.
        assert_eq!(
            relay_pump_decision(confirmed(), &[e], RELAY_EVENT_MAX_AGE_MS + 1),
            RelayPumpDecision::Idle
        );
    }

    #[test]
    fn a_subscribe_request_expires_with_the_endpoints_own_retention() {
        let e = entry("a", RelayKind::Subscribe, 0);
        assert!(!is_stale(&e, RELAY_SUBSCRIBE_MAX_AGE_MS));
        assert!(is_stale(&e, RELAY_SUBSCRIBE_MAX_AGE_MS + 1));
        // The two numbers are one number, and a subscribe row lives far longer
        // than an event. Const-block asserts: relationships between constants
        // belong to the compiler, not the test runner (the same idiom the
        // graduated disk thresholds use in `notify`).
        const {
            assert!(RELAY_SUBSCRIBE_MAX_AGE_MS == CONFIRM_LINK_TTL_DAYS * 24 * 60 * 60 * 1_000);
            assert!(RELAY_SUBSCRIBE_MAX_AGE_MS > RELAY_EVENT_MAX_AGE_MS);
        }
    }

    #[test]
    fn a_request_to_stop_never_goes_stale() {
        let e = entry("a", RelayKind::Unsubscribe, 0);
        assert_eq!(max_age_ms(RelayKind::Unsubscribe), None);
        assert!(!is_stale(&e, i64::MAX));
        assert_eq!(
            relay_pump_decision(confirmed(), &[e], i64::MAX),
            RelayPumpDecision::Send("a".into()),
            "dropping this silently would leave the endpoint sending"
        );
    }

    #[test]
    fn a_backwards_clock_never_ages_a_row_out() {
        // A laptop that wakes and corrects its time can put `now` before
        // `created_at`. Age zero, not age i64::MAX.
        let e = entry("a", RelayKind::Send, 1_800_000_000_000);
        assert!(!is_stale(&e, 0));
        assert!(!is_stale(&e, i64::MIN));
    }

    #[test]
    fn the_stale_sweep_lists_its_victims_oldest_first() {
        let now = RELAY_EVENT_MAX_AGE_MS * 2;
        let q = vec![
            entry("fresh", RelayKind::Send, now),
            entry("old-b", RelayKind::Send, 5),
            entry("old-a", RelayKind::Send, 5),
            entry("ancient", RelayKind::Send, 0),
            entry("stop", RelayKind::Unsubscribe, 0),
        ];
        assert_eq!(
            stale_victims(&q, now),
            vec![
                "ancient".to_string(),
                "old-a".to_string(),
                "old-b".to_string()
            ],
            "deterministic within a millisecond, and the unsubscribe survives"
        );
        assert!(stale_victims(&[], now).is_empty());
    }

    // ── The bound ────────────────────────────────────────────────────────────

    #[test]
    fn the_queue_bound_drops_the_oldest() {
        let q: Vec<RelayEntry> = (0..RELAY_QUEUE_MAX + 3)
            .map(|i| entry(&format!("id-{i:03}"), RelayKind::Send, i as i64))
            .collect();
        let victims = overflow_victims(&q, RELAY_QUEUE_MAX);
        assert_eq!(victims, vec!["id-000", "id-001", "id-002"]);
        assert!(overflow_victims(&q[..RELAY_QUEUE_MAX], RELAY_QUEUE_MAX).is_empty());
        assert!(overflow_victims(&[], RELAY_QUEUE_MAX).is_empty());
    }

    #[test]
    fn the_bound_is_deterministic_when_rows_share_a_millisecond() {
        let q = vec![
            entry("b", RelayKind::Send, 5),
            entry("a", RelayKind::Send, 5),
            entry("c", RelayKind::Send, 9),
        ];
        assert_eq!(
            overflow_victims(&q, 1),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn the_relay_queue_is_smaller_than_the_telemetry_one_and_shares_its_ladder() {
        // The ladder is imported, not re-typed: one definition, no drift.
        assert_eq!(BACKOFF_STEPS_MS.len(), MAX_ATTEMPTS as usize);
        assert_eq!(BACKOFF_STEPS_MS, crate::telemetry::queue::BACKOFF_STEPS_MS);
        const {
            assert!(RELAY_QUEUE_MAX < crate::telemetry::queue::QUEUE_MAX);
        }
    }

    // ── The transitions ──────────────────────────────────────────────────────

    #[test]
    fn an_attempt_is_counted_before_it_is_made() {
        // A crash mid-request must cost an attempt. If the count moved only
        // AFTER a verdict, a row that hangs the process would be retried
        // forever with `attempts` stuck at zero.
        let mut q = vec![entry("a", RelayKind::Send, 0)];
        mark_sending(&mut q, "a");
        assert_eq!(q[0].attempts, 1);
        assert_eq!(q[0].status, RelayStatus::Sending);
        mark_sending(&mut q, "missing"); // a stale id is a no-op, not a panic
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn a_delivered_row_and_a_refused_one_both_leave() {
        let mut q = vec![
            entry("a", RelayKind::Send, 0),
            entry("b", RelayKind::Send, 1),
        ];
        assert!(on_success(&mut q, "a"));
        assert!(!on_success(&mut q, "a"), "already gone");
        assert!(on_permanent_failure(&mut q, "b"));
        assert!(q.is_empty());
    }

    #[test]
    fn a_transient_failure_walks_the_shared_ladder_and_then_gives_up() {
        let mut q = vec![entry("a", RelayKind::Send, 0)];
        for step in 0..MAX_ATTEMPTS {
            mark_sending(&mut q, "a");
            on_failure(&mut q, "a", "no route to host", 100_000);
            if step + 1 < MAX_ATTEMPTS {
                assert_eq!(q[0].status, RelayStatus::Pending);
                assert_eq!(
                    q[0].next_attempt,
                    100_000 + BACKOFF_STEPS_MS[step as usize],
                    "rung {step} must come from the imported ladder"
                );
            }
        }
        assert_eq!(
            q[0].status,
            RelayStatus::Failed,
            "the ladder ends after MAX_ATTEMPTS"
        );
        assert_eq!(q[0].last_error.as_deref(), Some("no route to host"));
        // …and a Failed row is never selected again.
        assert_eq!(
            relay_pump_decision(confirmed(), &q, i64::MAX / 2),
            RelayPumpDecision::Idle
        );
    }

    #[test]
    fn a_force_quit_mid_send_is_recoverable() {
        // Not a nicety for the relay: a stranded `unsubscribe` row means the
        // endpoint keeps sending to somebody who asked it to stop.
        let mut q = vec![
            entry("a", RelayKind::Unsubscribe, 0),
            entry("b", RelayKind::Send, 1),
        ];
        q[0].status = RelayStatus::Sending;
        assert_eq!(reset_stale_sending(&mut q), 1);
        assert!(q.iter().all(|e| e.status == RelayStatus::Pending));
        assert_eq!(reset_stale_sending(&mut q), 0);
        assert_eq!(
            relay_pump_decision(confirmed(), &q, 1_000),
            RelayPumpDecision::Send("a".into())
        );
    }

    // ── Once-semantics ───────────────────────────────────────────────────────

    #[test]
    fn an_unseen_event_is_never_suppressed() {
        for scope in [SeenScope::Failure, SeenScope::Missed, SeenScope::Receipt] {
            assert!(
                !seen_decision(scope, None, 1_800_000_000_000),
                "{scope:?} with no sighting must send"
            );
        }
    }

    #[test]
    fn a_failure_is_suppressed_for_a_window_and_then_speaks_again() {
        let seen = 1_000i64;
        assert!(seen_decision(SeenScope::Failure, Some(seen), seen));
        assert!(seen_decision(
            SeenScope::Failure,
            Some(seen),
            seen + ALERT_THROTTLE_MS - 1
        ));
        assert!(!seen_decision(
            SeenScope::Failure,
            Some(seen),
            seen + ALERT_THROTTLE_MS
        ));
        // The window is the SMTP gate's, imported rather than restated: the two
        // pipes must not throttle the same person differently.
        assert_eq!(ALERT_THROTTLE_MS, crate::email::ALERT_THROTTLE_MS);
    }

    #[test]
    fn a_missed_sunday_and_a_receipt_are_said_exactly_once() {
        // check_missed runs at startup and after every wake. Without a durable
        // sighting, one Sunday is five identical e-mails; with one, any sighting
        // at all — a second ago or last year — closes the subject for good.
        for scope in [SeenScope::Missed, SeenScope::Receipt] {
            for (seen, now) in [
                (0i64, 0i64),
                (0, 1),
                (0, 1_800_000_000_000),
                (1_800_000_000_000, 0),
            ] {
                assert!(
                    seen_decision(scope, Some(seen), now),
                    "{scope:?} must be once and only once (seen {seen}, now {now})"
                );
            }
        }
    }

    #[test]
    fn a_backwards_clock_stays_quiet_rather_than_repeating_itself() {
        // Too quiet for ten minutes after a time correction is a cost; a
        // duplicate alert is a defect.
        assert!(seen_decision(SeenScope::Failure, Some(1_000), 0));
        assert!(seen_decision(SeenScope::Failure, Some(1_000), i64::MIN));
    }

    // ── The wire labels ──────────────────────────────────────────────────────

    #[test]
    fn the_row_and_scope_labels_are_stable_lowercase_words() {
        assert_eq!(RelayKind::Subscribe.as_str(), "subscribe");
        assert_eq!(RelayKind::Send.as_str(), "send");
        assert_eq!(RelayKind::Unsubscribe.as_str(), "unsubscribe");
        assert_eq!(SeenScope::Failure.as_str(), "failure");
        assert_eq!(SeenScope::Missed.as_str(), "missed");
        assert_eq!(SeenScope::Receipt.as_str(), "receipt");
        for kind in [
            RelayKind::Subscribe,
            RelayKind::Send,
            RelayKind::Unsubscribe,
        ] {
            let json = serde_json::to_string(&kind).expect("serialise");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
        // A `notify_seen` scope is named after the message kind it guards, so
        // the two vocabularies cannot describe the same event differently.
        assert_eq!(
            SeenScope::Failure.as_str(),
            RelayMessageKind::Failure.as_str()
        );
        assert_eq!(
            SeenScope::Missed.as_str(),
            RelayMessageKind::Missed.as_str()
        );
        assert_eq!(
            SeenScope::Receipt.as_str(),
            RelayMessageKind::Receipt.as_str()
        );
    }

    #[test]
    fn a_queued_row_round_trips_through_its_stored_shape() {
        let e = entry("a", RelayKind::Send, 42);
        let json = serde_json::to_string(&e).expect("serialise");
        assert!(json.contains("\"kind\":\"send\""));
        assert!(json.contains("\"event\":\"failure\""));
        assert!(json.contains("\"createdAt\":42"));
        assert_eq!(
            serde_json::from_str::<RelayEntry>(&json).expect("round-trip"),
            e
        );
        // A row that causes no mail carries no event.
        let bye = entry("b", RelayKind::Unsubscribe, 0);
        assert!(serde_json::to_string(&bye)
            .expect("serialise")
            .contains("\"event\":null"));
    }
}
