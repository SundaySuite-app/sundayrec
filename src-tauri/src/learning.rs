//! Where a human corrected the detector — the persistence seam (E8 P B).
//!
//! The decisions live in [`sundayrec_core::trim_feedback`]; this file is the I/O
//! edge, and right now it is deliberately a stub with a log line.
//!
//! # ⚠️ SEAM — NOT YET CONNECTED TO STORAGE
//!
//! Etappe 8 phase A adds a `Feedback` sidecar (`<stem>.feedback.json`, a new arm
//! of [`sundayrec_core::editor::Sidecar`]) carrying sermon-pick corrections, and
//! that branch OWNS the sidecar's creation — its arm, its record type, and its
//! read/write helpers. The trim deltas measured here belong in the same record,
//! but writing them requires a type this branch must not invent, because two
//! independently-invented versions of one on-disk format is worse than a delay.
//!
//! So [`record_trim_deltas`] computes and hands over exactly what the record
//! needs, and stops. **To connect it** (the whole job, once both branches are on
//! `main`):
//!
//! 1. read the existing `Sidecar::Feedback` for `recording_path` (absent = a
//!    fresh record),
//! 2. set its trim-delta fields from the [`TrimDeltas`] argument,
//! 3. write it back through phase A's sidecar writer.
//!
//! The call site and the arithmetic behind it are already live and unit-tested;
//! only the two lines that touch the disk are missing.
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
//! The log line below obeys the same rule: the deltas are durations, so they are
//! safe to log; the path is not logged.

use sundayrec_core::trim_feedback::TrimDeltas;

/// Persist how far the operator moved the proposed sermon trim.
///
/// Best-effort and infallible by design — see the module docs for what is still
/// missing. This runs when a service is being published, so a storage failure
/// must never become a dialog in front of a volunteer: the deltas are training
/// data for a later tuning stage, and losing one recording's worth costs
/// nothing that the operator can act on. Failures belong in the log.
pub fn record_trim_deltas(recording_path: &str, deltas: TrimDeltas) {
    // SEAM: phase A's sidecar write goes here — see the module docs.
    let _ = recording_path;
    tracing::info!(
        start_delta_sec = deltas.start_delta_sec,
        end_delta_sec = deltas.end_delta_sec,
        "review: operator adjusted the proposed sermon trim (E8 seam: not yet persisted)"
    );
}
