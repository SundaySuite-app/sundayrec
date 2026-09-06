//! The send path — and the proof that it cannot run without consent.
//!
//! E3 shipped this file with the trait, the outbox and the gate but no
//! implementation, so that E4 would add a sender rather than a gate. That is
//! what happened: [`super::http_sender::HttpTelemetrySender`] is the sender, and
//! not a line of the consent logic below moved to accommodate it.
//!
//! A build still sends nothing unless BOTH consent is active and
//! [`TelemetryEndpoint::resolve`] finds a URL and a key — see [`maybe_spawn`].
//!
//! ## Why the guarantee is structural rather than careful
//!
//! "With consent off there must be zero network activity — not even DNS" is
//! easy to state and easy to violate, because the violation looks like ordinary
//! code:
//!
//! ```ignore
//! let entry = select_next(&queue, now);        // reads the queue
//! let client = http_client();                  // builds a client
//! if !consent_active(&pool).await { return; }  // …checks consent third
//! ```
//!
//! By the time the check runs a connection pool exists and, on some stacks, a
//! DNS resolution has already been kicked off. So the check is not a statement
//! in the middle of a function here: it is the FIRST argument of
//! [`pump_decision`](sundayrec_core::telemetry::queue::pump_decision), which
//! returns [`PumpDecision::Blocked`] without reading the queue at all, and
//! [`pump_once`] never even loads the queue when consent is inactive.
//!
//! The test that proves it injects a sender which PANICS if it is called, runs
//! the whole flow with consent off, and asserts nothing happened — with a
//! positive control using a counting sender on the same queue, so the assertion
//! cannot pass vacuously because the wiring was never connected.
//!
//! E4 adds the version of that test the promise is actually made at. A trait
//! double can only show that an object was not called; the claim in the privacy
//! text is that nothing leaves the machine. So `net_tests` binds a real
//! `TcpListener`, points the real sender at it, and counts ACCEPTED CONNECTIONS
//! — zero, at the socket, with a positive control on the same listener proving
//! the endpoint was reachable all along.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};

use sundayrec_core::telemetry::queue::{self, PumpDecision};

use super::config::TelemetryEndpoint;
use super::http_sender::HttpTelemetrySender;
use crate::error::AppResult;

/// Why a send did not succeed — and, crucially, whether trying again could
/// ever help.
///
/// Splitting this out is what stops the outbox doing the wrong thing with half
/// its failures. Before E4 every failure took the backoff ladder, which is
/// correct for a network that is down and absurd for a payload the endpoint has
/// just explained it will never accept: six spread-out attempts over 24 hours,
/// six identical rejections of the same bytes, and then a `Failed` row waiting
/// for the [`queue::QUEUE_MAX`] bound to reclaim it.
///
/// The mapping from HTTP status to variant is
/// [`super::http_sender::classify`]; the endpoint's half of the agreement is
/// `sunday-telemetry/src/ingest.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendFailure {
    /// The network, or the endpoint, or the moment. Back off and retry.
    Transient(String),
    /// This payload will never be accepted. Drop it.
    Permanent(String),
}

impl SendFailure {
    /// The message for the settings panel and the log.
    pub fn message(&self) -> &str {
        match self {
            Self::Transient(m) | Self::Permanent(m) => m,
        }
    }
}

/// The future a [`TelemetrySender`] returns. Boxed by hand rather than via an
/// `async-trait` dependency: one type alias is cheaper than a proc-macro crate
/// for a trait with a single method.
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SendFailure>> + Send + 'a>>;

/// Deliver one payload. `Ok(())` means the endpoint accepted it.
///
/// Implemented by [`super::http_sender::HttpTelemetrySender`] over
/// `util::http_client()`'s bounded rustls client — the same pure-builder +
/// thin-socket split the rest of the shell's network seams use. The
/// trait stays here, with the pump, so the tests below can substitute a sender
/// that panics if it is ever called.
pub trait TelemetrySender: Send + Sync {
    fn send<'a>(&'a self, payload_json: &'a str) -> SendFuture<'a>;
}

/// What one pump iteration did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpOutcome {
    /// Consent is not active. The queue was not read and the sender was not
    /// touched.
    Blocked,
    /// Nothing was due.
    Idle,
    /// One payload was delivered and removed from the outbox.
    Sent,
    /// One attempt failed transiently; the entry was backed off or retired.
    Failed,
    /// The endpoint refused the payload permanently, so it was discarded
    /// without a retry.
    Dropped,
}

/// Run one iteration of the send loop.
///
/// Consent is resolved FIRST and the queue is only loaded if it is active — so
/// the blocked path performs exactly one read of one settings row and returns.
pub async fn pump_once(
    pool: &SqlitePool,
    sender: &dyn TelemetrySender,
    now_ms: i64,
) -> AppResult<PumpOutcome> {
    let active = super::consent_active(pool).await;
    let entries = if active {
        super::queue_store::load_queue(pool).await?
    } else {
        Vec::new()
    };

    let id = match queue::pump_decision(active, &entries, now_ms) {
        PumpDecision::Blocked => return Ok(PumpOutcome::Blocked),
        PumpDecision::Idle => return Ok(PumpOutcome::Idle),
        PumpDecision::Send(id) => id,
    };

    let mut entries = entries;
    queue::mark_sending(&mut entries, &id);
    let entry = match entries.iter().find(|e| e.id == id) {
        Some(e) => e.clone(),
        None => return Ok(PumpOutcome::Idle),
    };
    super::queue_store::upsert_entry(pool, &entry).await?;

    match sender.send(&entry.payload_json).await {
        Ok(()) => {
            queue::on_success(&mut entries, &id);
            super::queue_store::delete_entry(pool, &id).await?;
            Ok(PumpOutcome::Sent)
        }
        Err(SendFailure::Permanent(msg)) => {
            // The endpoint will refuse these bytes every time. Retrying is not
            // patience, it is six identical rejections — so the row goes, and
            // the reason goes to the log rather than to a badge the user can
            // neither cause nor fix. The endpoint logged WHICH field.
            tracing::warn!(
                "telemetry: the endpoint permanently rejected a report ({msg}); it has been \
                 discarded rather than retried. This is a client/endpoint disagreement — see \
                 the endpoint's rejection log for the offending field."
            );
            queue::on_permanent_failure(&mut entries, &id);
            super::queue_store::delete_entry(pool, &id).await?;
            Ok(PumpOutcome::Dropped)
        }
        Err(SendFailure::Transient(msg)) => {
            queue::on_failure(&mut entries, &id, msg, now_ms);
            if let Some(updated) = entries.iter().find(|x| x.id == id) {
                super::queue_store::upsert_entry(pool, updated).await?;
            }
            Ok(PumpOutcome::Failed)
        }
    }
}

/// How often the sender loop looks for something to do.
///
/// One minute, matching the first rung of the backoff ladder: a shorter interval
/// would spin against entries that are not due yet, and a longer one would make
/// the first rung meaningless. The loop itself is cheap when idle — with consent
/// off it reads one settings row and sleeps again.
pub const PUMP_INTERVAL: Duration = Duration::from_secs(60);

/// Whether the sender loop is already running in this process.
///
/// [`maybe_spawn`] is called from two places — once at startup, and again when
/// the user grants consent — so it has to be idempotent. Without this, a user
/// toggling consent off and on three times would be sending from four loops.
static SENDER_STARTED: AtomicBool = AtomicBool::new(false);

/// Whether the sender loop has any work at all — the gate [`maybe_spawn`]
/// consults before it builds anything.
///
/// TWO reasons, not one, and the second is the whole point of this function
/// existing rather than the expression being inlined:
///
///   1. **consent is active** — there may be reports to deliver. [`pump_once`]
///      re-checks this on every beat, so a revoke mid-session stops the sending
///      without the loop having to be torn down.
///   2. **a deletion is parked** — the user pressed «Slett mine data» and the
///      remote `DELETE` has not been carried out yet. [`drain_deletions`]
///      explains at length why a deletion is deliberately NOT behind the consent
///      gate, and this is the other half of that argument: gating the LOOP on
///      consent made the ungated deletion unreachable in exactly the case it was
///      written for. Revoke, restart, then press «Slett mine data» and the id was
///      parked in a list nothing would ever read — PRIVACY.md promises «Den blir
///      ikke glemt», and before this it was, silently and permanently.
///
/// An install that has never consented has never minted an install id, so it can
/// never have a parked deletion either: the "no telemetry machinery at all"
/// property this gate was protecting is unchanged.
pub(super) async fn should_run(pool: &SqlitePool) -> bool {
    if super::consent_active(pool).await {
        return true;
    }
    !super::pending_deletions(pool)
        .await
        .unwrap_or_default()
        .is_empty()
}

/// Spawn the sender loop, if there is anything to spawn.
///
/// Returns whether a task was started. Two conditions, in this order:
///
///   1. **[`should_run`]** — checked FIRST, before an endpoint is resolved or a
///      client is constructed, so an install with nothing to do has no task, no
///      connection pool and nothing that could resolve a hostname;
///   2. **this build has an endpoint** — [`TelemetryEndpoint::resolve`] returns
///      `None` when the URL or the key is missing, and then there is still no
///      task. Reports keep queueing locally, exactly as they did in E3.
///
/// Supervised, like the drain: a sender task that dies silently means telemetry
/// stops without anyone noticing, which is benign for the user and invisible to
/// us — the worst combination for something whose purpose is making problems
/// visible.
///
/// ## Called at startup AND on consent
///
/// Startup alone is not enough. A user who grants consent during a session would
/// otherwise have their reports queue up until the next launch — collected,
/// consented to, and going nowhere, which looks exactly like the feature being
/// broken. So `telemetry_consent_set` calls this too, and `SENDER_STARTED` makes
/// the second call a no-op.
///
/// The consequence worth stating: once started, the loop lives for the rest of
/// the process even if consent is later revoked. That is not a hole in the E3
/// guarantee — [`pump_once`] resolves consent before it reads the queue, so a
/// running loop with consent off does exactly one settings-row read per minute
/// and touches nothing else. What "no task is spawned" buys is that an install
/// which has NEVER consented has no telemetry machinery running at all, and that
/// is still true.
pub async fn maybe_spawn(app: &AppHandle, pool: &SqlitePool) -> bool {
    if SENDER_STARTED.load(Ordering::SeqCst) {
        return false;
    }
    if !should_run(pool).await {
        tracing::debug!(
            "telemetry: consent is not active and no deletion is owed — no sender task is \
             spawned, so there is nothing that could reach the network"
        );
        return false;
    }

    let Some(endpoint) = TelemetryEndpoint::resolve() else {
        tracing::info!(
            "telemetry: the sender has work to do but this build has no endpoint configured \
             ({}/{} unset) — reports are queued locally and nothing is sent",
            super::config::BASE_URL_VAR,
            super::config::WRITE_KEY_VAR,
        );
        return false;
    };

    // Claim the slot before spawning, so two concurrent calls (a startup racing
    // a consent grant) cannot both win.
    if SENDER_STARTED.swap(true, Ordering::SeqCst) {
        return false;
    }

    let app_for_loop = app.clone();
    crate::supervise::supervised_spawn(
        app.clone(),
        "telemetry::sender",
        crate::supervise::TaskAlert {
            title: None,
            body: sundayrec_core::alerts::AlertText::QualityTaskRestarted,
        },
        move || {
            let app = app_for_loop.clone();
            let endpoint = endpoint.clone();
            async move {
                // Built once per supervised run, not per iteration: the point of
                // the shared client is its connection pool.
                let sender = HttpTelemetrySender::new(endpoint);
                loop {
                    tokio::time::sleep(PUMP_INTERVAL).await;
                    let Some(db) = app.try_state::<crate::db::Db>() else {
                        continue;
                    };
                    // Consent can be revoked between iterations. `pump_once`
                    // re-checks it before touching the queue, so this loop never
                    // has to remember to.
                    match pump_once(&db.pool, &sender, super::now_ms()).await {
                        Ok(PumpOutcome::Sent) => {
                            tracing::debug!("telemetry: a report was delivered")
                        }
                        Ok(PumpOutcome::Dropped) => {}
                        Ok(_) => {}
                        Err(e) => tracing::warn!("telemetry: send loop error: {e}"),
                    }
                    // The remote half of "delete my data", drained on the same
                    // beat. It has its own consent story — see `drain_deletions`.
                    drain_deletions(&db.pool, &sender).await;
                }
            }
        },
    );
    true
}

/// Send the remote DELETE for every install id waiting for one.
///
/// The local half of "slett dataene mine" already ran when the user asked:
/// `regenerate_install_id` retired the old id, purged everything queued under
/// it, and parked it in `telemetry.pendingDeletions`. Until now that list only
/// ever grew.
///
/// ## Why this is not behind the consent gate
///
/// Everything else in this file refuses to touch the network without active
/// consent, and this deliberately does not. A deletion request is not a
/// contribution of data — it is the withdrawal of data already contributed, and
/// the request the user is most entitled to have carried out. Gating it on
/// consent would mean that revoking consent AND asking for deletion (the obvious
/// pair of actions) leaves the data on the server forever, because the second
/// request can no longer be delivered.
///
/// What it sends is one install id in a URL and nothing else: no payload, no
/// settings, no counters. So the "with consent off, nothing about this machine
/// is transmitted" guarantee is intact — the id being sent is one the user has
/// explicitly asked to have erased, and it is already on the server.
///
/// One id per call, oldest first, so a long list drains over successive
/// iterations rather than blocking one of them. A transient failure leaves the
/// id parked for the next round; a permanent one clears it, because a 4xx here
/// means the endpoint will never accept this deletion and retrying it forever is
/// worse than stopping.
async fn drain_deletions(pool: &SqlitePool, sender: &HttpTelemetrySender) {
    let pending = match super::pending_deletions(pool).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("telemetry: could not read pending deletions: {e}");
            return;
        }
    };
    let Some(id) = pending.first() else { return };

    match sender.delete_install(id).await {
        Ok(()) => {
            tracing::info!(
                "telemetry: a retired install's data was deleted from the endpoint at the \
                 user's request"
            );
            if let Err(e) = super::clear_pending_deletion(pool, id).await {
                // Not clearing means one redundant DELETE next round. The
                // endpoint treats an unknown id as a success with zero counts,
                // so that is harmless — worth saying, because the alternative
                // reading is that data comes back.
                tracing::warn!(
                    "telemetry: deletion succeeded but the id could not be cleared: {e}"
                );
            }
        }
        Err(SendFailure::Permanent(msg)) => {
            tracing::warn!(
                "telemetry: the endpoint permanently refused a deletion request ({msg}); \
                 the id has been dropped rather than retried forever"
            );
            let _ = super::clear_pending_deletion(pool, id).await;
        }
        Err(SendFailure::Transient(msg)) => {
            tracing::debug!("telemetry: deletion will be retried ({msg})");
        }
    }
}

/// Fixtures shared by both test modules below.
#[cfg(test)]
mod tests_support {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s — the same
    // allow the test modules below carry, for the same reason. It serialises
    // tests against process-global counter state; each runs on its own
    // single-threaded runtime, and nothing in the guarded region blocks on it.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::db::store::open_pool;
    use crate::telemetry::counters;
    use sundayrec_core::telemetry::queue::{TelemetryEntry, TelemetryStatus};

    /// A throwaway database AND the counter-state lock: `consent_set` writes the
    /// process-global counter mirror, so every test here has to serialise on it
    /// or two tests decide each other's assertions.
    pub(super) async fn temp_pool() -> (
        SqlitePool,
        tempfile::TempDir,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let guard = counters::test_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir, guard)
    }

    pub(super) fn entry(id: &str, created_at: i64) -> TelemetryEntry {
        TelemetryEntry {
            id: id.to_string(),
            created_at,
            schema_ver: 1,
            dedup_key: format!("quality:{created_at}"),
            payload_json: "{\"schema\":1}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: TelemetryStatus::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s — see the
    // same allow in `telemetry::counters`. Correct here: it serialises tests
    // against process-global counter state, each runs on its own
    // single-threaded `#[tokio::test]` runtime, and nothing in the guarded
    // region can block on the same lock.
    #![allow(clippy::await_holding_lock)]

    use super::tests_support::*;
    use super::*;
    use crate::telemetry::{consent_set, queue_store as store};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use sundayrec_core::telemetry::queue::{TelemetryEntry, TelemetryStatus};

    /// A sender that must never be called. Panicking (rather than recording a
    /// flag) is deliberate: a flag can be asserted on and forgotten, a panic
    /// fails the test wherever it happens.
    struct PanickingSender;
    impl TelemetrySender for PanickingSender {
        fn send<'a>(&'a self, _payload: &'a str) -> SendFuture<'a> {
            panic!(
                "the telemetry sender was invoked without active consent — \
                 this is the failure E3.3 exists to make impossible"
            );
        }
    }

    /// How a [`CountingSender`] answers.
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    enum Mode {
        #[default]
        Ok,
        /// The network is down — the outbox should back off.
        Transient,
        /// The endpoint refuses these bytes — the outbox should drop them.
        Permanent,
    }

    /// A sender that counts its calls, for the positive controls.
    #[derive(Clone, Default)]
    struct CountingSender {
        calls: Arc<AtomicUsize>,
        mode: Mode,
    }
    impl TelemetrySender for CountingSender {
        fn send<'a>(&'a self, _payload: &'a str) -> SendFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mode = self.mode;
            Box::pin(async move {
                match mode {
                    Mode::Ok => Ok(()),
                    Mode::Transient => Err(SendFailure::Transient("no route to host".into())),
                    Mode::Permanent => Err(SendFailure::Permanent(
                        "endpoint rejected the payload: 400 Bad Request".into(),
                    )),
                }
            })
        }
    }

    #[tokio::test]
    async fn with_consent_off_the_send_path_is_unreachable() {
        let (pool, _d, _g) = temp_pool().await;
        // A queue with a due entry — everything is ready to send EXCEPT consent.
        // (Enqueue directly: the normal path would refuse to queue at all.)
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        // Never asked.
        assert_eq!(
            pump_once(&pool, &PanickingSender, 1_800_000_000_000)
                .await
                .unwrap(),
            PumpOutcome::Blocked
        );
        // Explicitly denied.
        consent_set(&pool, false).await.unwrap();
        // (The revoke purged the queue — put a due entry back to keep the test
        // honest about WHY nothing was sent.)
        store::insert_capped(&pool, &entry("b", 0)).await.unwrap();
        assert_eq!(
            pump_once(&pool, &PanickingSender, 1_800_000_000_000)
                .await
                .unwrap(),
            PumpOutcome::Blocked
        );
        // …and no task is spawned either.
        assert!(!maybe_spawn_probe(&pool).await);

        // The entry is untouched: not marked sending, not attempted.
        let back = store::load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].status, TelemetryStatus::Pending);
        assert_eq!(back[0].attempts, 0);
    }

    /// `maybe_spawn` without an `AppHandle` (which a unit test cannot build):
    /// the gate is the part under test, and it is the same expression.
    async fn maybe_spawn_probe(pool: &SqlitePool) -> bool {
        super::should_run(pool).await
    }

    #[tokio::test]
    async fn a_parked_deletion_starts_the_loop_even_with_consent_off() {
        // The pair of actions the deletion promise is written for: stop
        // contributing, AND remove what was already contributed. Before
        // `should_run` looked at the deletion list, a revoke followed by a
        // restart meant no loop was ever spawned, so the parked id sat in
        // `telemetry.pendingDeletions` forever — PRIVACY.md says «Den blir ikke
        // glemt», and it was.
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        crate::telemetry::regenerate_install_id(&pool)
            .await
            .unwrap();
        consent_set(&pool, false).await.unwrap();

        assert!(
            !crate::telemetry::pending_deletions(&pool)
                .await
                .unwrap()
                .is_empty(),
            "revoking consent must not drop a deletion the user already asked for"
        );
        assert!(
            maybe_spawn_probe(&pool).await,
            "an owed deletion is work, even though nothing may be SENT"
        );

        // …and once it has been carried out, the reason to run is gone again.
        let owed = crate::telemetry::pending_deletions(&pool).await.unwrap();
        for id in &owed {
            crate::telemetry::clear_pending_deletion(&pool, id)
                .await
                .unwrap();
        }
        assert!(
            !maybe_spawn_probe(&pool).await,
            "with consent off and nothing owed there is no task at all"
        );
    }

    #[tokio::test]
    async fn the_positive_control_proves_the_sender_is_really_wired() {
        // If this test did not exist, the one above could pass because nothing
        // was connected rather than because consent blocked it.
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = CountingSender::default();
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Sent
        );
        assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "a delivered payload leaves the outbox"
        );
    }

    #[tokio::test]
    async fn revoking_consent_mid_backlog_stops_the_pump_and_empties_it() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        for i in 0..3 {
            store::insert_capped(&pool, &entry(&format!("id-{i}"), i))
                .await
                .unwrap();
        }
        consent_set(&pool, false).await.unwrap();

        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "revoking must not leave a paused pile waiting for a change of mind"
        );
        assert_eq!(
            pump_once(&pool, &PanickingSender, 1_800_000_000_000)
                .await
                .unwrap(),
            PumpOutcome::Blocked
        );
    }

    #[tokio::test]
    async fn a_failed_send_backs_off_rather_than_spinning() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = CountingSender {
            mode: Mode::Transient,
            ..Default::default()
        };
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Failed
        );
        let back = store::load_queue(&pool).await.unwrap();
        assert_eq!(back[0].attempts, 1);
        assert_eq!(back[0].last_error.as_deref(), Some("no route to host"));
        assert_eq!(back[0].next_attempt, now + queue::BACKOFF_STEPS_MS[0]);

        // The immediate next pump finds nothing due — no hot loop.
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Idle
        );
        assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_empty_queue_with_consent_on_is_idle_not_an_error() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        let sender = CountingSender::default();
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Idle
        );
        assert_eq!(sender.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_permanently_rejected_report_is_dropped_rather_than_retried() {
        // THE E4 behaviour change. Before the permanent/transient split, a 400
        // took the same ladder as a dead network: six attempts spread over 24
        // hours, six identical rejections of bytes the endpoint had already
        // explained it would never accept.
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = CountingSender {
            mode: Mode::Permanent,
            ..Default::default()
        };
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Dropped
        );
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "a payload the endpoint will never accept must not occupy the outbox"
        );

        // And the next pump does not call the sender again — there is nothing
        // left to call it about.
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Idle
        );
        assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_rejected_report_does_not_block_the_ones_behind_it() {
        // The failure mode a naive "drop it" could still have: the pump always
        // selects the earliest due entry, so a bad payload that were merely
        // marked rather than removed would be re-selected forever and the good
        // ones behind it would never be sent.
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("bad", 0)).await.unwrap();
        store::insert_capped(&pool, &entry("good", 1))
            .await
            .unwrap();

        let rejecting = CountingSender {
            mode: Mode::Permanent,
            ..Default::default()
        };
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &rejecting, now).await.unwrap(),
            PumpOutcome::Dropped
        );

        let accepting = CountingSender::default();
        assert_eq!(
            pump_once(&pool, &accepting, now).await.unwrap(),
            PumpOutcome::Sent
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_two_failure_kinds_are_not_interchangeable() {
        // Both are `Err`. The difference between them is the entire point, so
        // assert it on one queue rather than trusting the two tests above to be
        // comparable.
        let now = 1_800_000_000_000i64;

        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();
        pump_once(
            &pool,
            &CountingSender {
                mode: Mode::Transient,
                ..Default::default()
            },
            now,
        )
        .await
        .unwrap();
        let after_transient = store::load_queue(&pool).await.unwrap();
        assert_eq!(after_transient.len(), 1, "transient keeps the payload");
        assert_eq!(after_transient[0].attempts, 1);

        // Same queue, same entry, now permanently rejected.
        let due = TelemetryEntry {
            next_attempt: now,
            status: TelemetryStatus::Pending,
            ..after_transient[0].clone()
        };
        store::upsert_entry(&pool, &due).await.unwrap();
        pump_once(
            &pool,
            &CountingSender {
                mode: Mode::Permanent,
                ..Default::default()
            },
            now,
        )
        .await
        .unwrap();
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "permanent keeps nothing"
        );
    }

    #[tokio::test]
    async fn no_endpoint_configured_means_no_sender_task() {
        // A build with consent on but no endpoint must behave exactly as E3 did:
        // reports queue locally and nothing reaches the network. Proven through
        // the real resolver rather than a stub, so a change to its rules shows up
        // here.
        assert_eq!(
            TelemetryEndpoint::normalize(None, None),
            None,
            "no url and no key must not produce an endpoint"
        );
        assert_eq!(
            TelemetryEndpoint::normalize(Some("https://t.example".into()), None),
            None,
            "a url without a key must not produce an endpoint"
        );
    }
}

/// Socket-level tests: the real `HttpTelemetrySender`, a real TCP connection.
///
/// The tests above prove the pump's logic with an injected sender. That is the
/// right tool for the state machine and the wrong one for the claim the privacy
/// text actually makes, which is not "the sender object is not called" but "with
/// consent off, nothing leaves this machine". A trait double cannot distinguish
/// those: a bug that opened a connection somewhere else in the path would leave
/// every one of those tests green.
///
/// So these bind a real listener, point the real sender at it, and count
/// ACCEPTED CONNECTIONS. Zero is zero at the socket, which is the level the
/// promise is made at.
#[cfg(test)]
mod net_tests {
    #![allow(clippy::await_holding_lock)]

    use super::tests_support::*;
    use super::*;
    use crate::telemetry::consent_set;
    use crate::telemetry::queue_store as store;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// What one request looked like on the wire.
    #[derive(Debug, Clone, Default)]
    struct Seen {
        head: String,
        body: String,
    }

    /// A one-shot HTTP server that answers `status_line` and records the request.
    ///
    /// Hand-rolled over `TcpListener` rather than pulled in as a test dependency:
    /// the whole point is to observe the raw socket, and thirty lines that do
    /// exactly that are easier to trust here than a framework.
    async fn serve(
        listener: TcpListener,
        status_line: &'static str,
        connections: Arc<AtomicUsize>,
        seen: Arc<tokio::sync::Mutex<Vec<Seen>>>,
    ) {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            connections.fetch_add(1, Ordering::SeqCst);

            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            // Read until the headers are complete, then until content-length
            // bytes of body have arrived.
            loop {
                let n = match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                let Some(split) = text.find("\r\n\r\n") else {
                    continue;
                };
                let head = text[..split].to_string();
                let body_so_far = &text[split + 4..];
                let want: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        (k.trim().eq_ignore_ascii_case("content-length"))
                            .then(|| v.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                if body_so_far.len() >= want {
                    seen.lock().await.push(Seen {
                        head,
                        body: body_so_far.to_string(),
                    });
                    break;
                }
            }

            let _ = sock
                .write_all(
                    format!(
                        "HTTP/1.1 {status_line}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            let _ = sock.shutdown().await;
        }
    }

    struct Harness {
        endpoint: TelemetryEndpoint,
        connections: Arc<AtomicUsize>,
        seen: Arc<tokio::sync::Mutex<Vec<Seen>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    async fn harness(status_line: &'static str) -> Harness {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let task = tokio::spawn(serve(
            listener,
            status_line,
            connections.clone(),
            seen.clone(),
        ));
        let endpoint = TelemetryEndpoint::normalize(
            Some(format!("http://{addr}")),
            Some("test-write-key".into()),
        )
        .expect("endpoint");
        Harness {
            endpoint,
            connections,
            seen,
            _task: task,
        }
    }

    #[tokio::test]
    async fn with_consent_off_not_a_single_connection_is_opened() {
        // THE test. Not "the sender was not called" — "the socket was never
        // touched", measured at a listener the client would have to connect to.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("202 Accepted").await;
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Blocked
        );
        consent_set(&pool, false).await.unwrap();
        store::insert_capped(&pool, &entry("b", 0)).await.unwrap();
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Blocked
        );

        // Give anything in flight a chance to arrive before asserting zero, or
        // this passes because it was fast rather than because it was silent.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            h.connections.load(Ordering::SeqCst),
            0,
            "consent is off and a socket was opened anyway"
        );
    }

    #[tokio::test]
    async fn with_consent_on_the_payload_really_goes_over_the_wire() {
        // The positive control for the test above: same harness, same sender,
        // consent on. Without this, "zero connections" could mean the endpoint
        // was misconfigured rather than that the gate held.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("202 Accepted").await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Sent
        );
        assert_eq!(h.connections.load(Ordering::SeqCst), 1);

        let seen = h.seen.lock().await;
        let req = seen.first().expect("a request arrived");
        assert!(req.head.starts_with("POST /v1/ingest "), "{}", req.head);
        assert!(
            req.head
                .to_lowercase()
                .contains("x-sundayrec-key: test-write-key"),
            "the write key must be on the request: {}",
            req.head
        );
        assert!(req
            .head
            .to_lowercase()
            .contains("content-type: application/json"));
        assert_eq!(
            req.body, "{\"schema\":1}",
            "the payload must go out byte-for-byte as it was queued"
        );
        assert!(
            store::load_queue(&pool).await.unwrap().is_empty(),
            "a delivered payload leaves the outbox"
        );
    }

    #[tokio::test]
    async fn a_400_drops_the_payload_and_does_not_storm() {
        // The end-to-end version of the contract: a schema rejection must cost
        // exactly ONE connection, not six spread over 24 hours.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("400 Bad Request").await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Dropped
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());

        // Pump repeatedly: there must be nothing left to send, so no further
        // connection is opened however often the loop runs.
        for _ in 0..5 {
            assert_eq!(
                pump_once(&pool, &sender, now).await.unwrap(),
                PumpOutcome::Idle
            );
        }
        assert_eq!(
            h.connections.load(Ordering::SeqCst),
            1,
            "a permanently rejected payload must cost exactly one request"
        );
    }

    #[tokio::test]
    async fn a_429_is_kept_and_backed_off_rather_than_dropped() {
        // The one 4xx that must NOT be treated as permanent. Getting this wrong
        // throws away a payload the endpoint asked us to send again later.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("429 Too Many Requests").await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Failed
        );

        let back = store::load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), 1, "a rate-limited payload is kept");
        assert_eq!(back[0].attempts, 1);
        assert_eq!(back[0].next_attempt, now + queue::BACKOFF_STEPS_MS[0]);

        // …and it is not retried before its time, so a rate limit is not met
        // with a hot loop.
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Idle
        );
        assert_eq!(h.connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_5xx_is_retried_on_the_ladder() {
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("503 Service Unavailable").await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        let now = 1_800_000_000_000i64;
        assert_eq!(
            pump_once(&pool, &sender, now).await.unwrap(),
            PumpOutcome::Failed
        );
        assert_eq!(store::load_queue(&pool).await.unwrap().len(), 1);
        assert_eq!(h.connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_retired_install_id_is_deleted_at_the_endpoint() {
        // The remote half of "slett dataene mine". Until E4 the pending list
        // only ever grew.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("200 OK").await;
        consent_set(&pool, true).await.unwrap();
        let old_id = crate::telemetry::ensure_install_id(&pool).await.unwrap();
        crate::telemetry::regenerate_install_id(&pool)
            .await
            .unwrap();
        assert_eq!(
            crate::telemetry::pending_deletions(&pool).await.unwrap(),
            vec![old_id.clone()],
            "the local half parks the retired id"
        );

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        drain_deletions(&pool, &sender).await;

        let seen = h.seen.lock().await;
        let req = seen.first().expect("a request arrived");
        assert!(
            req.head
                .starts_with(&format!("DELETE /v1/install/{old_id} ")),
            "{}",
            req.head
        );
        assert!(req.head.to_lowercase().contains("x-sundayrec-key:"));
        drop(seen);

        assert!(
            crate::telemetry::pending_deletions(&pool)
                .await
                .unwrap()
                .is_empty(),
            "a confirmed deletion clears the id"
        );
    }

    #[tokio::test]
    async fn a_failed_deletion_stays_parked_for_the_next_round() {
        // The user asked for their data to be removed. A machine that was
        // offline when they asked must still send it later — so a transient
        // failure must not silently discard the request.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("503 Service Unavailable").await;
        consent_set(&pool, true).await.unwrap();
        let old_id = crate::telemetry::ensure_install_id(&pool).await.unwrap();
        crate::telemetry::regenerate_install_id(&pool)
            .await
            .unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        drain_deletions(&pool, &sender).await;

        assert_eq!(
            crate::telemetry::pending_deletions(&pool).await.unwrap(),
            vec![old_id],
            "a deletion that could not be delivered must still be pending"
        );
    }

    #[tokio::test]
    async fn a_deletion_is_sent_even_after_consent_is_withdrawn() {
        // The pairing a consent gate on this path would break: revoke consent
        // AND ask for deletion — the obvious two actions — and the data would
        // stay on the server forever because the second request could no longer
        // be delivered.
        let (pool, _d, _g) = temp_pool().await;
        let h = harness("200 OK").await;
        consent_set(&pool, true).await.unwrap();
        let old_id = crate::telemetry::ensure_install_id(&pool).await.unwrap();
        crate::telemetry::regenerate_install_id(&pool)
            .await
            .unwrap();
        consent_set(&pool, false).await.unwrap();

        let sender = HttpTelemetrySender::new(h.endpoint.clone());
        drain_deletions(&pool, &sender).await;

        assert_eq!(h.connections.load(Ordering::SeqCst), 1);
        let seen = h.seen.lock().await;
        let head = &seen.first().expect("a request arrived").head;
        assert!(
            head.starts_with(&format!("DELETE /v1/install/{old_id} ")),
            "{head}"
        );
        // And it carried NOTHING about the machine beyond the id being erased.
        assert!(!head
            .to_lowercase()
            .contains("content-type: application/json"));
        drop(seen);
        assert!(crate::telemetry::pending_deletions(&pool)
            .await
            .unwrap()
            .is_empty());
    }
}
