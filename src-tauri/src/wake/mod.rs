//! Wake-from-sleep plumbing (Fase 5.2) — the impure OS shell over the pure
//! [`sundayrec_core::wake`] decision core.
//!
//! Ported from the Electron `src/main/wake.ts` + `wake-verification.ts`. The
//! *decisions* — which wake points to register, how to format a `pmset` time,
//! classifying errors, parsing the power tools' output, matching expected vs
//! observed wakes, platform capabilities — live in the core and carry the tests.
//! This module owns the OS side of it, split into four seams so the OS side is
//! testable too:
//!
//!   - [`plan`] — pure command PLANS (program + argv) for every invocation,
//!     including the two-layer quoting the elevated macOS path needs;
//!   - [`shell`] — the `Shell` trait that runs them, with a `FakeShell` the tests
//!     drive the escalation ladders through;
//!   - [`win_timer`] — the Windows mechanism, `SetWaitableTimer(fResume = TRUE)`;
//!   - [`mac_read`] — the unprivileged IOKit read of scheduled power events.
//!
//! ## Platform mechanisms, honestly
//!
//! **macOS.** Writing a scheduled wake means `IOPMSchedulePowerEvent`, which
//! requires root — `pmset` is itself just a shell over it. So there is no binding
//! that avoids the privilege: we run `pmset` unelevated first (it succeeds when
//! the user already holds the right) and escalate to ONE `osascript … with
//! administrator privileges` prompt only when that fails. Reading, by contrast,
//! is unprivileged, so verification goes through IOKit ([`mac_read`]) and falls
//! back to `pmset -g sched` text only when IOKit reports nothing.
//!
//! **Windows.** The wake is an in-process `SetWaitableTimer` with `fResume =
//! TRUE`. No elevation, no UAC ladder, no scheduled task left behind on the
//! machine — **and no wake at all if SundayRec is not running**. That is the
//! owner-approved model: the app autostarts and lives in the tray, and a machine
//! that woke with SundayRec closed would not have recorded anyway. Neither
//! mechanism can start a machine from S5 (full shutdown). See [`win_timer`].
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Argument shaping, the escalation ladders and the output parsing are unit-tested
//! here and in the core, and the macOS IOKit read runs for real in the gate. What
//! remains unproven without a real box: whether the admin prompt behaves, whether
//! `SetWaitableTimer` truly resumes a sleeping Windows machine, and whether the
//! Windows code even compiles here (it does not — that is the `windows-check` CI
//! lane's job; nothing on this Mac builds it).
//!
//! ## Honestly deferred
//!
//! The Electron `testWake` (schedule a near-future wake, *sleep the machine*, and
//! measure the resume via `powerMonitor`) is NOT ported: Tauri has no built-in
//! power-resume event, and sleeping the user's machine without a reliable resume
//! signal is worse than not offering it. The pure verdict
//! ([`sundayrec_core::wake::classify_test_wake_delta`]) is ready for when a
//! power-monitor capability lands.

pub mod mac_read;
pub mod plan;
pub mod shell;
pub mod win_timer;

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use sundayrec_core::wake::{
    classify_win_error, compare_expected_to_observed, decide_reschedule, key_of,
    parse_mac_sleep_config, parse_pmset_batt, parse_pmset_sched, parse_pmset_standby,
    parse_powercfg_waketimers, parse_win_wake_timers, parse_wmic_battery_status, wake_points,
    SleepConfig, VerifiedWake, WakeErrorReason, WakeIdleReason, WakePlatform, WakeRescheduleAction,
    WinErrorKind, WAKE_LEAD_MINUTES, WAKE_MATCH_TOLERANCE_MS,
};

use crate::util::lock_recover;
use plan::{
    plan_mac_batt, plan_mac_cancel_all, plan_mac_elevated_schedule, plan_mac_fix_sleep,
    plan_mac_sched, plan_mac_schedule_one, plan_mac_sleep_config, plan_win_battery_cim,
    plan_win_battery_wmic, plan_win_fix_wake_timers, plan_win_wake_timers_query,
    plan_win_waketimers, WAKE_OWNER,
};
use shell::{run_text, RealShell, Shell};
use win_timer::{plan_wake_timers, WaitableTimers};

/// The outcome of an OS wake-scheduling attempt. Mirrors the Electron `WakeResult`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeResult {
    pub ok: bool,
    pub count: Option<u32>,
    /// ISO-like local string of the first/next scheduled wake, or `None`.
    pub next_wake: Option<String>,
    /// Why it failed: `disabled | cancelled | permission | unsupported | error`.
    pub reason: Option<String>,
    pub message: Option<String>,
    /// Why a SUCCESSFUL reschedule armed nothing (`ok: true, count: 0`).
    ///
    /// The engine never sets this — it does not know about the level-1 switch,
    /// only about the points it was handed. `commands::wake::wake_reschedule`
    /// fills it in, because "nothing to arm" and "why nothing to arm" are the
    /// difference between a button that looks broken and one that explains
    /// itself. `#[serde(default)]` so an older stored/queued payload still
    /// deserialises.
    #[serde(default)]
    pub idle_reason: Option<WakeIdleReason>,
}

impl WakeResult {
    fn ok(count: u32, next_wake: Option<String>) -> Self {
        Self {
            ok: true,
            count: Some(count),
            next_wake,
            reason: None,
            message: None,
            idle_reason: None,
        }
    }
    fn fail(reason: WakeErrorReason, message: Option<String>) -> Self {
        Self {
            ok: false,
            count: None,
            next_wake: None,
            reason: Some(reason.as_str().to_string()),
            message,
            idle_reason: None,
        }
    }
}

/// One OS-observed wake, for the verification panel.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ObservedWake.ts")]
#[serde(rename_all = "camelCase")]
pub struct ObservedWake {
    pub scheduled_at: String,
    pub owner_label: String,
}

/// The verification snapshot: what we asked the OS to schedule vs what it
/// reports, plus power facts. Mirrors the Electron `WakeStatus` minus the
/// `capabilities` field — the UI reads those from the separate
/// `wake_capabilities` command, so this src-tauri type doesn't embed the
/// core-crate [`sundayrec_core::wake::WakeCapabilities`] (a cross-crate ts-rs
/// embed produces a broken relative import path; commands returning core types
/// separately are the codebase convention).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeStatus.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeStatus {
    pub expected_wakes: Vec<String>,
    pub observed_wakes: Vec<ObservedWake>,
    pub has_mismatch: bool,
    pub on_battery: Option<bool>,
    pub standby_enabled: Option<bool>,
}

/// Result of a "fix sleep settings" action.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeFixResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeFixResult {
    pub ok: bool,
    pub message: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
//   The process-wide OS handles
// ─────────────────────────────────────────────────────────────────────────────

/// The real shell. A unit struct, so one shared instance is free.
fn real_shell() -> Arc<dyn Shell> {
    static SHELL: LazyLock<Arc<dyn Shell>> = LazyLock::new(|| Arc::new(RealShell));
    SHELL.clone()
}

/// The process's wake timers. Deliberately ONE instance for the whole process:
/// a waitable timer belongs to whoever armed it, so the engine's reschedule and
/// the manual test-wake have to operate on the same set.
fn global_timers() -> Arc<dyn WaitableTimers> {
    static TIMERS: LazyLock<Arc<dyn WaitableTimers>> = LazyLock::new(win_timer::real_timers);
    TIMERS.clone()
}

// ─────────────────────────────────────────────────────────────────────────────
//   Engine (Tauri-managed state) — dedups repeated scheduling
// ─────────────────────────────────────────────────────────────────────────────

/// Managed-state handle. Holds the last successfully-scheduled wake-point key so
/// an unchanged reschedule (the common case — the supervisor recomputes often)
/// is a cheap no-op. Mirrors the Electron `lastScheduledByPlatform` dedup.
pub struct WakeEngine {
    last_key: Mutex<Option<String>>,
    shell: Arc<dyn Shell>,
    timers: Arc<dyn WaitableTimers>,
    platform: WakePlatform,
}

impl Default for WakeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeEngine {
    pub fn new() -> Self {
        Self {
            last_key: Mutex::new(None),
            shell: real_shell(),
            timers: global_timers(),
            platform: current_platform(),
        }
    }

    /// Build an engine over substituted OS handles — the seam the ladder tests
    /// drive.
    #[cfg(test)]
    fn with(
        shell: Arc<dyn Shell>,
        timers: Arc<dyn WaitableTimers>,
        platform: WakePlatform,
    ) -> Self {
        Self {
            last_key: Mutex::new(None),
            shell,
            timers,
            platform,
        }
    }

    /// Schedule OS wakes for the `upcoming` recording starts (the lead is
    /// subtracted here). De-dupes against the last call unless `allow_admin`
    /// (a user-initiated reschedule always runs). Returns `disabled` when the
    /// user has turned wake off. Port of `wake.ts` `reschedule` + `scheduleOsWakes`.
    pub async fn reschedule(
        &self,
        upcoming: &[NaiveDateTime],
        now: NaiveDateTime,
        wake_from_sleep: bool,
        allow_admin: bool,
    ) -> WakeResult {
        if !wake_from_sleep {
            return WakeResult::fail(WakeErrorReason::Disabled, None);
        }
        let points = wake_points(upcoming, now, WAKE_LEAD_MINUTES);
        let key = key_of(&points);

        // Dedup / stale-clear decision (pure, tested in core). We read the last
        // applied key, then decide; a poisoned lock is recovered rather than
        // panicking the supervisor thread that calls this.
        let last_key = lock_recover(&self.last_key).clone();
        if let WakeRescheduleAction::SkipUnchanged =
            decide_reschedule(last_key.as_deref(), &key, points.is_empty(), allow_admin)
        {
            return WakeResult::ok(points.len() as u32, points.first().map(fmt_local));
        }

        let result = schedule_os_wakes(
            self.shell.as_ref(),
            self.timers.as_ref(),
            self.platform,
            &points,
            now,
            allow_admin,
        )
        .await;
        if result.ok {
            // Record the applied set — INCLUDING the empty key after a clear, so a
            // later re-add of the same time is recognised as a real change and
            // re-registers (rather than dedup'ing against a now-cancelled timer).
            *lock_recover(&self.last_key) = Some(key);
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Scheduling
// ─────────────────────────────────────────────────────────────────────────────

async fn schedule_os_wakes(
    shell: &dyn Shell,
    timers: &dyn WaitableTimers,
    platform: WakePlatform,
    points: &[NaiveDateTime],
    now: NaiveDateTime,
    allow_admin: bool,
) -> WakeResult {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => {
            schedule_mac(shell, points, allow_admin).await
        }
        WakePlatform::Win => schedule_windows(timers, points, now),
        _ => WakeResult::fail(WakeErrorReason::Unsupported, None),
    }
}

/// macOS ladder: cancel ours, try each `pmset` unelevated, and only if some
/// failed escalate to ONE admin prompt covering the whole set.
async fn schedule_mac(
    shell: &dyn Shell,
    points: &[NaiveDateTime],
    allow_admin: bool,
) -> WakeResult {
    // Clear our previously-scheduled wakes (best-effort).
    let _ = run_text(shell, &plan_mac_cancel_all()).await;

    if points.is_empty() {
        return WakeResult::ok(0, None);
    }

    let mut scheduled = 0u32;
    for d in points {
        if run_text(shell, &plan_mac_schedule_one(*d)).await.is_ok() {
            scheduled += 1;
        }
    }
    if scheduled as usize == points.len() {
        return WakeResult::ok(scheduled, points.first().map(fmt_local));
    }

    if !allow_admin {
        // A background reschedule must never raise a modal password prompt on a
        // machine nobody is sitting at; the UI surfaces this as "click Planlegg".
        return WakeResult::fail(WakeErrorReason::Permission, None);
    }

    match run_text(shell, &plan_mac_elevated_schedule(points, WAKE_OWNER)).await {
        Ok(_) => WakeResult::ok(points.len() as u32, points.first().map(fmt_local)),
        Err(msg) => {
            if is_admin_prompt_cancel(&msg) {
                WakeResult::fail(WakeErrorReason::Cancelled, None)
            } else {
                WakeResult::fail(WakeErrorReason::Permission, Some(msg))
            }
        }
    }
}

/// True when an `osascript … with administrator privileges` failure is the user
/// dismissing the prompt rather than something being wrong.
///
/// osascript reports this as `User canceled.` with AppleScript error `-128`. Both
/// spellings and the numeric code are accepted: the wording is localisable, the
/// code is not, and telling "you clicked Avbryt" apart from "your password is
/// wrong" is the difference between a calm UI message and a scary one.
fn is_admin_prompt_cancel(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("user canceled") || lower.contains("user cancelled") || lower.contains("-128")
}

/// Windows: cancel whatever we armed, then arm one `SetWaitableTimer` per point.
/// Synchronous — there is no process to spawn any more.
fn schedule_windows(
    timers: &dyn WaitableTimers,
    points: &[NaiveDateTime],
    now: NaiveDateTime,
) -> WakeResult {
    // Always cancel first: the timers are ours, and a reschedule replaces the
    // whole set rather than adding to it.
    timers.clear();

    if points.is_empty() {
        return WakeResult::ok(0, None);
    }
    let plans = plan_wake_timers(points, now);
    if plans.is_empty() {
        // Every point was already past by the time we got here.
        return WakeResult::ok(0, None);
    }
    let next = plans.first().map(|p| fmt_local(&p.at));
    match timers.arm(&plans) {
        Ok(count) => WakeResult::ok(count, next),
        Err(msg) => WakeResult::fail(WakeErrorReason::Error, Some(msg)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Test-wake (manual diagnostic)
// ─────────────────────────────────────────────────────────────────────────────

/// The outcome of scheduling a manual test-wake. Mirrors the Electron
/// `testWake`'s return: on success a `jobId` the renderer can cancel, plus the
/// scheduled wall-clock time the resume handler will compare against.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TestWakeResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct TestWakeResult {
    pub ok: bool,
    /// Opaque id the renderer passes to `wake_cancel_test`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// ISO-like local string of the scheduled wake.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Schedule a single OS wake `seconds_ahead` from now and return a job id. Port
/// of the Electron `testWake(secondsAhead)` scheduling half. The resume
/// *listening* (which records a `test_ok`/`test_fail` outcome via the failure
/// history) is OS-level and GUI-driven — the pure verdict lives in
/// [`sundayrec_core::wake::test_wake_outcome`].
///
/// Note it REPLACES the currently-scheduled wakes on both platforms (macOS
/// `cancelall`, Windows clear-then-arm), exactly as the Electron original did.
/// The next supervisor tick re-registers the real schedule.
///
/// ⚠️ HARDWARE-UNVERIFIED — the actual wake can't be proven in the gate (the
/// machine has to sleep, then wake). See SMOKE-TEST.md.
pub async fn schedule_test_wake(seconds_ahead: i64) -> TestWakeResult {
    let secs = seconds_ahead.clamp(5, 3600);
    let now = chrono::Local::now().naive_local();
    let target = now + chrono::Duration::seconds(secs);
    let result = schedule_os_wakes(
        real_shell().as_ref(),
        global_timers().as_ref(),
        current_platform(),
        std::slice::from_ref(&target),
        now,
        true,
    )
    .await;
    if result.ok {
        TestWakeResult {
            ok: true,
            job_id: Some(format!("test-wake-{}", target.and_utc().timestamp_millis())),
            scheduled_at: Some(fmt_local(&target)),
            reason: None,
        }
    } else {
        TestWakeResult {
            ok: false,
            job_id: None,
            scheduled_at: None,
            reason: result.reason,
        }
    }
}

/// Cancel any pending SundayRec test-wake (best-effort). Mirrors the Electron
/// `cancelTestWake` — clears our scheduled wakes. ⚠️ HARDWARE-UNVERIFIED.
pub async fn cancel_test_wake() -> bool {
    match current_platform() {
        WakePlatform::MacArm | WakePlatform::MacIntel => {
            run_text(real_shell().as_ref(), &plan_mac_cancel_all())
                .await
                .is_ok()
        }
        WakePlatform::Win => {
            // Closing the handles IS the cancel; there is nothing to fail.
            global_timers().clear();
            true
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Sleep config + fixes
// ─────────────────────────────────────────────────────────────────────────────

/// Read the OS sleep/power configuration. Port of `getSleepConfig`.
pub async fn get_sleep_config() -> SleepConfig {
    read_sleep_config(real_shell().as_ref(), current_platform()).await
}

async fn read_sleep_config(shell: &dyn Shell, platform: WakePlatform) -> SleepConfig {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => {
            match run_text(shell, &plan_mac_sleep_config()).await {
                Ok(out) => parse_mac_sleep_config(&out),
                Err(e) => SleepConfig {
                    error: Some(e),
                    ..Default::default()
                },
            }
        }
        WakePlatform::Win => match run_text(shell, &plan_win_wake_timers_query()).await {
            Ok(out) => SleepConfig {
                wake_timers_enabled: parse_win_wake_timers(&out),
                ..Default::default()
            },
            Err(e) => SleepConfig {
                error: Some(e),
                ..Default::default()
            },
        },
        _ => SleepConfig::default(),
    }
}

/// Disable autopoweroff + raise standbydelay so a Mac stays in (wakeable) sleep.
/// Port of `fixMacSleep`. Requires an admin prompt.
pub async fn fix_mac_sleep() -> WakeFixResult {
    match run_text(real_shell().as_ref(), &plan_mac_fix_sleep()).await {
        Ok(_) => WakeFixResult {
            ok: true,
            message: None,
        },
        Err(msg) => WakeFixResult {
            ok: false,
            message: Some(if is_admin_prompt_cancel(&msg) {
                "cancelled".to_string()
            } else {
                msg
            }),
        },
    }
}

/// Enable wake timers (AC + DC) in the active power scheme. Port of `fixWinWakeTimers`.
///
/// This is the ONE Windows path that can still need elevation — the wake itself
/// no longer does — so the permission classification lives here and nowhere else.
pub async fn fix_win_wake_timers() -> WakeFixResult {
    match run_text(real_shell().as_ref(), &plan_win_fix_wake_timers()).await {
        Ok(_) => WakeFixResult {
            ok: true,
            message: None,
        },
        Err(msg) => WakeFixResult {
            ok: false,
            message: Some(match classify_win_error(&msg) {
                WinErrorKind::Permission => "admin_required".to_string(),
                WinErrorKind::Error => msg,
            }),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Verification
// ─────────────────────────────────────────────────────────────────────────────

/// How long a `verify_scheduled_wakes` answer stays valid for an UNCHANGED
/// `expected` set (R3, optional half).
///
/// The renderer already gates its OWN call sites down to four legitimate
/// reasons (`shouldRefreshWake` in `app/lib/status/next-recording-core.ts`),
/// each rare by construction — no more 60 s reserve-poll tick riding along
/// unconditionally, ~120 `pmset` spawns per two-hour service. This is the
/// floor under that on the backend side, for whatever else ends up calling
/// this crate: a second window, a rig probe, a retry loop.
///
/// Keyed on `expected` (via `key_of`, the same hash `WakeEngine::reschedule`
/// dedups on) and NOT blind: a genuine schedule change must never read back a
/// mismatch verdict computed against the OLD schedule. That would undo
/// exactly the freshness R3's four triggers exist to ask for — a settings
/// change is one of them precisely because the cache below would otherwise
/// serve a stale answer for up to five minutes after it.
const WAKE_VERIFY_CACHE_TTL: Duration = Duration::from_secs(300);

struct WakeVerifyCacheEntry {
    key: String,
    at: Instant,
    status: WakeStatus,
}

static WAKE_VERIFY_CACHE: LazyLock<Mutex<Option<WakeVerifyCacheEntry>>> =
    LazyLock::new(|| Mutex::new(None));

/// The cached answer for `key`, if one exists and is still inside the TTL.
fn cached_wake_status(key: &str) -> Option<WakeStatus> {
    let entry = lock_recover(&WAKE_VERIFY_CACHE);
    let entry = entry.as_ref()?;
    (entry.key == key && entry.at.elapsed() < WAKE_VERIFY_CACHE_TTL).then(|| entry.status.clone())
}

/// Compare what we expect the OS to have scheduled (from `expected`) against
/// what it actually reports, plus power facts. Port of `verifyScheduledWakes`.
///
/// `pmset -g batt` + `pmset -g sched`/`-g custom` (mac) are spawned fresh only
/// on a cache miss — see `WAKE_VERIFY_CACHE_TTL` above for why an unchanged
/// `expected` set answers from cache instead.
pub async fn verify_scheduled_wakes(expected: &[NaiveDateTime]) -> WakeStatus {
    let key = key_of(expected);
    if let Some(cached) = cached_wake_status(&key) {
        return cached;
    }
    let platform = current_platform();
    // The IOKit read happens here (it is synchronous and privilege-free) and is
    // handed in, so the fallback ladder below stays testable.
    let iokit = mac_read::read_scheduled_wakes(local_utc_offset_secs());
    let status = verify_with(real_shell().as_ref(), platform, iokit, expected).await;
    *lock_recover(&WAKE_VERIFY_CACHE) = Some(WakeVerifyCacheEntry {
        key,
        at: Instant::now(),
        status: status.clone(),
    });
    status
}

async fn verify_with(
    shell: &dyn Shell,
    platform: WakePlatform,
    iokit: Option<Vec<VerifiedWake>>,
    expected: &[NaiveDateTime],
) -> WakeStatus {
    let observed = observed_wakes(shell, platform, iokit).await;
    let on_battery = check_power_source(shell, platform).await;
    let standby_enabled = check_standby(shell, platform).await;
    let (has_mismatch, _missing) =
        compare_expected_to_observed(expected, &observed, WAKE_MATCH_TOLERANCE_MS);

    WakeStatus {
        expected_wakes: expected.iter().map(fmt_local).collect(),
        observed_wakes: observed
            .into_iter()
            .map(|o| ObservedWake {
                scheduled_at: fmt_local(&o.scheduled_at),
                owner_label: o.owner_label,
            })
            .collect(),
        has_mismatch,
        on_battery,
        standby_enabled,
    }
}

/// What the OS says it has scheduled.
///
/// On macOS the IOKit answer wins when it has anything to say; an EMPTY IOKit
/// answer still falls through to `pmset -g sched`, because the two sources are
/// known to disagree on Apple Silicon and reporting "nothing scheduled" would
/// make the UI tell the user to re-register a wake that already exists.
async fn observed_wakes(
    shell: &dyn Shell,
    platform: WakePlatform,
    iokit: Option<Vec<VerifiedWake>>,
) -> Vec<VerifiedWake> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => {
            if let Some(wakes) = iokit {
                if !wakes.is_empty() {
                    return wakes;
                }
            }
            match run_text(shell, &plan_mac_sched()).await {
                Ok(out) => parse_pmset_sched(&out, Some(Utc::now().year_ce().1 as i32)),
                Err(_) => Vec::new(),
            }
        }
        WakePlatform::Win => match run_text(shell, &plan_win_waketimers()).await {
            Ok(out) => parse_powercfg_waketimers(&out),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

async fn check_power_source(shell: &dyn Shell, platform: WakePlatform) -> Option<bool> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => run_text(shell, &plan_mac_batt())
            .await
            .ok()
            .and_then(|o| parse_pmset_batt(&o)),
        WakePlatform::Win => {
            if let Ok(o) = run_text(shell, &plan_win_battery_wmic()).await {
                return parse_wmic_battery_status(&o);
            }
            // Newer Windows may lack wmic — fall back to PowerShell CIM.
            run_text(shell, &plan_win_battery_cim())
                .await
                .ok()
                .and_then(|o| o.trim().parse::<i32>().ok().map(|s| s == 1))
        }
        _ => None,
    }
}

async fn check_standby(shell: &dyn Shell, platform: WakePlatform) -> Option<bool> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => run_text(shell, &plan_mac_sleep_config())
            .await
            .ok()
            .and_then(|o| parse_pmset_standby(&o)),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// The host class for wake purposes, from the running OS + arch.
pub fn current_platform() -> WakePlatform {
    match std::env::consts::OS {
        "macos" => {
            if std::env::consts::ARCH == "aarch64" {
                WakePlatform::MacArm
            } else {
                WakePlatform::MacIntel
            }
        }
        "windows" => WakePlatform::Win,
        "linux" => WakePlatform::Linux,
        _ => WakePlatform::Other,
    }
}

/// This machine's current UTC offset in seconds — the one impure input
/// [`mac_read::cf_absolute_to_local`] needs.
fn local_utc_offset_secs() -> i32 {
    use chrono::Offset;
    chrono::Local::now().offset().fix().local_minus_utc()
}

/// Format a wall-clock datetime as a zone-less local ISO string for the UI.
fn fmt_local(d: &NaiveDateTime) -> String {
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::shell::{CmdOutput, FakeShell};
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn dtm(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    /// A [`WaitableTimers`] that records what it was asked to do.
    #[derive(Default)]
    struct FakeTimers {
        clears: StdMutex<u32>,
        armed: StdMutex<Vec<String>>,
        fail_with: Option<String>,
    }

    impl FakeTimers {
        fn failing(msg: &str) -> Self {
            Self {
                fail_with: Some(msg.to_string()),
                ..Default::default()
            }
        }
        fn clears(&self) -> u32 {
            *lock_recover(&self.clears)
        }
        fn armed(&self) -> Vec<String> {
            lock_recover(&self.armed).clone()
        }
    }

    impl WaitableTimers for FakeTimers {
        fn clear(&self) {
            *lock_recover(&self.clears) += 1;
        }
        fn arm(&self, plans: &[win_timer::WinTimerPlan]) -> Result<u32, String> {
            if let Some(msg) = &self.fail_with {
                return Err(msg.clone());
            }
            let mut log = lock_recover(&self.armed);
            for p in plans {
                log.push(p.label());
            }
            Ok(plans.len() as u32)
        }
    }

    fn no_timers() -> Arc<dyn WaitableTimers> {
        Arc::new(FakeTimers::default())
    }

    // ── macOS ladder ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mac_cancels_first_then_schedules_each_point_unelevated() {
        let shell = Arc::new(FakeShell::new());
        let res = schedule_mac(
            shell.as_ref(),
            &[dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")],
            false,
        )
        .await;
        assert!(res.ok);
        assert_eq!(res.count, Some(2));
        assert_eq!(res.next_wake.as_deref(), Some("2026-05-31T10:20:00"));
        let log = shell.log();
        // Cancel-then-register, in that order — the reverse would delete the
        // wakes we had just filed.
        assert_eq!(log[0], "pmset schedule cancelall SundayRec");
        assert_eq!(log.len(), 3);
        assert!(log[1].contains("05/31/26 10:20:00"));
        assert!(log[2].contains("06/07/26 10:20:00"));
        // The happy path must NOT raise an admin prompt.
        assert_eq!(shell.count("osascript"), 0);
    }

    #[tokio::test]
    async fn mac_partial_failure_without_admin_reports_permission_and_stays_silent() {
        // One of two `pmset` calls fails and this is a BACKGROUND reschedule:
        // a modal password prompt on an unattended machine is worse than a
        // reported failure the UI can surface.
        let shell = Arc::new(
            FakeShell::new()
                .on("06/07/26", Ok(CmdOutput::failed(1, "not authorized")))
                .otherwise(Ok(CmdOutput::ok(""))),
        );
        let res = schedule_mac(
            shell.as_ref(),
            &[dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")],
            false,
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.reason.as_deref(), Some("permission"));
        assert_eq!(shell.count("osascript"), 0);
    }

    #[tokio::test]
    async fn mac_partial_failure_escalates_to_one_admin_prompt_for_the_whole_set() {
        let shell = Arc::new(
            FakeShell::new()
                .on("osascript", Ok(CmdOutput::ok("")))
                .on("schedule wake", Ok(CmdOutput::failed(1, "not authorized")))
                .otherwise(Ok(CmdOutput::ok(""))),
        );
        let points = [dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")];
        let res = schedule_mac(shell.as_ref(), &points, true).await;
        assert!(res.ok);
        assert_eq!(res.count, Some(2));
        // ONE prompt, not one per wake point — two password dialogs in a row is
        // how a volunteer decides the feature is broken.
        assert_eq!(shell.count("osascript"), 1);
        let script = shell
            .log()
            .into_iter()
            .find(|l| l.contains("osascript"))
            .unwrap();
        assert!(script.contains("05/31/26 10:20:00"));
        assert!(script.contains("06/07/26 10:20:00"));
    }

    #[tokio::test]
    async fn mac_admin_prompt_dismissal_is_cancelled_not_permission() {
        let shell = Arc::new(
            FakeShell::new()
                .on(
                    "osascript",
                    Ok(CmdOutput::failed(
                        1,
                        "execution error: User canceled. (-128)",
                    )),
                )
                .otherwise(Ok(CmdOutput::failed(1, "not authorized"))),
        );
        let res = schedule_mac(shell.as_ref(), &[dtm("2026-05-31 10:20")], true).await;
        assert!(!res.ok);
        assert_eq!(res.reason.as_deref(), Some("cancelled"));
        assert!(res.message.is_none());
    }

    #[tokio::test]
    async fn mac_admin_prompt_real_failure_is_permission_and_keeps_the_message() {
        let shell = Arc::new(
            FakeShell::new()
                .on(
                    "osascript",
                    Ok(CmdOutput::failed(1, "pmset: Operation not permitted")),
                )
                .otherwise(Ok(CmdOutput::failed(1, "not authorized"))),
        );
        let res = schedule_mac(shell.as_ref(), &[dtm("2026-05-31 10:20")], true).await;
        assert_eq!(res.reason.as_deref(), Some("permission"));
        assert!(res.message.unwrap().contains("Operation not permitted"));
    }

    #[tokio::test]
    async fn mac_empty_schedule_cancels_and_registers_nothing() {
        let shell = Arc::new(FakeShell::new());
        let res = schedule_mac(shell.as_ref(), &[], true).await;
        assert!(res.ok);
        assert_eq!(res.count, Some(0));
        assert_eq!(shell.log(), vec!["pmset schedule cancelall SundayRec"]);
    }

    // ── Windows mechanism ───────────────────────────────────────────────────

    #[test]
    fn windows_clears_then_arms_one_waitable_timer_per_point() {
        let timers = FakeTimers::default();
        let now = dtm("2026-05-31 10:00");
        let res = schedule_windows(
            &timers,
            &[dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")],
            now,
        );
        assert!(res.ok);
        assert_eq!(res.count, Some(2));
        assert_eq!(res.next_wake.as_deref(), Some("2026-05-31T10:20:00"));
        assert_eq!(timers.clears(), 1);
        assert_eq!(
            timers.armed(),
            vec!["2026-05-31T10:20:00", "2026-06-07T10:20:00"]
        );
    }

    #[test]
    fn windows_empty_schedule_clears_without_arming() {
        let timers = FakeTimers::default();
        let res = schedule_windows(&timers, &[], dtm("2026-05-31 10:00"));
        assert!(res.ok);
        assert_eq!(res.count, Some(0));
        // The clear still has to happen — that is how a removed slot's wake
        // stops firing.
        assert_eq!(timers.clears(), 1);
        assert!(timers.armed().is_empty());
    }

    #[test]
    fn windows_arm_failure_surfaces_an_error_not_a_silent_success() {
        let timers = FakeTimers::failing("SetWaitableTimer failed (error 5)");
        let res = schedule_windows(&timers, &[dtm("2026-05-31 10:20")], dtm("2026-05-31 10:00"));
        assert!(!res.ok);
        assert_eq!(res.reason.as_deref(), Some("error"));
        assert!(res.message.unwrap().contains("error 5"));
    }

    #[test]
    fn windows_reports_the_next_armed_wake_not_the_first_requested() {
        // The first point is already past, so it is never armed; reporting it as
        // "next wake" would show the user a time that will never happen.
        let timers = FakeTimers::default();
        let now = dtm("2026-05-31 10:00");
        let res = schedule_windows(
            &timers,
            &[dtm("2026-05-31 09:00"), dtm("2026-05-31 11:00")],
            now,
        );
        assert_eq!(res.count, Some(1));
        assert_eq!(res.next_wake.as_deref(), Some("2026-05-31T11:00:00"));
    }

    // ── Platform routing + dedup ────────────────────────────────────────────

    #[tokio::test]
    async fn unsupported_platform_reports_unsupported_without_touching_the_os() {
        let shell = Arc::new(FakeShell::new());
        let timers = FakeTimers::default();
        let res = schedule_os_wakes(
            shell.as_ref(),
            &timers,
            WakePlatform::Linux,
            &[dtm("2026-05-31 10:20")],
            dtm("2026-05-31 10:00"),
            true,
        )
        .await;
        assert_eq!(res.reason.as_deref(), Some("unsupported"));
        assert!(shell.log().is_empty());
        assert_eq!(timers.clears(), 0);
    }

    #[tokio::test]
    async fn engine_dedups_an_unchanged_schedule() {
        let shell = Arc::new(FakeShell::new());
        let engine = WakeEngine::with(shell.clone(), no_timers(), WakePlatform::MacArm);
        let now = dtm("2026-05-31 10:00");
        let upcoming = [dtm("2026-05-31 11:00")];

        let first = engine.reschedule(&upcoming, now, true, false).await;
        assert!(first.ok);
        let after_first = shell.log().len();
        assert!(after_first > 0);

        // Same set, not user-initiated → no OS work at all, no second prompt.
        let second = engine.reschedule(&upcoming, now, true, false).await;
        assert!(second.ok);
        assert_eq!(shell.log().len(), after_first);

        // User-initiated ALWAYS re-runs, even unchanged.
        let forced = engine.reschedule(&upcoming, now, true, true).await;
        assert!(forced.ok);
        assert!(shell.log().len() > after_first);
    }

    #[tokio::test]
    async fn engine_records_the_empty_key_so_a_re_add_re_registers() {
        // The invariant the dedup nearly loses: after the schedule empties out we
        // cancel the OS timers AND store the empty key. If we stored nothing, the
        // old key would still be current, and re-adding the same time would look
        // "unchanged" — dedup'd away against timers that no longer exist.
        let shell = Arc::new(FakeShell::new());
        let engine = WakeEngine::with(shell.clone(), no_timers(), WakePlatform::MacArm);
        let now = dtm("2026-05-31 10:00");
        let upcoming = [dtm("2026-05-31 11:00")];

        assert!(engine.reschedule(&upcoming, now, true, false).await.ok);
        // Schedule emptied → cancel, and the empty key is recorded.
        assert!(engine.reschedule(&[], now, true, false).await.ok);
        assert_eq!(lock_recover(&engine.last_key).as_deref(), Some(""));

        let before = shell.count("schedule wake");
        // Re-add the very same time: this MUST reach the OS again.
        assert!(engine.reschedule(&upcoming, now, true, false).await.ok);
        assert!(
            shell.count("schedule wake") > before,
            "re-add after a clear was silently dedup'd"
        );
    }

    #[tokio::test]
    async fn engine_reports_disabled_without_touching_the_os() {
        let shell = Arc::new(FakeShell::new());
        let engine = WakeEngine::with(shell.clone(), no_timers(), WakePlatform::MacArm);
        let res = engine
            .reschedule(
                &[dtm("2026-05-31 11:00")],
                dtm("2026-05-31 10:00"),
                false,
                true,
            )
            .await;
        assert_eq!(res.reason.as_deref(), Some("disabled"));
        assert!(shell.log().is_empty());
    }

    // ── Verification reads ──────────────────────────────────────────────────

    #[tokio::test]
    async fn mac_observed_wakes_prefer_iokit_and_skip_the_pmset_spawn() {
        let shell = Arc::new(FakeShell::new().otherwise(Err("pmset must not run".into())));
        let iokit = vec![VerifiedWake {
            scheduled_at: dtm("2026-05-31 10:20"),
            owner_label: "SundayRec".into(),
        }];
        let got = observed_wakes(shell.as_ref(), WakePlatform::MacArm, Some(iokit)).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].owner_label, "SundayRec");
        assert_eq!(shell.count("pmset"), 0);
    }

    #[tokio::test]
    async fn mac_observed_wakes_fall_back_to_pmset_when_iokit_reports_nothing() {
        // Apple Silicon can hold an active schedule IOKit does not list. Treating
        // an empty IOKit answer as authoritative would tell the user their wake is
        // missing and prompt a pointless admin re-register.
        let sched = "Scheduled power events:\n [0]  wake at 5/31/2026 10:20:00 by 'SundayRec'\n";
        let shell = Arc::new(FakeShell::new().on("pmset -g sched", Ok(CmdOutput::ok(sched))));
        let got = observed_wakes(shell.as_ref(), WakePlatform::MacArm, Some(Vec::new())).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].scheduled_at, dtm("2026-05-31 10:20"));
        assert_eq!(shell.count("pmset -g sched"), 1);
    }

    #[tokio::test]
    async fn windows_battery_read_falls_back_from_wmic_to_cim() {
        // `wmic` is gone from recent Windows builds; without the fallback the
        // power-source panel would read "unknown" on every modern laptop.
        let shell = Arc::new(
            FakeShell::new()
                .on("wmic", Err("program not found".into()))
                .on("Get-CimInstance", Ok(CmdOutput::ok("1\r\n"))),
        );
        let on_battery = check_power_source(shell.as_ref(), WakePlatform::Win).await;
        assert_eq!(on_battery, Some(true));
        assert_eq!(shell.count("wmic"), 1);
        assert_eq!(shell.count("Get-CimInstance"), 1);
    }

    #[tokio::test]
    async fn windows_battery_read_stops_at_wmic_when_it_answers() {
        let shell = Arc::new(
            FakeShell::new()
                .on("wmic", Ok(CmdOutput::ok("BatteryStatus=2\r\n")))
                .on("Get-CimInstance", Err("must not be reached".into())),
        );
        assert_eq!(
            check_power_source(shell.as_ref(), WakePlatform::Win).await,
            Some(false)
        );
        assert_eq!(shell.count("Get-CimInstance"), 0);
    }

    #[tokio::test]
    async fn verify_flags_a_mismatch_when_the_os_is_missing_one_of_our_wakes() {
        let shell = Arc::new(FakeShell::new().otherwise(Ok(CmdOutput::ok(""))));
        let expected = [dtm("2026-05-31 10:20"), dtm("2026-06-07 10:20")];
        let iokit = vec![VerifiedWake {
            scheduled_at: dtm("2026-05-31 10:20"),
            owner_label: "SundayRec".into(),
        }];
        let status =
            verify_with(shell.as_ref(), WakePlatform::MacArm, Some(iokit), &expected).await;
        assert!(status.has_mismatch);
        assert_eq!(status.expected_wakes.len(), 2);
        assert_eq!(status.observed_wakes.len(), 1);
        assert_eq!(status.observed_wakes[0].scheduled_at, "2026-05-31T10:20:00");
    }

    // ── Error classification ────────────────────────────────────────────────

    #[test]
    fn admin_prompt_cancel_is_told_apart_from_a_real_failure() {
        assert!(is_admin_prompt_cancel(
            "execution error: User canceled. (-128)"
        ));
        assert!(is_admin_prompt_cancel("User cancelled"));
        assert!(is_admin_prompt_cancel("AppleScript error -128"));
        assert!(!is_admin_prompt_cancel("Authentication failed"));
        assert!(!is_admin_prompt_cancel("pmset: command not found"));
    }

    // ── Verify cache (R3, optional half) ────────────────────────────────────
    //
    // One test, not three: `WAKE_VERIFY_CACHE` is a process-wide static, and
    // `verify_scheduled_wakes` itself is untestable here without spawning the
    // REAL `pmset` (it calls `real_shell()`, not an injected one — every other
    // test in this file goes through `verify_with`/`schedule_mac` precisely to
    // avoid that). So this drives `cached_wake_status` + the same
    // `WakeVerifyCacheEntry` the real function writes, directly — one test
    // keeps the whole narrative on one thread, so cargo's default parallel
    // test runner cannot interleave two mutations of the same static.
    #[test]
    fn wake_verify_cache_hits_the_same_schedule_misses_a_changed_one_and_expires() {
        let expected = [dtm("2026-05-31 10:20")];
        let key = key_of(&expected);
        assert!(
            cached_wake_status(&key).is_none(),
            "an empty cache starts as a miss"
        );

        let status = WakeStatus {
            expected_wakes: vec!["2026-05-31T10:20:00".to_string()],
            observed_wakes: vec![],
            has_mismatch: false,
            on_battery: Some(false),
            standby_enabled: Some(true),
        };
        *lock_recover(&WAKE_VERIFY_CACHE) = Some(WakeVerifyCacheEntry {
            key: key.clone(),
            at: Instant::now(),
            status: status.clone(),
        });

        // The hit: same key, fresh entry.
        let hit = cached_wake_status(&key).expect("a fresh entry is a hit");
        assert_eq!(hit.expected_wakes, status.expected_wakes);
        assert_eq!(hit.has_mismatch, status.has_mismatch);
        assert_eq!(hit.on_battery, status.on_battery);

        // The miss that matters most: a DIFFERENT schedule must NEVER read
        // the old verdict back. This is the exact bug R3's four renderer
        // triggers exist to prevent — a settings change is one of them
        // precisely because a blind (un-keyed) cache would otherwise serve a
        // stale mismatch for up to `WAKE_VERIFY_CACHE_TTL` after it.
        let other_key = key_of(&[dtm("2026-06-07 10:20")]);
        assert!(
            cached_wake_status(&other_key).is_none(),
            "a changed schedule must not read the previous schedule's cached verdict"
        );

        // The TTL: backdated well past it, without sleeping — the fast way
        // to prove the expiry branch fires.
        *lock_recover(&WAKE_VERIFY_CACHE) = Some(WakeVerifyCacheEntry {
            key: key.clone(),
            at: Instant::now() - WAKE_VERIFY_CACHE_TTL - Duration::from_secs(1),
            status,
        });
        assert!(
            cached_wake_status(&key).is_none(),
            "an entry older than the TTL is a miss, not a stale hit"
        );

        // Leave the static as this test found it — it is process-wide, and a
        // stray `Some` here would be a fixture only this test knows about,
        // silently changing whatever runs next in the same process.
        *lock_recover(&WAKE_VERIFY_CACHE) = None;
    }
}
