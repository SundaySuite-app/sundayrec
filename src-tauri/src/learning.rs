//! Where a human corrected the detector — the persistence seam (E8).
//!
//! The decisions live in [`sundayrec_core::trim_feedback`] (what the deltas mean
//! and which ones are real) and [`sundayrec_core::feedback`] (what a record
//! holds and what a later one replaces). This file is the I/O edge, and its
//! whole job is to make sure a failure here never becomes a failure there.
//!
//! # Privacy — binding, and the reason this file exists at all
//!
//! `recording_path` is here to LOCATE the sidecar and for no other purpose. It
//! must never be written INTO the record, and neither must the recording's name,
//! its wall-clock time, or any transcript or sermon text. A duration is
//! anonymous; a duration next to a time of day identifies one congregation's one
//! service. The record is offsets within a recording, meaningless without the
//! recording — which is precisely why it can sit beside it safely.
//!
//! The log lines below obey the same rule: the deltas are durations, so they are
//! safe to log; the path is not logged.
//!
//! # The second half: acting on them (E10)
//!
//! Everything below the `record_trim_deltas` block is the persistence seam for
//! [`sundayrec_core::local_adaptivity`] — reading the corrections back, storing
//! what this install has learned from them, and being the one place that can put
//! it back. The decisions (what may be adjusted, by how much, on how much
//! evidence, and why it is off unless asked for) are all in that module.

use sundayrec_core::feedback::TrimOutcome;
use sundayrec_core::local_adaptivity::{self, DetectorTuning, LocalNudge};
use sundayrec_core::trim_feedback::TrimDeltas;

/// Persist how far the operator moved the proposed sermon trim, into the
/// recording's `<stem>.feedback.json`.
///
/// Best-effort and infallible BY SIGNATURE, which is the point of it. This runs
/// while a service is being published, so a storage failure must never become a
/// dialog in front of a volunteer: the deltas are training data for a later
/// tuning stage, and losing one recording's worth costs nothing the operator
/// could act on even if they were told. Failures belong in the log — etappe 2's
/// file layer catches these, so a support case can still answer "did it write".
///
/// ## Why the UNCHANGED case must reach this function
///
/// It would be cheaper for the caller to drop deltas that read as unchanged, and
/// that is exactly what it used to do. But "the operator left the boundaries
/// alone" and "the operator dragged a boundary back onto the proposal" arrive
/// here identically, and the second one has to WITHDRAW the adjustment recorded
/// earlier — a record that still claims a correction the person has since taken
/// back is not weak evidence, it is false evidence. Only
/// [`sundayrec_core::feedback::record_trim_adjustment`] can tell the two apart,
/// because only it can see what is already on file. Neither case is ever stored.
pub fn record_trim_deltas(recording_path: &str, deltas: TrimDeltas) {
    match crate::editor::record_trim_adjustment(recording_path, deltas) {
        Some(TrimOutcome::Recorded) => tracing::info!(
            start_delta_sec = deltas.start_delta_sec,
            end_delta_sec = deltas.end_delta_sec,
            "review: recorded how far the operator moved the proposed sermon trim"
        ),
        Some(TrimOutcome::Withdrawn) => tracing::info!(
            "review: the operator moved the trim back onto the proposal — earlier \
             adjustment withdrawn"
        ),
        Some(TrimOutcome::NotAnAdjustment) => tracing::debug!(
            "review: the proposed sermon trim was published untouched — nothing to learn"
        ),
        None => tracing::warn!(
            start_delta_sec = deltas.start_delta_sec,
            end_delta_sec = deltas.end_delta_sec,
            "review: could not persist the trim adjustment — the feedback sidecar \
             was left as it was"
        ),
    }
}

// ── Acting on them: the per-install nudge (E10) ────────────────────────────

/// The `app_setting` key this install's learned nudge is stored under.
///
/// Its own row, deliberately NOT a field on the settings blob. The blob is
/// preferences — things the operator chose, and things a settings EXPORT is
/// meant to carry to another machine. This is derived state about one
/// congregation's own corrections, and carrying it to another machine would hand
/// a second church a nudge earned from a first church's Sundays.
const NUDGE_KEY: &str = "localAdaptivityNudge";

/// The detector tuning to build the next episode prep with.
pub async fn current_tuning(db: &crate::db::Db) -> DetectorTuning {
    local_adaptivity::SHIPPED_TUNING.with_local_nudge(&refresh_nudge(db).await)
}

/// Fold every correction on file into the stored nudge, and persist the result.
///
/// Infallible by signature. A read failure here must not take a Sunday with it:
/// the shipped detector is what every install had until this etappe, so falling
/// back to it — or to the previous value — is a working recorder, not a degraded
/// one.
///
/// ## Why the CARD calls this too, and not a cheap cached read
///
/// There is an obvious cheaper design where the card reads the stored row and
/// only an analysis pass recomputes it. It is wrong in the one case that matters
/// most: an operator who has been correcting for months and flips the switch on
/// today. The stored row is still zero, so the card would say "nothing adjusted
/// yet, 0 corrections so far" — and then next Sunday the boundary moves forty
/// seconds. A card that is only correct between Sundays is the invisible state
/// this whole feature was built to avoid, so it pays the same history walk the
/// detector pays, and both see the same answer.
///
/// Does nothing at all — including the walk — when the operator has not turned
/// adaptivity on, so an install that never opts in never pays for the feature
/// and never accumulates a stored value to be surprised by if they later do.
/// The renderer additionally declines to call this while a recording is in
/// progress, on the same "cheap is not the same as safe mid-service" rule as
/// [`crate::commands::db::learning_feedback_summary`].
pub async fn refresh_nudge(db: &crate::db::Db) -> LocalNudge {
    if !enabled(db).await {
        return LocalNudge::SHIPPED;
    }
    let previous = stored_nudge(db).await;
    let Ok(rows) = crate::db::store::list_recordings(&db.pool).await else {
        tracing::warn!("local adaptivity: could not read the history — keeping the previous nudge");
        return previous;
    };
    let files: Vec<_> = rows
        .iter()
        .map(|r| crate::editor::read_feedback_for_summary(&r.file_path))
        .collect();
    let next = local_adaptivity::advance(previous, &files);
    if next != previous {
        tracing::info!(
            start_sec = next.sermon_start_sec.value,
            start_at_limit = next.sermon_start_sec.at_limit,
            end_sec = next.sermon_end_sec.value,
            end_at_limit = next.sermon_end_sec.at_limit,
            "local adaptivity: the proposed sermon boundaries moved"
        );
    }
    write_nudge(db, &next).await;
    next
}

/// Put this install back to the shipped detector: zero the nudge AND clear the
/// enable flag.
///
/// Both halves, because either alone is a reset that undoes itself. Clearing
/// only the value leaves adaptivity on, so the next analysed episode re-derives
/// the same nudge from the same corrections and the operator's click did
/// nothing they can see. Clearing only the flag leaves a value on disk that
/// silently returns the day they switch adaptivity back on.
pub async fn reset_nudge(db: &crate::db::Db) -> LocalNudge {
    let nudge = local_adaptivity::reset();
    write_nudge(db, &nudge).await;
    if let Ok(mut settings) = crate::settings::load(&db.pool).await {
        settings.local_adaptivity = false;
        let _ = crate::settings::save(&db.pool, settings).await;
    }
    tracing::info!("local adaptivity: reset to the shipped detector at the operator's request");
    nudge
}

async fn enabled(db: &crate::db::Db) -> bool {
    crate::settings::load(&db.pool)
        .await
        .map(|s| s.local_adaptivity)
        .unwrap_or(false)
}

/// The stored value, or the shipped one for anything unreadable. A blob written
/// by a newer build that this one cannot parse is treated as "nothing learned"
/// rather than as an error — same rule as [`crate::editor::read_feedback_for_summary`].
async fn stored_nudge(db: &crate::db::Db) -> LocalNudge {
    crate::db::store::get_setting(&db.pool, NUDGE_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<LocalNudge>(&raw).ok())
        .unwrap_or(LocalNudge::SHIPPED)
}

async fn write_nudge(db: &crate::db::Db, nudge: &LocalNudge) {
    let Ok(json) = serde_json::to_string(nudge) else {
        return;
    };
    if crate::db::store::set_setting(&db.pool, NUDGE_KEY, &json)
        .await
        .is_err()
    {
        tracing::warn!("local adaptivity: could not persist the nudge");
    }
}
