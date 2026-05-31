//! Wake-from-sleep commands (Fase 5.2) — thin IPC over [`crate::wake`].
//!
//! The renderer:
//!   - reads `wake_capabilities` to show what this machine can/can't do,
//!   - reads `wake_get_sleep_config` to surface sleep settings that sabotage
//!     wake (+ a "Fiks automatisk" button → `wake_fix_sleep`),
//!   - calls `wake_verify` to compare the OS-scheduled wakes against what we
//!     expect from the current schedule,
//!   - calls `wake_reschedule` (user-initiated, may prompt for admin) to (re)
//!     register the OS wake timers now.

use chrono::Local;
use tauri::State;

use sundayrec_core::schedule::upcoming_dates;
use sundayrec_core::wake::{detect_capabilities, wake_points, WakeCapabilities, WAKE_LEAD_MINUTES};

use sundayrec_core::wake::WakeFailureEntry;

use crate::db::store;
use crate::db::Db;
use crate::error::AppResult;
use crate::settings;
use crate::wake::{
    cancel_test_wake, current_platform, fix_mac_sleep, fix_win_wake_timers, get_sleep_config,
    schedule_test_wake, verify_scheduled_wakes, TestWakeResult, WakeEngine, WakeFixResult,
    WakeResult, WakeStatus,
};

/// How many days of upcoming starts wake scheduling/verification considers.
const WAKE_HORIZON_DAYS: i64 = 14;

/// What this host can do re: wake-from-sleep (capabilities + Norwegian guidance).
#[tauri::command]
pub fn wake_capabilities() -> WakeCapabilities {
    detect_capabilities(current_platform())
}

/// The OS sleep/power configuration (mac standby/autopoweroff, win wake-timers).
#[tauri::command]
pub async fn wake_get_sleep_config() -> sundayrec_core::wake::SleepConfig {
    get_sleep_config().await
}

/// Apply the platform's sleep fix (mac: disable autopoweroff + raise standbydelay;
/// win: enable wake timers). Prompts for admin. No-op result on unsupported OS.
#[tauri::command]
pub async fn wake_fix_sleep() -> WakeFixResult {
    use sundayrec_core::wake::WakePlatform;
    match current_platform() {
        WakePlatform::MacArm | WakePlatform::MacIntel => fix_mac_sleep().await,
        WakePlatform::Win => fix_win_wake_timers().await,
        _ => WakeFixResult {
            ok: false,
            message: Some("unsupported".to_string()),
        },
    }
}

/// Compare the OS-scheduled wakes against what the current schedule expects.
#[tauri::command]
pub async fn wake_verify(db: State<'_, Db>) -> AppResult<WakeStatus> {
    let expected = expected_wakes(&db.pool).await?;
    Ok(verify_scheduled_wakes(&expected).await)
}

/// (Re)register OS wake timers for the upcoming schedule now. User-initiated, so
/// `allow_admin = true` — a Mac may show one admin prompt.
#[tauri::command]
pub async fn wake_reschedule(
    engine: State<'_, WakeEngine>,
    db: State<'_, Db>,
) -> AppResult<WakeResult> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let upcoming = upcoming_dates(&s.slots, &s.special_recordings, now, WAKE_HORIZON_DAYS);
    Ok(engine
        .reschedule(&upcoming, now, s.wake_from_sleep, true)
        .await)
}

/// Schedule a manual test-wake `seconds_ahead` from now (default 60 s). Returns
/// a job id the UI can cancel. HARDWARE-UNVERIFIED — the wake itself needs the
/// machine to sleep then wake; only the scheduling spawn is wired here.
#[tauri::command]
pub async fn wake_test(seconds_ahead: Option<i64>) -> TestWakeResult {
    schedule_test_wake(seconds_ahead.unwrap_or(60)).await
}

/// Cancel a pending test-wake (best-effort). Returns whether the cancel ran.
#[tauri::command]
pub async fn wake_cancel_test() -> bool {
    cancel_test_wake().await
}

/// The wake-failure / test-wake history, newest-first (capped at 20). DB-backed.
#[tauri::command]
pub async fn wake_failure_history(db: State<'_, Db>) -> AppResult<Vec<WakeFailureEntry>> {
    store::list_wake_failures(&db.pool).await
}

/// Clear the wake-failure history. Returns `true` once cleared.
#[tauri::command]
pub async fn wake_clear_failure_history(db: State<'_, Db>) -> AppResult<bool> {
    store::clear_wake_failures(&db.pool).await?;
    Ok(true)
}

/// The wake points we expect the OS to have scheduled, derived from the current
/// schedule (upcoming starts minus the lead).
async fn expected_wakes(pool: &sqlx::SqlitePool) -> AppResult<Vec<chrono::NaiveDateTime>> {
    let s = settings::load(pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let upcoming = upcoming_dates(&s.slots, &s.special_recordings, now, WAKE_HORIZON_DAYS);
    Ok(wake_points(&upcoming, now, WAKE_LEAD_MINUTES))
}
