//! Scheduler commands (Fase 5.1) — the thin IPC layer over [`crate::scheduler`].
//!
//! The renderer:
//!   - saves the slots through the normal `settings_save`, then calls
//!     `scheduler_reschedule` so the supervisor picks up the change immediately,
//!   - reads `scheduler_status` for the "next recording" + the next 14 days.
//!
//! Live updates arrive as `scheduler://{next,missed,preflight}` events.
//!
//!
//! The late-start safety net ([`crate::scheduler::check_missed`]) is driven by
//! the supervisor itself at startup and after a suspected sleep; the renderer
//! never calls it.

use tauri::State;

use crate::db::Db;
use crate::error::AppResult;
use crate::scheduler::{ScheduleStatus, SchedulerEngine, status};

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
