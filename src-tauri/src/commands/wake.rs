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
use sundayrec_core::wake::{
    detect_capabilities, wake_idle_reason, wake_points, WakeCapabilities, WAKE_LEAD_MINUTES,
};

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
///
/// The result is the whole point of the command: it is the ONE wake path that
/// may prompt for elevation, so `ok`/`reason`/`message` is how the volunteer
/// learns whether the machine will actually wake up — and [`WakeIdleReason`] is
/// how they learn why an `ok` answer armed nothing.
#[tauri::command]
pub async fn wake_reschedule(
    engine: State<'_, WakeEngine>,
    db: State<'_, Db>,
) -> AppResult<WakeResult> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let upcoming = upcoming_for_wake(&s, now);
    let mut res = engine
        .reschedule(&upcoming, now, s.wake_from_sleep, true)
        .await;
    // "Armed 0 wakes" is not an answer to a button press. Say which nothing it
    // is — the level-1 switch, or an empty horizon.
    if res.ok && res.count.unwrap_or(0) == 0 {
        res.idle_reason = wake_idle_reason(s.auto_record_enabled, upcoming.len());
    }
    Ok(res)
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

/// The recording starts inside the wake horizon — **through
/// [`Settings::active_slots`], never `settings.slots`**.
///
/// ⚠️ Both wake commands read the schedule, and both used the raw list. That is
/// the level-1 switch («Ta opp automatisk») honoured everywhere except the two
/// places that decide when the MACHINE gets out of bed: with the switch off,
/// `wake_reschedule` armed a 10:50 wake for a 11:00 recording the scheduler
/// would then refuse to make, and `wake_verify` reported the cancelled wakes as
/// missing and told the volunteer their machine was misconfigured. One helper,
/// so the two cannot drift apart again — and so the rule has one test rather
/// than two hopes.
fn upcoming_for_wake(
    s: &sundayrec_core::settings::Settings,
    now: chrono::NaiveDateTime,
) -> Vec<chrono::NaiveDateTime> {
    upcoming_dates(
        s.active_slots(),
        &s.special_recordings,
        now,
        WAKE_HORIZON_DAYS,
    )
}

/// The wake points we expect the OS to have scheduled, derived from the current
/// schedule (upcoming starts minus the lead).
async fn expected_wakes(pool: &sqlx::SqlitePool) -> AppResult<Vec<chrono::NaiveDateTime>> {
    let s = settings::load(pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    Ok(wake_points(
        &upcoming_for_wake(&s, now),
        now,
        WAKE_LEAD_MINUTES,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::schedule::ScheduleSlot;
    use sundayrec_core::settings::Settings;
    use sundayrec_core::wake::WakeIdleReason;

    /// A profile with one weekly slot, Sunday 11:00.
    fn sunday_profile(auto_record_enabled: bool) -> Settings {
        Settings {
            auto_record_enabled,
            slots: vec![ScheduleSlot {
                // 6 = Sunday (0 = Monday).
                days: vec![6],
                start: "11:00".to_string(),
                stop: "12:30".to_string(),
                max: None,
            }],
            ..Default::default()
        }
    }

    /// A Wednesday, so the next Sunday slot is comfortably inside the horizon.
    fn now() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 8, 19)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
    }

    #[test]
    fn wake_reschedule_arms_nothing_when_auto_record_is_off() {
        // The `wake_reschedule` site. Raw `settings.slots` here woke the machine
        // at 10:50 on a Sunday for a recording the scheduler refuses to make.
        assert!(
            upcoming_for_wake(&sunday_profile(false), now()).is_empty(),
            "a disarmed weekly plan must not wake the machine"
        );
        // …and the switch is the only thing that changed.
        assert_eq!(
            upcoming_for_wake(&sunday_profile(true), now()).len(),
            1,
            "armed, the same profile plans the coming Sunday"
        );
    }

    #[test]
    fn wake_verify_expects_nothing_when_auto_record_is_off() {
        // The `expected_wakes` site (the `wake_verify` command). Raw
        // `settings.slots` here made verification report the wakes it had itself
        // cancelled as MISSING — an honest OS blamed for the app's own
        // bookkeeping.
        let expected = wake_points(
            &upcoming_for_wake(&sunday_profile(false), now()),
            now(),
            WAKE_LEAD_MINUTES,
        );
        assert!(expected.is_empty(), "nothing planned, nothing expected");
        assert!(!wake_points(
            &upcoming_for_wake(&sunday_profile(true), now()),
            now(),
            WAKE_LEAD_MINUTES
        )
        .is_empty());
    }

    #[test]
    fn a_dated_special_still_wakes_a_disarmed_profile() {
        // `active_slots` gates the WEEKLY plan only: a concert somebody entered
        // by hand is not covered by «Ta opp automatisk», so the machine must
        // still get out of bed for it — and the button must not claim the plan
        // is switched off when it just armed one.
        let mut s = sunday_profile(false);
        s.special_recordings = vec![sundayrec_core::schedule::SpecialRecording {
            id: None,
            date: "2026-08-21".to_string(),
            name: "Konsert".to_string(),
            start: "19:00".to_string(),
            stop: "20:30".to_string(),
            device_id: None,
        }];
        let upcoming = upcoming_for_wake(&s, now());
        assert_eq!(upcoming.len(), 1);
        assert_eq!(
            wake_idle_reason(s.auto_record_enabled, upcoming.len()),
            None
        );
    }

    #[test]
    fn an_empty_result_names_the_switch_that_emptied_it() {
        // What `wake_reschedule` hands the UI when it armed nothing.
        assert_eq!(
            wake_idle_reason(
                false,
                upcoming_for_wake(&sunday_profile(false), now()).len()
            ),
            Some(WakeIdleReason::AutoRecordOff)
        );
        let empty = Settings::default();
        assert_eq!(
            wake_idle_reason(
                empty.auto_record_enabled,
                upcoming_for_wake(&empty, now()).len()
            ),
            Some(WakeIdleReason::NothingUpcoming),
            "armed but nothing scheduled is a different nothing"
        );
    }
}
