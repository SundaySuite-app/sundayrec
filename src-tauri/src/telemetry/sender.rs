//! The send path — and the proof that it cannot run without consent.
//!
//! **This build sends nothing.** There is no endpoint, no HTTP client and no
//! sender implementation in E3; the network half lands in E4. What is here is
//! the SHAPE the sender must fit, arranged so that consent is checked before
//! anything else can happen — so E4 adds a sender, not a gate.
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

use std::future::Future;
use std::pin::Pin;

use sqlx::SqlitePool;
use tauri::AppHandle;

use sundayrec_core::telemetry::queue::{self, PumpDecision};

use crate::error::AppResult;

/// The future a [`TelemetrySender`] returns. Boxed by hand rather than via an
/// `async-trait` dependency: one type alias is cheaper than a proc-macro crate
/// for a trait with a single method.
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Deliver one payload. `Ok(())` means the endpoint accepted it.
///
/// E4 implements this over `cloud::http_client()`'s bounded rustls client, the
/// same pure-builder + thin-socket split `webhook.rs` and `notify::post_webhook`
/// already use. E3 has no implementation at all, which is the strongest possible
/// version of "nothing sends anywhere yet".
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
    /// One attempt failed; the entry was backed off or retired.
    Failed,
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
        Err(e) => {
            queue::on_failure(&mut entries, &id, e, now_ms);
            if let Some(updated) = entries.iter().find(|x| x.id == id) {
                super::queue_store::upsert_entry(pool, updated).await?;
            }
            Ok(PumpOutcome::Failed)
        }
    }
}

/// Spawn the sender loop, if there is anything to spawn.
///
/// Returns whether a task was started. In this build the answer is always
/// `false` — but the consent check comes first regardless, so the E4 change is
/// "construct a sender and hand it to a supervised loop" rather than "remember
/// to add a gate".
pub async fn maybe_spawn(app: &AppHandle, pool: &SqlitePool) -> bool {
    let _ = app;
    if !super::consent_active(pool).await {
        tracing::debug!(
            "telemetry: consent is not active — no sender task is spawned, so there is \
             nothing that could reach the network"
        );
        return false;
    }
    tracing::debug!(
        "telemetry: consent is active; reports are queued locally. This build has no \
         sender — the endpoint lands in a later phase."
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::open_pool;
    use crate::telemetry::{consent_set, queue_store as store};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use sundayrec_core::telemetry::queue::{TelemetryEntry, TelemetryStatus};

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn entry(id: &str, created_at: i64) -> TelemetryEntry {
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

    /// A sender that counts its calls, for the positive control.
    #[derive(Clone, Default)]
    struct CountingSender {
        calls: Arc<AtomicUsize>,
        fail: bool,
    }
    impl TelemetrySender for CountingSender {
        fn send<'a>(&'a self, _payload: &'a str) -> SendFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    Err("no route to host".to_string())
                } else {
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn with_consent_off_the_send_path_is_unreachable() {
        let (pool, _d) = temp_pool().await;
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
    /// the consent branch is the part under test, and it is the same expression.
    async fn maybe_spawn_probe(pool: &SqlitePool) -> bool {
        crate::telemetry::consent_active(pool).await
    }

    #[tokio::test]
    async fn the_positive_control_proves_the_sender_is_really_wired() {
        // If this test did not exist, the one above could pass because nothing
        // was connected rather than because consent blocked it.
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::insert_capped(&pool, &entry("a", 0)).await.unwrap();

        let sender = CountingSender {
            fail: true,
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
        let (pool, _d) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        let sender = CountingSender::default();
        assert_eq!(
            pump_once(&pool, &sender, 1_800_000_000_000).await.unwrap(),
            PumpOutcome::Idle
        );
        assert_eq!(sender.calls.load(Ordering::SeqCst), 0);
    }
}
