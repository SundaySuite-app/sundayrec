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
//! ## Trust boundary
//!
//! `telemetry_count` is the one command that takes a value from the renderer,
//! and it takes a NAME. Names are validated against
//! [`CounterName::from_wire`](sundayrec_core::telemetry::CounterName::from_wire)
//! — the same closed list the wire contract uses — so a compromised webview
//! cannot invent `"export.Gudstjeneste 6. april"` and turn a numeric payload
//! into a text one. An unknown name is an ERROR rather than a silent no-op: a
//! typo in a TS seam should be loud in development, not a counter that quietly
//! never moves.
//!
//! Everything here is featureless. Telemetry that only exists in some builds
//! would make the privacy text a lie in the others.

use tauri::State;

use sundayrec_core::telemetry::consent::TelemetryConsent;
use sundayrec_core::telemetry::queue::TelemetryQueueStatus;
use sundayrec_core::telemetry::{CounterName, TelemetryPreview};

use crate::db::Db;
use crate::error::{AppError, AppResult};
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
#[tauri::command]
pub async fn telemetry_regenerate_install_id(db: State<'_, Db>) -> AppResult<()> {
    telemetry::regenerate_install_id(&db.pool).await.map(|_| ())
}

/// Increment a named feature counter from a renderer-side seam.
///
/// The name must be one of the closed list; anything else is rejected. Counting
/// is a no-op when consent is not active — nothing is accumulated even in
/// memory, so turning telemetry on later cannot retroactively reveal what was
/// done while it was off.
#[tauri::command]
pub fn telemetry_count(name: String) -> AppResult<()> {
    let counter = CounterName::from_wire(&name).ok_or_else(|| {
        AppError::Validation(format!(
            "ukjent telemetri-teller «{name}» — navnet må stå i den faste lista"
        ))
    })?;
    telemetry::counters::count(counter);
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

/// What is waiting in the outbox: how many, how old the oldest is, and what
/// went wrong last.
///
/// Exists so the settings panel can be specific instead of reassuring. "3
/// rapporter venter, eldste fra i går, siste feil: no route to host" is a
/// sentence a user can act on; a green tick that means nothing is not.
#[tauri::command]
pub async fn telemetry_queue_status(db: State<'_, Db>) -> AppResult<TelemetryQueueStatus> {
    telemetry::queue_status(&db.pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::telemetry::ALL_COUNTERS;

    #[test]
    fn the_count_command_accepts_only_the_closed_list() {
        // The counter map is process-global; take the same lock the telemetry
        // tests use so 24 increments here cannot land in another test's
        // assertions. (It also leaves counting inactive, which is fine — what is
        // under test is the VALIDATION, not the increment.)
        let _g = telemetry::counters::test_lock();
        // Every name a TS seam may legitimately send.
        for &c in ALL_COUNTERS {
            assert!(
                telemetry_count(c.as_wire().to_string()).is_ok(),
                "{} must be callable from the renderer",
                c.as_wire()
            );
        }
    }

    #[test]
    fn the_count_command_rejects_anything_else() {
        let _g = telemetry::counters::test_lock();
        // The small injection guard: a compromised webview must not be able to
        // put a sermon title, a path, or an e-mail into a numeric payload by
        // dressing it up as a counter name.
        for bad in [
            "",
            " ",
            "editor.opened ",
            "EDITOR.OPENED",
            "editor.export.Gudstjeneste 6. april",
            "../../etc/passwd",
            "ola@menighet.no",
            "recording.started",
            "{\"$ne\":null}",
            &"a".repeat(10_000),
        ] {
            let err = telemetry_count(bad.to_string()).expect_err(bad);
            assert!(
                matches!(err, AppError::Validation(_)),
                "{bad:?} must be rejected as validation, got {err:?}"
            );
        }
    }
}
