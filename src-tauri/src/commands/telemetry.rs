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

use sundayrec_core::telemetry::TelemetryPreview;
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
///
/// `true` also starts the sender, if this build has an endpoint and it is not
/// already running. Without that, a user who says yes mid-session would have
/// their reports collected, consented to, and queued until the next launch —
/// which is indistinguishable from the feature being broken. `maybe_spawn` is
/// idempotent, so toggling consent does not accumulate loops.
#[tauri::command]
pub async fn telemetry_consent_set(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    granted: bool,
) -> AppResult<TelemetryConsent> {
    let state = telemetry::consent_set(&db.pool, granted).await?;
    if granted {
        telemetry::sender::maybe_spawn(&app, &db.pool).await;
    }
    Ok(state)
}

/// "Delete my data" — the local half.
///
/// Retires the current install id and mints a new, unrelated one, so every
/// future report belongs to a different install. The retired id is parked in
/// `telemetry.pendingDeletions`, and the sender loop drains that list against
/// the endpoint's `DELETE /v1/install/:id` — see
/// [`telemetry::sender::maybe_spawn`]'s `drain_deletions`, including why the
/// deletion is deliberately NOT behind the consent gate that everything else is.
///
/// Consent is NOT withdrawn: asking for the existing data to be removed is a
/// different request from asking to stop contributing, and conflating them would
/// make one of the two impossible.
///
/// The remote half is best-effort by nature — a machine that is offline when the
/// user asks will send the DELETE when it next has a network. The local half
/// (this call) is immediate and unconditional: from this moment nothing the
/// machine sends is associated with the retired id.
///
/// Returns nothing: the new id is an internal detail, and handing it to the
/// renderer would make it available to anything running there.
///
/// The `maybe_spawn` at the end is what makes «så snart maskinen har nett»
/// true in THIS session rather than only after the next launch. With consent
/// off there is no sender loop running — that is the whole point of the consent
/// gate — so without this call the parked id would wait for a restart before
/// anything even looked at it. `maybe_spawn` is idempotent and now starts for an
/// owed deletion as well as for active consent (`sender::should_run`), so this
/// costs nothing when a loop is already running and starts one when the user has
/// asked for something that needs the network.
#[tauri::command]
pub async fn telemetry_regenerate_install_id(
    app: tauri::AppHandle,
    db: State<'_, Db>,
) -> AppResult<()> {
    telemetry::regenerate_install_id(&db.pool).await?;
    telemetry::sender::maybe_spawn(&app, &db.pool).await;
    Ok(())
}

/// "Vis hva som sendes" — the exact payload, as pretty JSON.
///
/// The transparency affordance the privacy text promises, and it must be the
/// REAL payload through the REAL builder: a mock here would be a promise about
/// code that never runs, which is worse than showing nothing. Read-only — it
/// mints no install id, advances no watermark and spends no counter, so the user
/// can open it as often as they like, before or after deciding.
///
/// See [`TelemetryPreview`] for why the answer differs with consent on and off,
/// and what the UI must say about each.
#[tauri::command]
pub async fn telemetry_preview_payload(
    app: tauri::AppHandle,
    db: State<'_, Db>,
) -> AppResult<TelemetryPreview> {
    telemetry::preview_payload(&app, &db.pool).await
}
