//! Opt-in telemetry (E3) — the persistence seam around the pure contract.
//!
//! The WIRE CONTRACT (what a payload may contain, and why its types cannot hold
//! anything else) lives in [`sundayrec_core::telemetry`]. The CONSENT STATE
//! MACHINE lives in [`sundayrec_core::telemetry::consent`]. This module is the
//! shell they need: it owns the database rows, the randomness, and the clock.
//!
//! ## The install id
//!
//! A random UUID v7 in the `app_setting` bag. Random — not derived from the
//! machine's serial, the MAC address, the user's e-mail, the church name, or a
//! Sunday Account. That is not a style preference: a derived id is a stable
//! pseudonym for a PERSON, and the owner decision was full anonymity. A random
//! id is a pseudonym for an INSTALL, and the user can throw it away
//! ([`regenerate_install_id`]) and become someone else entirely.
//!
//! It is minted **lazily, and only when consent is active**. Creating an id for
//! a user who has not said yes would already be collection — there would be a
//! per-install identifier sitting in the database of someone who declined.
//! [`install_id_if_any`] is what every read path uses; it never creates.
//!
//! ## Deletion
//!
//! "Delete my data" has two halves. The local half is here:
//! [`regenerate_install_id`] mints a new id, so every future payload belongs to
//! an unrelated install, and parks the OLD id in
//! [`KEY_PENDING_DELETIONS`] so a later phase's sender can issue the remote
//! DELETE for it. Parking it rather than dropping it is the difference between
//! "we stopped adding to your pile" and "your pile is gone".
//!
//! ## The drain
//!
//! [`drain`] is the one function that turns local history into a queued payload:
//! it reads what is newer than the watermarks, builds a payload, enqueues it if
//! there is anything in it, and advances the watermarks. It is idempotent, so
//! WHEN it runs affects only latency, never correctness — which is why it is
//! driven by a plain periodic task and a startup call rather than by hooks
//! threaded through the recorder. A recorder that has to remember to notify
//! telemetry is a recorder with a new way to be wrong.
//!
//! ## What is deliberately absent
//!
//! No network client, no endpoint URL, no sender implementation. E3 builds the
//! client half only; see [`sender`] for how the consent gate is arranged so the
//! later phase adds a sender rather than a check.

use std::time::Duration;

use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use sundayrec_core::telemetry::consent::{
    evaluate, parse_record, ConsentRecord, TelemetryConsent, CONSENT_VERSION,
};
use sundayrec_core::telemetry::queue::{TelemetryEntry, TelemetryQueueStatus, TelemetryStatus};
use sundayrec_core::telemetry::{TelemetryPayload, TelemetryPreview, TELEMETRY_SCHEMA};

use crate::db::store;
use crate::error::AppResult;

pub mod companion;
pub mod config;
pub mod corrections;
pub mod counters;
pub mod http_sender;
pub mod payload;
/// The outbox's sqlx persistence. Named `queue_store` at every use site (see the
/// re-export below) so it never reads as `crate::db::store`, which this module
/// also uses heavily.
pub mod queue_persistence;
pub mod sender;
pub use queue_persistence as queue_store;

/// How often the periodic drain runs. Fifteen minutes: long enough that it is
/// invisible (one settings read and, usually, two small file reads), short
/// enough that a crash from this morning's service is queued before the machine
/// is shut down after it.
pub const DRAIN_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The `app_setting` key holding the random install id (a bare UUID string,
/// JSON-encoded like every other value in the bag).
pub const KEY_INSTALL_ID: &str = "telemetry.installId";

/// The `app_setting` key holding the [`ConsentRecord`] JSON. ABSENT means never
/// asked, which is not the same as "no" — see the core module's state machine.
pub const KEY_CONSENT: &str = "telemetry.consent";

/// The `app_setting` key holding install ids whose remote data the user asked to
/// have deleted, as a JSON array of strings. E4's sender drains it.
pub const KEY_PENDING_DELETIONS: &str = "telemetry.pendingDeletions";

/// How many retired install ids are remembered for deletion. A user who presses
/// "delete my data" ten times before the app is next online has still only asked
/// for one thing; the oldest are dropped rather than growing the row without
/// bound.
pub const MAX_PENDING_DELETIONS: usize = 10;

/// Wall-clock unix milliseconds (UTC).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Consent ─────────────────────────────────────────────────────────────────

/// Read the consent state. Every failure path — no row, unreadable row,
/// malformed JSON — evaluates to `NeverAsked`, i.e. not active.
pub async fn consent_get(pool: &SqlitePool) -> AppResult<TelemetryConsent> {
    let raw = store::get_setting(pool, KEY_CONSENT).await?;
    Ok(evaluate(parse_record(raw.as_deref())))
}

/// Whether telemetry may be collected and sent right now. THE gate: every
/// enqueue path and the (future) sender consult this and nothing else.
///
/// A database error reads as `false`. Failing closed is the only defensible
/// direction — a transient sqlite error must not become a send the user did not
/// agree to.
pub async fn consent_active(pool: &SqlitePool) -> bool {
    consent_get(pool).await.map(|c| c.active).unwrap_or(false)
}

/// Record the user's answer at the CURRENT scope version.
///
/// Granting mints the install id if there is not one yet (the first moment it is
/// legitimate to have one). Revoking is destructive on purpose: the queue is
/// purged and the accumulated counters are dropped, so "off" means there is
/// nothing left to send rather than a paused pile waiting for a change of mind.
/// See [`on_consent_revoked`].
pub async fn consent_set(pool: &SqlitePool, granted: bool) -> AppResult<TelemetryConsent> {
    let record = ConsentRecord::decide(granted, now_ms());
    store::set_setting(pool, KEY_CONSENT, &serde_json::to_string(&record)?).await?;

    if granted {
        let id = ensure_install_id(pool).await?;
        // Start the watermarks at NOW. A machine that has recorded for years has
        // years of crash records and quality history on disk; consent given today
        // is consent to share what happens from today. Back-filling the archive
        // would answer the same question about a period nobody agreed to.
        payload::set_watermarks(pool, payload::Watermarks::starting_now(now_ms())).await?;
        counters::set_active(true);
        tracing::info!(
            consent_version = CONSENT_VERSION,
            install_id_present = !id.is_empty(),
            "telemetry: consent GRANTED — reporting starts from now, not from the archive"
        );
    } else {
        on_consent_revoked(pool).await?;
        tracing::info!("telemetry: consent DENIED/revoked — nothing is collected or queued");
    }
    consent_get(pool).await
}

/// Everything that must be true the instant consent stops being active.
///
/// Called on an explicit revoke and when the install id is retired. Kept as its
/// own function because each part of E3 adds a line to it, and a revoke that
/// forgets one of them leaves collected data on a machine whose owner said no.
pub async fn on_consent_revoked(pool: &SqlitePool) -> AppResult<()> {
    // The synchronous counter path checks this mirror, so flipping it FIRST
    // means nothing new is accumulated while the purge below runs. The banded
    // corrections consult the SAME mirror — one flag for both accumulators,
    // because two mirrors of one fact are two things that can disagree, and the
    // way they would disagree is "still counting after a revoke".
    counters::set_active(false);
    // `set_active(false)` drops the counter map itself; the correction and
    // companion maps are separate `OnceLock`s and have to be told too. A revoke
    // must not leave counts in RAM waiting for a change of mind.
    corrections::clear();
    companion::clear();
    // Reset the watermarks so a later re-grant starts from ITS own "now" rather
    // than sweeping up everything that happened while telemetry was off.
    purge_collected(pool, payload::Watermarks::default()).await
}

/// Drop everything collected but not yet delivered, and restart the watermarks
/// at `reset_to`.
///
/// Two callers with two different resets, and the difference is the whole
/// reason this is a parameter. A REVOKE resets to zero: telemetry is off, and
/// the next grant will set its own starting point. A REGENERATE resets to NOW:
/// consent is still active and collection continues, but nothing that happened
/// under the retired install id may be re-reported under the new one — which is
/// exactly what a zero watermark would have caused.
async fn purge_collected(pool: &SqlitePool, reset_to: payload::Watermarks) -> AppResult<()> {
    let dropped = queue_store::purge(pool).await?;
    payload::clear_pending_findings(pool).await?;
    payload::set_watermarks(pool, reset_to).await?;
    counters::purge(pool).await?;
    corrections::purge(pool).await?;
    companion::purge(pool).await?;
    if dropped > 0 {
        tracing::info!("telemetry: purged {dropped} queued report(s)");
    }
    Ok(())
}

// ── The drain ───────────────────────────────────────────────────────────────

/// Turn whatever is newer than the watermarks into ONE queued payload.
///
/// Returns whether anything was enqueued. Does nothing at all without active
/// consent — the first statement, before any file is read.
///
/// The order matters: enqueue FIRST, then advance the watermarks. The reverse
/// would lose a batch if the insert failed, and re-queuing a batch is harmless
/// (the `dedup_key` unique index absorbs it) while losing one is silent.
pub async fn drain(app: &AppHandle, pool: &SqlitePool) -> AppResult<bool> {
    let Ok(app_data_dir) = app.path().app_data_dir() else {
        return Ok(false);
    };
    let version = app.package_info().version.to_string();
    drain_in(pool, &app_data_dir, &version).await
}

/// [`drain`] without an `AppHandle` — the whole of the behaviour, taking the two
/// facts the handle was only being asked for.
///
/// Split out so the consent gate is testable. A unit test cannot construct an
/// `AppHandle`, and "nothing is enqueued without consent" is precisely the
/// property that must not rest on an untested line.
pub async fn drain_in(
    pool: &SqlitePool,
    app_data_dir: &std::path::Path,
    app_version: &str,
) -> AppResult<bool> {
    if !consent_active(pool).await {
        return Ok(false);
    }
    // A zero watermark would report the entire local archive. Granting consent
    // writes a real one, so seeing zeros here means the row was lost or
    // hand-cleared — set the starting point and report nothing THIS round rather
    // than back-filling years of history on a technicality.
    let since = payload::watermarks(pool).await?;
    if since == payload::Watermarks::default() {
        payload::set_watermarks(pool, payload::Watermarks::starting_now(now_ms())).await?;
        tracing::warn!("telemetry: watermarks were missing — restarting reporting from now");
        return Ok(false);
    }

    let install_id = install_id_if_any(pool).await?;
    let home = home_dir();
    let ctx = payload::GatherContext {
        app_data_dir,
        app_version,
        home: home.as_deref(),
        now_ms: now_ms(),
        consent_version: CONSENT_VERSION,
        // Peeked, not taken: a payload that turns out empty, or an enqueue that
        // fails, must leave the counts where they are.
        counters: counters::snapshot(),
        corrections: corrections::snapshot(),
        companion_outcomes: companion::snapshot(),
    };
    let (built, next) = payload::build(pool, &ctx, since, install_id.as_deref()).await?;

    if built.is_empty() {
        return Ok(false);
    }
    let queued = enqueue(pool, &built, ctx.now_ms).await?;
    payload::set_watermarks(pool, next).await?;
    payload::clear_pending_findings(pool).await?;
    // Only now are the counts spent — and SUBTRACTED, so a click that landed
    // while the payload was being written is not lost.
    counters::consume(&ctx.counters);
    counters::persist(pool).await?;
    corrections::consume(&ctx.corrections);
    corrections::persist(pool).await?;
    companion::consume(&ctx.companion_outcomes);
    companion::persist(pool).await?;
    if queued {
        tracing::info!(
            crashes = built.crashes.len(),
            quality = built.quality.len(),
            findings = built.findings.len(),
            corrections = built.corrections.len(),
            companion_outcomes = built.companion_outcomes.len(),
            "telemetry: queued one report (nothing is sent — no endpoint in this build)"
        );
    }
    Ok(queued)
}

/// Put one built payload into the outbox. The `dedup_key` names the newest
/// record the payload covers, so two drains racing to report the same batch
/// produce one row.
async fn enqueue(pool: &SqlitePool, p: &TelemetryPayload, now_ms: i64) -> AppResult<bool> {
    let newest = p
        .crashes
        .iter()
        .map(|c| c.at)
        .chain(p.quality.iter().map(|q| q.at))
        .chain(p.wake_failures.iter().map(|w| w.at))
        .max()
        .unwrap_or(now_ms);
    let kind = if !p.crashes.is_empty() {
        "crash"
    } else if !p.quality.is_empty() {
        "quality"
    } else {
        "counters"
    };
    let entry = TelemetryEntry {
        id: store::new_id(),
        created_at: now_ms,
        schema_ver: TELEMETRY_SCHEMA,
        dedup_key: format!("{kind}:{newest}"),
        payload_json: serde_json::to_string(p)?,
        attempts: 0,
        next_attempt: now_ms,
        last_error: None,
        status: TelemetryStatus::Pending,
    };
    queue_store::insert_capped(pool, &entry).await
}

// ── Transparency ────────────────────────────────────────────────────────────

/// Build the preview the settings UI shows for "vis hva som sendes".
///
/// The REAL payload, through the REAL builder — a mock would be worse than no
/// preview at all, because it would be a promise about code that never runs.
/// Read-only: no install id is minted, no watermark advances, no counter is
/// spent. Calling it a hundred times changes nothing.
pub async fn preview_payload(app: &AppHandle, pool: &SqlitePool) -> AppResult<TelemetryPreview> {
    let Ok(app_data_dir) = app.path().app_data_dir() else {
        return Err(crate::error::AppError::Internal(
            "fant ikke app-datamappen".into(),
        ));
    };
    let version = app.package_info().version.to_string();
    preview_payload_in(pool, &app_data_dir, &version).await
}

/// [`preview_payload`] without an `AppHandle`, so the behaviour is testable.
pub async fn preview_payload_in(
    pool: &SqlitePool,
    app_data_dir: &std::path::Path,
    app_version: &str,
) -> AppResult<TelemetryPreview> {
    let active = consent_active(pool).await;
    // With consent on: exactly the next payload. With consent off there is no
    // "next", so fall back to the whole local history — see `TelemetryPreview`
    // for why that is the honest answer rather than an empty shell.
    let since = if active {
        payload::watermarks(pool).await?
    } else {
        payload::Watermarks::default()
    };
    let home = home_dir();
    let ctx = payload::GatherContext {
        app_data_dir,
        app_version,
        home: home.as_deref(),
        now_ms: now_ms(),
        consent_version: CONSENT_VERSION,
        counters: counters::snapshot(),
        corrections: corrections::snapshot(),
        companion_outcomes: companion::snapshot(),
    };
    // `install_id_if_any`, never `ensure_install_id`: a user who has not opted in
    // must be able to read this page without it minting an identifier for them.
    // They see the nil UUID, which is the truth — they do not have one yet.
    let install_id = install_id_if_any(pool).await?;
    let (built, _) = payload::build(pool, &ctx, since, install_id.as_deref()).await?;
    Ok(TelemetryPreview {
        json: serde_json::to_string_pretty(&built)?,
        is_next_payload: active,
        is_empty: built.is_empty(),
    })
}

/// What is waiting in the outbox, for a settings panel that has to be honest
/// about it.
pub async fn queue_status(pool: &SqlitePool) -> AppResult<TelemetryQueueStatus> {
    Ok(sundayrec_core::telemetry::queue::queue_status(
        &queue_store::load_queue(pool).await?,
    ))
}

/// Everything telemetry does at startup, in order.
///
/// Called once from `setup`, after the pool is managed. With consent off it
/// reads one settings row and returns — no ring is scanned, no id is minted, no
/// task is spawned.
pub async fn startup(app: &AppHandle, pool: &SqlitePool) {
    if !consent_active(pool).await {
        // Leave the counter mirror at its `false` default: with consent off no
        // seam accumulates anything, not even in memory.
        tracing::debug!("telemetry: consent is not active — nothing is collected");
        return;
    }
    counters::set_active(true);
    if let Err(e) = counters::load(pool).await {
        tracing::warn!("telemetry: could not restore counters: {e}");
    }
    if let Err(e) = corrections::load(pool).await {
        tracing::warn!("telemetry: could not restore banded corrections: {e}");
    }
    if let Err(e) = companion::load(pool).await {
        tracing::warn!("telemetry: could not restore companion outcomes: {e}");
    }
    // A force-quit mid-send strands a row in `sending` forever otherwise.
    match queue_store::reset_stale_sending(pool).await {
        Ok(n) if n > 0 => tracing::info!("telemetry: requeued {n} report(s) stranded mid-send"),
        Ok(_) => {}
        Err(e) => tracing::warn!("telemetry: could not reset stranded reports: {e}"),
    }
    // The crashes and quality records from the LAST run.
    if let Err(e) = drain(app, pool).await {
        tracing::warn!("telemetry: startup drain failed: {e}");
    }
    sender::maybe_spawn(app, pool).await;
}

/// Arm the periodic drain. Supervised (E2.2), because a drain task that dies
/// silently means telemetry stops without anyone noticing — which is a benign
/// failure for the user and an invisible one for us, the worst combination for
/// something whose whole purpose is to make problems visible.
pub fn spawn_periodic_drain(app: AppHandle) {
    crate::supervise::supervised_spawn(
        app.clone(),
        "telemetry::drain",
        crate::supervise::TaskAlert {
            title: "SundayRec",
            body: "Bakgrunnsoppgaven for kvalitetsrapporter startet på nytt.",
        },
        move || {
            let app = app.clone();
            async move {
                loop {
                    tokio::time::sleep(DRAIN_INTERVAL).await;
                    let Some(db) = app.try_state::<crate::db::Db>() else {
                        continue;
                    };
                    if let Err(e) = drain(&app, &db.pool).await {
                        tracing::warn!("telemetry: periodic drain failed: {e}");
                    }
                    // Flush the counters even when nothing was queued, so a quit
                    // between drains loses at most one interval's clicks — and
                    // the banded corrections with them, which matter more per
                    // item: a click is one of thousands, a correction is a
                    // person having told us something.
                    if counters::is_active() {
                        if let Err(e) = counters::persist(&db.pool).await {
                            tracing::warn!("telemetry: could not persist counters: {e}");
                        }
                        if let Err(e) = corrections::persist(&db.pool).await {
                            tracing::warn!("telemetry: could not persist banded corrections: {e}");
                        }
                        if let Err(e) = companion::persist(&db.pool).await {
                            tracing::warn!("telemetry: could not persist companion outcomes: {e}");
                        }
                    }
                }
            }
        },
    );
}

/// The user's home directory, for the free-text scrubbers. Same lookup the
/// crash ring uses.
fn home_dir() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|v| v.to_string_lossy().into_owned())
}

// ── Install id ──────────────────────────────────────────────────────────────

/// The install id if one has been minted, WITHOUT minting one.
///
/// Every read path uses this. The preview command in particular must be able to
/// show a payload before consent is granted, and it does so with
/// [`sundayrec_core::telemetry::NIL_INSTALL_ID`] rather than by quietly creating
/// a real identifier for someone who has not said yes.
pub async fn install_id_if_any(pool: &SqlitePool) -> AppResult<Option<String>> {
    let raw = store::get_setting(pool, KEY_INSTALL_ID).await?;
    Ok(raw.and_then(|v| serde_json::from_str::<String>(&v).ok()))
}

/// The install id, minting a fresh random UUID v7 on first need.
///
/// Only called from paths that have already established that consent is active.
pub async fn ensure_install_id(pool: &SqlitePool) -> AppResult<String> {
    if let Some(existing) = install_id_if_any(pool).await? {
        return Ok(existing);
    }
    let id = new_install_id();
    store::set_setting(pool, KEY_INSTALL_ID, &serde_json::to_string(&id)?).await?;
    tracing::info!("telemetry: minted a new random install id");
    Ok(id)
}

/// A fresh random install id. UUID v7 — time-ordered, but the ordering is the
/// only thing derived from anything: the remaining 74 bits are random, and
/// nothing about the machine, the user or the congregation is an input.
fn new_install_id() -> String {
    Uuid::now_v7().to_string()
}

/// The local half of "delete my data": become a different install.
///
/// Mints a new id, parks the old one for the remote DELETE a later phase will
/// send, and purges anything queued under the old id (those payloads carry it,
/// so keeping them would re-upload what the user just asked to have removed).
/// Returns the NEW id.
pub async fn regenerate_install_id(pool: &SqlitePool) -> AppResult<String> {
    let previous = install_id_if_any(pool).await?;
    if let Some(old) = previous.as_deref() {
        park_for_deletion(pool, old).await?;
    }
    let id = new_install_id();
    store::set_setting(pool, KEY_INSTALL_ID, &serde_json::to_string(&id)?).await?;
    // Anything already queued is addressed to the retired id, and the history it
    // was built from must not be re-reported under the new one.
    purge_collected(pool, payload::Watermarks::starting_now(now_ms())).await?;
    tracing::info!(
        had_previous = previous.is_some(),
        "telemetry: install id regenerated — future reports belong to a new, unrelated install"
    );
    Ok(id)
}

/// Remember an install id whose remote data should be deleted, newest last,
/// capped at [`MAX_PENDING_DELETIONS`].
async fn park_for_deletion(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let mut ids = pending_deletions(pool).await?;
    if !ids.iter().any(|existing| existing == id) {
        ids.push(id.to_string());
    }
    if ids.len() > MAX_PENDING_DELETIONS {
        ids.drain(..ids.len() - MAX_PENDING_DELETIONS);
    }
    store::set_setting(pool, KEY_PENDING_DELETIONS, &serde_json::to_string(&ids)?).await?;
    Ok(())
}

/// Install ids waiting for a remote DELETE, oldest first. A malformed row reads
/// as empty rather than failing — a broken list must not block the regenerate
/// that is trying to append to it.
pub async fn pending_deletions(pool: &SqlitePool) -> AppResult<Vec<String>> {
    let raw = store::get_setting(pool, KEY_PENDING_DELETIONS).await?;
    Ok(raw
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default())
}

/// Drop an id from the pending-deletion list (E4 calls this once the endpoint
/// has confirmed the delete).
pub async fn clear_pending_deletion(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let ids: Vec<String> = pending_deletions(pool)
        .await?
        .into_iter()
        .filter(|existing| existing != id)
        .collect();
    store::set_setting(pool, KEY_PENDING_DELETIONS, &serde_json::to_string(&ids)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s, which is
    // what clippy is warning about. It is correct HERE and only here: it exists
    // to serialise tests against process-global counter state, every test that
    // takes it runs on its own single-threaded `#[tokio::test]` runtime, and
    // nothing inside the guarded region can block on the same lock. A
    // `tokio::sync::Mutex` would not work — the synchronous `#[test]`s need the
    // same lock.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use sundayrec_core::telemetry::consent::ConsentStatus;
    use sundayrec_core::telemetry::sanitize_install_id;

    /// A throwaway database AND the counter-state lock: `consent_set` writes the
    /// process-global counter mirror, so every test here has to serialise on it
    /// or two tests decide each other's assertions.
    async fn temp_pool() -> (
        SqlitePool,
        tempfile::TempDir,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let guard = counters::test_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir, guard)
    }

    #[tokio::test]
    async fn a_fresh_install_has_never_been_asked_and_has_no_id() {
        let (pool, _d, _g) = temp_pool().await;
        let c = consent_get(&pool).await.unwrap();
        assert_eq!(c.status, ConsentStatus::NeverAsked);
        assert!(c.needs_prompt);
        assert!(!c.active);
        assert!(!consent_active(&pool).await);
        assert_eq!(
            install_id_if_any(&pool).await.unwrap(),
            None,
            "an id must not exist before anyone has said yes"
        );
    }

    #[tokio::test]
    async fn granting_mints_exactly_one_id_and_keeps_it() {
        let (pool, _d, _g) = temp_pool().await;
        let c = consent_set(&pool, true).await.unwrap();
        assert_eq!(c.status, ConsentStatus::Granted);
        assert!(c.active);
        assert!(!c.needs_prompt);

        let id = install_id_if_any(&pool).await.unwrap().expect("minted");
        assert_eq!(
            sanitize_install_id(&id),
            id,
            "the minted id must survive the wire sanitizer unchanged"
        );
        // Granting again is idempotent for the id.
        consent_set(&pool, true).await.unwrap();
        assert_eq!(install_id_if_any(&pool).await.unwrap(), Some(id));
    }

    #[tokio::test]
    async fn denying_records_the_answer_without_minting_an_id() {
        let (pool, _d, _g) = temp_pool().await;
        let c = consent_set(&pool, false).await.unwrap();
        assert_eq!(c.status, ConsentStatus::Denied);
        assert!(!c.active);
        assert!(
            !c.needs_prompt,
            "a 'no' at the current scope must not be re-asked"
        );
        assert_eq!(
            install_id_if_any(&pool).await.unwrap(),
            None,
            "saying no must not leave an identifier behind"
        );
    }

    #[tokio::test]
    async fn consent_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.sqlite");
        let id = {
            let pool = store::open_pool(&path).await.expect("open");
            consent_set(&pool, true).await.unwrap();
            let id = install_id_if_any(&pool).await.unwrap().expect("minted");
            pool.close().await;
            id
        };
        let pool = store::open_pool(&path).await.expect("reopen");
        assert!(consent_active(&pool).await);
        assert_eq!(install_id_if_any(&pool).await.unwrap(), Some(id));
    }

    #[tokio::test]
    async fn a_v1_row_from_an_installed_copy_closes_the_gate_and_re_asks() {
        // The E8.D1 upgrade as it actually happens: not a constructed
        // `ConsentRecord`, but the literal bytes `consent_set` wrote under
        // v0.10.0, read back by a build whose CONSENT_VERSION is 2. The unit
        // tests in consent.rs prove `evaluate`; this proves the whole chain
        // that runs at startup — sqlite row → parse → evaluate → THE gate.
        let (pool, _d, _g) = temp_pool().await;
        for (raw, what) in [
            (
                r#"{"granted":true,"version":1,"decidedAt":1754000000000}"#,
                "a v1 YES",
            ),
            (
                r#"{"granted":false,"version":1,"decidedAt":1754000000000}"#,
                "a v1 NO",
            ),
        ] {
            store::set_setting(&pool, KEY_CONSENT, raw).await.unwrap();
            let c = consent_get(&pool).await.unwrap();
            assert!(
                !consent_active(&pool).await,
                "{what} must not permit sending under the widened scope"
            );
            assert!(c.needs_prompt, "{what} must be put the new question");
            assert_eq!(c.version, 1, "{what} is remembered as answered at v1");
        }

        // And a NO stays a NO across the bump — read back from storage, not
        // from a value we just constructed in memory.
        let c = consent_get(&pool).await.unwrap();
        assert_eq!(c.status, ConsentStatus::Denied);
    }

    #[tokio::test]
    async fn a_hand_edited_consent_row_reads_as_never_asked() {
        let (pool, _d, _g) = temp_pool().await;
        for junk in ["", "null", "{", "true", "{\"granted\":true}"] {
            store::set_setting(&pool, KEY_CONSENT, junk).await.unwrap();
            let c = consent_get(&pool).await.unwrap();
            assert_eq!(c.status, ConsentStatus::NeverAsked, "{junk:?}");
            assert!(!consent_active(&pool).await, "{junk:?}");
        }
    }

    #[tokio::test]
    async fn regenerating_makes_a_new_install_and_parks_the_old_id() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        let first = install_id_if_any(&pool).await.unwrap().expect("minted");

        let second = regenerate_install_id(&pool).await.unwrap();
        assert_ne!(second, first, "a new install, not a renamed one");
        assert_eq!(
            install_id_if_any(&pool).await.unwrap(),
            Some(second.clone())
        );
        assert_eq!(
            pending_deletions(&pool).await.unwrap(),
            vec![first.clone()],
            "the retired id must be remembered so the remote copy can be deleted"
        );
        // Consent itself is untouched: deleting your data is not withdrawing.
        assert!(consent_active(&pool).await);

        // The endpoint confirming the delete clears it.
        clear_pending_deletion(&pool, &first).await.unwrap();
        assert!(pending_deletions(&pool).await.unwrap().is_empty());
        // …and the current id is never parked by that.
        assert_eq!(install_id_if_any(&pool).await.unwrap(), Some(second));
    }

    #[tokio::test]
    async fn the_pending_deletion_list_is_bounded_and_deduplicated() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        for _ in 0..(MAX_PENDING_DELETIONS + 5) {
            regenerate_install_id(&pool).await.unwrap();
        }
        let ids = pending_deletions(&pool).await.unwrap();
        assert_eq!(ids.len(), MAX_PENDING_DELETIONS);
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "no id is parked twice");
    }

    #[tokio::test]
    async fn a_malformed_pending_list_does_not_block_a_regenerate() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        store::set_setting(&pool, KEY_PENDING_DELETIONS, "not json")
            .await
            .unwrap();
        let new = regenerate_install_id(&pool).await.unwrap();
        assert_eq!(install_id_if_any(&pool).await.unwrap(), Some(new));
        assert_eq!(pending_deletions(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn revoking_after_granting_stops_the_gate_immediately() {
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        assert!(consent_active(&pool).await);
        consent_set(&pool, false).await.unwrap();
        assert!(!consent_active(&pool).await);
        assert_eq!(
            consent_get(&pool).await.unwrap().status,
            ConsentStatus::Denied
        );
    }

    // ── The drain ────────────────────────────────────────────────────────────

    /// Put a crash record on disk so a drain has something to find.
    fn write_crash(app_data_dir: &std::path::Path, timestamp: &str, message: &str) {
        let dir = app_data_dir.join("crashes");
        std::fs::create_dir_all(&dir).unwrap();
        let json = serde_json::json!({
            "schema": 1,
            "kind": "panic",
            "timestamp": timestamp,
            "appVersion": "0.10.0",
            "os": "macos",
            "arch": "aarch64",
            "thread": "main",
            "message": message,
            "location": null,
            "backtrace": null,
            "task": null,
        });
        std::fs::write(
            dir.join(format!("crash-{}-0000.json", timestamp.len())),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
    }

    /// An RFC 3339 timestamp `minutes_ago` in the past — the shape the crash
    /// ring writes.
    fn minutes_ago(minutes: i64) -> String {
        (chrono::Local::now() - chrono::Duration::minutes(minutes)).to_rfc3339()
    }

    /// Grant consent and then wind the watermarks back, so a fixture written
    /// "an hour ago" counts as new. Granting alone stamps them at NOW, which is
    /// correct in production and useless for a test that cannot wait an hour.
    async fn grant_reporting_from(pool: &SqlitePool, minutes: i64) {
        consent_set(pool, true).await.unwrap();
        let since = now_ms() - minutes * 60_000;
        payload::set_watermarks(
            pool,
            payload::Watermarks {
                crash_at: since,
                quality_at: since,
                wake_at: since,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn nothing_is_enqueued_without_consent() {
        // THE enqueue-side half of the consent guarantee: a machine with a crash
        // to report and telemetry off produces an empty outbox. Asserted for both
        // "never asked" and "explicitly denied", because they are different rows.
        let (pool, dir, _g) = temp_pool().await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");

        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());

        consent_set(&pool, false).await.unwrap();
        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());

        // …and no install id was invented along the way.
        assert_eq!(install_id_if_any(&pool).await.unwrap(), None);
    }

    #[tokio::test]
    async fn with_consent_the_same_crash_is_enqueued_exactly_once() {
        // The positive control for the test above — and the idempotence the
        // drain's schedule-independence rests on.
        let (pool, dir, _g) = temp_pool().await;
        grant_reporting_from(&pool, 120).await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");

        assert!(drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        let queued = queue_store::load_queue(&pool).await.unwrap();
        assert_eq!(queued.len(), 1);
        assert!(queued[0].payload_json.contains("en krasj"));
        assert!(queued[0].dedup_key.starts_with("crash:"));

        // A second drain finds nothing new.
        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert_eq!(queue_store::load_queue(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_granted_install_does_not_report_what_happened_before_it_said_yes() {
        let (pool, dir, _g) = temp_pool().await;
        write_crash(dir.path(), "2024-03-01T12:00:00+01:00", "gammel krasj");
        consent_set(&pool, true).await.unwrap();
        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_missing_watermark_restarts_reporting_rather_than_back_filling() {
        // The defensive branch: a hand-cleared or lost watermark row must not be
        // read as "report the entire archive".
        let (pool, dir, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        write_crash(dir.path(), "2024-03-01T12:00:00+01:00", "arkivert krasj");
        store::delete_setting(&pool, payload::KEY_WATERMARKS)
            .await
            .unwrap();

        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());
        assert_ne!(
            payload::watermarks(&pool).await.unwrap(),
            payload::Watermarks::default(),
            "the drain must have restarted the watermark, not left it at zero"
        );
    }

    #[tokio::test]
    async fn revoking_empties_the_outbox_and_the_finding_cache() {
        let (pool, dir, _g) = temp_pool().await;
        grant_reporting_from(&pool, 120).await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");
        assert!(drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert_eq!(queue_store::load_queue(&pool).await.unwrap().len(), 1);

        consent_set(&pool, false).await.unwrap();
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());
        assert!(payload::pending_findings(&pool).await.unwrap().is_empty());
        assert_eq!(
            payload::watermarks(&pool).await.unwrap(),
            payload::Watermarks::default()
        );
    }

    // ── The transparency surfaces ────────────────────────────────────────────

    #[tokio::test]
    async fn the_preview_is_readable_before_consent_and_mints_nothing() {
        // The whole point of the affordance: a user deciding whether to opt in
        // must be able to SEE the answer first, and looking must not itself be
        // an act of collection.
        let (pool, dir, _g) = temp_pool().await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");

        let p = preview_payload_in(&pool, dir.path(), "0.10.0")
            .await
            .unwrap();
        assert!(
            !p.is_next_payload,
            "with consent off there IS no next payload — the UI must be told so"
        );
        assert!(!p.is_empty, "…but the user still sees their own real data");
        assert!(p.json.contains("en krasj"));
        assert!(
            p.json.contains("\"schema\": 1"),
            "pretty-printed: {}",
            p.json
        );
        assert!(
            p.json.contains(sundayrec_core::telemetry::NIL_INSTALL_ID),
            "no id exists yet, and the preview says so rather than making one"
        );

        // Nothing was created or advanced by looking.
        assert_eq!(install_id_if_any(&pool).await.unwrap(), None);
        assert!(queue_store::load_queue(&pool).await.unwrap().is_empty());
        assert_eq!(
            payload::watermarks(&pool).await.unwrap(),
            payload::Watermarks::default()
        );
        // …and it is idempotent apart from `builtAt`, which is the clock and
        // has to move.
        let again = preview_payload_in(&pool, dir.path(), "0.10.0")
            .await
            .unwrap();
        let (mut first, mut second): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(&p.json).unwrap(),
            serde_json::from_str(&again.json).unwrap(),
        );
        first["builtAt"] = serde_json::Value::Null;
        second["builtAt"] = serde_json::Value::Null;
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn with_consent_the_preview_is_literally_the_next_payload() {
        let (pool, dir, _g) = temp_pool().await;
        grant_reporting_from(&pool, 120).await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");

        let p = preview_payload_in(&pool, dir.path(), "0.10.0")
            .await
            .unwrap();
        assert!(p.is_next_payload);
        assert!(!p.is_empty);

        // And it IS: draining now queues a payload with the same records.
        assert!(drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        let queued = &queue_store::load_queue(&pool).await.unwrap()[0];
        let previewed: serde_json::Value = serde_json::from_str(&p.json).unwrap();
        let sent: serde_json::Value = serde_json::from_str(&queued.payload_json).unwrap();
        for field in ["crashes", "quality", "findings", "wakeFailures", "settings"] {
            assert_eq!(previewed[field], sent[field], "{field} differs");
        }

        // Once drained there is nothing left to send, and the preview says so
        // rather than repeating what already went.
        let after = preview_payload_in(&pool, dir.path(), "0.10.0")
            .await
            .unwrap();
        assert!(after.is_empty);
        assert!(after.is_next_payload);
    }

    #[tokio::test]
    async fn the_preview_never_carries_what_the_scope_excludes() {
        // An end-to-end version of the core's type-level test: real settings on a
        // real machine, through the real builder, out to the real JSON string the
        // user is shown.
        let (pool, dir, _g) = temp_pool().await;
        crate::settings::save(
            &pool,
            sundayrec_core::settings::Settings {
                device_name: Some("Kari sin Qu-5".into()),
                save_folder: Some("/Users/kari/Menigheten/Opptak".into()),
                church_name: "Nordstrand menighet".into(),
                responsible_person: "Kari Nordmann".into(),
                email_address: "kari@menighet.no".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        write_crash(
            dir.path(),
            &minutes_ago(30),
            "kunne ikke åpne /Users/kari/Opptak/gudstjeneste.wav",
        );

        let p = preview_payload_in(&pool, dir.path(), "0.10.0")
            .await
            .unwrap();
        for needle in [
            "Kari",
            "kari",
            "Qu-5",
            "Nordstrand",
            "menighet",
            "/Users/",
            "Opptak",
            "gudstjeneste.wav",
            "@",
        ] {
            assert!(
                !p.json.contains(needle),
                "{needle:?} reached the preview:\n{}",
                p.json
            );
        }
    }

    #[tokio::test]
    async fn the_queue_status_is_specific_rather_than_reassuring() {
        let (pool, dir, _g) = temp_pool().await;
        assert_eq!(
            queue_status(&pool).await.unwrap(),
            TelemetryQueueStatus::default(),
            "an untouched install reports an empty queue, not an error"
        );

        grant_reporting_from(&pool, 120).await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");
        drain_in(&pool, dir.path(), "0.10.0").await.unwrap();

        let s = queue_status(&pool).await.unwrap();
        assert_eq!(s.pending, 1);
        assert_eq!(s.failed, 0);
        assert!(s.oldest_at.is_some());
        assert_eq!(s.last_error, None);
    }

    #[tokio::test]
    async fn counters_reach_the_payload_and_are_spent_exactly_once() {
        use sundayrec_core::telemetry::CounterName;
        let (pool, dir, _g) = temp_pool().await;
        grant_reporting_from(&pool, 120).await;

        counters::count(CounterName::EditorOpened);
        counters::count(CounterName::EditorOpened);
        counters::count(CounterName::RecordingStartedManual);

        // Counters ALONE are worth a report — nothing else has happened here.
        assert!(drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        let queued = queue_store::load_queue(&pool).await.unwrap();
        assert_eq!(queued.len(), 1);
        assert!(queued[0].payload_json.contains("editor.opened"));
        assert!(queued[0].dedup_key.starts_with("counters:"));

        // Spent: the next drain has nothing to say.
        assert!(counters::snapshot().is_empty());
        assert!(!drain_in(&pool, dir.path(), "0.10.0").await.unwrap());
        assert_eq!(queue_store::load_queue(&pool).await.unwrap().len(), 1);

        // …and they survived to storage, so a quit does not lose them.
        counters::clear();
        counters::load(&pool).await.unwrap();
        assert!(counters::snapshot().is_empty());
    }

    #[tokio::test]
    async fn revoking_clears_the_counters_too() {
        use sundayrec_core::telemetry::CounterName;
        let (pool, _d, _g) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        counters::count(CounterName::StreamingStarted);
        assert_eq!(counters::snapshot().len(), 1);

        consent_set(&pool, false).await.unwrap();
        assert!(
            !counters::is_active(),
            "the synchronous mirror follows consent"
        );
        assert!(counters::snapshot().is_empty());
        counters::load(&pool).await.unwrap();
        assert!(
            counters::snapshot().is_empty(),
            "and the persisted row is gone as well"
        );

        // Counting after a revoke accumulates nothing at all.
        counters::count(CounterName::StreamingStarted);
        assert!(counters::snapshot().is_empty());
    }

    #[tokio::test]
    async fn regenerating_drops_the_queue_but_keeps_collecting() {
        // Deleting your data is not withdrawing consent — collection continues,
        // but nothing already gathered may be re-reported under the new id.
        let (pool, dir, _g) = temp_pool().await;
        grant_reporting_from(&pool, 120).await;
        write_crash(dir.path(), &minutes_ago(60), "en krasj");
        assert!(drain_in(&pool, dir.path(), "0.10.0").await.unwrap());

        regenerate_install_id(&pool).await.unwrap();
        assert!(
            queue_store::load_queue(&pool).await.unwrap().is_empty(),
            "payloads addressed to the retired id must not be delivered"
        );
        assert!(consent_active(&pool).await, "consent is untouched");
        assert_ne!(
            payload::watermarks(&pool).await.unwrap(),
            payload::Watermarks::default(),
            "…and the watermark restarts at now, so the same crash is not re-reported"
        );
        assert!(
            !drain_in(&pool, dir.path(), "0.10.0").await.unwrap(),
            "the crash the retired id already covered must not come back"
        );
    }
}
