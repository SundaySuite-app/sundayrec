//! Windows wake mechanism: `SetWaitableTimer(fResume = TRUE)`.
//!
//! ## Why this replaced the scheduled tasks
//!
//! The old path shelled out to PowerShell to `Register-ScheduledTask` one
//! `-WakeToRun` task per wake point, then read them back by scraping
//! `powercfg -waketimers` text. That mechanism has three costs we no longer pay:
//! registering a task in `\SundayRec\` is a machine-wide, persistent side effect
//! the app had to remember to clean up; a `-RunLevel Highest` task wants
//! elevation, so the code carried an elevated→unelevated retry ladder and a
//! UAC-prompt story; and every attempt spawned a PowerShell, which on a cold
//! church PC is seconds of latency for something that should be a syscall.
//!
//! `CreateWaitableTimerW` + `SetWaitableTimer` with `fResume = TRUE` arms a timer
//! that resumes the machine from S3/S4 when it expires. It needs **no elevation**,
//! leaves **no persistent state**, and is a handful of microseconds.
//!
//! ## What it honestly cannot do — and why that is acceptable here
//!
//! A waitable timer is owned by the process that armed it. **If SundayRec exits,
//! the timer dies with it**, and the machine will not wake. That is a real
//! reduction versus a scheduled task, and it is the owner-approved trade: the app
//! autostarts at login and lives in the tray, so "SundayRec is running" is the
//! premise of unattended recording anyway — a machine that woke up with no
//! SundayRec running would not have recorded regardless.
//!
//! Neither mechanism can wake from S5 (full shutdown); that needs a BIOS RTC
//! setting no software can reach. And an armed timer still obeys the
//! "Allow wake timers" power setting — arming succeeds while the wake silently
//! does not happen, which is why the sleep-config probe surfaces that setting.
//!
//! ## Testability
//!
//! The decision — which points become timers and what due-time each gets — is
//! [`plan_wake_timers`], pure and tested off-Windows. The arming itself is behind
//! [`WaitableTimers`] so the ladder above can be tested with a fake; the real
//! implementation is the thin unavoidable FFI edge.

use chrono::NaiveDateTime;

use sundayrec_core::wake::format_win_datetime;

/// Don't arm a timer for something less than a second away — the wake would land
/// after the moment it was for, and `SetWaitableTimer` with a due time already in
/// the past signals immediately.
pub const MIN_LEAD_MS: i64 = 1_000;

/// One timer we intend to arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinTimerPlan {
    /// The `SetWaitableTimer` due time, in units of 100 ns. **Negative = relative
    /// to now**, which is what we always use: an absolute due time is a UTC
    /// FILETIME, and going through the UTC conversion for a wall-clock schedule
    /// is one more place a DST boundary can put the wake an hour off.
    pub due_time_100ns: i64,
    /// The wall clock this timer is for — kept so the caller can report the next
    /// ARMED wake rather than the first requested point (they differ whenever a
    /// point was dropped for being past).
    pub at: NaiveDateTime,
}

impl WinTimerPlan {
    /// Human-readable wall clock, for logs and diagnostics.
    pub fn label(&self) -> String {
        format_win_datetime(self.at)
    }
}

/// Turn wake points into timer plans, dropping anything already past or too
/// imminent to be worth arming.
pub fn plan_wake_timers(points: &[NaiveDateTime], now: NaiveDateTime) -> Vec<WinTimerPlan> {
    points
        .iter()
        .filter_map(|d| {
            let ms = (*d - now).num_milliseconds();
            if ms < MIN_LEAD_MS {
                return None;
            }
            Some(WinTimerPlan {
                // 1 ms = 10 000 × 100 ns; negative for a relative due time.
                due_time_100ns: -(ms * 10_000),
                at: *d,
            })
        })
        .collect()
}

/// Arm and cancel the process-owned wake timers.
///
/// Implemented for real only on Windows ([`Win32WaitableTimers`]); every other
/// target gets [`UnsupportedTimers`], which is never reached because the platform
/// branch in [`super`] guards it — it exists so this module compiles everywhere
/// and the ladder above stays testable on a Mac.
pub trait WaitableTimers: Send + Sync {
    /// Cancel and close every timer this handle armed. Idempotent; must be safe
    /// to call before anything was ever armed.
    fn clear(&self);

    /// Arm one timer per plan. Returns how many were armed; on `Err` the caller
    /// should assume none of them will fire.
    fn arm(&self, plans: &[WinTimerPlan]) -> Result<u32, String>;
}

/// The stand-in on platforms with no waitable timers.
#[derive(Debug, Default)]
pub struct UnsupportedTimers;

impl WaitableTimers for UnsupportedTimers {
    fn clear(&self) {}
    fn arm(&self, _plans: &[WinTimerPlan]) -> Result<u32, String> {
        Err("waitable timers are a Windows mechanism".to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The Win32 edge — ⚠️ HARDWARE-UNVERIFIED
// ─────────────────────────────────────────────────────────────────────────────

/// Real `SetWaitableTimer` arming. Holds every live handle so the timers outlive
/// the call (a closed handle is a cancelled timer) and can be cancelled on the
/// next reschedule.
///
/// ⚠️ HARDWARE-UNVERIFIED — compiled by the `windows-check` CI lane, but whether
/// the machine actually resumes can only be proven by sleeping a real Windows box
/// (docs/SMOKE-TEST.md §11).
#[cfg(windows)]
#[derive(Default)]
pub struct Win32WaitableTimers {
    /// `HANDLE` values, kept as `isize` so the field stays `Send + Sync` without
    /// a wrapper — a Win32 `HANDLE` is a process-wide token, not a thread one.
    handles: std::sync::Mutex<Vec<isize>>,
}

#[cfg(windows)]
impl WaitableTimers for Win32WaitableTimers {
    fn clear(&self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::CancelWaitableTimer;

        let mut handles = crate::util::lock_recover(&self.handles);
        for h in handles.drain(..) {
            // SAFETY: every value here came from a successful
            // `CreateWaitableTimerW` in `arm` and is closed exactly once, because
            // `drain` removes it from the list as we go.
            unsafe {
                CancelWaitableTimer(h as _);
                CloseHandle(h as _);
            }
        }
    }

    fn arm(&self, plans: &[WinTimerPlan]) -> Result<u32, String> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{CreateWaitableTimerW, SetWaitableTimer};

        let mut handles = crate::util::lock_recover(&self.handles);
        let mut armed = 0u32;
        for plan in plans {
            // SAFETY: an unnamed, manual-reset timer with default security. A
            // null return is the documented failure signal and is checked.
            let handle = unsafe { CreateWaitableTimerW(std::ptr::null(), 1, std::ptr::null()) };
            if handle.is_null() {
                let code = unsafe { GetLastError() };
                return Err(format!("CreateWaitableTimer failed (error {code})"));
            }
            let due = plan.due_time_100ns;
            // SAFETY: `handle` is a live timer; `due` outlives the call; no
            // completion routine, so both APC arguments are null. The final `1`
            // is `fResume` — the entire point of this module.
            let ok = unsafe {
                SetWaitableTimer(handle, &due as *const i64, 0, None, std::ptr::null_mut(), 1)
            };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                // SAFETY: closing the handle we just created and are abandoning.
                unsafe { CloseHandle(handle) };
                return Err(format!(
                    "SetWaitableTimer failed for {} (error {code})",
                    plan.label()
                ));
            }
            handles.push(handle as isize);
            armed += 1;
        }
        Ok(armed)
    }
}

#[cfg(windows)]
impl Drop for Win32WaitableTimers {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The real backend for this build. Shared (`Arc`) because the timers are
/// process-owned state: the engine's reschedule and the manual test-wake MUST
/// arm and clear the SAME set, or a test-wake would leave the real Sunday timers
/// behind while believing it had cleared them.
pub fn real_timers() -> std::sync::Arc<dyn WaitableTimers> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(Win32WaitableTimers::default())
    }
    #[cfg(not(windows))]
    {
        std::sync::Arc::new(UnsupportedTimers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dtm(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn plan_wake_timers_uses_negative_relative_hundred_nanoseconds() {
        let now = dtm("2026-05-31 10:00");
        let plans = plan_wake_timers(&[dtm("2026-05-31 10:10")], now);
        assert_eq!(plans.len(), 1);
        // 10 minutes = 600 000 ms = 6 000 000 000 units of 100 ns, NEGATIVE
        // because a positive due time would be read as an absolute UTC FILETIME
        // and land the wake ~56 years in the past.
        assert_eq!(plans[0].due_time_100ns, -6_000_000_000);
        assert!(plans[0].due_time_100ns < 0);
    }

    #[test]
    fn plan_wake_timers_drops_past_and_imminent_points() {
        let now = dtm("2026-05-31 10:00");
        // Already gone.
        assert!(plan_wake_timers(&[dtm("2026-05-31 09:59")], now).is_empty());
        // Exactly now — a zero due time would signal instantly, never waking
        // anything, and would count as a "scheduled" wake in the result.
        assert!(plan_wake_timers(&[dtm("2026-05-31 10:00")], now).is_empty());
        // A fortnight out still plans (and does not overflow i64).
        let far = plan_wake_timers(&[dtm("2026-06-14 10:00")], now);
        assert_eq!(far.len(), 1);
        assert_eq!(far[0].due_time_100ns, -(14 * 24 * 60 * 60 * 1000 * 10_000));
    }

    #[test]
    fn plan_wake_timers_labels_each_timer_with_its_wall_clock() {
        let now = dtm("2026-05-31 10:00");
        let plans = plan_wake_timers(&[dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")], now);
        assert_eq!(
            plans.iter().map(|p| p.label()).collect::<Vec<_>>(),
            vec!["2026-05-31T10:20:00", "2026-06-07T10:20:00"]
        );
        assert_eq!(plans[0].at, dtm("2026-05-31 10:20"));
    }
}
