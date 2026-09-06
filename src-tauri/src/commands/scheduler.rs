//! Scheduler commands (Fase 5.1) — the thin IPC layer over [`crate::scheduler`].
//!
//! The renderer:
//!   - saves slots/specials through the normal `settings_save`, then calls
//!     `scheduler_reschedule` so the supervisor picks up the change immediately,
//!   - reads `scheduler_status` for the "next recording" + the next 14 days.
//!
//! Live updates arrive as `scheduler://{next,missed,preflight}` events.
//!
//! ## F2-T1: `scheduler_check_missed` is not a renderer call, and never was
//!
//! This doc used to claim the renderer "calls `scheduler_check_missed` on
//! launch/focus to late-start anything in progress and surface recordings
//! that never ran." It does not, and the reachability baseline has classified
//! the command unreachable since it was measured (nothing in `app/`, `e2e/` or
//! the tray names it). The late-start safety net is real and does run on
//! launch/focus — [`crate::scheduler::check_missed`] is called directly by the
//! supervisor's own startup task, and again after a suspected system
//! sleep/clock jump (`scheduler/mod.rs`'s `supervisor` loop; found dark and
//! wired up in the 2026-08-04 night sweep, per that module's own comment).
//! The BACKEND drives it, not the renderer — the doc line above was simply
//! wrong about who calls what.
//!
//! `scheduler_check_missed` itself stays registered as a manual/diagnostic
//! door onto the same check (same shape as `review_process_reminders`:
//! docs/archive/COMMAND_AUDIT_2026-08.md §4.10/§4.8) — useful for reproducing
//! a missed-recording report on demand without waiting for the next launch or
//! wake, never required for the net itself to function. It is expected to
//! stay in the reachability baseline's `unreachable` list; that is the correct
//! classification for a door nothing currently opens, not a bug.

use tauri::{AppHandle, State};

use crate::db::Db;
use crate::error::AppResult;
use crate::scheduler::{
    check_missed, status, MissedRecordingInfo, ScheduleStatus, SchedulerEngine,
};

/// Wake the supervisor to recompute its timers (call after saving schedule
/// settings) and return the fresh status.
#[tauri::command]
pub async fn scheduler_reschedule(
    engine: State<'_, SchedulerEngine>,
    db: State<'_, Db>,
) -> AppResult<ScheduleStatus> {
    engine.reschedule();
    status(&db.pool).await
}

/// The next scheduled start + the next 14 days of starts.
#[tauri::command]
pub async fn scheduler_status(db: State<'_, Db>) -> AppResult<ScheduleStatus> {
    status(&db.pool).await
}

/// Late-start anything currently in its window and return scheduled recordings
/// that were missed. Also emits `scheduler://missed`.
///
/// Manual/diagnostic entry point — see the module doc (F2-T1). The supervisor
/// already runs this same [`check_missed`] on its own at startup and after a
/// suspected sleep; nothing needs to call this command for the net to work.
#[tauri::command]
pub async fn scheduler_check_missed(
    app: AppHandle,
    db: State<'_, Db>,
) -> AppResult<Vec<MissedRecordingInfo>> {
    check_missed(&app, &db.pool).await
}
