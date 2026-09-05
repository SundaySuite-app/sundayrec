//! The relay's socket, its reading of what came back, and the pump loop.
//!
//! Shaped after [`crate::telemetry::sender`] and
//! [`crate::telemetry::http_sender`] together — the same pure-decision /
//! thin-socket split, the same "the gate is the FIRST argument" discipline, the
//! same supervised loop. What differs is forced by the feature, and each
//! difference is called out where it happens.
//!
//! ## The gate is structural, not careful
//!
//! "A machine that never signed up must be indistinguishable, on the network,
//! from one that never heard of the relay" is easy to state and easy to violate,
//! because the violation looks like ordinary code: read the queue, build a
//! client, THEN check whether there is a subscription — by which time a
//! connection pool exists and, on some stacks, a DNS lookup is already in
//! flight. So the subscription is read first and is the first argument of
//! `relay_pump_decision`, which answers `Blocked` before the queue is touched;
//! [`pump_once`] does not even load the queue when nothing is enrolled. The test
//! injects a sender that PANICS if it is called and runs the whole flow with no
//! record, with a positive control on the same queue so the assertion cannot
//! pass vacuously.
//!
//! ## Three failure kinds, not two
//!
//! Telemetry needs `Transient` and `Permanent`. The relay needs a third, and it
//! is the one that matters most to the person using it: `410
//! recipient_suppressed` means the address bounces or somebody marked a mail as
//! spam, and the endpoint will not try again. Treated as a plain permanent drop,
//! that is a volunteer who believes they are covered and hears nothing, forever,
//! with no way to find out. So it flips the local record to
//! [`RelaySubscriptionState::Suppressed`], which the panel says out loud.
//!
//! ## Sixty seconds, and a doorbell
//!
//! The telemetry pump sleeps a minute between beats because nothing it carries
//! is urgent. An alert about a service that is failing right now IS urgent, and
//! "up to a minute late" is the wrong default for it. So the loop waits on a
//! `select!` between the interval and a [`Notify`] that every enqueue path
//! rings ([`kick`]) — alerts leave in seconds, and the interval remains as the
//! thing that retries a backed-off row and polls for a confirmation.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use sundayrec_core::relay::{
    self, RelayEntry, RelayKind, RelayPumpDecision, RELAY_QUEUE_MAX, RELAY_SUBSCRIBE_MAX_AGE_MS,
};

use super::config::{RelayEndpoint, WRITE_KEY_HEADER};
use super::{gate_of, store, wire, RelaySubscription, RelaySubscriptionState};
use crate::error::AppResult;
use crate::telemetry::http_sender::{classify, transport_error};
use crate::telemetry::sender::SendFailure;
use crate::util::http_client;

/// This app's slug in the Worker's app table — the `:app` segment of every
/// app-scoped relay route.
pub const RELAY_APP_SLUG: &str = "sundayrec";

/// A per-request cap well under the shared client's 120 s, matching the
/// telemetry sender's for the same reason: this is a few kilobytes to an edge
/// worker, and if it has not answered in fifteen seconds it is not going to.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the pump wakes on its own. See the module header for why it also
/// wakes on a doorbell.
pub const PUMP_INTERVAL: Duration = Duration::from_secs(60);

/// How often the endpoint is asked whether the confirmation link has been
/// clicked, while the local record still says `pending`.
///
/// Fifteen minutes. The link is usually clicked on a PHONE, which this machine
/// cannot observe at all — so without a poll the gate would go on silently
/// dropping every alert for somebody who confirmed an hour ago. Fifteen is the
/// telemetry drain's interval and is chosen the same way: invisible in cost, and
/// well inside the window in which anybody would think to look at the panel
/// again. The panel's own open also polls (A5), so the impatient case is
/// covered by the user, not by a tighter timer.
pub const STATUS_POLL_INTERVAL_MS: i64 = 15 * 60 * 1_000;

/// How long a `notify_seen` sighting is kept.
///
/// Twice the longest freshness cap in `sundayrec_core::relay`: once a row of any
/// kind would be dropped as stale rather than sent, the sighting that would have
/// suppressed it has nothing left to suppress. Doubling it means a clock that
/// wandered cannot turn the sweep into a duplicate e-mail.
pub const SEEN_RETENTION_MS: i64 = 2 * RELAY_SUBSCRIBE_MAX_AGE_MS;

// ─────────────────────────────────────────────────────────────────────────────
//   The seam
// ─────────────────────────────────────────────────────────────────────────────

/// Why a request did not succeed — and what that means for the row AND for the
/// subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayFailure {
    /// The network, or the endpoint, or the moment. Back off and retry.
    Transient(String),
    /// This request will never be accepted. Drop the row.
    Permanent(String),
    /// `410` and the endpoint named the recipient: the address bounces, or
    /// somebody complained. Drop the row AND record it locally, so the panel can
    /// say why nothing is arriving instead of leaving a volunteer to discover it
    /// the next time a service fails.
    Suppressed(String),
}

impl RelayFailure {
    /// The message for the log and the panel.
    pub fn message(&self) -> &str {
        match self {
            Self::Transient(m) | Self::Permanent(m) | Self::Suppressed(m) => m,
        }
    }
}

/// The future a [`RelaySender`] returns. Boxed by hand rather than via an
/// `async-trait` dependency — one type alias against a proc-macro crate, the
/// same trade [`crate::telemetry::sender::SendFuture`] makes.
pub type RelayFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RelayFailure>> + Send + 'a>>;

/// What the endpoint says about a subscription.
///
/// Every field is optional and unknown fields are ignored, which is the relay's
/// OPTIONAL-forever law stated in a type: the Worker ships first and may add
/// fields, and a client that refused to parse an answer with something new in it
/// would turn a Worker deploy into a fleet that stops noticing confirmations.
/// Both a `state` word and the timestamps are read because either alone is
/// enough for the Worker to be right — see [`RemoteStatus::state_now`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteStatus {
    /// The endpoint's own word, if it sends one.
    pub state: Option<String>,
    /// Unix ms the link was clicked.
    pub confirmed_at: Option<i64>,
    /// Unix ms the address was suppressed.
    pub suppressed_at: Option<i64>,
}

impl RemoteStatus {
    /// What the local record should say, or `None` when the answer carried
    /// nothing that changes it.
    ///
    /// Suppression is checked first: an address that both confirmed and later
    /// bounced is suppressed, and reading the fields in the other order would
    /// re-open a subscription the endpoint has closed.
    pub fn state_now(&self) -> Option<RelaySubscriptionState> {
        let word = self.state.as_deref();
        if self.suppressed_at.is_some() || word == Some("suppressed") {
            return Some(RelaySubscriptionState::Suppressed);
        }
        if self.confirmed_at.is_some() || word == Some("confirmed") {
            return Some(RelaySubscriptionState::Confirmed);
        }
        None
    }
}

/// Perform one relay operation. The whole network seam, in two methods.
///
/// `execute` takes the ROW rather than a URL and a body, because which request a
/// row becomes is a property of the row (`RelayKind`) and a caller that had to
/// pick would eventually pick wrong — a `send` posted to the subscribe route is
/// a 400, which is a permanent drop, which is a lost alert. The pump therefore
/// has no routing logic at all.
pub trait RelaySender: Send + Sync {
    /// Carry out what this row says, once. `Ok(())` means the endpoint accepted
    /// it.
    fn execute<'a>(&'a self, entry: &'a RelayEntry) -> RelayFuture<'a, ()>;
    /// Ask whether a subscription has been confirmed.
    fn fetch_status<'a>(&'a self, sub_id: &'a str) -> RelayFuture<'a, RemoteStatus>;
}

// ─────────────────────────────────────────────────────────────────────────────
//   The real sender
// ─────────────────────────────────────────────────────────────────────────────

/// Posts to the configured relay endpoint.
pub struct HttpRelaySender {
    client: reqwest::Client,
    endpoint: RelayEndpoint,
}

impl HttpRelaySender {
    /// Build a sender for `endpoint`, reusing the bounded rustls client the
    /// telemetry and update paths use — a bare `Client::new()` has NO timeout,
    /// so a server that accepts a request and never answers would wedge the pump
    /// forever.
    pub fn new(endpoint: RelayEndpoint) -> Self {
        Self {
            client: http_client(),
            endpoint,
        }
    }

    /// The endpoint this sender was built for — the panel needs it to render the
    /// confirmation and unsubscribe links.
    pub fn endpoint(&self) -> &RelayEndpoint {
        &self.endpoint
    }
}

/// Turn one answered request into the outbox's decision.
///
/// The body is read for client errors ONLY, and only far enough to find the
/// endpoint's `error` code. Everything else in it is the endpoint's business.
async fn finish(res: reqwest::Result<reqwest::Response>) -> Result<(), RelayFailure> {
    match res {
        Ok(r) => {
            let status = r.status();
            let body = if status.is_client_error() {
                r.text().await.unwrap_or_default()
            } else {
                String::new()
            };
            classify_relay(status, &body)
        }
        Err(e) => Err(RelayFailure::Transient(transport_error(&e))),
    }
}

impl RelaySender for HttpRelaySender {
    fn execute<'a>(&'a self, entry: &'a RelayEntry) -> RelayFuture<'a, ()> {
        Box::pin(async move {
            let req = match entry.kind {
                RelayKind::Subscribe => self
                    .client
                    .post(self.endpoint.subscribe_url())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(entry.payload_json.clone()),
                RelayKind::Send => self
                    .client
                    .post(self.endpoint.send_url())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(entry.payload_json.clone()),
                RelayKind::Unsubscribe => {
                    // The id lives in the ROW, not in the local record — by the
                    // time this succeeds the record is about to be deleted, so
                    // there would be nothing left to read it from.
                    let body: wire::UnsubscribeBody = serde_json::from_str(&entry.payload_json)
                        .map_err(|e| {
                            RelayFailure::Permanent(format!(
                                "the queued unsubscribe row is unreadable ({e})"
                            ))
                        })?;
                    self.client
                        .delete(self.endpoint.subscription_url(&body.sub_id))
                }
            };
            finish(
                req.header(WRITE_KEY_HEADER, &self.endpoint.write_key)
                    .timeout(REQUEST_TIMEOUT)
                    .send()
                    .await,
            )
            .await
        })
    }

    fn fetch_status<'a>(&'a self, sub_id: &'a str) -> RelayFuture<'a, RemoteStatus> {
        Box::pin(async move {
            let res = self
                .client
                .get(self.endpoint.subscription_url(sub_id))
                .header(WRITE_KEY_HEADER, &self.endpoint.write_key)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await;
            match res {
                Ok(r) => {
                    let status = r.status();
                    if !status.is_success() {
                        let body = if status.is_client_error() {
                            r.text().await.unwrap_or_default()
                        } else {
                            String::new()
                        };
                        return Err(classify_relay(status, &body).unwrap_err());
                    }
                    r.json::<RemoteStatus>().await.map_err(|e| {
                        // A 200 we cannot parse is not the row's fault and not
                        // worth a retry ladder; it is a client/endpoint
                        // disagreement, and the poll runs again in fifteen
                        // minutes anyway.
                        RelayFailure::Permanent(format!("unreadable status answer ({e})"))
                    })
                }
                Err(e) => Err(RelayFailure::Transient(transport_error(&e))),
            }
        })
    }
}

/// The endpoint error codes that mean "this ADDRESS is finished", not "this
/// request was wrong".
///
/// `recipient_suppressed` — it bounced, or somebody reported a mail as spam.
/// `not_allowed` — the endpoint refuses to send there at all. Both are facts
/// about the address that outlive the row, which is why they are the two that
/// change local state.
const SUPPRESSING_CODES: &[&str] = &["recipient_suppressed", "not_allowed"];

/// Turn one HTTP status (plus, for a 4xx, its body) into a decision.
///
/// The status half is [`crate::telemetry::http_sender::classify`], called rather
/// than re-implemented: the 2xx/400/401/413/408/429/5xx contract is the same
/// Worker's, and two tables would drift on the day one route's behaviour
/// changed. What is added here is the ONE case telemetry has no equivalent for —
/// see [`RelayFailure::Suppressed`] and the module header.
///
/// The 410 is required as well as the code. A `not_allowed` arriving with some
/// other status is a request that was refused; only `410 Gone` says the
/// recipient itself is gone, and letting a code alone flip local state would let
/// an unrelated refusal silently switch a working subscription off.
pub fn classify_relay(status: StatusCode, body: &str) -> Result<(), RelayFailure> {
    match classify(status) {
        Ok(()) => Ok(()),
        Err(SendFailure::Transient(m)) => Err(RelayFailure::Transient(m)),
        Err(SendFailure::Permanent(m)) => match error_code(body) {
            Some(code)
                if status == StatusCode::GONE && SUPPRESSING_CODES.contains(&code.as_str()) =>
            {
                Err(RelayFailure::Suppressed(format!(
                    "the endpoint will not send to this address ({code})"
                )))
            }
            _ => Err(RelayFailure::Permanent(m)),
        },
    }
}

/// The `error` field of an endpoint refusal, if the body is JSON that has one.
///
/// Best-effort by design: an HTML error page from something in front of the
/// Worker, an empty body, a truncated one — all read as "no code", which lands
/// on the plain permanent branch. A parser that could fail here would turn a
/// proxy's error page into a client bug.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The pump
// ─────────────────────────────────────────────────────────────────────────────

/// What one pump iteration did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPumpOutcome {
    /// No subscription exists on this machine. The queue was not read and the
    /// sender was not touched.
    Blocked,
    /// Nothing was both eligible and due.
    Idle,
    /// One row was delivered and removed.
    Sent,
    /// One attempt failed transiently; the row was backed off or retired.
    Failed,
    /// The endpoint refused the row permanently, so it was discarded without a
    /// retry.
    Dropped,
}

/// Run one iteration of the pump.
///
/// The subscription is read FIRST and exactly once — it is the gate, and reading
/// it per-row would let a record cleared mid-beat change the rules underneath a
/// request already in flight.
pub async fn pump_once(
    pool: &SqlitePool,
    sender: &dyn RelaySender,
    now_ms: i64,
) -> AppResult<RelayPumpOutcome> {
    let subscription = store::subscription_get(pool).await?;
    let gate = gate_of(subscription.as_ref());

    // Not enrolled: no queue read, no hostname, no socket. See the module docs.
    if !gate.enrolled {
        return Ok(RelayPumpOutcome::Blocked);
    }

    let mut entries = store::load_queue(pool).await?;

    // The freshness sweep, before the selection rather than after it. A row that
    // has outlived its usefulness is deleted and LOGGED — a queue that quietly
    // empties itself is indistinguishable from one that delivered everything.
    for victim in relay::stale_victims(&entries, now_ms) {
        if let Some(e) = entries.iter().find(|e| e.id == victim) {
            tracing::info!(
                kind = e.kind.as_str(),
                age_ms = now_ms.saturating_sub(e.created_at),
                "relay: dropping a queued message that is too old to be worth sending"
            );
        }
        store::delete_entry(pool, &victim).await?;
    }
    entries.retain(|e| !relay::is_stale(e, now_ms));

    let id = match relay::relay_pump_decision(gate, &entries, now_ms) {
        RelayPumpDecision::Blocked => return Ok(RelayPumpOutcome::Blocked),
        RelayPumpDecision::Idle => return Ok(RelayPumpOutcome::Idle),
        RelayPumpDecision::Send(id) => id,
    };

    // Count the attempt BEFORE making it: a crash mid-request must cost an
    // attempt, or a request that kills the process is retried forever.
    relay::mark_sending(&mut entries, &id);
    let Some(entry) = entries.iter().find(|e| e.id == id).cloned() else {
        return Ok(RelayPumpOutcome::Idle);
    };
    store::upsert_entry(pool, &entry).await?;

    match sender.execute(&entry).await {
        Ok(()) => {
            relay::on_success(&mut entries, &id);
            store::delete_entry(pool, &id).await?;
            on_row_left(pool, &entry).await?;
            Ok(RelayPumpOutcome::Sent)
        }
        Err(RelayFailure::Suppressed(msg)) => {
            // The address itself is finished. The row goes like any permanent
            // refusal, but the SUBSCRIPTION changes too — this is the whole
            // reason there are three failure kinds and not two.
            tracing::warn!("relay: {msg}; no further notifications will be sent to this address");
            if let Some(mut sub) = subscription {
                sub.state = RelaySubscriptionState::Suppressed;
                store::subscription_set(pool, &sub).await?;
            }
            relay::on_permanent_failure(&mut entries, &id);
            store::delete_entry(pool, &id).await?;
            on_row_left(pool, &entry).await?;
            Ok(RelayPumpOutcome::Dropped)
        }
        Err(RelayFailure::Permanent(msg)) => {
            tracing::warn!(
                kind = entry.kind.as_str(),
                "relay: the endpoint permanently rejected a message ({msg}); it has been \
                 discarded rather than retried. This is a client/endpoint disagreement — see \
                 the endpoint's rejection log for the offending field."
            );
            relay::on_permanent_failure(&mut entries, &id);
            store::delete_entry(pool, &id).await?;
            on_row_left(pool, &entry).await?;
            Ok(RelayPumpOutcome::Dropped)
        }
        Err(RelayFailure::Transient(msg)) => {
            relay::on_failure(&mut entries, &id, msg, now_ms);
            if let Some(updated) = entries.iter().find(|x| x.id == id) {
                store::upsert_entry(pool, updated).await?;
            }
            Ok(RelayPumpOutcome::Failed)
        }
    }
}

/// Bookkeeping for a row that has LEFT the queue — delivered or refused, it
/// makes no difference here.
///
/// ⚠️ This is the other half of the unsubscribe trap. The local record is the
/// pump's gate, so it must outlive the row that carries "stop sending me mail";
/// the moment that row is gone from the queue — by ANY route — the record has
/// done its job and is deleted. Deleting it earlier strands the request; never
/// deleting it leaves a machine that thinks it is still subscribed.
///
/// The "no unsubscribe row remains" check matters because a user can queue a
/// second one (a click, an error, a click again): clearing on the first
/// success would shut the gate on the second, which would then sit in the
/// outbox forever.
async fn on_row_left(pool: &SqlitePool, entry: &RelayEntry) -> AppResult<()> {
    if entry.kind != RelayKind::Unsubscribe {
        return Ok(());
    }
    if store::has_kind(pool, RelayKind::Unsubscribe).await? {
        return Ok(());
    }
    store::subscription_clear(pool).await?;
    tracing::info!("relay: unsubscribed — the local subscription record has been removed");
    Ok(())
}

/// Send everything that is due, newest backlog first, until the queue is quiet.
///
/// Bounded by [`RELAY_QUEUE_MAX`] so a pathological loop cannot spin. Stops on
/// the first transient failure: if one request could not reach the endpoint, the
/// next one on the same beat will not either, and trying anyway spends attempts
/// off the ladder for nothing. A PERMANENT drop keeps going — that row was
/// wrong, not the network.
pub async fn drain(pool: &SqlitePool, sender: &dyn RelaySender, now_ms: i64) -> AppResult<usize> {
    let mut sent = 0;
    for _ in 0..RELAY_QUEUE_MAX {
        match pump_once(pool, sender, now_ms).await? {
            RelayPumpOutcome::Sent => sent += 1,
            RelayPumpOutcome::Dropped => {}
            RelayPumpOutcome::Blocked | RelayPumpOutcome::Idle | RelayPumpOutcome::Failed => break,
        }
    }
    Ok(sent)
}

/// Whether the endpoint should be asked about this subscription right now.
///
/// Only while the local record says `pending`: a confirmed subscription needs no
/// poll (a later suppression arrives as a `410` on the next real send, which is
/// the moment it matters), and a suppressed one has nothing left to learn.
pub fn should_poll(sub: &RelaySubscription, now_ms: i64) -> bool {
    if sub.state != RelaySubscriptionState::Pending {
        return false;
    }
    match sub.last_checked {
        None => true,
        // Saturating, so a clock that jumped backwards produces a zero gap and
        // waits rather than polling on every beat.
        Some(t) => now_ms.saturating_sub(t) >= STATUS_POLL_INTERVAL_MS,
    }
}

/// Ask the endpoint whether the link has been clicked, and record the answer.
///
/// Returns the new state when it changed. Without this the gate would go on
/// silently dropping every alert for somebody who confirmed on their phone an
/// hour ago — the failure mode is invisible from inside the app, which is why it
/// gets a timer of its own rather than riding on a send.
pub async fn poll_status(
    pool: &SqlitePool,
    sender: &dyn RelaySender,
    now_ms: i64,
) -> AppResult<Option<RelaySubscriptionState>> {
    let Some(mut sub) = store::subscription_get(pool).await? else {
        return Ok(None);
    };
    if !should_poll(&sub, now_ms) {
        return Ok(None);
    }

    let answer = sender.fetch_status(&sub.sub_id).await;
    // The stamp moves whatever the answer was, including a failure. Polling
    // every minute while the endpoint is unreachable would be a request storm
    // for news that has not changed; fifteen minutes late is the right cost.
    sub.last_checked = Some(now_ms);

    let changed = match answer {
        Ok(remote) => match remote.state_now() {
            Some(RelaySubscriptionState::Confirmed) => {
                sub.state = RelaySubscriptionState::Confirmed;
                sub.confirmed_at = remote.confirmed_at.or(Some(now_ms));
                Some(RelaySubscriptionState::Confirmed)
            }
            Some(RelaySubscriptionState::Suppressed) => {
                sub.state = RelaySubscriptionState::Suppressed;
                Some(RelaySubscriptionState::Suppressed)
            }
            // Still waiting, or an answer with nothing in it that changes
            // anything. Either way the stamp above is the only write.
            _ => None,
        },
        Err(e) => {
            tracing::debug!(
                "relay: could not read the subscription status ({})",
                e.message()
            );
            None
        }
    };
    store::subscription_set(pool, &sub).await?;
    Ok(changed)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The loop
// ─────────────────────────────────────────────────────────────────────────────

/// The doorbell. See [`kick`].
static KICK: OnceLock<Notify> = OnceLock::new();

fn kick_signal() -> &'static Notify {
    KICK.get_or_init(Notify::new)
}

/// Wake the pump now instead of on the next beat.
///
/// Rung by every path that queues a row. `notify_one` stores a permit when
/// nothing is waiting yet, so a kick that arrives while the pump is mid-send —
/// or before the loop has started — is not lost; it is consumed on the next
/// `notified()`.
///
/// Safe to call from anywhere, including a machine with no relay at all: the
/// permit simply sits there and nothing ever reads it.
pub fn kick() {
    kick_signal().notify_one();
}

/// Whether the pump loop is already running in this process.
///
/// [`maybe_spawn`] is called from two places — at startup, and when the user
/// enrols an address — so it has to be idempotent. Without this, somebody
/// subscribing and unsubscribing three times would be sending from four loops.
static RELAY_STARTED: AtomicBool = AtomicBool::new(false);

/// Whether the pump has any work at all: a subscription record exists.
///
/// ONE reason, unlike telemetry's two. There is no relay equivalent of a parked
/// deletion — an unsubscribe is a QUEUED ROW, and the record that gates it is
/// deliberately kept until that row leaves, so "there is something to do" and
/// "there is a record" are the same fact by construction.
pub(crate) async fn should_run(pool: &SqlitePool) -> bool {
    store::subscription_get(pool).await.ok().flatten().is_some()
}

/// Spawn the pump loop, if there is anything to spawn. Returns whether a task
/// was started.
///
/// Two conditions, in this order:
///
///   1. **[`should_run`]** — checked FIRST, before an endpoint is resolved or a
///      client is constructed, so an install that never enrolled an address has
///      no task, no connection pool and nothing that could resolve a hostname;
///   2. **this build has an endpoint** — [`RelayEndpoint::resolve`] returns
///      `None` when the URL or the key is missing, and then there is still no
///      task.
///
/// Supervised, like the telemetry sender and the drain: a pump that died
/// silently would mean alerts stop arriving with nobody noticing — which is the
/// exact failure this whole feature exists to prevent.
///
/// ## Called at startup AND on enrolment
///
/// Startup alone is not enough: a user who enrols during a session would
/// otherwise have their confirmation mail sit in the outbox until the next
/// launch, which looks exactly like the feature being broken.
/// `relay_subscribe` calls this too, and `RELAY_STARTED` makes the second call a
/// no-op.
pub async fn maybe_spawn(app: &AppHandle, pool: &SqlitePool) -> bool {
    if RELAY_STARTED.load(Ordering::SeqCst) {
        return false;
    }
    if !should_run(pool).await {
        tracing::debug!(
            "relay: no subscription on this machine — no pump task is spawned, so there is \
             nothing that could reach the network"
        );
        return false;
    }

    let Some(endpoint) = RelayEndpoint::resolve() else {
        tracing::info!(
            "relay: a subscription exists but this build has no relay endpoint configured \
             ({}/{} unset) — messages queue locally and nothing is sent",
            super::config::BASE_URL_VAR,
            super::config::WRITE_KEY_VAR,
        );
        return false;
    };

    // Claim the slot before spawning, so two concurrent calls (a startup racing
    // an enrolment) cannot both win.
    if RELAY_STARTED.swap(true, Ordering::SeqCst) {
        return false;
    }

    let app_for_loop = app.clone();
    crate::supervise::supervised_spawn(
        app.clone(),
        "notify::relay",
        crate::supervise::TaskAlert {
            title: None,
            body: sundayrec_core::alerts::AlertText::EmailTaskRestarted,
        },
        move || {
            let app = app_for_loop.clone();
            let endpoint = endpoint.clone();
            async move {
                // Built once per supervised run, not per iteration: the point of
                // the shared client is its connection pool.
                let sender = HttpRelaySender::new(endpoint);

                // Rows stranded in `sending` by a force-quit, recovered before
                // the first beat. A stranded unsubscribe is the endpoint mailing
                // somebody who asked it to stop, indefinitely.
                if let Some(db) = app.try_state::<crate::db::Db>() {
                    match store::reset_stale_sending(&db.pool).await {
                        Ok(n) if n > 0 => {
                            tracing::info!("relay: recovered {n} message(s) stranded mid-send")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("relay: could not recover stranded rows: {e}"),
                    }
                }

                loop {
                    // The interval OR the doorbell, whichever comes first. See
                    // the module header: an alert about a service failing right
                    // now must not wait up to a minute.
                    tokio::select! {
                        _ = tokio::time::sleep(PUMP_INTERVAL) => {}
                        _ = kick_signal().notified() => {}
                    }
                    let Some(db) = app.try_state::<crate::db::Db>() else {
                        continue;
                    };
                    let now = crate::util::now_ms();
                    match drain(&db.pool, &sender, now).await {
                        Ok(n) if n > 0 => tracing::debug!("relay: delivered {n} message(s)"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("relay: pump error: {e}"),
                    }
                    match poll_status(&db.pool, &sender, now).await {
                        Ok(Some(state)) => {
                            tracing::info!("relay: the endpoint now reports {state:?}")
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("relay: status poll error: {e}"),
                    }
                    if let Err(e) = store::seen_trim(&db.pool, now - SEEN_RETENTION_MS).await {
                        tracing::warn!("relay: could not trim the sighting ledger: {e}");
                    }
                }
            }
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use sundayrec_core::email::RelayMessageKind;
    use sundayrec_core::relay::{
        RelayStatus, BACKOFF_STEPS_MS, MAX_ATTEMPTS, RELAY_EVENT_MAX_AGE_MS,
    };

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn entry(id: &str, kind: RelayKind, created_at: i64) -> RelayEntry {
        RelayEntry {
            id: id.to_string(),
            created_at,
            kind,
            event: match kind {
                RelayKind::Send => Some(RelayMessageKind::Failure),
                _ => None,
            },
            dedup_key: format!("{}:{id}", kind.as_str()),
            payload_json: "{\"subId\":\"sub-1\"}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: RelayStatus::Pending,
        }
    }

    fn subscription(state: RelaySubscriptionState) -> RelaySubscription {
        RelaySubscription {
            sub_id: "sub-1".into(),
            address: "frivillig@kirka.no".into(),
            state,
            enrolled_at: 0,
            confirmed_at: None,
            last_checked: None,
            unsub_token: "c".repeat(64),
        }
    }

    /// A sender that fails the test if it is ever consulted. The strongest
    /// available form of "nothing left this machine".
    struct PanicSender;

    impl RelaySender for PanicSender {
        fn execute<'a>(&'a self, _e: &'a RelayEntry) -> RelayFuture<'a, ()> {
            panic!("the sender must not be touched when nothing is enrolled");
        }
        fn fetch_status<'a>(&'a self, _s: &'a str) -> RelayFuture<'a, RemoteStatus> {
            panic!("the sender must not be touched when nothing is enrolled");
        }
    }

    /// Records what it was asked to do and answers from a script.
    #[derive(Default)]
    struct ScriptedSender {
        calls: Mutex<Vec<RelayKind>>,
        outcomes: Mutex<VecDeque<Result<(), RelayFailure>>>,
        status: Mutex<Option<RemoteStatus>>,
        polls: Mutex<u32>,
    }

    impl ScriptedSender {
        fn with(outcomes: Vec<Result<(), RelayFailure>>) -> Arc<Self> {
            Arc::new(Self {
                outcomes: Mutex::new(outcomes.into()),
                ..Default::default()
            })
        }
        fn answering(status: RemoteStatus) -> Arc<Self> {
            Arc::new(Self {
                status: Mutex::new(Some(status)),
                ..Default::default()
            })
        }
        fn calls(&self) -> Vec<RelayKind> {
            self.calls.lock().expect("lock").clone()
        }
    }

    impl RelaySender for ScriptedSender {
        fn execute<'a>(&'a self, e: &'a RelayEntry) -> RelayFuture<'a, ()> {
            // Resolved synchronously: no lock is ever held across an await.
            let outcome = {
                self.calls.lock().expect("lock").push(e.kind);
                self.outcomes
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .unwrap_or(Ok(()))
            };
            Box::pin(async move { outcome })
        }
        fn fetch_status<'a>(&'a self, _s: &'a str) -> RelayFuture<'a, RemoteStatus> {
            let answer = {
                *self.polls.lock().expect("lock") += 1;
                self.status.lock().expect("lock").clone()
            };
            Box::pin(async move {
                answer.ok_or_else(|| RelayFailure::Transient("no answer scripted".into()))
            })
        }
    }

    // ── The gate ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_machine_that_never_enrolled_never_touches_the_sender() {
        let (pool, _d) = temp_pool().await;
        // A full queue of every kind, and nothing enrolled.
        for (i, kind) in [
            RelayKind::Subscribe,
            RelayKind::Send,
            RelayKind::Unsubscribe,
        ]
        .into_iter()
        .enumerate()
        {
            store::upsert_entry(&pool, &entry(&format!("id-{i}"), kind, 0))
                .await
                .unwrap();
        }
        assert_eq!(
            pump_once(&pool, &PanicSender, 1_000).await.unwrap(),
            RelayPumpOutcome::Blocked
        );
        assert_eq!(drain(&pool, &PanicSender, 1_000).await.unwrap(), 0);
        assert!(poll_status(&pool, &PanicSender, 1_000)
            .await
            .unwrap()
            .is_none());
        assert!(!should_run(&pool).await);
        assert_eq!(
            store::load_queue(&pool).await.unwrap().len(),
            3,
            "a blocked pump changes nothing"
        );

        // The positive control, on the SAME queue — so the assertions above
        // cannot pass merely because the wiring was never connected.
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        let s = ScriptedSender::with(vec![]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert!(should_run(&pool).await);
    }

    #[tokio::test]
    async fn an_unconfirmed_address_gets_the_request_and_nothing_else() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Pending))
            .await
            .unwrap();
        store::upsert_entry(&pool, &entry("alert", RelayKind::Send, 0))
            .await
            .unwrap();
        store::upsert_entry(&pool, &entry("ask", RelayKind::Subscribe, 1))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![]);
        assert_eq!(drain(&pool, s.as_ref(), 1_000).await.unwrap(), 1);
        assert_eq!(
            s.calls(),
            vec![RelayKind::Subscribe],
            "double opt-in: only the row that ASKS may leave"
        );
        let left = store::load_queue(&pool).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].kind, RelayKind::Send);
    }

    // ── The happy path ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_delivered_message_leaves_the_queue() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("a", RelayKind::Send, 0))
            .await
            .unwrap();

        assert_eq!(
            pump_once(&pool, ScriptedSender::with(vec![]).as_ref(), 1_000)
                .await
                .unwrap(),
            RelayPumpOutcome::Sent
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
        assert_eq!(
            pump_once(&pool, ScriptedSender::with(vec![]).as_ref(), 1_000)
                .await
                .unwrap(),
            RelayPumpOutcome::Idle
        );
    }

    #[tokio::test]
    async fn the_drain_empties_a_backlog_on_one_beat() {
        // A kick has to deliver what it was rung for, not the first row of it.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        for i in 0..4 {
            store::insert_capped(&pool, &entry(&format!("id-{i}"), RelayKind::Send, i))
                .await
                .unwrap();
        }
        assert_eq!(
            drain(&pool, ScriptedSender::with(vec![]).as_ref(), 1_000)
                .await
                .unwrap(),
            4
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
    }

    // ── The refusals ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_schema_rejection_is_dropped_without_a_retry() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("a", RelayKind::Send, 0))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![classify_relay(StatusCode::BAD_REQUEST, "{}")]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Dropped
        );
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "six identical rejections of the same bytes help nobody"
        );
        assert_eq!(s.calls().len(), 1, "and it is not sent a second time");
    }

    #[tokio::test]
    async fn a_gone_recipient_stops_the_alerts_and_the_panel_can_say_why() {
        // The failure mode this third variant exists for: an address that
        // bounces, treated as a plain drop, is a volunteer who believes they
        // are covered and hears nothing, forever, with no way to find out.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("a", RelayKind::Send, 0))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![classify_relay(
            StatusCode::GONE,
            r#"{"error":"recipient_suppressed"}"#,
        )]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Dropped
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
        let sub = store::subscription_get(&pool)
            .await
            .unwrap()
            .expect("record");
        assert_eq!(sub.state, RelaySubscriptionState::Suppressed);
        assert!(
            sub.gate().enrolled,
            "and it is still enrolled, so the user can still unsubscribe"
        );
    }

    #[tokio::test]
    async fn a_transient_failure_backs_off_and_keeps_the_row() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("a", RelayKind::Send, 0))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![Err(RelayFailure::Transient(
            "no route to host".into(),
        ))]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 100_000).await.unwrap(),
            RelayPumpOutcome::Failed
        );
        let back = store::load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), 1, "the church wifi comes back; the row waits");
        assert_eq!(back[0].attempts, 1);
        assert_eq!(back[0].status, RelayStatus::Pending);
        assert_eq!(back[0].next_attempt, 100_000 + BACKOFF_STEPS_MS[0]);
        assert_eq!(back[0].last_error.as_deref(), Some("no route to host"));
    }

    #[tokio::test]
    async fn an_exhausted_ladder_stops_asking() {
        // On an `unsubscribe` row, which is the only kind that can reach the
        // end of the ladder: its last two rungs are six and twenty-four hours,
        // and an EVENT is swept as stale after six (the test below). "Stop
        // sending me mail" is the one message with no expiry, so it walks the
        // whole ladder — and then stops, rather than retrying forever.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye", RelayKind::Unsubscribe, 0))
            .await
            .unwrap();
        let s = ScriptedSender::with(
            (0..MAX_ATTEMPTS)
                .map(|_| Err(RelayFailure::Transient("down".into())))
                .collect(),
        );
        let mut now = 1;
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(
                pump_once(&pool, s.as_ref(), now).await.unwrap(),
                RelayPumpOutcome::Failed
            );
            now += BACKOFF_STEPS_MS[BACKOFF_STEPS_MS.len() - 1];
        }
        let back = store::load_queue(&pool).await.unwrap();
        assert_eq!(back[0].status, RelayStatus::Failed);
        assert_eq!(
            pump_once(&pool, s.as_ref(), now).await.unwrap(),
            RelayPumpOutcome::Idle
        );
        assert_eq!(s.calls().len(), MAX_ATTEMPTS as usize);
        assert!(
            store::subscription_get(&pool).await.unwrap().is_some(),
            "a row that ran out of attempts has NOT left the queue — it is still \
             there in `failed`, so the record it is gated on must stay too"
        );
    }

    #[tokio::test]
    async fn an_alert_gives_up_on_freshness_before_it_gives_up_on_the_ladder() {
        // The consequence of the two numbers meeting: the ladder's last rungs
        // are 6 h and 24 h, and an event is worth nothing after 6 h. So a
        // failure alert on a machine whose network never comes back is dropped
        // by the freshness sweep long before its attempts run out — which is
        // the right answer, and worth pinning so a later change to either
        // number is a conversation rather than a surprise.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("a", RelayKind::Send, 0))
            .await
            .unwrap();
        let s = ScriptedSender::with(
            (0..MAX_ATTEMPTS)
                .map(|_| Err(RelayFailure::Transient("down".into())))
                .collect(),
        );
        let mut now = 1;
        let mut attempts = 0;
        while pump_once(&pool, s.as_ref(), now).await.unwrap() == RelayPumpOutcome::Failed {
            attempts += 1;
            now += BACKOFF_STEPS_MS[BACKOFF_STEPS_MS.len() - 1];
        }
        assert!(
            attempts < MAX_ATTEMPTS,
            "the sweep should have taken it first ({attempts} of {MAX_ATTEMPTS})"
        );
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "and it is gone from the queue, not sitting in `failed`"
        );
    }

    // ── Freshness ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_alert_from_last_month_is_swept_rather_than_sent() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("old", RelayKind::Send, 0))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("stop", RelayKind::Unsubscribe, 0))
            .await
            .unwrap();

        let now = RELAY_EVENT_MAX_AGE_MS * 10;
        let s = ScriptedSender::with(vec![]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), now).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert_eq!(
            s.calls(),
            vec![RelayKind::Unsubscribe],
            "the stale alert was never offered to the sender; the unsubscribe, which \
             never goes stale, was"
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
    }

    // ── ⚠️ The unsubscribe trap ──────────────────────────────────────────────

    #[tokio::test]
    async fn the_local_record_outlives_the_unsubscribe_row_it_queued() {
        // Clearing the record when the user clicks would shut the gate on the
        // very row that carries the request, and the endpoint would go on
        // sending. So: queued → record STILL THERE → row sent → record gone.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye", RelayKind::Unsubscribe, 0))
            .await
            .unwrap();
        assert!(
            store::subscription_get(&pool).await.unwrap().is_some(),
            "the record must survive queueing, or the row can never leave"
        );

        let s = ScriptedSender::with(vec![]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert_eq!(s.calls(), vec![RelayKind::Unsubscribe]);
        assert!(
            store::subscription_get(&pool).await.unwrap().is_none(),
            "and it is cleared once the request has actually left"
        );
        // The pump is now dark on this machine again.
        assert_eq!(
            pump_once(&pool, &PanicSender, 2_000).await.unwrap(),
            RelayPumpOutcome::Blocked
        );
    }

    #[tokio::test]
    async fn a_permanently_refused_unsubscribe_also_clears_the_record() {
        // The other exit. A row that the endpoint will never accept is gone
        // from the queue exactly as a delivered one is — and a record kept
        // after it would leave a machine that believes it is still subscribed,
        // with a pump that has nothing left to send.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye", RelayKind::Unsubscribe, 0))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![classify_relay(StatusCode::BAD_REQUEST, "{}")]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Dropped
        );
        assert!(store::subscription_get(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_second_unsubscribe_is_not_stranded_by_the_first_ones_success() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye-1", RelayKind::Unsubscribe, 0))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye-2", RelayKind::Unsubscribe, 1))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert!(
            store::subscription_get(&pool).await.unwrap().is_some(),
            "one row left, but another is still waiting on the same gate"
        );
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert!(store::subscription_get(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_suppressed_address_can_still_be_unsubscribed() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Suppressed))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("alert", RelayKind::Send, 0))
            .await
            .unwrap();
        store::insert_capped(&pool, &entry("bye", RelayKind::Unsubscribe, 1))
            .await
            .unwrap();

        let s = ScriptedSender::with(vec![]);
        assert_eq!(
            pump_once(&pool, s.as_ref(), 1_000).await.unwrap(),
            RelayPumpOutcome::Sent
        );
        assert_eq!(s.calls(), vec![RelayKind::Unsubscribe]);
        assert!(store::subscription_get(&pool).await.unwrap().is_none());
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_row_stranded_mid_send_is_recovered_and_then_sends() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Confirmed))
            .await
            .unwrap();
        let mut stranded = entry("a", RelayKind::Send, 0);
        stranded.status = RelayStatus::Sending;
        store::upsert_entry(&pool, &stranded).await.unwrap();

        // Before recovery the row is invisible to the gate — only `pending` is
        // selected — so nothing happens at all.
        assert_eq!(
            pump_once(&pool, &PanicSender, 1_000).await.unwrap(),
            RelayPumpOutcome::Idle
        );
        assert_eq!(store::reset_stale_sending(&pool).await.unwrap(), 1);
        assert_eq!(
            pump_once(&pool, ScriptedSender::with(vec![]).as_ref(), 1_000)
                .await
                .unwrap(),
            RelayPumpOutcome::Sent
        );
    }

    // ── The status poll ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_poll_flips_pending_to_confirmed() {
        // Without this the gate goes on silently dropping every alert for
        // somebody who clicked the link on their phone an hour ago.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Pending))
            .await
            .unwrap();
        let s = ScriptedSender::answering(RemoteStatus {
            state: Some("confirmed".into()),
            confirmed_at: Some(5_000),
            suppressed_at: None,
        });

        assert_eq!(
            poll_status(&pool, s.as_ref(), 9_000).await.unwrap(),
            Some(RelaySubscriptionState::Confirmed)
        );
        let sub = store::subscription_get(&pool)
            .await
            .unwrap()
            .expect("record");
        assert_eq!(sub.state, RelaySubscriptionState::Confirmed);
        assert_eq!(sub.confirmed_at, Some(5_000));
        assert_eq!(sub.last_checked, Some(9_000));
        assert!(sub.gate().confirmed);

        // …and a confirmed record is not polled again.
        assert!(
            poll_status(&pool, s.as_ref(), 9_000 + STATUS_POLL_INTERVAL_MS * 2)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(*s.polls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn the_poll_keeps_to_its_interval() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Pending))
            .await
            .unwrap();
        let s = ScriptedSender::answering(RemoteStatus::default());

        assert!(poll_status(&pool, s.as_ref(), 1_000)
            .await
            .unwrap()
            .is_none());
        assert_eq!(*s.polls.lock().unwrap(), 1, "never polled — poll now");
        // Too soon: the record is not re-read from the network.
        assert!(poll_status(&pool, s.as_ref(), 2_000)
            .await
            .unwrap()
            .is_none());
        assert_eq!(*s.polls.lock().unwrap(), 1);
        assert!(
            poll_status(&pool, s.as_ref(), 1_000 + STATUS_POLL_INTERVAL_MS)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(*s.polls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_suppression_learned_from_the_poll_is_recorded_too() {
        let (pool, _d) = temp_pool().await;
        store::subscription_set(&pool, &subscription(RelaySubscriptionState::Pending))
            .await
            .unwrap();
        let s = ScriptedSender::answering(RemoteStatus {
            state: Some("suppressed".into()),
            confirmed_at: Some(1),
            suppressed_at: Some(2),
        });
        assert_eq!(
            poll_status(&pool, s.as_ref(), 9_000).await.unwrap(),
            Some(RelaySubscriptionState::Suppressed)
        );
        assert_eq!(
            store::subscription_get(&pool).await.unwrap().unwrap().state,
            RelaySubscriptionState::Suppressed,
            "an address that confirmed and later bounced is suppressed, not confirmed"
        );
    }

    #[test]
    fn a_status_answer_with_fields_we_do_not_know_still_parses() {
        // The relay's OPTIONAL-forever law: the Worker ships first and may add
        // fields. A client that refused to parse would turn a Worker deploy
        // into a fleet that stops noticing confirmations.
        let r: RemoteStatus =
            serde_json::from_str(r#"{"state":"confirmed","sends":4,"lang":"nb"}"#).expect("parse");
        assert_eq!(r.state_now(), Some(RelaySubscriptionState::Confirmed));
        let empty: RemoteStatus = serde_json::from_str("{}").expect("parse");
        assert_eq!(empty.state_now(), None);
        // Timestamps alone are enough, with no word at all.
        let stamps: RemoteStatus = serde_json::from_str(r#"{"confirmedAt":7}"#).expect("parse");
        assert_eq!(stamps.state_now(), Some(RelaySubscriptionState::Confirmed));
    }

    // ── The classification ───────────────────────────────────────────────────

    #[test]
    fn the_status_contract_is_telemetrys_plus_one() {
        assert!(classify_relay(StatusCode::ACCEPTED, "").is_ok());
        for (code, want_permanent) in [(400u16, true), (401, true), (403, true), (413, true)] {
            let e = classify_relay(StatusCode::from_u16(code).unwrap(), "{}").unwrap_err();
            assert_eq!(
                matches!(e, RelayFailure::Permanent(_)),
                want_permanent,
                "{code}: {e:?}"
            );
        }
        for code in [408u16, 429, 500, 503] {
            let e = classify_relay(StatusCode::from_u16(code).unwrap(), "").unwrap_err();
            assert!(matches!(e, RelayFailure::Transient(_)), "{code}: {e:?}");
        }
    }

    #[test]
    fn only_a_410_that_names_the_recipient_suppresses() {
        for code in SUPPRESSING_CODES {
            let body = format!(r#"{{"error":"{code}"}}"#);
            assert!(
                matches!(
                    classify_relay(StatusCode::GONE, &body).unwrap_err(),
                    RelayFailure::Suppressed(_)
                ),
                "{code} at 410 must flip the local state"
            );
            // The same code at another status is a refused REQUEST, not a dead
            // ADDRESS. Letting a code alone decide would let an unrelated
            // refusal switch a working subscription off.
            assert!(matches!(
                classify_relay(StatusCode::FORBIDDEN, &body).unwrap_err(),
                RelayFailure::Permanent(_)
            ));
        }
        // A 410 the endpoint did not explain is a plain permanent drop.
        assert!(matches!(
            classify_relay(StatusCode::GONE, r#"{"error":"gone"}"#).unwrap_err(),
            RelayFailure::Permanent(_)
        ));
    }

    #[test]
    fn a_body_that_is_not_json_never_becomes_a_client_bug() {
        // A proxy's HTML error page in front of the Worker, an empty body, a
        // truncated one: all of them read as "no code" and land on the plain
        // permanent branch rather than panicking or erroring.
        for body in ["", "<html>502 Bad Gateway</html>", "{", "null", "[]"] {
            assert_eq!(error_code(body), None, "{body:?}");
            assert!(matches!(
                classify_relay(StatusCode::GONE, body).unwrap_err(),
                RelayFailure::Permanent(_)
            ));
        }
        assert_eq!(
            error_code(r#"{"error":"unscrubbed_path","field":"text"}"#),
            Some("unscrubbed_path".into())
        );
    }

    #[test]
    fn the_message_never_carries_a_url() {
        // The last error reaches a settings panel. It should say what went
        // wrong, not print an endpoint at a volunteer.
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::GONE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            let msg = classify_relay(status, r#"{"error":"recipient_suppressed"}"#)
                .unwrap_err()
                .message()
                .to_string();
            assert!(!msg.contains("http"), "{msg}");
        }
    }

    // ── The clock constants ──────────────────────────────────────────────────

    #[test]
    fn the_sighting_ledger_outlives_every_row_it_could_suppress() {
        // Once a row of any kind would be swept as stale rather than sent, the
        // sighting that would have suppressed it has nothing left to suppress —
        // so the retention is the longest cap, doubled.
        const {
            assert!(SEEN_RETENTION_MS > RELAY_SUBSCRIBE_MAX_AGE_MS);
            assert!(SEEN_RETENTION_MS > RELAY_EVENT_MAX_AGE_MS);
        }
    }

    #[test]
    fn a_kick_before_anybody_is_listening_is_not_lost() {
        // `notify_one` stores a permit. A row queued while the pump is mid-send
        // — or before the loop has even started — is picked up on the next
        // `notified()`, not on the next minute.
        kick();
        kick();
        let waiter = kick_signal().notified();
        // Two rings, one permit: the point is that the FIRST is not dropped.
        assert!(
            poll_ready_once(waiter),
            "a permit stored before anybody waited must wake the first waiter immediately"
        );
    }

    /// Poll a future exactly once and report whether it was already ready.
    ///
    /// Four lines instead of a `futures` dependency this repo does not
    /// otherwise carry, and no `unsafe`: `Waker::noop` is stable on the pinned
    /// 1.98 toolchain, so the hand-rolled `RawWakerVTable` this used to need is
    /// gone.
    fn poll_ready_once<F: Future>(f: F) -> bool {
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut f = Box::pin(f);
        matches!(f.as_mut().poll(&mut cx), Poll::Ready(_))
    }
}
