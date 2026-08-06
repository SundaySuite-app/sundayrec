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
//! ## What is deliberately absent
//!
//! No network client, no endpoint URL, no sender task. E3 builds the client half
//! only; nothing in this module can reach a socket.

use sqlx::SqlitePool;
use uuid::Uuid;

use sundayrec_core::telemetry::consent::{
    evaluate, parse_record, ConsentRecord, TelemetryConsent, CONSENT_VERSION,
};

use crate::db::store;
use crate::error::AppResult;

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
/// The purge lands in [`crate::telemetry::store`] once the queue exists (E3.3);
/// the hook is [`on_consent_revoked`].
pub async fn consent_set(pool: &SqlitePool, granted: bool) -> AppResult<TelemetryConsent> {
    let record = ConsentRecord::decide(granted, now_ms());
    store::set_setting(pool, KEY_CONSENT, &serde_json::to_string(&record)?).await?;

    if granted {
        let id = ensure_install_id(pool).await?;
        tracing::info!(
            consent_version = CONSENT_VERSION,
            install_id_present = !id.is_empty(),
            "telemetry: consent GRANTED"
        );
    } else {
        on_consent_revoked(pool).await?;
        tracing::info!("telemetry: consent DENIED/revoked — nothing is collected or queued");
    }
    consent_get(pool).await
}

/// Everything that must be true the instant consent stops being active.
///
/// Called on an explicit revoke. Kept as its own function because E3.3 and E3.4
/// each add a line to it, and a revoke that forgets one of them leaves collected
/// data on a machine whose owner said no.
pub async fn on_consent_revoked(pool: &SqlitePool) -> AppResult<()> {
    // E3.3 purges the outbox here; E3.4 clears the counter map.
    let _ = pool;
    Ok(())
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
    // Anything already queued is addressed to the retired id.
    on_consent_revoked(pool).await?;
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
    use super::*;
    use sundayrec_core::telemetry::consent::ConsentStatus;
    use sundayrec_core::telemetry::sanitize_install_id;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[tokio::test]
    async fn a_fresh_install_has_never_been_asked_and_has_no_id() {
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
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
    async fn a_hand_edited_consent_row_reads_as_never_asked() {
        let (pool, _d) = temp_pool().await;
        for junk in ["", "null", "{", "true", "{\"granted\":true}"] {
            store::set_setting(&pool, KEY_CONSENT, junk).await.unwrap();
            let c = consent_get(&pool).await.unwrap();
            assert_eq!(c.status, ConsentStatus::NeverAsked, "{junk:?}");
            assert!(!consent_active(&pool).await, "{junk:?}");
        }
    }

    #[tokio::test]
    async fn regenerating_makes_a_new_install_and_parks_the_old_id() {
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
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
        let (pool, _d) = temp_pool().await;
        consent_set(&pool, true).await.unwrap();
        assert!(consent_active(&pool).await);
        consent_set(&pool, false).await.unwrap();
        assert!(!consent_active(&pool).await);
        assert_eq!(
            consent_get(&pool).await.unwrap().status,
            ConsentStatus::Denied
        );
    }
}
