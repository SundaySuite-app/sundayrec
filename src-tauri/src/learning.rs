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

use sundayrec_core::feedback::TrimOutcome;
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
