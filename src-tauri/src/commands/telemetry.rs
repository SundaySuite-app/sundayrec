//! The telemetry IPC surface (E3) — consent, deletion, counters, transparency.
//!
//! ## Path policy
//!
//! None of these commands takes a path, and none of them can be made to touch
//! one: the only storage they reach is the `app_setting` bag and the telemetry
//! outbox table, both addressed by constants inside the process. As with
//! `commands::logs` (E2.3), having nothing for the renderer to name is strictly
//! stronger than validating what it names — so none of these appear in
//! `commands::path_ratchet`'s GUARDED/EXEMPT lists, and adding a path-shaped
//! parameter to any of them would fail that ratchet until it is classified.
//!
//! Everything here is featureless. Telemetry that only exists in some builds
//! would make the privacy text a lie in the others.

use tauri::State;

use sundayrec_core::telemetry::consent::TelemetryConsent;

use crate::db::Db;
use crate::error::AppResult;
use crate::telemetry;

/// The current consent state — status, scope version, whether to prompt, and
/// whether telemetry is active. The UI's single source of truth.
#[tauri::command]
pub async fn telemetry_consent_get(db: State<'_, Db>) -> AppResult<TelemetryConsent> {
    telemetry::consent_get(&db.pool).await
}

/// Record the user's answer, returning the resulting state.
///
/// `false` is destructive by design: it purges the outbox and the accumulated
/// counters, so "off" leaves nothing behind that a later "on" could flush.
#[tauri::command]
pub async fn telemetry_consent_set(
    db: State<'_, Db>,
    granted: bool,
) -> AppResult<TelemetryConsent> {
    telemetry::consent_set(&db.pool, granted).await
}

/// "Delete my data" — the local half.
///
/// Retires the current install id and mints a new, unrelated one, so every
/// future report belongs to a different install. The retired id is parked for
/// the remote DELETE a later phase sends. Consent is NOT withdrawn: asking for
/// the existing data to be removed is a different request from asking to stop
/// contributing, and conflating them would make one of the two impossible.
///
/// Returns nothing: the new id is an internal detail, and handing it to the
/// renderer would make it available to anything running there.
#[tauri::command]
pub async fn telemetry_regenerate_install_id(db: State<'_, Db>) -> AppResult<()> {
    telemetry::regenerate_install_id(&db.pool).await.map(|_| ())
}
