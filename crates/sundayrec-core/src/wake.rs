//! Wake-from-sleep decision core (Fase 5.2) — pure, OS-free.
//!
//! Ported from the Electron `src/main/wake.ts` and `src/main/wake-verification.ts`.
//! Those files interleaved the *decisions* (which wake points to schedule, how to
//! format a `pmset` time, classifying an error string, parsing the OS
//! power tools' text output, matching expected wakes against observed ones,
//! deciding platform capabilities) with the actual I/O (`execFile` of
//! `pmset`/`osascript`/`powershell`/`powercfg`, `powerSaveBlocker`,
//! `powerMonitor`). Here we keep ONLY the deterministic decisions; the
//! `src-tauri` `wake` shell owns the process spawning and the power blocker.
//!
//! Platform reality this module encodes (the canonical truth, from the Electron
//! header):
//!   - macOS Apple Silicon: `pmset` *wake* works, *poweron* does not; deep-sleep
//!     (standby) can sabotage wake.
//!   - macOS Intel: wake works, poweron needs a manual System-Settings toggle.
//!   - Windows: a `SetWaitableTimer(fResume = TRUE)` armed by the RUNNING
//!     process wakes the machine from S3/S4. It dies with the process, which is
//!     acceptable because SundayRec autostarts and lives in the tray. S5 needs a
//!     BIOS toggle we can't reach. Laptops often disable wake timers on battery.
//!   - Linux/other: no supported wake mechanism.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use ts_rs::TS;

/// Wake the machine this many minutes before a scheduled recording, so it's
/// fully up and the recorder/preflight have run. (`wake.ts` `LEAD_MINUTES`.)
pub const WAKE_LEAD_MINUTES: i64 = 10;

/// ±slack when matching an expected wake against an OS-observed one: `pmset`
/// rounds to the minute, `powercfg` can lag a few seconds.
/// (`wake-verification.ts` `WAKE_MATCH_TOLERANCE_MS`.)
pub const WAKE_MATCH_TOLERANCE_MS: i64 = 60_000;

/// Keep the app un-suspended when a recording is within this window, so the
/// supervisor's timers actually fire. (`wake.ts` `updateBlocker` `soonMs`.)
pub const BLOCKER_SOON_MS: i64 = 30 * 60_000;

// ─────────────────────────────────────────────────────────────────────────────
//   Platform + capabilities
// ─────────────────────────────────────────────────────────────────────────────

/// The host class for wake purposes. Serialised to the EXACT Electron
/// `WakePlatform` strings (`'mac-arm' | 'mac-intel' | 'win' | 'linux' | 'other'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakePlatform.ts")]
#[serde(rename_all = "kebab-case")]
pub enum WakePlatform {
    MacArm,
    MacIntel,
    Win,
    Linux,
    Other,
}

/// Honest, OS-grounded statement of what wake can and can't do on this host.
/// Mirrors the Electron `WakeCapabilities`. The `knownIssues`/`recommendations`
/// are user-facing Norwegian, ported verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeCapabilities.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeCapabilities {
    pub platform: WakePlatform,
    /// Wake from S3 sleep — usually true on supported platforms.
    pub can_wake_from_sleep: bool,
    /// Wake from S5 (off) — false on Apple Silicon, BIOS-dependent on Windows.
    pub can_wake_from_off: bool,
    /// Scheduling wakes typically needs an admin/UAC prompt.
    pub needs_admin: bool,
    pub known_issues: Vec<String>,
    pub recommendations: Vec<String>,
}

/// Build the capability statement for `platform`. Pure port of
/// `wake-verification.ts` `detectCapabilities` (the platform/arch branch is the
/// shell's job — it passes the resolved [`WakePlatform`] in).
pub fn detect_capabilities(platform: WakePlatform) -> WakeCapabilities {
    match platform {
        WakePlatform::MacArm => WakeCapabilities {
            platform,
            can_wake_from_sleep: true,
            can_wake_from_off: false,
            needs_admin: true,
            known_issues: vec![
                "Apple Silicon kan ikke starte fra fullstendig avslått tilstand — kun fra dvale."
                    .to_string(),
            ],
            recommendations: vec![
                "La maskinen stå i dvale (ikke slå den av) etter forberedelsene.".to_string(),
                "Slå av dyp dvale (standby) med «Fiks automatisk»-knappen nedenfor.".to_string(),
                "Tilkoblet strøm må være på — Mac vekker ikke pålitelig på batteri.".to_string(),
            ],
        },
        WakePlatform::MacIntel => WakeCapabilities {
            platform,
            can_wake_from_sleep: true,
            can_wake_from_off: true,
            needs_admin: true,
            known_issues: vec![
                "Intel Mac kan starte fra avslått, men du må aktivere «Start opp eller vekk» manuelt i Systemvalg → Batteri."
                    .to_string(),
            ],
            recommendations: vec![
                "Tilkoblet strøm må være på — Mac vekker ikke pålitelig på batteri.".to_string(),
            ],
        },
        WakePlatform::Win => WakeCapabilities {
            platform,
            can_wake_from_sleep: true,
            can_wake_from_off: false,
            needs_admin: false,
            known_issues: vec![
                "Vekkingen settes av SundayRec mens programmet kjører. Avslutter du SundayRec helt, forsvinner vekketimeren."
                    .to_string(),
                "Wake fra fullstendig avslått (S5) krever at «Wake on RTC from S5» er aktivert i BIOS — kan ikke aktiveres fra programvare."
                    .to_string(),
            ],
            recommendations: vec![
                "La SundayRec være i gang — den starter automatisk ved pålogging og ligger i systemkurven."
                    .to_string(),
                "Sett maskinen i dvale (Sleep/Hibernate), ikke skru den av.".to_string(),
                "Tilkoblet strøm bør være på — mange bærbare deaktiverer vekketimere på batteri."
                    .to_string(),
                "Hvis test-wake feiler, sjekk BIOS for «Wake on RTC» og slå på «Tillat vekketimere» i strømalternativer."
                    .to_string(),
            ],
        },
        WakePlatform::Linux => WakeCapabilities {
            platform,
            can_wake_from_sleep: false,
            can_wake_from_off: false,
            needs_admin: false,
            known_issues: vec![
                "Linux støttes ikke for automatisk oppvåkning fra SundayRec.".to_string(),
            ],
            recommendations: vec![
                "Bruk Mac eller Windows for å aktivere automatisk wake.".to_string(),
            ],
        },
        WakePlatform::Other => WakeCapabilities {
            platform,
            can_wake_from_sleep: false,
            can_wake_from_off: false,
            needs_admin: false,
            known_issues: vec!["Plattformen støttes ikke for automatisk oppvåkning.".to_string()],
            recommendations: vec![],
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Wake-point selection + scheduler-command builders
// ─────────────────────────────────────────────────────────────────────────────

/// Subtract the lead and drop any point already in the past → the wake times to
/// register with the OS. Pure port of the `scheduleOsWakes` mapping in `wake.ts`.
pub fn wake_points(
    upcoming: &[NaiveDateTime],
    now: NaiveDateTime,
    lead_minutes: i64,
) -> Vec<NaiveDateTime> {
    upcoming
        .iter()
        .map(|d| *d - Duration::minutes(lead_minutes))
        .filter(|d| *d > now)
        .collect()
}

/// True if any upcoming recording is within [`BLOCKER_SOON_MS`] — the app should
/// hold an app-suspension blocker so its in-process timers fire. Port of the
/// `hasSoon` decision in `wake.ts` `updateBlocker`.
pub fn should_block(upcoming: &[NaiveDateTime], now: NaiveDateTime) -> bool {
    upcoming.iter().any(|d| {
        let delta = (*d - now).num_milliseconds();
        delta > 0 && delta < BLOCKER_SOON_MS
    })
}

/// Stable dedup key for a set of wake points — if the next scheduling request
/// matches the last, the shell skips the work. Port of `wake.ts` `keyOf`.
pub fn key_of(dates: &[NaiveDateTime]) -> String {
    dates
        .iter()
        .map(|d| d.and_utc().timestamp_millis().to_string())
        .collect::<Vec<_>>()
        .join("|")
}

/// What the wake engine should do for a reschedule, given the previously-applied
/// wake-point key (`None` if nothing was ever applied), the freshly-computed key
/// for `new_points`, and whether the user explicitly initiated this (`forced`).
///
/// The decision is split out as a pure function so the dedup + stale-timer logic
/// can be tested without touching `pmset` or a wake timer. The crucial correctness
/// point: when the new set is *empty* we must still apply (to cancel any stale OS
/// wakes the previous key registered) and record the empty key — otherwise a
/// later re-add of the same time would dedup against a key whose OS timers were
/// already cancelled and silently never re-register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeRescheduleAction {
    /// Nothing to do — the OS already holds exactly this set; don't re-prompt.
    SkipUnchanged,
    /// Run the OS scheduling (cancel-then-register) and, on success, store `key`.
    Apply,
}

/// Decide whether to re-run OS wake scheduling. `forced` (a user-initiated
/// reschedule) always applies. An unchanged *non-empty* set is skipped. A changed
/// set, or any empty set whose previously-applied key was non-empty (i.e. we have
/// stale OS timers to clear), always applies.
pub fn decide_reschedule(
    last_key: Option<&str>,
    new_key: &str,
    new_is_empty: bool,
    forced: bool,
) -> WakeRescheduleAction {
    if forced {
        return WakeRescheduleAction::Apply;
    }
    // Identical to the last applied set ⇒ the OS is already correct.
    if last_key == Some(new_key) {
        return WakeRescheduleAction::SkipUnchanged;
    }
    // An empty set with no prior (or already-empty) key has nothing to clear.
    if new_is_empty && (last_key.is_none() || last_key == Some("")) {
        return WakeRescheduleAction::SkipUnchanged;
    }
    WakeRescheduleAction::Apply
}

/// `pmset schedule wake` time format: `MM/DD/YY HH:MM:00`. Port of `formatPmsetDate`.
pub fn format_pmset_date(d: NaiveDateTime) -> String {
    format!(
        "{:02}/{:02}/{:02} {:02}:{:02}:00",
        d.month(),
        d.day(),
        d.year() % 100,
        d.hour(),
        d.minute(),
    )
}

/// Windows wall-clock label: `YYYY-MM-DDTHH:MM:00`. Was the
/// `New-ScheduledTaskTrigger -At` argument; since the scheduled-task mechanism
/// was replaced by `SetWaitableTimer` (see the module header) it survives as the
/// human-readable label each armed timer carries in logs and diagnostics.
pub fn format_win_datetime(d: NaiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:00",
        d.year(),
        d.month(),
        d.day(),
        d.hour(),
        d.minute(),
    )
}

/// Why an OS wake-scheduling attempt failed — the `reason` the UI localises.
/// Serialises to the EXACT Electron `WakeResult.reason` union
/// (`'disabled' | 'cancelled' | 'permission' | 'unsupported' | 'error'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeErrorReason {
    /// The user has turned wake-from-sleep off.
    Disabled,
    /// The admin/UAC prompt was dismissed.
    Cancelled,
    /// Scheduling needs elevation we don't have.
    Permission,
    /// This platform has no supported wake mechanism.
    Unsupported,
    /// Any other failure.
    Error,
}

impl WakeErrorReason {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeErrorReason::Disabled => "disabled",
            WakeErrorReason::Cancelled => "cancelled",
            WakeErrorReason::Permission => "permission",
            WakeErrorReason::Unsupported => "unsupported",
            WakeErrorReason::Error => "error",
        }
    }

    /// The inverse of [`Self::as_str`] — the wire string back to the enum, so
    /// code that has to reason about a `WakeResult.reason` does it in types
    /// instead of by comparing string literals.
    ///
    /// `None` for anything unrecognised, which callers must treat as "a real
    /// failure": an unknown reason is the one that has never been triaged.
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "disabled" => WakeErrorReason::Disabled,
            "cancelled" => WakeErrorReason::Cancelled,
            "permission" => WakeErrorReason::Permission,
            "unsupported" => WakeErrorReason::Unsupported,
            "error" => WakeErrorReason::Error,
            _ => return None,
        })
    }

    /// Whether this failure is a *state of the machine* rather than a defect:
    /// the user turned wake off, dismissed the prompt, or the OS wants an
    /// elevation this call was never allowed to ask for.
    ///
    /// It does NOT mean harmless — a `Permission` here is a machine that will
    /// sleep through the service. It means "expected from an unprivileged,
    /// non-interactive attempt", i.e. not worth one log line per supervisor
    /// pass. [`should_log_background_wake`] decides how often it IS worth one.
    pub fn is_expected(self) -> bool {
        matches!(
            self,
            WakeErrorReason::Disabled | WakeErrorReason::Cancelled | WakeErrorReason::Permission
        )
    }
}

/// What the supervisor does with one background wake-reschedule outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeLogAction {
    /// Nothing happened worth saying (the reschedule succeeded).
    Silent,
    /// A real — or unclassified — failure. Logged every time, and it never eats
    /// the once-per-launch budget the expected failures share.
    Report,
    /// The first expected failure this launch: log it, and spend the budget.
    ReportOnce,
    /// An expected failure that has already been reported: count it, say
    /// nothing.
    Suppress,
}

impl WakeLogAction {
    /// Whether the supervisor writes a log line.
    pub fn logs(self) -> bool {
        matches!(self, Self::Report | Self::ReportOnce)
    }

    /// Whether this outcome spends from the once-per-launch budget.
    ///
    /// Only the expected failures do. A `Report` that counted would silence the
    /// NEXT `permission` — the very failure the budget exists to make visible
    /// once.
    pub fn counts(self) -> bool {
        matches!(self, Self::ReportOnce | Self::Suppress)
    }
}

/// Whether the scheduler's *background* (unprivileged, non-interactive) wake
/// reschedule should write a log line for this outcome.
///
/// ## The hole this closes
///
/// The supervisor logged every failure EXCEPT `permission`/`disabled`/
/// `cancelled` — and `permission` is the one that matters most: a Mac that needs
/// root to write a power event never gets asked from the supervisor (the
/// interactive `wake_reschedule` is the only path that may prompt), so the wake
/// is silently never armed and the machine sleeps through the service. Filtered
/// to nothing, that failure existed in no log at all, and the first evidence was
/// a missing recording.
///
/// ## …without turning the log into a metronome
///
/// The supervisor re-runs on every settings change and every timer, so logging
/// each expected failure would bury the interesting lines. `quiet_reports_so_far`
/// is how many expected failures this process has already reported: the first
/// one is written, the rest are counted and silent. Once per launch is enough
/// for a support log — it answers "was the wake ever armed?" — and it re-arms
/// on the next start, which is also when the user's answer to it can change.
pub fn background_wake_log_action(
    ok: bool,
    reason: Option<&str>,
    quiet_reports_so_far: u32,
) -> WakeLogAction {
    if ok {
        return WakeLogAction::Silent;
    }
    match reason.and_then(WakeErrorReason::from_wire) {
        // Expected from an unprivileged pass: once per process.
        Some(r) if r.is_expected() => {
            if quiet_reports_so_far == 0 {
                WakeLogAction::ReportOnce
            } else {
                WakeLogAction::Suppress
            }
        }
        // A real failure — or one nobody has classified. Always.
        _ => WakeLogAction::Report,
    }
}

/// Why a *successful* wake reschedule armed nothing at all.
///
/// `ok: true, count: 0` is the same answer for "the schedule is empty", "the
/// weekly plan is switched off" and "everything upcoming is already past the
/// lead" — and a volunteer who just pressed «Registrer vekkinger» deserves to
/// know which. Carried alongside the count so the UI can say it; `None`
/// whenever something was actually armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeIdleReason.ts")]
#[serde(rename_all = "camelCase")]
pub enum WakeIdleReason {
    /// «Ta opp automatisk» is off, so the weekly slots plan nothing —
    /// [`crate::settings::Settings::active_slots`] answers with an empty slice
    /// and there is nothing to wake FOR. Arming wakes anyway would be the
    /// machine waking at 10:50 on a Sunday for a recording it will refuse to
    /// make.
    AutoRecordOff,
    /// The plan is armed, but nothing falls inside the horizon (no slots, no
    /// specials, or everything upcoming is already inside the wake lead).
    NothingUpcoming,
}

/// Which [`WakeIdleReason`], if any, explains an empty wake set.
///
/// `auto_record_enabled` is the level-1 switch; `upcoming_count` is how many
/// starts the horizon produced from the active slots plus the specials. The
/// switch is checked only when the set is empty, because a disarmed weekly plan
/// with a dated special still has something to wake for.
pub fn wake_idle_reason(
    auto_record_enabled: bool,
    upcoming_count: usize,
) -> Option<WakeIdleReason> {
    if upcoming_count > 0 {
        None
    } else if !auto_record_enabled {
        Some(WakeIdleReason::AutoRecordOff)
    } else {
        Some(WakeIdleReason::NothingUpcoming)
    }
}

/// How a Windows `powercfg` failure should be classified. Port of `classifyWinError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinErrorKind {
    /// Access-denied / unauthorized / privilege — the call needs elevation.
    Permission,
    /// Anything else.
    Error,
}

/// Classify a Windows power-tool stderr string. Based on `wake.ts`
/// `classifyWinError`, with two deliberate improvements: the Electron pattern
/// `access.?denied` only matches `accessdenied` / `access denied`, NOT the
/// canonical Windows wording "**Access is denied.**" — so the original would
/// mis-classify the most common permission failure as a generic error. We accept
/// `access is denied`, and `administrator` (the wording `powercfg /setacvalueindex`
/// uses), too.
///
/// Since the wake *scheduling* moved off `Register-ScheduledTask` and onto an
/// in-process `SetWaitableTimer` (which needs no elevation at all), the only
/// remaining caller is the `powercfg` "allow wake timers" fix.
pub fn classify_win_error(msg: &str) -> WinErrorKind {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)access\s*(is\s+)?denied|unauthorized|privilege|administrator").unwrap()
    });
    if RE.is_match(msg) {
        WinErrorKind::Permission
    } else {
        WinErrorKind::Error
    }
}

/// The delta classification a test-wake applies once it observes a resume.
/// `> 30 s` late ⇒ the wake fired too late to be useful. Port of the threshold
/// in `wake.ts` `testWake`'s resume handler. (The resume *listening* is OS-level
/// and lives in the shell / a later slice; this is the pure verdict.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestWakeVerdict {
    Ok,
    TooLate,
}

/// `delta_sec` = observed − scheduled, in seconds (negative = woke early).
pub fn classify_test_wake_delta(delta_sec: i64) -> TestWakeVerdict {
    if delta_sec > 30 {
        TestWakeVerdict::TooLate
    } else {
        TestWakeVerdict::Ok
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Wake-failure / test-wake history
// ─────────────────────────────────────────────────────────────────────────────

/// Newest-first cap on the failure log. Matches the Electron `WAKE_FAILURE_MAX`
/// (`store.ts`) — older entries are trimmed.
pub const WAKE_FAILURE_MAX: usize = 20;

/// The kind of wake outcome a [`WakeFailureEntry`] records. Serialised to the
/// EXACT Electron strings (`'missed' | 'test_ok' | 'test_fail'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeFailureKind.ts")]
#[serde(rename_all = "snake_case")]
pub enum WakeFailureKind {
    /// A scheduled recording's wake never produced a run.
    Missed,
    /// A manual test-wake fired within tolerance.
    TestOk,
    /// A manual test-wake fired too late (or didn't fire).
    TestFail,
}

/// One wake-failure or test-wake outcome. Mirrors the renderer
/// `WakeFailureEntry` (camelCase) field-for-field so saved rows carry across.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "WakeFailureEntry.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeFailureEntry {
    /// Unix ms when the outcome was recorded.
    #[ts(type = "number")]
    pub timestamp: i64,
    /// ISO string — the time the wake was supposed to fire.
    pub scheduled_at: String,
    /// What kind of outcome this is.
    pub kind: WakeFailureKind,
    /// Human-readable label (slot name, "Spesialopptak", "Test-wake").
    pub label: String,
    /// Free-form reason (e.g. `no_resume`, `too_late`, `on_battery`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Actual delta in seconds between expected and observed (test-wake only).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub delta_sec: Option<i64>,
}

/// Map a test-wake delta to the [`WakeFailureKind`] + a reason, mirroring the
/// Electron `testWake` resume handler: within tolerance ⇒ `test_ok`, too late ⇒
/// `test_fail`/`too_late`. A `None` delta means no resume was observed at all
/// ⇒ `test_fail`/`no_resume`.
pub fn test_wake_outcome(delta_sec: Option<i64>) -> (WakeFailureKind, Option<String>) {
    match delta_sec {
        None => (WakeFailureKind::TestFail, Some("no_resume".into())),
        Some(d) => match classify_test_wake_delta(d) {
            TestWakeVerdict::Ok => (WakeFailureKind::TestOk, None),
            TestWakeVerdict::TooLate => (WakeFailureKind::TestFail, Some("too_late".into())),
        },
    }
}

/// Prepend `entry` to `history` (newest-first) and trim to [`WAKE_FAILURE_MAX`].
/// Pure mirror of the Electron `addWakeFailureEntry` (`unshift` + `slice(0,MAX)`).
pub fn cap_failure_history(
    mut history: Vec<WakeFailureEntry>,
    entry: WakeFailureEntry,
) -> Vec<WakeFailureEntry> {
    history.insert(0, entry);
    history.truncate(WAKE_FAILURE_MAX);
    history
}

// ─────────────────────────────────────────────────────────────────────────────
//   Observed-wake parsing + tolerance match
// ─────────────────────────────────────────────────────────────────────────────

/// A wake the OS reports it has actually scheduled (from `pmset -g sched` /
/// `powercfg -waketimers`). Internal — the shell maps it to ISO strings for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWake {
    pub scheduled_at: NaiveDateTime,
    pub owner_label: String,
}

/// Parse `pmset -g sched`, capturing only absolute one-off wakes in the
/// "Scheduled power events" section (repeating events are skipped — we don't
/// schedule them). `ref_year` guards against year typos. Port of `parsePmsetSched`.
pub fn parse_pmset_sched(stdout: &str, ref_year: Option<i32>) -> Vec<VerifiedWake> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)\bwake\s+at\s+(\d{1,2})/(\d{1,2})/(\d{2,4})\s+(\d{1,2}):(\d{2})(?::(\d{2}))?\s+by\s+['"]?([^'"]+?)['"]?\s*$"#,
        )
        .unwrap()
    });
    static SCHED_HDR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^Scheduled power events:?").unwrap());
    static REPEAT_HDR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^Repeating power events:?").unwrap());
    let (row, sched_hdr, repeat_hdr) = (&*ROW, &*SCHED_HDR, &*REPEAT_HDR);

    let mut out = Vec::new();
    let mut in_one_off = false;
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if sched_hdr.is_match(line) {
            in_one_off = true;
            continue;
        }
        if repeat_hdr.is_match(line) {
            in_one_off = false;
            continue;
        }
        if !in_one_off {
            continue;
        }
        if let Some(c) = row.captures(line) {
            let month: u32 = c[1].parse().unwrap_or(0);
            let day: u32 = c[2].parse().unwrap_or(0);
            let mut year: i32 = c[3].parse().unwrap_or(0);
            if year < 100 {
                year += 2000;
            }
            let hour: u32 = c[4].parse().unwrap_or(99);
            let min: u32 = c[5].parse().unwrap_or(99);
            let sec: u32 = c.get(6).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            if let Some(r) = ref_year {
                if (year - r).abs() > 5 {
                    continue;
                }
            }
            if let Some(dt) = NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|d| d.and_hms_opt(hour, min, sec))
            {
                out.push(VerifiedWake {
                    scheduled_at: dt,
                    owner_label: c[7].trim().to_string(),
                });
            }
        }
    }
    out
}

/// Parse `powercfg -waketimers`, extracting each timer's expiry + owner.
/// Port of `parsePowercfgWaketimers`, extended for the wake mechanism actually
/// in use.
///
/// `powercfg` labels a timer by who set it, and the two forms differ:
///
/// ```text
/// Timer set by [SYSTEM\TaskScheduler] … Reason: … 'NT TASK\SundayRec\SundayRec-Wake-1' …
/// Timer set by [PROCESS] \Device\HarddiskVolume3\Program Files\SundayRec\SundayRec.exe …
/// ```
///
/// The first is the old `Register-ScheduledTask` mechanism; the second is what a
/// `SetWaitableTimer` armed by the running process reports. The `[PROCESS]` form
/// carries no quotes at all, so the quoted-token branch alone would have labelled
/// every one of our own timers `unknown` — the verification panel would show the
/// timer but never recognise it as ours.
pub fn parse_powercfg_waketimers(stdout: &str) -> Vec<VerifiedWake> {
    static BLOCK_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n\s*\r?\n").unwrap());
    static EXPIRES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)expires\s+at\s+(\d{1,2}):(\d{2})(?::(\d{2}))?\s*(AM|PM)?\s+on\s+(\d{1,2})/(\d{1,2})/(\d{2,4})",
        )
        .unwrap()
    });
    static TASK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)['"]([^'"]*SundayRec[^'"]*)['"]"#).unwrap());
    static PROCESS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)set\s+by\s+\[PROCESS\]\s+(\S.*?)\s+expires").unwrap());
    static REASON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Reason:\s*(.+)").unwrap());
    let (block_split, expires, task, process, reason) =
        (&*BLOCK_SPLIT, &*EXPIRES, &*TASK, &*PROCESS, &*REASON);

    let mut out = Vec::new();
    for block in block_split.split(stdout) {
        let Some(c) = expires.captures(block) else {
            continue;
        };
        let mut hour: u32 = c[1].parse().unwrap_or(99);
        let min: u32 = c[2].parse().unwrap_or(99);
        let sec: u32 = c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        match c.get(4).map(|m| m.as_str().to_uppercase()).as_deref() {
            Some("PM") if hour < 12 => hour += 12,
            Some("AM") if hour == 12 => hour = 0,
            _ => {}
        }
        let month: u32 = c[5].parse().unwrap_or(0);
        let day: u32 = c[6].parse().unwrap_or(0);
        let mut year: i32 = c[7].parse().unwrap_or(0);
        if year < 100 {
            year += 2000;
        }
        let Some(dt) =
            NaiveDate::from_ymd_opt(year, month, day).and_then(|d| d.and_hms_opt(hour, min, sec))
        else {
            continue;
        };

        // Owner: task name from the quoted path, else the `[PROCESS]` executable,
        // else the Reason line, else 'unknown'.
        let owner = if let Some(t) = task.captures(block) {
            let path = &t[1];
            basename(path)
        } else if let Some(p) = process.captures(block) {
            basename(p[1].trim())
        } else if let Some(r) = reason.captures(block) {
            r[1].trim().chars().take(80).collect()
        } else {
            "unknown".to_string()
        };
        out.push(VerifiedWake {
            scheduled_at: dt,
            owner_label: owner,
        });
    }
    out
}

/// The last `\`- or `/`-separated component of a Windows path, or the whole
/// string when it has no separator (or ends in one).
fn basename(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Compare expected wakes to observed ones within `tolerance_ms`. Returns
/// `(has_mismatch, missing)`. Port of `compareExpectedToObserved`.
pub fn compare_expected_to_observed(
    expected: &[NaiveDateTime],
    observed: &[VerifiedWake],
    tolerance_ms: i64,
) -> (bool, Vec<NaiveDateTime>) {
    let mut missing = Vec::new();
    for exp in expected {
        let found = observed
            .iter()
            .any(|o| (o.scheduled_at - *exp).num_milliseconds().abs() <= tolerance_ms);
        if !found {
            missing.push(*exp);
        }
    }
    (!missing.is_empty(), missing)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Sleep-config + power-source parsing
// ─────────────────────────────────────────────────────────────────────────────

/// The sleep/power configuration the UI surfaces (with "fix" buttons). Mirrors
/// the Electron `SleepConfig`; every probe is optional so a partial read still
/// renders. `wakeTimersEnabled` is Windows-only; the mac fields are macOS-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "SleepConfig.ts")]
#[serde(rename_all = "camelCase")]
pub struct SleepConfig {
    // mac
    pub autopoweroff: Option<bool>,
    pub autopoweroff_delay: Option<i32>,
    pub standby: Option<bool>,
    pub standby_delay: Option<i32>,
    pub hibernate_mode: Option<i32>,
    // windows
    pub wake_timers_enabled: Option<bool>,
    // common
    pub error: Option<String>,
}

/// Read an integer `\b{key}\s+(\d+)` from `pmset -g` output.
fn pmset_int(stdout: &str, key: &str) -> Option<i32> {
    let re = Regex::new(&format!(r"\b{}\s+(\d+)", regex::escape(key))).unwrap();
    re.captures(stdout).and_then(|c| c[1].parse().ok())
}

/// Parse `pmset -g` into the macOS half of [`SleepConfig`]. Port of the darwin
/// branch of `getSleepConfig`.
pub fn parse_mac_sleep_config(stdout: &str) -> SleepConfig {
    SleepConfig {
        autopoweroff: pmset_int(stdout, "autopoweroff").map(|v| v == 1),
        autopoweroff_delay: Some(pmset_int(stdout, "autopoweroffdelay").unwrap_or(0)),
        standby: pmset_int(stdout, "standby").map(|v| v == 1),
        standby_delay: Some(pmset_int(stdout, "standbydelay").unwrap_or(0)),
        hibernate_mode: Some(pmset_int(stdout, "hibernatemode").unwrap_or(3)),
        wake_timers_enabled: None,
        error: None,
    }
}

/// Parse the "Allow wake timers" index from a `powercfg /query …` block →
/// `Some(true)` if enabled, `Some(false)` if disabled, `None` if not found.
/// Port of the win32 branch of `getSleepConfig`.
pub fn parse_win_wake_timers(stdout: &str) -> Option<bool> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)Current AC Power Setting Index:\s+(0x[0-9a-f]+)").unwrap()
    });
    let c = RE.captures(stdout)?;
    let val = i64::from_str_radix(c[1].trim_start_matches("0x"), 16).ok()?;
    Some(val > 0)
}

/// True if on battery, false if AC / no battery (desktop), `None` if unknown.
/// Port of `parsePmsetBatt`.
pub fn parse_pmset_batt(stdout: &str) -> Option<bool> {
    static AC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)AC\s*Power").unwrap());
    static BATT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Battery\s*Power").unwrap());
    static INTERNAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)InternalBattery").unwrap());
    static ANY_BATTERY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Battery").unwrap());
    if AC.is_match(stdout) {
        return Some(false);
    }
    if BATT.is_match(stdout) {
        return Some(true);
    }
    let has_battery = INTERNAL.is_match(stdout) || ANY_BATTERY.is_match(stdout);
    if !has_battery {
        return Some(false); // desktop → on AC
    }
    None
}

/// Parse `wmic path Win32_Battery get BatteryStatus` → on-battery? Port of
/// `parseWmicBatteryStatus`. 1 = discharging (on battery); 2+ = AC.
pub fn parse_wmic_battery_status(stdout: &str) -> Option<bool> {
    static NUM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)BatteryStatus\s*=\s*(\d+)").unwrap());
    static MENTIONED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)BatteryStatus\s*=").unwrap());
    if let Some(c) = NUM.captures(stdout) {
        return c[1].parse::<i32>().ok().map(|s| s == 1);
    }
    if MENTIONED.is_match(stdout) {
        return None; // mentioned but non-numeric → malformed
    }
    Some(false) // no battery row → desktop → on AC
}

/// True if macOS standby (deep sleep) is enabled — it can sabotage wake on Apple
/// Silicon. `None` if the line is absent. Port of `parsePmsetStandby`.
pub fn parse_pmset_standby(stdout: &str) -> Option<bool> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bstandby\s+(\d+)\b").unwrap());
    RE.captures(stdout).map(|c| &c[1] == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }
    fn dtm(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn capabilities_per_platform() {
        let arm = detect_capabilities(WakePlatform::MacArm);
        assert!(arm.can_wake_from_sleep && !arm.can_wake_from_off && arm.needs_admin);
        let intel = detect_capabilities(WakePlatform::MacIntel);
        assert!(intel.can_wake_from_off);
        let win = detect_capabilities(WakePlatform::Win);
        assert!(win.can_wake_from_sleep && !win.can_wake_from_off && !win.needs_admin);
        let lin = detect_capabilities(WakePlatform::Linux);
        assert!(!lin.can_wake_from_sleep);
        assert!(!detect_capabilities(WakePlatform::Other).can_wake_from_sleep);
    }

    #[test]
    fn wake_platform_serialises_to_electron_strings() {
        assert_eq!(
            serde_json::to_string(&WakePlatform::MacArm).unwrap(),
            "\"mac-arm\""
        );
        assert_eq!(
            serde_json::to_string(&WakePlatform::MacIntel).unwrap(),
            "\"mac-intel\""
        );
        assert_eq!(
            serde_json::to_string(&WakePlatform::Win).unwrap(),
            "\"win\""
        );
    }

    #[test]
    fn wake_points_subtracts_lead_and_drops_past() {
        let now = dtm("2026-06-07 09:00");
        let up = vec![
            dtm("2026-06-07 09:05"), // 09:05 − 10min = 08:55 < now → dropped
            dtm("2026-06-07 11:00"), // → 10:50 future → kept
            dtm("2026-06-14 11:00"), // → next week 10:50 → kept
        ];
        let wp = wake_points(&up, now, WAKE_LEAD_MINUTES);
        assert_eq!(wp, vec![dtm("2026-06-07 10:50"), dtm("2026-06-14 10:50")]);
    }

    #[test]
    fn should_block_within_30_min() {
        let now = dtm("2026-06-07 10:40");
        assert!(should_block(&[dtm("2026-06-07 11:00")], now)); // 20 min away
        assert!(!should_block(&[dtm("2026-06-07 11:30")], now)); // 50 min away
        assert!(!should_block(&[dtm("2026-06-07 10:00")], now)); // in the past
    }

    #[test]
    fn key_of_is_order_sensitive_join() {
        let a = key_of(&[dtm("2026-06-07 11:00"), dtm("2026-06-14 11:00")]);
        let b = key_of(&[dtm("2026-06-07 11:00")]);
        assert!(a.contains('|'));
        assert_ne!(a, b);
    }

    #[test]
    fn wake_points_empty_input_and_stable_dedup_key() {
        let now = dtm("2026-06-07 09:00");
        // No upcoming recordings → no wake points, and an empty, stable key (the
        // WakeEngine treats an unchanged empty schedule as a cheap no-op).
        assert!(wake_points(&[], now, WAKE_LEAD_MINUTES).is_empty());
        assert_eq!(key_of(&[]), "");

        // The same upcoming set yields the same points and therefore the same
        // dedup key on a repeated reschedule — the engine's no-op contract.
        let up = vec![dtm("2026-06-07 11:00"), dtm("2026-06-14 11:00")];
        let wp1 = wake_points(&up, now, WAKE_LEAD_MINUTES);
        let wp2 = wake_points(&up, now, WAKE_LEAD_MINUTES);
        assert_eq!(key_of(&wp1), key_of(&wp2));
        assert!(!wp1.is_empty());
    }

    #[test]
    fn decide_reschedule_skips_only_truly_unchanged() {
        use WakeRescheduleAction::*;
        // Unchanged non-empty set → skip.
        assert_eq!(
            decide_reschedule(Some("123"), "123", false, false),
            SkipUnchanged
        );
        // Changed set → apply.
        assert_eq!(decide_reschedule(Some("123"), "456", false, false), Apply);
        // Forced (user-initiated) always applies, even when unchanged.
        assert_eq!(decide_reschedule(Some("123"), "123", false, true), Apply);
    }

    #[test]
    fn decide_reschedule_clears_stale_when_set_goes_empty() {
        use WakeRescheduleAction::*;
        // The schedule emptied out but the OS still holds the old "123" wake →
        // we MUST apply so the cancel-then-register clears the stale timer.
        assert_eq!(decide_reschedule(Some("123"), "", true, false), Apply);
        // Already-empty (nothing to clear) → skip, no needless pmset call.
        assert_eq!(decide_reschedule(Some(""), "", true, false), SkipUnchanged);
        assert_eq!(decide_reschedule(None, "", true, false), SkipUnchanged);
    }

    #[test]
    fn decide_reschedule_reapplies_after_empty_then_readd() {
        use WakeRescheduleAction::*;
        // Regression for the silent-no-op race: after the set went empty and we
        // recorded the empty key, re-adding the original time must re-register
        // (the OS timer was cancelled when the set emptied).
        assert_eq!(decide_reschedule(Some(""), "123", false, false), Apply);
    }

    #[test]
    fn wake_points_drops_a_point_landing_exactly_on_now() {
        // A point whose lead-adjusted time equals `now` is dropped (strict `>`),
        // so we never schedule a wake for a moment already upon us.
        let now = dtm("2026-06-07 10:50");
        let up = vec![dtm("2026-06-07 11:00")]; // − 10min lead = 10:50 == now
        assert!(wake_points(&up, now, WAKE_LEAD_MINUTES).is_empty());
    }

    #[test]
    fn pmset_and_win_date_formats() {
        let d = dt("2026-05-31 10:30:00");
        assert_eq!(format_pmset_date(d), "05/31/26 10:30:00");
        assert_eq!(format_win_datetime(d), "2026-05-31T10:30:00");
    }

    #[test]
    fn classify_win_error_detects_permission() {
        assert_eq!(
            classify_win_error("Access is denied."),
            WinErrorKind::Permission
        );
        assert_eq!(
            classify_win_error("Unauthorized operation"),
            WinErrorKind::Permission
        );
        // The wording `powercfg` uses when the shell is not elevated — this is
        // the only remaining caller now that scheduling is an in-process timer.
        assert_eq!(
            classify_win_error("You do not have permission; run as administrator"),
            WinErrorKind::Permission
        );
        assert_eq!(
            classify_win_error("some other failure"),
            WinErrorKind::Error
        );
    }

    #[test]
    fn test_wake_delta_threshold() {
        assert_eq!(classify_test_wake_delta(10), TestWakeVerdict::Ok);
        assert_eq!(classify_test_wake_delta(30), TestWakeVerdict::Ok);
        assert_eq!(classify_test_wake_delta(31), TestWakeVerdict::TooLate);
        assert_eq!(classify_test_wake_delta(-5), TestWakeVerdict::Ok); // woke early
    }

    #[test]
    fn test_wake_outcome_maps_delta() {
        assert_eq!(test_wake_outcome(Some(10)), (WakeFailureKind::TestOk, None));
        assert_eq!(
            test_wake_outcome(Some(120)),
            (WakeFailureKind::TestFail, Some("too_late".into()))
        );
        assert_eq!(
            test_wake_outcome(None),
            (WakeFailureKind::TestFail, Some("no_resume".into()))
        );
    }

    fn fail_entry(ts: i64) -> WakeFailureEntry {
        WakeFailureEntry {
            timestamp: ts,
            scheduled_at: "2026-06-01T10:00:00Z".into(),
            kind: WakeFailureKind::TestOk,
            label: "Test-wake".into(),
            reason: None,
            delta_sec: Some(2),
        }
    }

    #[test]
    fn cap_failure_history_prepends_newest_first() {
        let h = vec![fail_entry(1), fail_entry(2)];
        let out = cap_failure_history(h, fail_entry(3));
        assert_eq!(out[0].timestamp, 3); // newest first
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn cap_failure_history_trims_to_max() {
        let mut h = Vec::new();
        for i in 0..WAKE_FAILURE_MAX {
            h.push(fail_entry(i as i64));
        }
        let out = cap_failure_history(h, fail_entry(999));
        assert_eq!(out.len(), WAKE_FAILURE_MAX);
        assert_eq!(out[0].timestamp, 999); // newest kept
        assert_eq!(out.last().unwrap().timestamp, (WAKE_FAILURE_MAX - 2) as i64);
        // oldest trimmed
    }

    #[test]
    fn parse_pmset_sched_captures_one_off_wakes() {
        let out = "\
Repeating power events:
  wake at 11:30AM every weekday

Scheduled power events:
 [0]  wake at 5/31/2026 10:30:00 by 'SundayRec'
 [1]  wake at 06/07/2026 10:30:00 by 'SundayRec'
";
        let wakes = parse_pmset_sched(out, Some(2026));
        assert_eq!(wakes.len(), 2);
        assert_eq!(wakes[0].scheduled_at, dt("2026-05-31 10:30:00"));
        assert_eq!(wakes[0].owner_label, "SundayRec");
        assert_eq!(wakes[1].scheduled_at, dt("2026-06-07 10:30:00"));
    }

    #[test]
    fn parse_pmset_sched_skips_repeating_and_bad_year() {
        // The repeating event must not be captured (it's outside the one-off section).
        let out = "Repeating power events:\n  wake at 11:30AM every weekday\n";
        assert!(parse_pmset_sched(out, Some(2026)).is_empty());
        // A wildly off year is rejected by the ref_year guard.
        let bad = "Scheduled power events:\n [0]  wake at 5/31/2099 10:30:00 by 'SundayRec'\n";
        assert!(parse_pmset_sched(bad, Some(2026)).is_empty());
    }

    #[test]
    fn parse_powercfg_waketimers_extracts_time_and_owner() {
        let out = "\
Timer set by [SYSTEM\\TaskScheduler] expires at 5:30:00 PM on 5/31/2026.
  Reason: Windows will execute 'NT TASK\\SundayRec\\SundayRec-Wake-1' scheduled task
";
        let wakes = parse_powercfg_waketimers(out);
        assert_eq!(wakes.len(), 1);
        assert_eq!(wakes[0].scheduled_at, dt("2026-05-31 17:30:00"));
        assert_eq!(wakes[0].owner_label, "SundayRec-Wake-1");
    }

    #[test]
    fn parse_powercfg_waketimers_names_the_owning_process() {
        // What a `SetWaitableTimer(fResume = TRUE)` armed by the running app
        // looks like: no quoted task path at all, just `[PROCESS] <exe path>`.
        // Before the `[PROCESS]` branch this block parsed as owner `unknown`,
        // so the verification panel could not tell our own timer from anyone's.
        let out = "Timer set by [PROCESS] \\Device\\HarddiskVolume3\\Program Files\\SundayRec\\SundayRec.exe expires at 10:20:00 AM on 5/31/2026.\n  Reason: Scheduled wake";
        let wakes = parse_powercfg_waketimers(out);
        assert_eq!(wakes.len(), 1);
        assert_eq!(wakes[0].scheduled_at, dt("2026-05-31 10:20:00"));
        assert_eq!(wakes[0].owner_label, "SundayRec.exe");
    }

    #[test]
    fn parse_powercfg_handles_am_and_no_timers() {
        let am = "Timer set by [X] expires at 12:05:00 AM on 1/2/2026.\n  Reason: 'SundayRec-Wake-2' task";
        let wakes = parse_powercfg_waketimers(am);
        assert_eq!(wakes[0].scheduled_at, dt("2026-01-02 00:05:00"));
        assert!(
            parse_powercfg_waketimers("There are no active wake timers in the system.").is_empty()
        );
    }

    #[test]
    fn compare_expected_to_observed_within_tolerance() {
        let expected = vec![dt("2026-05-31 10:30:00"), dt("2026-06-07 10:30:00")];
        let observed = vec![
            VerifiedWake {
                scheduled_at: dt("2026-05-31 10:30:30"), // 30 s off → within 60 s
                owner_label: "SundayRec".to_string(),
            },
            // second expected has no match
        ];
        let (mismatch, missing) =
            compare_expected_to_observed(&expected, &observed, WAKE_MATCH_TOLERANCE_MS);
        assert!(mismatch);
        assert_eq!(missing, vec![dt("2026-06-07 10:30:00")]);

        // Both present within tolerance → no mismatch.
        let observed2 = vec![
            VerifiedWake {
                scheduled_at: dt("2026-05-31 10:30:00"),
                owner_label: "x".into(),
            },
            VerifiedWake {
                scheduled_at: dt("2026-06-07 10:29:30"),
                owner_label: "x".into(),
            },
        ];
        let (m2, miss2) =
            compare_expected_to_observed(&expected, &observed2, WAKE_MATCH_TOLERANCE_MS);
        assert!(!m2);
        assert!(miss2.is_empty());
    }

    #[test]
    fn parse_mac_sleep_config_reads_pmset_g() {
        let out = "\
 autopoweroff         1
 autopoweroffdelay    28800
 standby              1
 standbydelay         86400
 hibernatemode        3
";
        let cfg = parse_mac_sleep_config(out);
        assert_eq!(cfg.autopoweroff, Some(true));
        assert_eq!(cfg.autopoweroff_delay, Some(28800));
        assert_eq!(cfg.standby, Some(true));
        assert_eq!(cfg.standby_delay, Some(86400));
        assert_eq!(cfg.hibernate_mode, Some(3));
        assert_eq!(cfg.wake_timers_enabled, None);
    }

    #[test]
    fn parse_win_wake_timers_reads_index() {
        let on = "Current AC Power Setting Index: 0x00000001";
        let off = "Current AC Power Setting Index: 0x00000000";
        assert_eq!(parse_win_wake_timers(on), Some(true));
        assert_eq!(parse_win_wake_timers(off), Some(false));
        assert_eq!(parse_win_wake_timers("no index here"), None);
    }

    #[test]
    fn parse_pmset_batt_distinguishes_sources() {
        // AC present → not on battery.
        assert_eq!(parse_pmset_batt("Now drawing from 'AC Power'"), Some(false));
        // "Battery Power" (no AC) → on battery.
        assert_eq!(
            parse_pmset_batt("Now drawing from 'Battery Power'"),
            Some(true)
        );
        // Desktop: no battery mentioned at all → on AC.
        assert_eq!(parse_pmset_batt("no power info"), Some(false));
    }

    #[test]
    fn parse_wmic_battery_status_values() {
        assert_eq!(parse_wmic_battery_status("BatteryStatus=1"), Some(true));
        assert_eq!(parse_wmic_battery_status("BatteryStatus=2"), Some(false));
        assert_eq!(parse_wmic_battery_status("BatteryStatus=abc"), None);
        assert_eq!(parse_wmic_battery_status("no battery row"), Some(false));
    }

    #[test]
    fn parse_pmset_standby_flag() {
        assert_eq!(parse_pmset_standby(" standby              1"), Some(true));
        assert_eq!(parse_pmset_standby(" standby              0"), Some(false));
        assert_eq!(parse_pmset_standby("no standby line"), None);
    }

    // ── The background reschedule's log gate ────────────────────────────────

    #[test]
    fn a_permission_failure_is_reported_once_instead_of_never() {
        // THE regression: the supervisor filtered `permission` (and its two
        // siblings) to silence, so the one failure that means "this machine will
        // sleep through the service" appeared in no log at all. First one is
        // written; the rest are counted.
        assert_eq!(
            background_wake_log_action(false, Some("permission"), 0),
            WakeLogAction::ReportOnce
        );
        for seen in [1, 99] {
            assert_eq!(
                background_wake_log_action(false, Some("permission"), seen),
                WakeLogAction::Suppress,
                "{seen} reports in"
            );
        }
    }

    #[test]
    fn the_expected_failures_share_the_one_report_between_them() {
        // One line per launch, not one per reason: the second expected failure
        // of any kind is still a repeat of "the unprivileged pass cannot arm
        // wakes on this machine".
        for reason in ["permission", "disabled", "cancelled"] {
            assert!(
                background_wake_log_action(false, Some(reason), 0).logs(),
                "{reason}"
            );
            assert!(
                !background_wake_log_action(false, Some(reason), 1).logs(),
                "{reason}"
            );
            // …and both spend from the same budget, which is what makes them
            // share it.
            assert!(background_wake_log_action(false, Some(reason), 0).counts());
            assert!(background_wake_log_action(false, Some(reason), 1).counts());
        }
    }

    #[test]
    fn a_real_failure_is_never_quietened_and_never_spends_the_budget() {
        // `unsupported`/`error` — and anything nobody has classified, which is
        // the reason most likely to be new — are logged every time. And they
        // must NOT count: a burst of them would otherwise silence the next
        // `permission`, which is the one failure the budget exists to show.
        for reason in [Some("unsupported"), Some("error"), Some("banana"), None] {
            let action = background_wake_log_action(false, reason, 5);
            assert_eq!(action, WakeLogAction::Report, "{reason:?}");
            assert!(action.logs(), "{reason:?} must always be logged");
            assert!(!action.counts(), "{reason:?} must not spend the budget");
        }
    }

    #[test]
    fn success_says_nothing() {
        assert_eq!(
            background_wake_log_action(true, None, 0),
            WakeLogAction::Silent
        );
        // Even a "reason" carried along with an ok result cannot make it noisy.
        assert_eq!(
            background_wake_log_action(true, Some("error"), 0),
            WakeLogAction::Silent
        );
        assert!(!WakeLogAction::Silent.logs() && !WakeLogAction::Silent.counts());
    }

    #[test]
    fn every_reason_survives_the_round_trip_through_the_wire() {
        // `from_wire` is what lets the supervisor reason in types instead of in
        // string literals; a variant it cannot parse would be treated as a real
        // failure and log on every pass.
        for r in [
            WakeErrorReason::Disabled,
            WakeErrorReason::Cancelled,
            WakeErrorReason::Permission,
            WakeErrorReason::Unsupported,
            WakeErrorReason::Error,
        ] {
            assert_eq!(WakeErrorReason::from_wire(r.as_str()), Some(r), "{r:?}");
        }
        assert_eq!(WakeErrorReason::from_wire("nonsense"), None);
    }

    // ── Why a successful reschedule armed nothing ───────────────────────────

    #[test]
    fn an_empty_wake_set_says_whether_the_plan_is_switched_off() {
        // «Registrer vekkinger» answering `ok: true, count: 0` is not an answer.
        assert_eq!(
            wake_idle_reason(false, 0),
            Some(WakeIdleReason::AutoRecordOff)
        );
        assert_eq!(
            wake_idle_reason(true, 0),
            Some(WakeIdleReason::NothingUpcoming)
        );
    }

    #[test]
    fn a_wake_set_with_something_in_it_needs_no_excuse() {
        // Specials are not gated by the level-1 switch, so "switched off" with
        // an upcoming concert is a perfectly ordinary armed set.
        assert_eq!(wake_idle_reason(true, 3), None);
        assert_eq!(wake_idle_reason(false, 1), None);
    }
}
