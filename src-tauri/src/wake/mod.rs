//! Wake-from-sleep plumbing (Fase 5.2) — the impure OS shell over the pure
//! [`sundayrec_core::wake`] decision core.
//!
//! Ported from the Electron `src/main/wake.ts` + `wake-verification.ts`. The
//! *decisions* — which wake points to register, how to format a `pmset`/
//! `schtasks` time, classifying errors, parsing `pmset`/`powercfg` output,
//! matching expected vs observed wakes, platform capabilities — all live in the
//! core and carry the tests. This module only spawns the OS tools and assembles
//! their parsed results.
//!
//! ## ⚠️ OS/HARDWARE-UNVERIFIED
//!
//! Every function here shells out to `pmset` / `osascript` / `powershell` /
//! `powercfg` / `wmic`. The argument shaping + output parsing are unit-tested in
//! the core, but the actual scheduling, the admin/UAC elevation prompts, and
//! whether the machine *truly* wakes can only be confirmed on a real Mac/Windows
//! box. On this dev host the read-only probes (`get_sleep_config`, `verify`) may
//! run for real; the mutating ones (schedule, fix) are wired but unexercised.
//!
//! ## Honestly deferred
//!
//! The Electron `testWake` (schedule a near-future wake, *sleep the machine*,
//! and measure the resume via `powerMonitor`) is NOT ported here: Tauri has no
//! built-in power-resume event, and sleeping the user's machine without a
//! reliable resume signal is worse than not offering it. The pure verdict
//! ([`sundayrec_core::wake::classify_test_wake_delta`]) is ready for when a
//! power-monitor capability lands.

use std::sync::Mutex;
use std::time::Duration as StdDuration;

use chrono::{NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use ts_rs::TS;

use sundayrec_core::wake::{
    build_win_task_defs, classify_win_error, compare_expected_to_observed, format_pmset_date,
    key_of, parse_mac_sleep_config, parse_pmset_batt, parse_pmset_sched, parse_pmset_standby,
    parse_powercfg_waketimers, parse_win_wake_timers, parse_wmic_battery_status, wake_points,
    SleepConfig, WakeErrorReason, WakePlatform, WinErrorKind, WAKE_LEAD_MINUTES,
    WAKE_MATCH_TOLERANCE_MS,
};

/// The outcome of an OS wake-scheduling attempt. Mirrors the Electron `WakeResult`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/WakeResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeResult {
    pub ok: bool,
    pub count: Option<u32>,
    /// ISO-like local string of the first/next scheduled wake, or `None`.
    pub next_wake: Option<String>,
    /// Why it failed: `disabled | cancelled | permission | unsupported | error`.
    pub reason: Option<String>,
    pub message: Option<String>,
}

impl WakeResult {
    fn ok(count: u32, next_wake: Option<String>) -> Self {
        Self {
            ok: true,
            count: Some(count),
            next_wake,
            reason: None,
            message: None,
        }
    }
    fn fail(reason: WakeErrorReason, message: Option<String>) -> Self {
        Self {
            ok: false,
            count: None,
            next_wake: None,
            reason: Some(reason.as_str().to_string()),
            message,
        }
    }
}

/// One OS-observed wake, for the verification panel.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/ObservedWake.ts")]
#[serde(rename_all = "camelCase")]
pub struct ObservedWake {
    pub scheduled_at: String,
    pub owner_label: String,
}

/// The verification snapshot: what we asked the OS to schedule vs what it
/// reports, plus power facts. Mirrors the Electron `WakeStatus` minus the
/// `capabilities` field — the UI reads those from the separate
/// `wake_capabilities` command, so this src-tauri type doesn't embed the
/// core-crate [`WakeCapabilities`] (a cross-crate ts-rs embed produces a broken
/// relative import path; commands returning core types separately are the
/// codebase convention).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/WakeStatus.ts")]
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
#[ts(export, export_to = "../../src/lib/bindings/WakeFixResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeFixResult {
    pub ok: bool,
    pub message: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
//   Engine (Tauri-managed state) — dedups repeated scheduling
// ─────────────────────────────────────────────────────────────────────────────

/// Managed-state handle. Holds the last successfully-scheduled wake-point key so
/// an unchanged reschedule (the common case — the supervisor recomputes often)
/// is a cheap no-op. Mirrors the Electron `lastScheduledByPlatform` dedup.
#[derive(Default)]
pub struct WakeEngine {
    last_key: Mutex<Option<String>>,
}

impl WakeEngine {
    pub fn new() -> Self {
        Self::default()
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

        if !allow_admin && !points.is_empty() {
            if let Ok(last) = self.last_key.lock() {
                if last.as_deref() == Some(key.as_str()) {
                    return WakeResult::ok(points.len() as u32, points.first().map(fmt_local));
                }
            }
        }

        let result = schedule_os_wakes(&points, allow_admin).await;
        if result.ok && !points.is_empty() {
            if let Ok(mut last) = self.last_key.lock() {
                *last = Some(key);
            }
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Scheduling
// ─────────────────────────────────────────────────────────────────────────────

async fn schedule_os_wakes(points: &[NaiveDateTime], allow_admin: bool) -> WakeResult {
    match current_platform() {
        WakePlatform::MacArm | WakePlatform::MacIntel => schedule_mac(points, allow_admin).await,
        WakePlatform::Win => schedule_windows(points).await,
        _ => WakeResult::fail(WakeErrorReason::Unsupported, None),
    }
}

async fn schedule_mac(points: &[NaiveDateTime], allow_admin: bool) -> WakeResult {
    // Clear our previously-scheduled wakes (best-effort).
    let _ = run("pmset", &["schedule", "cancelall", "SundayRec"], 3000).await;

    if points.is_empty() {
        return WakeResult::ok(0, None);
    }

    let mut scheduled = 0u32;
    for d in points {
        let stamp = format_pmset_date(*d);
        if run("pmset", &["schedule", "wake", &stamp, "SundayRec"], 5000)
            .await
            .is_ok()
        {
            scheduled += 1;
        }
    }
    if scheduled as usize == points.len() {
        return WakeResult::ok(scheduled, points.first().map(fmt_local));
    }

    if !allow_admin {
        return WakeResult::fail(WakeErrorReason::Permission, None);
    }

    // Elevated retry: one osascript admin prompt running all the pmset commands.
    let cmds = points
        .iter()
        .map(|d| {
            format!(
                "pmset schedule wake \\\"{}\\\" SundayRec",
                format_pmset_date(*d)
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    let script = format!("do shell script \"{cmds}\" with administrator privileges");
    match run("osascript", &["-e", &script], 30000).await {
        Ok(_) => WakeResult::ok(points.len() as u32, points.first().map(fmt_local)),
        Err(msg) => {
            if msg.contains("User canceled") {
                WakeResult::fail(WakeErrorReason::Cancelled, None)
            } else {
                WakeResult::fail(WakeErrorReason::Permission, Some(msg))
            }
        }
    }
}

async fn schedule_windows(points: &[NaiveDateTime]) -> WakeResult {
    // Remove our previously-registered wake tasks (best-effort).
    let _ = run(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-ScheduledTask -TaskPath '\\SundayRec\\*' -ErrorAction SilentlyContinue | Unregister-ScheduledTask -Confirm:$false",
        ],
        10000,
    )
    .await;

    if points.is_empty() {
        return WakeResult::ok(0, None);
    }

    // Try elevated first, fall back to standard user on a permission error.
    for elevated in [true, false] {
        let defs = build_win_task_defs(points, elevated);
        match run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", &defs],
            20000,
        )
        .await
        {
            Ok(_) => return WakeResult::ok(points.len() as u32, points.first().map(fmt_local)),
            Err(msg) => match classify_win_error(&msg) {
                WinErrorKind::Permission if elevated => continue,
                WinErrorKind::Permission => {
                    return WakeResult::fail(WakeErrorReason::Permission, Some(msg))
                }
                WinErrorKind::Error => return WakeResult::fail(WakeErrorReason::Error, Some(msg)),
            },
        }
    }
    WakeResult::fail(WakeErrorReason::Permission, None)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Test-wake (manual diagnostic)
// ─────────────────────────────────────────────────────────────────────────────

/// The outcome of scheduling a manual test-wake. Mirrors the Electron
/// `testWake`'s return: on success a `jobId` the renderer can cancel, plus the
/// scheduled wall-clock time the resume handler will compare against.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/TestWakeResult.ts")]
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
/// ⚠️ HARDWARE-UNVERIFIED — spawns `pmset`/`schtasks`; the actual wake can't be
/// proven in the gate (the machine has to sleep, then wake). See SMOKE-TEST.md.
pub async fn schedule_test_wake(seconds_ahead: i64) -> TestWakeResult {
    let secs = seconds_ahead.clamp(5, 3600);
    let target = (Utc::now() + chrono::Duration::seconds(secs)).naive_local();
    let result = schedule_os_wakes(std::slice::from_ref(&target), true).await;
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
            run("pmset", &["schedule", "cancelall", "SundayRec"], 3000)
                .await
                .is_ok()
        }
        WakePlatform::Win => run(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-ScheduledTask -TaskPath '\\SundayRec\\*' -ErrorAction SilentlyContinue | Unregister-ScheduledTask -Confirm:$false",
            ],
            10000,
        )
        .await
        .is_ok(),
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Sleep config + fixes
// ─────────────────────────────────────────────────────────────────────────────

/// Read the OS sleep/power configuration. Port of `getSleepConfig`.
pub async fn get_sleep_config() -> SleepConfig {
    match current_platform() {
        WakePlatform::MacArm | WakePlatform::MacIntel => match run("pmset", &["-g"], 5000).await {
            Ok(out) => parse_mac_sleep_config(&out),
            Err(e) => SleepConfig {
                error: Some(e),
                ..Default::default()
            },
        },
        WakePlatform::Win => {
            let cmd = "$s = (powercfg /getactivescheme) -replace '.*GUID: ([\\w-]+).*','$1'; powercfg /query $s 238C9FA8-0AAD-41ED-83F4-97BE242C8F20 BD3B718A-0680-4D9D-8AB2-E1D2B4AC806D";
            match run("powershell", &["-NoProfile", "-Command", cmd], 10000).await {
                Ok(out) => SleepConfig {
                    wake_timers_enabled: parse_win_wake_timers(&out),
                    ..Default::default()
                },
                Err(e) => SleepConfig {
                    error: Some(e),
                    ..Default::default()
                },
            }
        }
        _ => SleepConfig::default(),
    }
}

/// Disable autopoweroff + raise standbydelay so a Mac stays in (wakeable) sleep.
/// Port of `fixMacSleep`. Requires an admin prompt.
pub async fn fix_mac_sleep() -> WakeFixResult {
    let cmd = "pmset -a autopoweroff 0; pmset -a standbydelay 86400";
    let script = format!("do shell script \"{cmd}\" with administrator privileges");
    match run("osascript", &["-e", &script], 30000).await {
        Ok(_) => WakeFixResult {
            ok: true,
            message: None,
        },
        Err(msg) => WakeFixResult {
            ok: false,
            message: Some(if msg.contains("User canceled") {
                "cancelled".to_string()
            } else {
                msg
            }),
        },
    }
}

/// Enable wake timers (AC + DC) in the active power scheme. Port of `fixWinWakeTimers`.
pub async fn fix_win_wake_timers() -> WakeFixResult {
    let cmd = "$s = (powercfg /getactivescheme) -replace '.*GUID: ([\\w-]+).*','$1'; powercfg /setacvalueindex $s 238C9FA8-0AAD-41ED-83F4-97BE242C8F20 BD3B718A-0680-4D9D-8AB2-E1D2B4AC806D 1; powercfg /setdcvalueindex $s 238C9FA8-0AAD-41ED-83F4-97BE242C8F20 BD3B718A-0680-4D9D-8AB2-E1D2B4AC806D 1; powercfg /setactive $s";
    match run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", cmd],
        15000,
    )
    .await
    {
        Ok(_) => WakeFixResult {
            ok: true,
            message: None,
        },
        Err(msg) => {
            let lower = msg.to_lowercase();
            let admin = [
                "access",
                "denied",
                "unauthorized",
                "privilege",
                "administrator",
            ]
            .iter()
            .any(|p| lower.contains(p));
            WakeFixResult {
                ok: false,
                message: Some(if admin {
                    "admin_required".to_string()
                } else {
                    msg
                }),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Verification
// ─────────────────────────────────────────────────────────────────────────────

/// Compare what we expect the OS to have scheduled (from `expected`) against
/// what it actually reports, plus capabilities + power facts. Port of
/// `verifyScheduledWakes`.
pub async fn verify_scheduled_wakes(expected: &[NaiveDateTime]) -> WakeStatus {
    let platform = current_platform();
    let observed = query_observed_wakes(platform).await;
    let on_battery = check_power_source(platform).await;
    let standby_enabled = check_standby(platform).await;
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

async fn query_observed_wakes(platform: WakePlatform) -> Vec<sundayrec_core::wake::VerifiedWake> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => {
            match run("pmset", &["-g", "sched"], 5000).await {
                Ok(out) => parse_pmset_sched(&out, Some(Utc::now().year_ce().1 as i32)),
                Err(_) => Vec::new(),
            }
        }
        WakePlatform::Win => match run("powercfg", &["-waketimers"], 5000).await {
            Ok(out) => parse_powercfg_waketimers(&out),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

async fn check_power_source(platform: WakePlatform) -> Option<bool> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => run("pmset", &["-g", "batt"], 5000)
            .await
            .ok()
            .and_then(|o| parse_pmset_batt(&o)),
        WakePlatform::Win => {
            if let Ok(o) = run(
                "wmic",
                &["path", "Win32_Battery", "get", "BatteryStatus", "/value"],
                5000,
            )
            .await
            {
                return parse_wmic_battery_status(&o);
            }
            // Newer Windows may lack wmic — fall back to PowerShell CIM.
            run(
                "powershell",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "(Get-CimInstance -ClassName Win32_Battery | Select-Object -First 1 -ExpandProperty BatteryStatus)",
                ],
                8000,
            )
            .await
            .ok()
            .and_then(|o| o.trim().parse::<i32>().ok().map(|s| s == 1))
        }
        _ => None,
    }
}

async fn check_standby(platform: WakePlatform) -> Option<bool> {
    match platform {
        WakePlatform::MacArm | WakePlatform::MacIntel => run("pmset", &["-g"], 5000)
            .await
            .ok()
            .and_then(|o| parse_pmset_standby(&o)),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Helpers
// ─────────────────────────────────────────────────────────────────────────────

use chrono::Datelike;

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

/// Format a wall-clock datetime as a zone-less local ISO string for the UI.
fn fmt_local(d: &NaiveDateTime) -> String {
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Run an external command with a timeout, returning stdout on success or a
/// stderr/error string on failure (non-zero exit or spawn error).
async fn run(program: &str, args: &[&str], timeout_ms: u64) -> Result<String, String> {
    let fut = Command::new(program).args(args).output();
    let output = match tokio::time::timeout(StdDuration::from_millis(timeout_ms), fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err(format!("{program} timed out")),
    };
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.trim().is_empty() {
            format!("{program} exited with {}", output.status)
        } else {
            stderr.into_owned()
        })
    }
}

/// Convert a stored epoch-ms (UI/JS) into the local-wall frame — kept for
/// symmetry with the scheduler, currently unused by wake (expected wakes come
/// from the scheduler's `NaiveDateTime`s directly).
#[allow(dead_code)]
fn epoch_ms_to_local(ms: i64) -> Option<NaiveDateTime> {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.naive_local())
}
