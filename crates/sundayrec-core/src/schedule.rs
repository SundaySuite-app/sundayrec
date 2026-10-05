//! The scheduler decision core (Fase 5.1) — pure, clock-free recurrence logic.
//!
//! Ported from the Electron main process `src/main/scheduler.ts`. That file is
//! the behavioural specification; this module rebuilds the *decisions* it makes
//! as deterministic Rust so they can be exercised entirely under `cargo test`,
//! with `now` passed in rather than read from the wall clock (the Electron
//! helpers already take `now: Date` for exactly this reason).
//!
//! What lives here (pure):
//!   - the [`ScheduleSlot`] / [`SpecialRecording`] types (serde-compatible with
//!     the Electron `types/index.ts` interfaces, so stored/exported profiles
//!     keep their meaning),
//!   - `HH:MM` / `YYYY-MM-DD` parsing with the Electron fallback defaults,
//!   - "is this slot/special active right now?" (the late-start window),
//!   - midnight-crossing and degenerate-slot detection,
//!   - the reminder / preflight *lead-time offset* math (with previous-day
//!     wrap-around),
//!   - "next occurrence" / "most-recent occurrence" of a weekly (weekday, time),
//!   - the upcoming-dates / next-recording selection used to drive wake
//!     scheduling and the tray tooltip,
//!   - special-recording pruning,
//!   - the missed-recording look-back decision (what to late-start, what to log).
//!
//! What stays in the `src-tauri` shell (impure): reading `Local::now()`, the
//! tokio timers that actually fire start/stop, sending notifications, and
//! persisting/triggering recordings. The shell converts real wall-clock time
//! (and stored history epoch-ms) into the `NaiveDateTime` local-wall frame this
//! module works in, so every comparison here is tz-free and reproducible.
//!
//! ## Weekday convention
//!
//! `ScheduleSlot.days` uses the UI convention **0 = Monday … 6 = Sunday**. That
//! happens to be exactly chrono's [`Weekday::num_days_from_monday`], so this
//! module operates directly in that space — there is no node-schedule
//! `0 = Sunday` conversion to mirror (node-schedule is the Electron timer engine,
//! not part of the decision logic).

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Default slot start time when the stored string is empty/malformed — mirrors
/// the Electron `(slot.start || '11:00')`.
pub const DEFAULT_START: &str = "11:00";
/// Default slot stop time — mirrors the Electron `(slot.stop || '12:00')`.
pub const DEFAULT_STOP: &str = "12:00";

/// Late-start grace window (ms): a slot whose start time passed at most this
/// long ago is still considered "active now". 5 min — the Electron
/// `slotActiveNow` default `windowMs`.
pub const DEFAULT_WINDOW_MS: i64 = 5 * 60_000;

/// Window for late-starting an in-progress slot on a missed-check pass (ms).
/// 60 min — a congregation that began 45 min late still gets the rest captured.
/// (`scheduler.ts` `MISSED_WINDOW_MS`.)
pub const MISSED_WINDOW_MS: i64 = 60 * 60_000;

/// How far back a missed-check looks for slots/specials that never ran (ms).
///
/// 7 days. The Electron build (and this one until runde 2 of `docs/VARSLING.md`)
/// looked back 24 h, so a machine that was switched off from Sunday until
/// Tuesday never said a word about the service it missed. A week covers the
/// ordinary case — the machine is next opened some day before the following
/// Sunday — and the `notify_seen` ledger keeps each occurrence to ONE
/// notification however many launches rediscover it (its retention,
/// `notify::seen::SEEN_RETENTION_MS` in the shell, is 8 days for exactly this).
pub const MISSED_LOG_WINDOW_MS: i64 = 7 * 24 * 60 * 60_000;

/// A history entry within ±this of an expected start "covers" it, so we don't
/// double-log a missed recording (ms). 30 min — the Electron `historyCovers`.
pub const HISTORY_COVER_MS: i64 = 30 * 60_000;

/// Background preflight runs this many minutes before a scheduled start, so the
/// user gets an alert in time to act. (`scheduler.ts` `PREFLIGHT_LEAD_MIN`.)
pub const PREFLIGHT_LEAD_MIN: i32 = 30;

// ── Scheduler-supervisor safeguards ──────────────────────────────────────────
//
// These bound the impure supervisor loop so a scheduled recording CANNOT be
// silently missed or run away. They are pure so the decisions are unit-tested.

/// The longest the supervisor sleeps before re-checking the WALL clock. A naive
/// `sleep(days)` is fragile: a tokio timer can drift or under-count across macOS
/// system-sleep, and a clock change (NTP/DST) during a multi-day wait makes the
/// recording fire late or never. Capping the wait means we re-evaluate against
/// the real clock at least this often, so the FINAL sleep before a recording is
/// always short + precise. 5 minutes balances responsiveness vs. churn.
pub const MAX_SUPERVISOR_SLEEP_MS: u64 = 5 * 60 * 1000;

/// Cap a computed wait (ms) so the supervisor re-checks the clock periodically.
pub fn capped_supervisor_sleep_ms(wait_ms: u64) -> u64 {
    wait_ms.min(MAX_SUPERVISOR_SLEEP_MS)
}

/// Whether a sleep of `capped_supervisor_sleep_ms(wait_ms)` will cover the WHOLE
/// remaining wait — i.e. the event is due when that sleep ends (so we should
/// fire), vs. a periodic re-check (so we should loop + recompute). Avoids needing
/// a second clock read after the sleep.
pub fn supervisor_should_fire(wait_ms: u64) -> bool {
    wait_ms <= MAX_SUPERVISOR_SLEEP_MS
}

/// A hard max-duration BACKSTOP (minutes) for a scheduled recording: even if the
/// scheduled Stop event is missed (the app/supervisor died after the start, the
/// machine slept through it), the recording can't run forever and fill the disk.
/// Longer than any realistic service; the user's chosen slot max wins when set.
pub const SCHEDULED_MAX_BACKSTOP_MINUTES: u32 = 240;

/// The manual-max a scheduled recording is started with: the user's slot `max`
/// when they set one (> 0), else the [`SCHEDULED_MAX_BACKSTOP_MINUTES`] safety net
/// so a missed Stop can never leave a recording running indefinitely.
pub fn scheduled_max_minutes(slot_max: u32) -> u32 {
    if slot_max > 0 {
        slot_max
    } else {
        SCHEDULED_MAX_BACKSTOP_MINUTES
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Types — serde-compatible with the Electron `types/index.ts` interfaces
// ─────────────────────────────────────────────────────────────────────────────

/// A weekly recurring recording window. Mirrors the Electron `ScheduleSlot`
/// (`types/index.ts:18`): `{ days: number[]; start: string; stop: string; max?: number }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ScheduleSlot.ts")]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSlot {
    /// Active weekdays, 0 = Monday … 6 = Sunday.
    #[serde(default)]
    pub days: Vec<u32>,
    /// Start time `HH:MM` (local wall clock).
    #[serde(default = "default_start")]
    pub start: String,
    /// Stop time `HH:MM` (local wall clock). May be < start (crosses midnight).
    #[serde(default = "default_stop")]
    pub stop: String,
    /// Optional hard cap in minutes for this slot's recording length.
    #[serde(default)]
    pub max: Option<i32>,
}

/// A one-off dated recording. Mirrors the Electron `SpecialRecording`
/// (`types/index.ts:25`): `{ id?; date; name; start; stop; deviceId? }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "SpecialRecording.ts")]
#[serde(rename_all = "camelCase")]
pub struct SpecialRecording {
    /// Stable id (UI-generated), optional for older stored entries.
    #[serde(default)]
    pub id: Option<String>,
    /// Calendar date `YYYY-MM-DD`.
    #[serde(default)]
    pub date: String,
    /// Human-readable label shown in history / notifications.
    #[serde(default)]
    pub name: String,
    /// Start time `HH:MM`.
    #[serde(default = "default_start")]
    pub start: String,
    /// Stop time `HH:MM`.
    #[serde(default = "default_stop")]
    pub stop: String,
    /// Optional capture-device override for this recording.
    #[serde(default)]
    pub device_id: Option<String>,
}

fn default_start() -> String {
    DEFAULT_START.to_string()
}
fn default_stop() -> String {
    DEFAULT_STOP.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
//   Parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `"HH:MM"` → `(hour, minute)`, falling back to `fallback` when the
/// string is empty or either component is missing/non-numeric.
///
/// The Electron code does `(slot.start || '11:00').split(':').map(Number)` and
/// passes the result to `Date.setHours`. We are stricter about garbage (a
/// half-formed `"9"` becomes the fallback rather than an `Invalid Date`), but
/// agree on every well-formed `HH:MM` and on the empty-string default.
pub fn parse_hm(s: &str, fallback: (u32, u32)) -> (u32, u32) {
    if s.trim().is_empty() {
        return fallback;
    }
    let mut parts = s.split(':');
    let h = parts.next().and_then(|p| p.trim().parse::<u32>().ok());
    let m = parts.next().and_then(|p| p.trim().parse::<u32>().ok());
    match (h, m) {
        (Some(h), Some(m)) if h < 24 && m < 60 => (h, m),
        _ => fallback,
    }
}

fn parse_date(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// Combine a `YYYY-MM-DD` date and `HH:MM` time into a wall-clock datetime,
/// using the Electron `new Date(`${date}T${time || '11:00'}`)` fallback.
pub fn parse_date_time(date: &str, time: &str, fallback: (u32, u32)) -> Option<NaiveDateTime> {
    let d = parse_date(date)?;
    let (h, m) = parse_hm(time, fallback);
    d.and_hms_opt(h, m, 0)
}

/// Weekday of `dt` in the UI convention (0 = Monday … 6 = Sunday).
pub fn weekday_mon0(dt: NaiveDateTime) -> u32 {
    dt.weekday().num_days_from_monday()
}

fn at_time(dt: NaiveDateTime, h: u32, m: u32) -> Option<NaiveDateTime> {
    dt.date().and_hms_opt(h, m, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Active-now (late-start) detection
// ─────────────────────────────────────────────────────────────────────────────

/// True if `now` falls within `[start, start + window]` on one of `days` and is
/// still before `stop`. Direct port of `scheduler.ts` `slotActiveNow`.
pub fn slot_active_now(
    start: &str,
    stop: &str,
    days: &[u32],
    now: NaiveDateTime,
    window_ms: i64,
) -> bool {
    let (sh, sm) = parse_hm(start, (11, 0));
    let (eh, em) = parse_hm(stop, (12, 0));
    let crosses = crosses_midnight(start, stop);
    let today = weekday_mon0(now);
    for &d in days {
        // The slot's start day: either today, or — for a crossing slot where
        // `now` is in the early hours — YESTERDAY (the 23:00–01:00 slot is
        // still active at 00:10). The Electron port checked only `today == d`
        // with a same-day stop, so a crossing slot could never late-start:
        // `now < stop_t` went false the instant `now >= start`.
        let start_t = if today == d {
            at_time(now, sh, sm)
        } else if crosses && weekday_mon0(now - Duration::days(1)) == d {
            at_time(now - Duration::days(1), sh, sm)
        } else {
            continue;
        };
        let Some(start_t) = start_t else { continue };
        // The stop belongs to the day AFTER the start for a crossing slot.
        let stop_t =
            at_time(start_t, eh, em).map(|t| if crosses { t + Duration::days(1) } else { t });
        let Some(stop_t) = stop_t else { continue };
        let late = (now - start_t).num_milliseconds();
        if late >= 0 && late <= window_ms && now < stop_t {
            return true;
        }
    }
    false
}

/// True if `now` falls within `[start, start + window]` for a dated special and
/// is still before `stop`. Direct port of `scheduler.ts` `specialActiveNow`.
pub fn special_active_now(
    date: &str,
    start: &str,
    stop: &str,
    now: NaiveDateTime,
    window_ms: i64,
) -> bool {
    let (Some(start_dt), Some(stop_dt)) = (
        parse_date_time(date, start, (11, 0)),
        parse_date_time(date, stop, (12, 0)),
    ) else {
        return false;
    };
    // A stop at-or-before the start means the special runs past midnight — the
    // stop belongs to the NEXT day (same rule the slot event builder applies).
    let stop_dt = if stop_dt <= start_dt {
        stop_dt + Duration::days(1)
    } else {
        stop_dt
    };
    let late = (now - start_dt).num_milliseconds();
    late >= 0 && late <= window_ms && now < stop_dt
}

// ─────────────────────────────────────────────────────────────────────────────
//   Slot shape helpers
// ─────────────────────────────────────────────────────────────────────────────

/// True if the stop time is earlier in the day than the start time, i.e. the
/// recording runs past midnight into the next day. (`scheduler.ts:87`.)
pub fn crosses_midnight(start: &str, stop: &str) -> bool {
    let (sh, sm) = parse_hm(start, (11, 0));
    let (eh, em) = parse_hm(stop, (12, 0));
    eh < sh || (eh == sh && em < sm)
}

/// True if start == stop. Such a slot is rejected by the scheduler because the
/// crosses-midnight branch would otherwise turn it into a 24-h recording
/// (`scheduler.ts:73`). The UI blocks it; this guards direct/imported edits.
pub fn is_degenerate(start: &str, stop: &str) -> bool {
    let (sh, sm) = parse_hm(start, (11, 0));
    let (eh, em) = parse_hm(stop, (12, 0));
    sh == eh && sm == em
}

// ─────────────────────────────────────────────────────────────────────────────
//   Lead-time offset (reminder + background preflight)
// ─────────────────────────────────────────────────────────────────────────────

/// A weekday/time computed by subtracting a lead from a start time, wrapping to
/// the previous day when the lead crosses midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadEvent {
    /// Weekday 0 = Monday … 6 = Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Given a start at `(sh, sm)` on `weekday`, return the (weekday, hour, minute)
/// of the event `lead_min` minutes earlier. If the subtraction pushes before
/// 00:00 the weekday shifts back one day. Direct port of the reminder/preflight
/// offset math at `scheduler.ts:127` and `:142`.
pub fn lead_event(weekday: u32, sh: u32, sm: u32, lead_min: i32) -> LeadEvent {
    let total = sh as i32 * 60 + sm as i32 - lead_min;
    let norm = total.rem_euclid(1440);
    let h = (norm / 60) as u32;
    let m = (norm % 60) as u32;
    let wd = if total < 0 {
        (weekday + 6) % 7
    } else {
        weekday
    };
    LeadEvent {
        weekday: wd,
        hour: h,
        minute: m,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Occurrence math
// ─────────────────────────────────────────────────────────────────────────────

/// The earliest datetime > `now` that lands on `weekday` (0 = Mon … 6 = Sun) at
/// `h:m`. If today is the weekday but the time has already passed, returns the
/// occurrence one week out. Mirrors what node-schedule's `nextInvocation` would
/// report for a weekly rule.
pub fn next_occurrence(weekday: u32, h: u32, m: u32, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let today = weekday_mon0(now);
    let mut days_ahead = (weekday as i64 - today as i64).rem_euclid(7);
    let today_at = at_time(now, h, m)?;
    if days_ahead == 0 && today_at <= now {
        days_ahead = 7;
    }
    (now.date() + Duration::days(days_ahead)).and_hms_opt(h, m, 0)
}

/// The latest datetime ≤ `now` that lands on `weekday` at `h:m`. Direct port of
/// `scheduler.ts` `mostRecentOccurrence`.
pub fn most_recent_occurrence(
    weekday: u32,
    h: u32,
    m: u32,
    now: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let today = weekday_mon0(now);
    let mut days_back = (today as i64 - weekday as i64).rem_euclid(7);
    let today_at = at_time(now, h, m)?;
    if days_back == 0 && today_at > now {
        days_back = 7;
    }
    (now.date() - Duration::days(days_back)).and_hms_opt(h, m, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Upcoming / next selection
// ─────────────────────────────────────────────────────────────────────────────

/// All future START occurrences (one per active slot-weekday, plus each future
/// special) within `days_ahead` days of `now`, sorted ascending. Drives wake
/// scheduling and the "next 14 days" UI. Mirrors `getUpcomingDates`: one entry
/// per recurrence job (its single next invocation), bounded by the cutoff.
pub fn upcoming_dates(
    slots: &[ScheduleSlot],
    specials: &[SpecialRecording],
    now: NaiveDateTime,
    days_ahead: i64,
) -> Vec<NaiveDateTime> {
    let cutoff = now + Duration::days(days_ahead);
    let mut out: Vec<NaiveDateTime> = Vec::new();

    for slot in slots {
        if is_degenerate(&slot.start, &slot.stop) {
            continue;
        }
        let (sh, sm) = parse_hm(&slot.start, (11, 0));
        for &d in &slot.days {
            if let Some(inv) = next_occurrence(d, sh, sm, now) {
                if inv > now && inv < cutoff {
                    out.push(inv);
                }
            }
        }
    }
    for sp in specials {
        if let Some(start) = parse_date_time(&sp.date, &sp.start, (11, 0)) {
            if start > now && start < cutoff {
                out.push(start);
            }
        }
    }
    out.sort();
    out
}

/// The single nearest future START across all slots and specials, or `None` if
/// nothing is scheduled ahead. Port of `getNextRecording` (minus the job key).
pub fn next_recording(
    slots: &[ScheduleSlot],
    specials: &[SpecialRecording],
    now: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let mut best: Option<NaiveDateTime> = None;
    let mut consider = |dt: NaiveDateTime| {
        if dt > now && best.map(|b| dt < b).unwrap_or(true) {
            best = Some(dt);
        }
    };
    for slot in slots {
        if is_degenerate(&slot.start, &slot.stop) {
            continue;
        }
        let (sh, sm) = parse_hm(&slot.start, (11, 0));
        for &d in &slot.days {
            if let Some(inv) = next_occurrence(d, sh, sm, now) {
                consider(inv);
            }
        }
    }
    for sp in specials {
        if let Some(start) = parse_date_time(&sp.date, &sp.start, (11, 0)) {
            consider(start);
        }
    }
    best
}

// ─────────────────────────────────────────────────────────────────────────────
//   Upcoming-event enumeration (drives the supervisor timer loop)
// ─────────────────────────────────────────────────────────────────────────────

/// What a [`ScheduledEvent`] tells the supervisor to do when its time arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledEventKind {
    /// Begin recording the source slot/special.
    Start,
    /// Stop the current recording.
    Stop,
    /// Fire the "recording starts in N minutes" reminder notification.
    Reminder,
    /// Run the background preflight check (`PREFLIGHT_LEAD_MIN` before start).
    Preflight,
}

impl ScheduledEventKind {
    /// Firing order among events that share the same instant: a Stop goes
    /// first so a recording that ends exactly when the next one begins (a
    /// 11:00–12:30 slot and a 12:30–13:30 special) is closed before the
    /// follow-up Start looks for a free recorder; the lead-in notices come last.
    fn fire_rank(self) -> u8 {
        match self {
            ScheduledEventKind::Stop => 0,
            ScheduledEventKind::Start => 1,
            ScheduledEventKind::Reminder => 2,
            ScheduledEventKind::Preflight => 3,
        }
    }
}

/// A single timed action the scheduler supervisor should perform. The shell
/// sorts these, sleeps until the nearest, fires it, then recomputes — so this
/// enumeration replaces the per-job node-schedule timers from the Electron
/// build with one deterministic, testable list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledEvent {
    /// When to fire (local wall clock).
    pub at: NaiveDateTime,
    /// What to do.
    pub kind: ScheduledEventKind,
    /// Which slot/special this came from (index into the input slices).
    pub source: TriggerKind,
}

/// Enumerate every START/STOP/REMINDER/PREFLIGHT moment within `horizon_days`
/// of `now`, sorted ascending by time (and, within the same instant, Stop <
/// Start < Reminder < Preflight). The supervisor sleeps until the first entry,
/// fires every event that is due ([`events_due`]), and re-enumerates from the
/// moment it fired ([`enumeration_base`]) — so a fired event naturally drops
/// off (its next occurrence rolls a week/horizon out) while a sibling at the
/// same instant is not mistaken for the past. Mirrors the set of timers
/// `reschedule()` registers in `scheduler.ts`, minus the DST-gap *warning*
/// (which is a node-schedule artefact handled in the shell, not a decision).
///
/// - Degenerate slots (start == stop) contribute nothing (Electron skips them).
/// - `reminder_min == 0` suppresses REMINDER events (the Electron `reminderMin > 0` guard).
/// - STOP events are emitted for the slot/special's stop time even when the
///   matching start has already passed, exactly like the Electron stop job —
///   so an app launched mid-service still stops on time. `STOP` is idempotent
///   in the shell (a no-op when nothing is recording).
pub fn upcoming_events(
    slots: &[ScheduleSlot],
    specials: &[SpecialRecording],
    now: NaiveDateTime,
    reminder_min: i32,
    horizon_days: i64,
) -> Vec<ScheduledEvent> {
    let cutoff = now + Duration::days(horizon_days);
    let mut out: Vec<ScheduledEvent> = Vec::new();

    let mut push = |at: Option<NaiveDateTime>, kind, source| {
        if let Some(at) = at {
            if at > now && at < cutoff {
                out.push(ScheduledEvent { at, kind, source });
            }
        }
    };

    for (i, slot) in slots.iter().enumerate() {
        if is_degenerate(&slot.start, &slot.stop) {
            continue;
        }
        let (sh, sm) = parse_hm(&slot.start, (11, 0));
        let (eh, em) = parse_hm(&slot.stop, (12, 0));
        let crosses = crosses_midnight(&slot.start, &slot.stop);
        let src = TriggerKind::Slot(i);
        for &d in &slot.days {
            push(
                next_occurrence(d, sh, sm, now),
                ScheduledEventKind::Start,
                src,
            );
            let stop_wd = if crosses { (d + 1) % 7 } else { d };
            push(
                next_occurrence(stop_wd, eh, em, now),
                ScheduledEventKind::Stop,
                src,
            );
            if reminder_min > 0 {
                let le = lead_event(d, sh, sm, reminder_min);
                push(
                    next_occurrence(le.weekday, le.hour, le.minute, now),
                    ScheduledEventKind::Reminder,
                    src,
                );
            }
            let pf = lead_event(d, sh, sm, PREFLIGHT_LEAD_MIN);
            push(
                next_occurrence(pf.weekday, pf.hour, pf.minute, now),
                ScheduledEventKind::Preflight,
                src,
            );
        }
    }

    for (i, sp) in specials.iter().enumerate() {
        let src = TriggerKind::Special(i);
        let start = parse_date_time(&sp.date, &sp.start, (11, 0));
        push(start, ScheduledEventKind::Start, src);
        // A stop at-or-before the start crosses midnight → next day. Slots
        // already shift their stop weekday; specials parsed the stop onto the
        // SAME date, which put it before the start (an idempotent no-op) — a
        // 23:00–00:30 special ran to the 240-min backstop instead of 1.5 h.
        let stop = parse_date_time(&sp.date, &sp.stop, (12, 0)).map(|t| match start {
            Some(s) if t <= s => t + Duration::days(1),
            _ => t,
        });
        push(stop, ScheduledEventKind::Stop, src);
        if let Some(start) = start {
            if reminder_min > 0 {
                push(
                    Some(start - Duration::minutes(reminder_min as i64)),
                    ScheduledEventKind::Reminder,
                    src,
                );
            }
            push(
                Some(start - Duration::minutes(PREFLIGHT_LEAD_MIN as i64)),
                ScheduledEventKind::Preflight,
                src,
            );
        }
    }

    // Stable and total: events at the same instant fire in a fixed order,
    // whatever order the slots/specials happened to be listed in.
    out.sort_by_key(|e| (e.at, e.kind.fire_rank()));
    out
}

/// Every event in `events` (sorted, as [`upcoming_events`] returns them) that
/// is due at `fire_at`, in firing order. The supervisor fires ALL of them in
/// one go: firing only `events.first()` and re-enumerating from «now» made the
/// siblings that share its instant look like the past, so a recording starting
/// the moment another one ends was never started.
pub fn events_due(events: &[ScheduledEvent], fire_at: NaiveDateTime) -> Vec<ScheduledEvent> {
    events
        .iter()
        .take_while(|e| e.at <= fire_at)
        .cloned()
        .collect()
}

/// Whether a due event still fires after the late-start net (`check_missed`)
/// has just run on waking from an oversleep. The net already started whatever
/// is inside its window, so a stale Start would be a false «skipped» or a
/// second start, and a stale Reminder/Preflight is noise; a Stop still fires,
/// so a recording the net started late is not left to the max-duration
/// backstop. Without the net (a normal wake) everything due fires.
pub fn fire_after_missed_net(kind: ScheduledEventKind, net_ran: bool) -> bool {
    !net_ran || kind == ScheduledEventKind::Stop
}

/// The longest a scheduled Start waits for a Stop fired just before it to
/// release the recorder: the stop's finalise bound plus a margin.
pub const STOP_SETTLE_MS: u64 = crate::timeouts::RecorderTimeouts::STOP_FINALIZE_MS + 15_000;

/// How far from the wall clock a recorded fire time may be and still anchor
/// the next enumeration. Past it the fire is old news (or the clock jumped).
/// Derived from [`STOP_SETTLE_MS`]: a fire group that waited out a whole
/// Stop→Start settle ends that long after its anchor, and an event that came
/// due meanwhile must still count as unfired.
pub const FIRE_ANCHOR_WINDOW_SECS: i64 = (STOP_SETTLE_MS / 1000) as i64 + 60;

/// The instant [`upcoming_events`] should enumerate from. After a fire it is
/// the fire time rather than `now`: anything strictly after it has not been
/// fired yet — including an event that came due while the fire itself was
/// busy (a start can take seconds) — and nothing at or before it is fired
/// twice. Without a recent fire it is simply `now`.
pub fn enumeration_base(now: NaiveDateTime, fired_through: Option<NaiveDateTime>) -> NaiveDateTime {
    match fired_through {
        Some(f) if (f - now).num_seconds().abs() <= FIRE_ANCHOR_WINDOW_SECS => f,
        _ => now,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Special pruning
// ─────────────────────────────────────────────────────────────────────────────

/// Drop specials whose stop time ended more than 7 days before `now`, so the
/// stored list doesn't grow unbounded. Port of the prune pass at
/// `scheduler.ts:154`.
///
/// Deviation from Electron (intentional): a special whose `date`/`stop` can't be
/// parsed is **kept**, not silently dropped. The Electron `new Date('…')`
/// produces an `Invalid Date` that fails the `>=` test and is pruned; we'd
/// rather not delete a user's entry over a parse hiccup. The UI/validation keeps
/// malformed entries from being created in the first place.
pub fn prune_specials(
    specials: &[SpecialRecording],
    now: NaiveDateTime,
) -> (Vec<SpecialRecording>, usize) {
    let threshold = now - Duration::days(7);
    let kept: Vec<SpecialRecording> = specials
        .iter()
        .filter(|s| {
            parse_date_time(&s.date, &s.stop, (12, 0))
                .map(|stop| stop >= threshold)
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    let pruned = specials.len() - kept.len();
    (kept, pruned)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Missed-recording look-back + active-trigger detection
// ─────────────────────────────────────────────────────────────────────────────

/// Which scheduled item a missed-check found active right now, with the dedup
/// key the [`missed_recordings`] pass uses to avoid double-counting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTrigger {
    /// Index into the originating slots/specials slice.
    pub kind: TriggerKind,
    /// Stable dedup key shared with [`missed_recordings`].
    pub key: String,
}

/// Whether an [`ActiveTrigger`] came from a weekly slot or a dated special.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Slot(usize),
    Special(usize),
}

/// Dedup key for a slot occurrence — `slot:<weekday>:<start>-<stop>`. Internal
/// to this module (both [`active_within`] and [`missed_recordings`] use it), so
/// the exact shape only has to be self-consistent.
fn slot_key(slot: &ScheduleSlot, when: NaiveDateTime) -> String {
    format!("slot:{}:{}-{}", weekday_mon0(when), slot.start, slot.stop)
}

fn special_key(sp: &SpecialRecording) -> String {
    format!("special:{}:{}", sp.date, sp.start)
}

/// Slots/specials whose start time is within the late-start `window_ms` of
/// `now`. ALL of them, deliberately: every occurrence in here has been *handled*
/// by this pass and must therefore be fed to [`missed_recordings`] as
/// `triggered`, whether or not it is the one that got the microphone.
///
/// Which single one that is, is [`late_start_choice`]'s decision — see there for
/// why "each of these is started" was never a thing the recorder could do.
/// Mirrors the trigger half of `checkMissedRecordings`.
pub fn active_within(
    slots: &[ScheduleSlot],
    specials: &[SpecialRecording],
    now: NaiveDateTime,
    window_ms: i64,
) -> Vec<ActiveTrigger> {
    let mut out = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        if slot_active_now(&slot.start, &slot.stop, &slot.days, now, window_ms) {
            out.push(ActiveTrigger {
                kind: TriggerKind::Slot(i),
                key: slot_key(slot, now),
            });
        }
    }
    for (i, sp) in specials.iter().enumerate() {
        if special_active_now(&sp.date, &sp.start, &sp.stop, now, window_ms) {
            out.push(ActiveTrigger {
                kind: TriggerKind::Special(i),
                key: special_key(sp),
            });
        }
    }
    out
}

/// The ONE trigger a late-start pass may act on — the first of `active`, or
/// `None` when the recorder is already busy.
///
/// ## Why one, and why this is a decision rather than a loop
///
/// The recorder has a single session. `RecorderEngine::start` begins by stopping
/// whatever is running, so "start each active trigger" does not mean two
/// recordings — it means the second start *kills* the first, and the church is
/// left with a 200 ms fragment plus a recording that began late by however long
/// the first take lasted. That is exactly what happened at 11:20 on a Sunday
/// with a weekly slot and a hand-entered special at the same time: `check_missed`
/// read the engine state ONCE before its loop, so both triggers passed a guard
/// that had gone stale the moment the first one started (F1 finding A4).
///
/// Re-reading the state inside the loop would have fixed the symptom. Returning
/// at most one trigger fixes the shape: there is no longer a loop for a later
/// edit to re-break, and the invariant "a pass starts at most one recording" is
/// a property of this function that a test can hold, instead of a discipline the
/// shell has to keep.
///
/// ## Which one
///
/// The FIRST — which, given [`active_within`]'s order, means a weekly slot beats
/// a special at the same minute. Not because a slot deserves it, but because
/// that is what the on-time path already does: [`upcoming_events`] sorts stably
/// by time with slots pushed first, so `fire()` starts the slot and skips the
/// special as "a recording is already active". A service must produce the same
/// file whether the app was running at 11:00 or launched at 11:20.
///
/// (Note that the buggy behaviour produced the opposite — the special, started
/// second, was the survivor. Aligning on the on-time path therefore changes
/// which of two same-minute entries names the file. Two entries for one service
/// is a schedule mistake either way; the app now makes the same mistake twice
/// instead of two different ones, and stops destroying a take to do it.)
///
/// `already_recording` is the caller's FRESH reading of the engine — the whole
/// point of the finding is that a stale one is worthless.
pub fn late_start_choice(
    active: &[ActiveTrigger],
    already_recording: bool,
) -> Option<&ActiveTrigger> {
    if already_recording {
        return None;
    }
    active.first()
}

/// A stretch of wall-clock time during which a recording is KNOWN to have been
/// running, even though no history row says so yet.
///
/// Reconstructed from a crash-recovery manifest that startup recovery has not
/// finished with: `start` is the session's own start, `last_seen` the newest
/// evidence on disk that it was still writing (see
/// `recorder::recovery::pending_windows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveredWindow {
    /// When the interrupted session started.
    pub start: NaiveDateTime,
    /// The last moment it was demonstrably still recording. Never before
    /// `start`.
    pub last_seen: NaiveDateTime,
}

/// True if `when` falls inside any window — either close enough to a window's
/// start to be the same occurrence, or inside a stretch that was demonstrably
/// still recording.
///
/// Two clauses because a window answers two different questions. The first
/// mirrors [`history_covers`] exactly (±[`HISTORY_COVER_MS`] of the start),
/// because that is the row this recovery is *about to write*: a pending window
/// is a history entry that has not landed yet. The second catches the occurrence
/// that started while an earlier, longer recording was still running — a special
/// at 11:30 inside an 11:00–12:30 take.
///
/// There is deliberately NO grace after `last_seen`: a recording that ended at
/// 10:00 says nothing about a service at 10:20, and a tail grace here would
/// silently suppress a genuinely missed one.
fn windows_cover(windows: &[CoveredWindow], when: NaiveDateTime) -> bool {
    windows.iter().any(|w| {
        (w.start - when).num_milliseconds().abs() < HISTORY_COVER_MS
            || (when >= w.start && when <= w.last_seen)
    })
}

/// A scheduled recording that the missed-check determined never ran and is too
/// stale to late-start — the shell tells the operator natively and, when a wake
/// was supposed to get the machine up for it, logs it to the wake-failure ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedRecording {
    /// The expected (wall-clock) start time.
    pub when: NaiveDateTime,
    /// The CANONICAL label — Norwegian, frozen. Hashed into the durable
    /// `notify_seen` key that makes the missed notice fire once, so it must not
    /// follow the UI language (see `sundayrec_core::alerts`'s header). What a
    /// person reads is built from [`Self::kind`] instead.
    pub label: String,
    /// What the occurrence was, for wording it in the volunteer's language
    /// (`alerts::missed_label`).
    pub kind: MissedKind,
}

/// What a [`MissedRecording`] was — the facts its sentence is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissedKind {
    /// A weekly slot, by its own start/stop (`"11:00"`, `"13:00"`).
    Weekly { start: String, stop: String },
    /// A dated special recording; `None` when it was never given a name.
    Special { name: Option<String> },
}

/// Decide which slots/specials in the last 24 h should be logged as *missed*:
/// their start time is older than the late-start window (can't be run now) but
/// recent enough to matter, isn't already covered by a `triggered` late-start,
/// isn't already present in `history` (within ±30 min), and isn't inside a
/// `covered` window. Direct port of `scheduler.ts` `logMissedRecordings`, plus
/// the `covered` clause Electron never had.
///
/// ## Why `covered` exists (F1 finding A10)
///
/// `history` is the database, and after a crash the database is *behind the
/// truth*. The recording ran; the row that says so is written by startup
/// recovery, which has to concat the fragments first — minutes of ffmpeg for a
/// three-hour service. The missed-check runs at startup too, from its own task,
/// and asked the database a question recovery had not finished answering. The
/// answer was "nothing recorded on Sunday", and that answer becomes a native
/// notification telling a volunteer their recording was lost while it is, in
/// fact, being salvaged in the next process over.
///
/// `covered` is the evidence recovery has not written down yet: one window per
/// unfinalised manifest still on disk. Waiting for recovery to finish instead
/// was considered and rejected — it would gate the late-start net (the thing
/// that rescues the *second half of the sermon*) behind a concat that can take
/// minutes, so an app relaunched at 11:20 would sit still until 11:30. The late
/// start after a crash is WANTED; only the false "was not recorded" is the bug.
///
/// `history`, `covered` and `now` are in the same local-wall `NaiveDateTime`
/// frame — the shell converts stored epoch-ms into local time before calling, so
/// every comparison here is tz-free.
pub fn missed_recordings(
    slots: &[ScheduleSlot],
    specials: &[SpecialRecording],
    now: NaiveDateTime,
    history: &[NaiveDateTime],
    covered: &[CoveredWindow],
    triggered: &HashSet<String>,
) -> Vec<MissedRecording> {
    let mut out = Vec::new();
    let start_cutoff = now - Duration::milliseconds(MISSED_LOG_WINDOW_MS);

    for slot in slots {
        let (sh, sm) = parse_hm(&slot.start, (11, 0));
        for &d in &slot.days {
            let Some(candidate) = most_recent_occurrence(d, sh, sm, now) else {
                continue;
            };
            let age = (now - candidate).num_milliseconds();
            if age <= MISSED_WINDOW_MS {
                continue; // still inside late-start window
            }
            if candidate < start_cutoff {
                continue; // older than 24 h
            }
            if triggered.contains(&slot_key(slot, candidate)) {
                continue;
            }
            if history_covers(history, candidate) {
                continue;
            }
            if windows_cover(covered, candidate) {
                continue;
            }
            out.push(MissedRecording {
                when: candidate,
                label: format!("Ukentlig opptak ({}–{})", slot.start, slot.stop),
                kind: MissedKind::Weekly {
                    start: slot.start.clone(),
                    stop: slot.stop.clone(),
                },
            });
        }
    }

    for sp in specials {
        let Some(start) = parse_date_time(&sp.date, &sp.start, (11, 0)) else {
            continue;
        };
        let age = (now - start).num_milliseconds();
        if age <= MISSED_WINDOW_MS {
            continue;
        }
        if start < start_cutoff {
            continue;
        }
        if triggered.contains(&special_key(sp)) {
            continue;
        }
        if history_covers(history, start) {
            continue;
        }
        if windows_cover(covered, start) {
            continue;
        }
        let named = !sp.name.trim().is_empty();
        let label = if named {
            sp.name.clone()
        } else {
            "Spesialopptak".to_string()
        };
        out.push(MissedRecording {
            when: start,
            label,
            kind: MissedKind::Special {
                name: named.then(|| sp.name.clone()),
            },
        });
    }

    out
}

/// True if any history start time is within ±[`HISTORY_COVER_MS`] of `when`.
fn history_covers(history: &[NaiveDateTime], when: NaiveDateTime) -> bool {
    history
        .iter()
        .any(|&h| (h - when).num_milliseconds().abs() < HISTORY_COVER_MS)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Special device override
// ─────────────────────────────────────────────────────────────────────────────
//
// A special recording may name its own capture device (a wedding on a USB mic
// while the weekly service records from the mixer). The special stores the id
// the renderer's device picker uses; the recorder opens devices by NAME. These
// are the pure halves of turning one into the other — the enumeration itself
// and the operator warning live in the `src-tauri` scheduler.
//
// The Sunday-critical rule is the shape of every function below: a weekly slot,
// or a special WITHOUT a device, never reaches any of this beyond
// [`special_device_wanted`] answering `None`, so its recording is composed from
// exactly the settings it always was.

/// The device a trigger wants on top of the global settings: the special's own
/// `device_id`, when the trigger is a special that has a non-blank one.
///
/// `None` for every weekly slot, for a special without a device (the renderer
/// wrote `deviceId: null` for years), for a blank one, and for an index that
/// points past the list — the global device in all four cases.
pub fn special_device_wanted(specials: &[SpecialRecording], kind: TriggerKind) -> Option<&str> {
    match kind {
        TriggerKind::Slot(_) => None,
        TriggerKind::Special(i) => specials
            .get(i)
            .and_then(|sp| sp.device_id.as_deref())
            .filter(|id| !id.trim().is_empty()),
    }
}

/// What a special recording records from, decided against the inputs that are
/// enumerated right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialDevice {
    /// No device of its own: the global settings decide, unchanged.
    Global,
    /// Its device is present. `id` is the picker id (what `device_channels` is
    /// keyed by), `name` what the recorder opens.
    Use { id: String, name: String },
    /// It asked for a device nothing answers to. The caller records on the
    /// global device instead — and says so.
    Missing { wanted: String },
}

/// Resolve a special's stored device against the enumerated inputs, given as
/// `(id, name)` pairs in the picker's id space.
///
/// Match on the id first — that is what the picker stored — then on the name,
/// for a profile that carries a backend name instead of a picker id. Both
/// matches are EXACT. The recorder's own lookup is fuzzy (substring, word
/// overlap) because a stored label from an older build may not be the OS name;
/// here a fuzzy hit would be a silent guess at a DIFFERENT device, while a miss
/// costs a recording on the usual device plus a warning that names the one that
/// was not there. The second is the one an operator can act on.
pub fn resolve_special_device(wanted: Option<&str>, inputs: &[(String, String)]) -> SpecialDevice {
    let Some(wanted) = wanted.filter(|w| !w.trim().is_empty()) else {
        return SpecialDevice::Global;
    };
    let hit = inputs
        .iter()
        .find(|(id, _)| id == wanted)
        .or_else(|| inputs.iter().find(|(_, name)| name == wanted));
    match hit {
        Some((id, name)) => SpecialDevice::Use {
            id: id.clone(),
            name: name.clone(),
        },
        None => SpecialDevice::Missing {
            wanted: wanted.to_string(),
        },
    }
}

/// The settings a special with its own (present) device records with: the
/// global settings, pointed at that device.
///
/// `device_id` + `device_name` follow the device, and [`Settings::validate`]
/// then derives the channel pair from `device_channels[id]` — the pair the
/// operator chose for THAT device in the device picker, or default routing when
/// it has none. One case `validate` leaves alone on purpose (an EMPTY map keeps
/// the flat pair, for profiles older than the map) is closed here: switching to
/// a different device whose pair nobody chose must not inherit the global
/// device's channels — channel 16/17 of a mixer means nothing on a USB mic.
///
/// Everything else — format, folder, silence, split, video — stays the global
/// settings'. A special changes WHERE the sound comes from, nothing more.
pub fn settings_for_special_device(
    global: &crate::settings::Settings,
    id: &str,
    name: &str,
) -> crate::settings::Settings {
    let mut s = global.clone();
    let switching = s.device_id.as_deref() != Some(id);
    s.device_id = Some(id.to_string());
    s.device_name = Some(name.to_string());
    if switching && !s.device_channels.contains_key(id) {
        s.input_channel_l = None;
        s.input_channel_r = None;
    }
    s.validate();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn supervisor_sleep_is_capped_and_fire_decision_matches() {
        // Far away: capped, do NOT fire (loop + recompute against the clock).
        let week = 7 * 24 * 60 * 60 * 1000;
        assert_eq!(capped_supervisor_sleep_ms(week), MAX_SUPERVISOR_SLEEP_MS);
        assert!(!supervisor_should_fire(week));
        // Within the cap: sleep the exact wait, then fire.
        assert_eq!(capped_supervisor_sleep_ms(1234), 1234);
        assert!(supervisor_should_fire(1234));
        // Exactly at the cap boundary: one sleep covers it → fire.
        assert_eq!(
            capped_supervisor_sleep_ms(MAX_SUPERVISOR_SLEEP_MS),
            MAX_SUPERVISOR_SLEEP_MS
        );
        assert!(supervisor_should_fire(MAX_SUPERVISOR_SLEEP_MS));
        // Just over the cap: re-check, don't fire yet.
        assert!(!supervisor_should_fire(MAX_SUPERVISOR_SLEEP_MS + 1));
        // Due now: fire immediately.
        assert_eq!(capped_supervisor_sleep_ms(0), 0);
        assert!(supervisor_should_fire(0));
    }

    #[test]
    fn scheduled_max_minutes_enforces_a_backstop() {
        // The user's max wins when set.
        assert_eq!(scheduled_max_minutes(90), 90);
        assert_eq!(scheduled_max_minutes(1), 1);
        // No max (0 = "unlimited") → the safety backstop, never 0 (no infinite run).
        assert_eq!(scheduled_max_minutes(0), SCHEDULED_MAX_BACKSTOP_MINUTES);
        const { assert!(SCHEDULED_MAX_BACKSTOP_MINUTES >= 180) };
    }

    // 2026-06-07 is a Sunday; 2026-06-08 a Monday. We anchor weekday tests here.
    // chrono Mon=0 ⇒ Monday 2026-06-08 has weekday_mon0 == 0.
    #[test]
    fn weekday_convention_is_monday_zero() {
        assert_eq!(weekday_mon0(dt("2026-06-08 10:00")), 0); // Mon
        assert_eq!(weekday_mon0(dt("2026-06-09 10:00")), 1); // Tue
        assert_eq!(weekday_mon0(dt("2026-06-07 10:00")), 6); // Sun
    }

    #[test]
    fn parse_hm_handles_defaults_and_garbage() {
        assert_eq!(parse_hm("09:30", (11, 0)), (9, 30));
        assert_eq!(parse_hm("", (11, 0)), (11, 0));
        assert_eq!(parse_hm("   ", (11, 0)), (11, 0));
        assert_eq!(parse_hm("9", (11, 0)), (11, 0)); // missing minute → fallback
        assert_eq!(parse_hm("ab:cd", (12, 0)), (12, 0));
        assert_eq!(parse_hm("25:00", (12, 0)), (12, 0)); // out of range → fallback
        assert_eq!(parse_hm("10:75", (12, 0)), (12, 0));
        assert_eq!(parse_hm("00:00", (11, 0)), (0, 0));
    }

    #[test]
    fn slot_active_now_inside_and_outside_window() {
        // Sunday slot 11:00–12:00; window 5 min.
        let days = [6]; // Sun
                        // Exactly at start.
        assert!(slot_active_now(
            "11:00",
            "12:00",
            &days,
            dt("2026-06-07 11:00"),
            DEFAULT_WINDOW_MS
        ));
        // 4 min late → still active.
        assert!(slot_active_now(
            "11:00",
            "12:00",
            &days,
            dt("2026-06-07 11:04"),
            DEFAULT_WINDOW_MS
        ));
        // 6 min late → outside the start window.
        assert!(!slot_active_now(
            "11:00",
            "12:00",
            &days,
            dt("2026-06-07 11:06"),
            DEFAULT_WINDOW_MS
        ));
        // Before start.
        assert!(!slot_active_now(
            "11:00",
            "12:00",
            &days,
            dt("2026-06-07 10:59"),
            DEFAULT_WINDOW_MS
        ));
        // Wrong weekday (Monday).
        assert!(!slot_active_now(
            "11:00",
            "12:00",
            &days,
            dt("2026-06-08 11:00"),
            DEFAULT_WINDOW_MS
        ));
    }

    #[test]
    fn slot_active_now_respects_stop_even_with_wide_window() {
        // A 60-min missed-window: at 11:30 the start is 30 min ago (inside the
        // window) but we're still before the 12:00 stop → active.
        assert!(slot_active_now(
            "11:00",
            "12:00",
            &[6],
            dt("2026-06-07 11:30"),
            MISSED_WINDOW_MS
        ));
        // At 12:01 we're past stop → not active even though within 60 min of start.
        assert!(!slot_active_now(
            "11:00",
            "12:00",
            &[6],
            dt("2026-06-07 12:01"),
            MISSED_WINDOW_MS
        ));
    }

    #[test]
    fn slot_active_now_handles_midnight_crossing() {
        // 2026-06-07 is a Sunday (mon0 day 6). A 23:00–01:00 slot: the stop is
        // on MONDAY — the old same-day stop made `now < stop` false the moment
        // the slot started, so a crossing slot could never late-start.
        // 23:05 Sunday, 60-min window → active (stop is Monday 01:00).
        assert!(slot_active_now(
            "23:00",
            "01:00",
            &[6],
            dt("2026-06-07 23:05"),
            MISSED_WINDOW_MS
        ));
        // 00:10 MONDAY: the slot that started Sunday 23:00 is still running and
        // 70 min late is outside the 60-min window → not a late-start …
        assert!(!slot_active_now(
            "23:00",
            "01:00",
            &[6],
            dt("2026-06-08 00:10"),
            MISSED_WINDOW_MS
        ));
        // … but 90-min window catches it (relaunch shortly after midnight).
        assert!(slot_active_now(
            "23:00",
            "01:00",
            &[6],
            dt("2026-06-08 00:10"),
            90 * 60 * 1000
        ));
        // Past the Monday 01:00 stop → never active.
        assert!(!slot_active_now(
            "23:00",
            "01:00",
            &[6],
            dt("2026-06-08 01:05"),
            i64::MAX / 4
        ));
    }

    #[test]
    fn special_active_now_handles_midnight_crossing() {
        // A 23:00–00:30 special: the stop belongs to the next day. The old
        // same-date parse put the stop BEFORE the start → never active.
        assert!(special_active_now(
            "2026-12-31",
            "23:00",
            "00:30",
            dt("2026-12-31 23:20"),
            MISSED_WINDOW_MS
        ));
        // Past the (next-day) stop → inactive.
        assert!(!special_active_now(
            "2026-12-31",
            "23:00",
            "00:30",
            dt("2027-01-01 00:35"),
            MISSED_WINDOW_MS
        ));
    }

    #[test]
    fn special_stop_event_crosses_midnight() {
        // The event builder must schedule the 23:00–00:30 special's STOP on the
        // next day — the same-date parse made it a pre-start no-op, so the
        // recording ran to the 240-min backstop instead of 1.5 h.
        let sp = SpecialRecording {
            id: None,
            date: "2026-12-31".into(),
            name: "Nyttårsgudstjeneste".into(),
            start: "23:00".into(),
            stop: "00:30".into(),
            device_id: None,
        };
        let events = upcoming_events(&[], std::slice::from_ref(&sp), dt("2026-12-31 20:00"), 0, 7);
        let stop = events
            .iter()
            .find(|e| matches!(e.kind, ScheduledEventKind::Stop))
            .expect("a stop event must exist");
        assert_eq!(stop.at, dt("2027-01-01 00:30"));
    }

    #[test]
    fn special_active_now_matches_dated_window() {
        assert!(special_active_now(
            "2026-06-07",
            "11:00",
            "12:00",
            dt("2026-06-07 11:02"),
            DEFAULT_WINDOW_MS
        ));
        assert!(!special_active_now(
            "2026-06-07",
            "11:00",
            "12:00",
            dt("2026-06-08 11:02"), // wrong day
            DEFAULT_WINDOW_MS
        ));
        assert!(!special_active_now(
            "bad-date",
            "11:00",
            "12:00",
            dt("2026-06-07 11:02"),
            DEFAULT_WINDOW_MS
        ));
    }

    #[test]
    fn crosses_midnight_and_degenerate() {
        assert!(!crosses_midnight("11:00", "12:00"));
        assert!(crosses_midnight("23:00", "01:00"));
        assert!(crosses_midnight("23:30", "23:00"));
        assert!(!crosses_midnight("23:00", "23:30"));

        assert!(is_degenerate("11:00", "11:00"));
        assert!(!is_degenerate("11:00", "12:00"));
    }

    #[test]
    fn lead_event_same_day_and_wraparound() {
        // 11:00 start, 30-min lead → 10:30 same weekday.
        assert_eq!(
            lead_event(6, 11, 0, 30),
            LeadEvent {
                weekday: 6,
                hour: 10,
                minute: 30
            }
        );
        // 00:10 start (Monday=0), 30-min lead → 23:40 previous day (Sunday=6).
        assert_eq!(
            lead_event(0, 0, 10, 30),
            LeadEvent {
                weekday: 6,
                hour: 23,
                minute: 40
            }
        );
        // Lead 0 → unchanged.
        assert_eq!(
            lead_event(3, 9, 15, 0),
            LeadEvent {
                weekday: 3,
                hour: 9,
                minute: 15
            }
        );
    }

    #[test]
    fn next_occurrence_today_future_and_past() {
        // now = Sunday 10:00. Next Sunday-11:00 is today.
        assert_eq!(
            next_occurrence(6, 11, 0, dt("2026-06-07 10:00")),
            Some(dt("2026-06-07 11:00"))
        );
        // now = Sunday 11:30 → today's 11:00 passed → next week.
        assert_eq!(
            next_occurrence(6, 11, 0, dt("2026-06-07 11:30")),
            Some(dt("2026-06-14 11:00"))
        );
        // now = Monday → next Sunday is 6 days out.
        assert_eq!(
            next_occurrence(6, 11, 0, dt("2026-06-08 09:00")),
            Some(dt("2026-06-14 11:00"))
        );
    }

    #[test]
    fn most_recent_occurrence_today_and_back() {
        // now = Sunday 11:30 → most recent Sunday-11:00 is today.
        assert_eq!(
            most_recent_occurrence(6, 11, 0, dt("2026-06-07 11:30")),
            Some(dt("2026-06-07 11:00"))
        );
        // now = Sunday 10:00 → today's 11:00 hasn't happened → last week.
        assert_eq!(
            most_recent_occurrence(6, 11, 0, dt("2026-06-07 10:00")),
            Some(dt("2026-05-31 11:00"))
        );
        // now = Monday → most recent Sunday was yesterday.
        assert_eq!(
            most_recent_occurrence(6, 11, 0, dt("2026-06-08 09:00")),
            Some(dt("2026-06-07 11:00"))
        );
    }

    fn sunday_slot() -> ScheduleSlot {
        ScheduleSlot {
            days: vec![6],
            start: "11:00".to_string(),
            stop: "12:00".to_string(),
            max: None,
        }
    }

    #[test]
    fn next_recording_picks_nearest() {
        let slots = vec![sunday_slot()];
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-09".to_string(), // Tuesday, sooner than next Sunday
            name: "Konsert".to_string(),
            start: "19:00".to_string(),
            stop: "21:00".to_string(),
            device_id: None,
        }];
        // now = Monday 2026-06-08 09:00 → nearest is Tuesday's special.
        assert_eq!(
            next_recording(&slots, &specials, dt("2026-06-08 09:00")),
            Some(dt("2026-06-09 19:00"))
        );
    }

    #[test]
    fn next_recording_picks_slot_when_it_beats_a_later_special() {
        // The mirror of next_recording_picks_nearest: a slot sooner than a
        // special wins (cross-source nearest, the other direction).
        let slots = vec![sunday_slot()]; // Sun 11:00
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-20".to_string(), // far-off Saturday
            name: "Konsert".to_string(),
            start: "19:00".to_string(),
            stop: "21:00".to_string(),
            device_id: None,
        }];
        // now = Saturday 2026-06-06 09:00 → tomorrow's Sunday slot is nearest.
        assert_eq!(
            next_recording(&slots, &specials, dt("2026-06-06 09:00")),
            Some(dt("2026-06-07 11:00"))
        );
    }

    #[test]
    fn next_recording_is_none_when_nothing_future_or_all_degenerate() {
        // No slots, no specials → nothing to fire.
        assert!(next_recording(&[], &[], dt("2026-06-07 09:00")).is_none());
        // A degenerate slot is skipped, leaving nothing.
        let deg = vec![ScheduleSlot {
            days: vec![6],
            start: "11:00".to_string(),
            stop: "11:00".to_string(),
            max: None,
        }];
        assert!(next_recording(&deg, &[], dt("2026-06-07 09:00")).is_none());
        // A special entirely in the past contributes no future fire.
        let past = vec![SpecialRecording {
            id: None,
            date: "2026-06-01".to_string(),
            name: "Forbi".to_string(),
            start: "11:00".to_string(),
            stop: "12:00".to_string(),
            device_id: None,
        }];
        assert!(next_recording(&[], &past, dt("2026-06-07 09:00")).is_none());
    }

    #[test]
    fn next_recording_rolls_past_an_occurrence_that_just_started() {
        // `now` is exactly at the slot start: that occurrence is no longer in the
        // future (strict `> now`), so the next fire rolls a week forward.
        let slots = vec![sunday_slot()];
        assert_eq!(
            next_recording(&slots, &[], dt("2026-06-07 11:00")),
            Some(dt("2026-06-14 11:00"))
        );
    }

    #[test]
    fn missed_recordings_catches_up_a_slot_and_a_special_together() {
        // now = Monday 2026-06-08 09:00. Sunday's 11:00 slot occurred ~22h ago
        // (past the late-start window, inside 24h) and was never recorded, and a
        // Saturday special also went un-run — both should surface as catch-up.
        let slots = vec![sunday_slot()];
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-07".to_string(), // Sunday too, 14:00 — distinct from slot
            name: "Dåp".to_string(),
            start: "14:00".to_string(),
            stop: "15:00".to_string(),
            device_id: None,
        }];
        let now = dt("2026-06-08 09:00");
        let missed = missed_recordings(&slots, &specials, now, &[], &[], &HashSet::new());
        let labels: Vec<_> = missed.iter().map(|m| m.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.contains("Ukentlig")));
        assert!(labels.contains(&"Dåp"));
        assert_eq!(missed.len(), 2);

        // A history row covering the slot occurrence removes it from catch-up;
        // the un-covered special still surfaces.
        let history = vec![dt("2026-06-07 11:00")];
        let missed2 = missed_recordings(&slots, &specials, now, &history, &[], &HashSet::new());
        assert_eq!(missed2.len(), 1);
        assert_eq!(missed2[0].label, "Dåp");
    }

    #[test]
    fn upcoming_dates_bounds_and_sorts() {
        let slots = vec![ScheduleSlot {
            days: vec![6, 0], // Sun + Mon
            start: "11:00".to_string(),
            stop: "12:00".to_string(),
            max: None,
        }];
        // now = Sunday 2026-06-07 09:00, window 14 days.
        let up = upcoming_dates(&slots, &[], dt("2026-06-07 09:00"), 14);
        // Sun 11:00 today, Mon 11:00 tomorrow (next occurrences of each job).
        assert_eq!(up, vec![dt("2026-06-07 11:00"), dt("2026-06-08 11:00")]);
        // Degenerate slot contributes nothing.
        let deg = vec![ScheduleSlot {
            days: vec![6],
            start: "11:00".to_string(),
            stop: "11:00".to_string(),
            max: None,
        }];
        assert!(upcoming_dates(&deg, &[], dt("2026-06-07 09:00"), 14).is_empty());
    }

    #[test]
    fn prune_specials_drops_old_keeps_recent_and_malformed() {
        let now = dt("2026-06-30 12:00");
        let specials = vec![
            // ended 10 days ago → pruned
            SpecialRecording {
                id: None,
                date: "2026-06-20".to_string(),
                name: "Gammel".to_string(),
                start: "11:00".to_string(),
                stop: "12:00".to_string(),
                device_id: None,
            },
            // ended 2 days ago → kept
            SpecialRecording {
                id: None,
                date: "2026-06-28".to_string(),
                name: "Nylig".to_string(),
                start: "11:00".to_string(),
                stop: "12:00".to_string(),
                device_id: None,
            },
            // malformed date → kept (deviation from Electron, documented)
            SpecialRecording {
                id: None,
                date: "garbage".to_string(),
                name: "Rar".to_string(),
                start: "11:00".to_string(),
                stop: "12:00".to_string(),
                device_id: None,
            },
        ];
        let (kept, pruned) = prune_specials(&specials, now);
        assert_eq!(pruned, 1);
        let names: Vec<_> = kept.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Nylig", "Rar"]);
    }

    #[test]
    fn active_within_finds_current_slot_with_key() {
        let slots = vec![sunday_slot()];
        let triggers = active_within(&slots, &[], dt("2026-06-07 11:03"), DEFAULT_WINDOW_MS);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].kind, TriggerKind::Slot(0));
        assert_eq!(triggers[0].key, "slot:6:11:00-12:00");
    }

    #[test]
    fn missed_recordings_logs_stale_uncovered_occurrence() {
        let slots = vec![sunday_slot()];
        // now = Sunday 13:00 → 11:00 start is 2 h ago: older than the 60-min
        // late-start window, within 24 h, not triggered, not in history → missed.
        let now = dt("2026-06-07 13:00");
        let missed = missed_recordings(&slots, &[], now, &[], &[], &HashSet::new());
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].when, dt("2026-06-07 11:00"));
        assert_eq!(missed[0].label, "Ukentlig opptak (11:00–12:00)");
    }

    #[test]
    fn missed_recordings_suppressed_when_covered_or_triggered_or_fresh() {
        let slots = vec![sunday_slot()];
        let now = dt("2026-06-07 13:00");

        // Covered by a history entry within ±30 min of 11:00 → suppressed.
        let history = vec![dt("2026-06-07 11:10")];
        assert!(missed_recordings(&slots, &[], now, &history, &[], &HashSet::new()).is_empty());

        // Triggered this pass → suppressed.
        let mut triggered = HashSet::new();
        triggered.insert("slot:6:11:00-12:00".to_string());
        assert!(missed_recordings(&slots, &[], now, &[], &[], &triggered).is_empty());

        // Still inside the 60-min late-start window (11:30) → not yet "missed".
        assert!(missed_recordings(
            &slots,
            &[],
            dt("2026-06-07 11:30"),
            &[],
            &[],
            &HashSet::new()
        )
        .is_empty());
    }

    #[test]
    fn missed_recordings_look_back_seven_days_and_no_further() {
        let slots = vec![sunday_slot()];
        // now = Tuesday 14:00. Sunday 7 June (two days back) is reported;
        // Sunday 31 May (nine days back) is outside the window.
        let now = dt("2026-06-09 14:00");
        let missed = missed_recordings(&slots, &[], now, &[], &[], &HashSet::new());
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].when, dt("2026-06-07 11:00"));
    }

    // ── A4: one pass, one start ─────────────────────────────────────────────

    #[test]
    fn late_start_choice_starts_exactly_one_of_two_simultaneous_triggers() {
        // The Sunday this finding is named after: a weekly 11:00 slot AND a
        // hand-entered special at the same minute, app launched at 11:20.
        let slots = vec![sunday_slot()];
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-07".to_string(),
            name: "Konfirmasjon".to_string(),
            start: "11:00".to_string(),
            stop: "12:00".to_string(),
            device_id: None,
        }];
        let active = active_within(&slots, &specials, dt("2026-06-07 11:20"), MISSED_WINDOW_MS);
        // BOTH are active — that half is deliberate, because both keys have to
        // reach `missed_recordings` as handled.
        assert_eq!(active.len(), 2, "slot + special both inside the window");

        // …but the recorder gets ONE, and it is the same one `fire()` would have
        // picked on time (slots sort first in `upcoming_events`).
        let chosen = late_start_choice(&active, false).expect("an idle engine starts one");
        assert_eq!(chosen.kind, TriggerKind::Slot(0));

        // The bug itself: the second start used to pass a guard read before the
        // first one. A fresh reading refuses it.
        assert_eq!(
            late_start_choice(&active, true),
            None,
            "a busy recorder starts nothing — `start()` stops what is running first"
        );
    }

    #[test]
    fn late_start_choice_has_nothing_to_do_without_triggers() {
        assert_eq!(late_start_choice(&[], false), None);
        assert_eq!(late_start_choice(&[], true), None);
    }

    // ── A10: a recovery in flight is not a missed recording ─────────────────

    #[test]
    fn covered_windows_table() {
        // One interrupted session: started 11:00, last wrote at 11:35 (the crash).
        let w = [CoveredWindow {
            start: dt("2026-06-07 11:00"),
            last_seen: dt("2026-06-07 11:35"),
        }];
        let cases: [(&str, bool, &str); 7] = [
            ("2026-06-07 11:00", true, "the window's own start"),
            (
                "2026-06-07 11:20",
                true,
                "inside the stretch that was recording",
            ),
            (
                "2026-06-07 11:35",
                true,
                "the last moment it was seen alive",
            ),
            (
                "2026-06-07 10:31",
                true,
                "29 min BEFORE the start — inside ±HISTORY_COVER_MS, same as the \
                 history row this recovery is about to write",
            ),
            (
                "2026-06-07 10:29",
                false,
                "31 min before → a different occurrence",
            ),
            (
                "2026-06-07 11:50",
                false,
                "15 min after the last write: nothing was recording, and there is \
                 deliberately no tail grace",
            ),
            ("2026-06-07 13:00", false, "long after"),
        ];
        for (when, expected, why) in cases {
            assert_eq!(windows_cover(&w, dt(when)), expected, "{when}: {why}");
        }
        assert!(
            !windows_cover(&[], dt("2026-06-07 11:20")),
            "no windows, no cover"
        );
    }

    #[test]
    fn missed_recordings_suppressed_by_a_recovery_still_in_flight() {
        // The crash Sunday: the 11:00 service recorded until 11:35, the app died,
        // it was relaunched at 12:30. Startup recovery is still concatenating, so
        // `history` is EMPTY — the row does not exist yet.
        let slots = vec![sunday_slot()];
        let now = dt("2026-06-07 12:30");
        assert_eq!(
            missed_recordings(&slots, &[], now, &[], &[], &HashSet::new()).len(),
            1,
            "precondition: without the manifest this is a (false) missed report"
        );

        let pending = [CoveredWindow {
            start: dt("2026-06-07 11:00"),
            last_seen: dt("2026-06-07 11:35"),
        }];
        assert!(
            missed_recordings(&slots, &[], now, &[], &pending, &HashSet::new()).is_empty(),
            "a manifest on disk says it DID record — recovery just has not written \
             the row yet"
        );

        // And the window does not become a blanket amnesty: an evening service
        // outside it is still missed.
        let evening = vec![ScheduleSlot {
            days: vec![6],
            start: "18:00".to_string(),
            stop: "19:00".to_string(),
            max: None,
        }];
        let missed = missed_recordings(
            &evening,
            &[],
            dt("2026-06-07 20:00"),
            &[],
            &pending,
            &HashSet::new(),
        );
        assert_eq!(
            missed.len(),
            1,
            "18:00 is nowhere near the 11:00–11:35 window"
        );
    }

    #[test]
    fn a_special_inside_a_longer_interrupted_recording_is_not_missed() {
        // A special at 11:30 while the weekly take (11:00 →) was still running:
        // covered by the STRETCH clause, not by the ±30 min start clause.
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-07".to_string(),
            name: "Dåp".to_string(),
            start: "11:30".to_string(),
            stop: "12:00".to_string(),
            device_id: None,
        }];
        let pending = [CoveredWindow {
            start: dt("2026-06-07 11:00"),
            last_seen: dt("2026-06-07 12:30"),
        }];
        assert!(
            missed_recordings(
                &[],
                &specials,
                dt("2026-06-07 13:00"),
                &[],
                &pending,
                &HashSet::new()
            )
            .is_empty(),
            "something WAS recording at 11:30"
        );
    }

    #[test]
    fn special_recording_serde_matches_electron_camel_case() {
        let sp = SpecialRecording {
            id: Some("x1".to_string()),
            date: "2026-06-07".to_string(),
            name: "Konsert".to_string(),
            start: "19:00".to_string(),
            stop: "21:00".to_string(),
            device_id: Some("dev-2".to_string()),
        };
        let json = serde_json::to_value(&sp).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("deviceId"));
        assert!(!obj.contains_key("device_id"));
        // Round-trips from a partial Electron blob (missing id/deviceId).
        let back: SpecialRecording = serde_json::from_str(
            r#"{ "date": "2026-06-07", "name": "X", "start": "10:00", "stop": "11:00" }"#,
        )
        .unwrap();
        assert_eq!(back.id, None);
        assert_eq!(back.device_id, None);
        assert_eq!(back.start, "10:00");
    }

    #[test]
    fn upcoming_events_enumerates_start_stop_reminder_preflight() {
        let slots = vec![sunday_slot()];
        // now = Sunday 09:00; reminder 15 min; horizon 2 days.
        let now = dt("2026-06-07 09:00");
        let ev = upcoming_events(&slots, &[], now, 15, 2);
        // Expect today: preflight 10:30, reminder 10:45, start 11:00, stop 12:00.
        let want = [
            (dt("2026-06-07 10:30"), ScheduledEventKind::Preflight),
            (dt("2026-06-07 10:45"), ScheduledEventKind::Reminder),
            (dt("2026-06-07 11:00"), ScheduledEventKind::Start),
            (dt("2026-06-07 12:00"), ScheduledEventKind::Stop),
        ];
        assert_eq!(ev.len(), want.len());
        for (got, (at, kind)) in ev.iter().zip(want.iter()) {
            assert_eq!(got.at, *at);
            assert_eq!(got.kind, *kind);
            assert_eq!(got.source, TriggerKind::Slot(0));
        }
    }

    /// Mimic the supervisor: pick the due group at `fire_at`, then enumerate
    /// again from the anchored base. Returns (kind, source) of what fired.
    fn fire_and_reenumerate(
        slots: &[ScheduleSlot],
        specials: &[SpecialRecording],
        now: NaiveDateTime,
        fire_at: NaiveDateTime,
    ) -> (Vec<(ScheduledEventKind, TriggerKind)>, Vec<ScheduledEvent>) {
        let events = upcoming_events(slots, specials, now, 0, 2);
        let fired: Vec<_> = events_due(&events, fire_at)
            .iter()
            .map(|e| (e.kind, e.source))
            .collect();
        // The next pass runs a little after the fire, so `now` has moved on.
        let later = fire_at + Duration::seconds(2);
        let next = upcoming_events(
            slots,
            specials,
            enumeration_base(later, Some(fire_at)),
            0,
            2,
        );
        (fired, next)
    }

    #[test]
    fn back_to_back_recordings_fire_stop_then_start_at_the_shared_instant() {
        // Weekly 11:00–12:30 service, and a special 12:30–13:30 the same Sunday.
        let slots = vec![ScheduleSlot {
            days: vec![6],
            start: "11:00".to_string(),
            stop: "12:30".to_string(),
            max: None,
        }];
        let specials = vec![SpecialRecording {
            id: None,
            date: "2026-06-07".to_string(),
            name: "Dåp".to_string(),
            start: "12:30".to_string(),
            stop: "13:30".to_string(),
            device_id: None,
        }];
        let (fired, next) = fire_and_reenumerate(
            &slots,
            &specials,
            dt("2026-06-07 12:00"),
            dt("2026-06-07 12:30"),
        );
        // BOTH events at 12:30 fire — Stop first, so the special's Start finds a
        // free recorder — in one group.
        assert_eq!(
            fired,
            vec![
                (ScheduledEventKind::Stop, TriggerKind::Slot(0)),
                (ScheduledEventKind::Start, TriggerKind::Special(0)),
            ]
        );
        // …and the next enumeration neither repeats them nor skips ahead past
        // the special's own Stop.
        assert_eq!(next[0].kind, ScheduledEventKind::Stop);
        assert_eq!(next[0].source, TriggerKind::Special(0));
        assert_eq!(next[0].at, dt("2026-06-07 13:30"));
    }

    #[test]
    fn two_slots_sharing_a_boundary_fire_in_a_fixed_order_whatever_the_listing_order() {
        let a = ScheduleSlot {
            days: vec![6],
            start: "09:00".to_string(),
            stop: "10:00".to_string(),
            max: None,
        };
        let b = ScheduleSlot {
            days: vec![6],
            start: "10:00".to_string(),
            stop: "11:00".to_string(),
            max: None,
        };
        for slots in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
            let (fired, _) =
                fire_and_reenumerate(&slots, &[], dt("2026-06-07 09:30"), dt("2026-06-07 10:00"));
            let kinds: Vec<_> = fired.iter().map(|f| f.0).collect();
            assert_eq!(
                kinds,
                vec![ScheduledEventKind::Stop, ScheduledEventKind::Start]
            );
        }
    }

    #[test]
    fn reminder_and_preflight_do_not_displace_a_start_at_the_same_instant() {
        // reminder 30 min before a 12:00 start = 11:30, the same instant the
        // 11:00–11:30 slot stops. Every one of them is due; Stop leads.
        let slots = vec![
            ScheduleSlot {
                days: vec![6],
                start: "11:00".to_string(),
                stop: "11:30".to_string(),
                max: None,
            },
            ScheduleSlot {
                days: vec![6],
                start: "12:00".to_string(),
                stop: "13:00".to_string(),
                max: None,
            },
        ];
        let events = upcoming_events(&slots, &[], dt("2026-06-07 11:10"), 30, 1);
        let due = events_due(&events, dt("2026-06-07 11:30"));
        assert_eq!(due.len(), 3); // Stop, Reminder, Preflight (12:00 − 30 min)
        assert_eq!(due[0].kind, ScheduledEventKind::Stop);
        assert_eq!(due[1].kind, ScheduledEventKind::Reminder);
        assert_eq!(due[2].kind, ScheduledEventKind::Preflight);
    }

    #[test]
    fn after_the_missed_net_only_stops_still_fire() {
        use ScheduledEventKind::*;
        for k in [Start, Stop, Reminder, Preflight] {
            assert!(
                fire_after_missed_net(k, false),
                "{k:?} fires on a normal wake"
            );
        }
        assert!(fire_after_missed_net(Stop, true));
        for k in [Start, Reminder, Preflight] {
            assert!(
                !fire_after_missed_net(k, true),
                "{k:?} is stale after the net"
            );
        }
    }

    #[test]
    fn enumeration_base_anchors_on_a_recent_fire_only() {
        let now = dt("2026-06-07 12:30");
        assert_eq!(enumeration_base(now, None), now);
        // A fire a few seconds ago (or a hair in the future: the timer woke
        // early) anchors; an event that came due while firing stays visible.
        let recent = now - Duration::seconds(5);
        assert_eq!(enumeration_base(now, Some(recent)), recent);
        let ahead = now + Duration::seconds(1);
        assert_eq!(enumeration_base(now, Some(ahead)), ahead);
        // The anchor outlives a full Stop→Start settle (plus margin)…
        let settle = Duration::milliseconds(STOP_SETTLE_MS as i64);
        assert!(FIRE_ANCHOR_WINDOW_SECS > settle.num_seconds());
        let after_settle = now - settle;
        assert_eq!(enumeration_base(now, Some(after_settle)), after_settle);
        // An old fire — or a clock that jumped — is ignored.
        assert_eq!(
            enumeration_base(now, Some(now - Duration::minutes(10))),
            now
        );
        assert_eq!(
            enumeration_base(now, Some(now + Duration::minutes(10))),
            now
        );
    }

    #[test]
    fn upcoming_events_suppresses_reminder_when_zero_and_skips_degenerate() {
        let slots = vec![sunday_slot()];
        let now = dt("2026-06-07 09:00");
        // reminder_min = 0 → no Reminder events.
        let ev = upcoming_events(&slots, &[], now, 0, 2);
        assert!(!ev.iter().any(|e| e.kind == ScheduledEventKind::Reminder));
        assert!(ev.iter().any(|e| e.kind == ScheduledEventKind::Start));

        // Degenerate slot → nothing at all.
        let deg = vec![ScheduleSlot {
            days: vec![6],
            start: "11:00".to_string(),
            stop: "11:00".to_string(),
            max: None,
        }];
        assert!(upcoming_events(&deg, &[], now, 15, 7).is_empty());
    }

    #[test]
    fn upcoming_events_emits_stop_even_when_start_passed() {
        // now = Sunday 11:30 (past the 11:00 start, before 12:00 stop). The
        // start rolls to next week but the stop is still today → emitted, so an
        // app launched mid-service still stops on time.
        let slots = vec![sunday_slot()];
        let ev = upcoming_events(&slots, &[], dt("2026-06-07 11:30"), 0, 1);
        let stops: Vec<_> = ev
            .iter()
            .filter(|e| e.kind == ScheduledEventKind::Stop)
            .collect();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].at, dt("2026-06-07 12:00"));
    }

    #[test]
    fn upcoming_events_handles_midnight_crossing_stop_next_day() {
        // 23:00–01:00 Saturday slot. now = Saturday 22:00.
        let slots = vec![ScheduleSlot {
            days: vec![5], // Sat
            start: "23:00".to_string(),
            stop: "01:00".to_string(),
            max: None,
        }];
        let now = dt("2026-06-06 22:00"); // Sat
        let ev = upcoming_events(&slots, &[], now, 0, 2);
        let start = ev
            .iter()
            .find(|e| e.kind == ScheduledEventKind::Start)
            .unwrap();
        let stop = ev
            .iter()
            .find(|e| e.kind == ScheduledEventKind::Stop)
            .unwrap();
        assert_eq!(start.at, dt("2026-06-06 23:00"));
        // Stop is on Sunday 01:00 (the next day).
        assert_eq!(stop.at, dt("2026-06-07 01:00"));
    }

    #[test]
    fn schedule_slot_defaults_fill_from_partial_json() {
        let s: ScheduleSlot = serde_json::from_str(r#"{ "days": [6] }"#).unwrap();
        assert_eq!(s.days, vec![6]);
        assert_eq!(s.start, "11:00");
        assert_eq!(s.stop, "12:00");
        assert_eq!(s.max, None);
    }

    // ── Special device override ─────────────────────────────────────────────

    fn special_with(device_id: Option<&str>) -> SpecialRecording {
        SpecialRecording {
            id: None,
            date: "2026-12-24".into(),
            name: "Julekonsert".into(),
            start: "16:00".into(),
            stop: "17:30".into(),
            device_id: device_id.map(str::to_string),
        }
    }

    /// The picker-id space the scheduler hands over: host devices by name,
    /// ASIO devices with the renderer's `asio::` prefix.
    fn inputs() -> Vec<(String, String)> {
        [
            ("asio::Focusrite USB ASIO", "Focusrite USB ASIO"),
            ("Behringer X32", "Behringer X32"),
            ("Rode NT-USB", "Rode NT-USB"),
        ]
        .iter()
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect()
    }

    #[test]
    fn only_a_special_with_a_non_blank_device_wants_one() {
        let specials = vec![
            special_with(None),
            special_with(Some("")),
            special_with(Some("   ")),
            special_with(Some("Rode NT-USB")),
        ];
        // A weekly slot NEVER wants a device — whatever the specials say.
        for i in 0..specials.len() {
            assert_eq!(special_device_wanted(&specials, TriggerKind::Slot(i)), None);
        }
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(0)),
            None
        );
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(1)),
            None
        );
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(2)),
            None
        );
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(3)),
            Some("Rode NT-USB")
        );
        // An index past the list is the global device, not a panic.
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(9)),
            None
        );
    }

    #[test]
    fn the_renderers_special_json_reaches_the_resolver_intact() {
        // Byte-for-byte what `withSpecial` (app/pages/setup/advanced/
        // specials-core.ts) writes — the picker id, `asio::` prefix and all,
        // and `null` for «Samme som vanlig opptak».
        let specials: Vec<SpecialRecording> = serde_json::from_str(
            r#"[
                {"id":null,"date":"2099-06-20","name":"Bryllup","start":"14:00",
                 "stop":"15:30","deviceId":"asio::Focusrite USB ASIO"},
                {"id":null,"date":"2099-06-21","name":"Konsert","start":"19:00",
                 "stop":"20:30","deviceId":null}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(0)),
            Some("asio::Focusrite USB ASIO")
        );
        assert_eq!(
            special_device_wanted(&specials, TriggerKind::Special(1)),
            None
        );
        assert_eq!(
            resolve_special_device(
                special_device_wanted(&specials, TriggerKind::Special(0)),
                &inputs()
            ),
            SpecialDevice::Use {
                id: "asio::Focusrite USB ASIO".into(),
                name: "Focusrite USB ASIO".into()
            }
        );
        // …and back out again unchanged, so a settings save does not lose it.
        let back = serde_json::to_value(&specials[0]).unwrap();
        assert_eq!(back["deviceId"], "asio::Focusrite USB ASIO");
    }

    #[test]
    fn resolve_special_device_table() {
        let list = inputs();
        let cases: Vec<(Option<&str>, SpecialDevice)> = vec![
            // No device of its own → the global settings, untouched.
            (None, SpecialDevice::Global),
            (Some(""), SpecialDevice::Global),
            (Some("  "), SpecialDevice::Global),
            // The picker id, exactly — host device.
            (
                Some("Rode NT-USB"),
                SpecialDevice::Use {
                    id: "Rode NT-USB".into(),
                    name: "Rode NT-USB".into(),
                },
            ),
            // The picker id, exactly — ASIO device: the id keeps its prefix
            // (it keys `device_channels`), the name is what the recorder opens.
            (
                Some("asio::Focusrite USB ASIO"),
                SpecialDevice::Use {
                    id: "asio::Focusrite USB ASIO".into(),
                    name: "Focusrite USB ASIO".into(),
                },
            ),
            // Name-only match: a profile that stored the backend name of an
            // ASIO device rather than the picker id still finds it.
            (
                Some("Focusrite USB ASIO"),
                SpecialDevice::Use {
                    id: "asio::Focusrite USB ASIO".into(),
                    name: "Focusrite USB ASIO".into(),
                },
            ),
            // Not there → Missing, carrying what was asked for.
            (
                Some("Zoom H6"),
                SpecialDevice::Missing {
                    wanted: "Zoom H6".into(),
                },
            ),
            // Exact means exact: no fuzzy hit on a different device.
            (
                Some("Behringer"),
                SpecialDevice::Missing {
                    wanted: "Behringer".into(),
                },
            ),
            (
                Some("rode nt-usb"),
                SpecialDevice::Missing {
                    wanted: "rode nt-usb".into(),
                },
            ),
        ];
        for (wanted, expected) in cases {
            assert_eq!(
                resolve_special_device(wanted, &list),
                expected,
                "wanted = {wanted:?}"
            );
        }
        // An empty enumeration answers Missing, never Use.
        assert_eq!(
            resolve_special_device(Some("Behringer X32"), &[]),
            SpecialDevice::Missing {
                wanted: "Behringer X32".into()
            }
        );
    }

    #[test]
    fn the_id_wins_over_a_name_that_happens_to_match_another_entry() {
        // Contrived, but it pins the ORDER: the stored id is the operator's
        // choice; a name match is only the fallback.
        let list = vec![
            ("Mic A".to_string(), "Mic B".to_string()),
            ("Mic B".to_string(), "Mic B (2)".to_string()),
        ];
        assert_eq!(
            resolve_special_device(Some("Mic B"), &list),
            SpecialDevice::Use {
                id: "Mic B".into(),
                name: "Mic B (2)".into()
            }
        );
    }

    #[test]
    fn a_special_device_carries_its_own_channel_pair() {
        use crate::settings::{DeviceChannels, Settings};
        let mut map = std::collections::HashMap::new();
        map.insert(
            "Behringer X32".to_string(),
            DeviceChannels {
                channel_l: 16,
                channel_r: 17,
            },
        );
        map.insert(
            "asio::Focusrite USB ASIO".to_string(),
            DeviceChannels {
                channel_l: 2,
                channel_r: 3,
            },
        );
        let global = Settings {
            device_id: Some("Behringer X32".into()),
            device_name: Some("Behringer X32".into()),
            device_channels: map,
            ..Settings::default()
        }
        .validated();
        assert_eq!(
            (global.input_channel_l, global.input_channel_r),
            (Some(16), Some(17))
        );

        // A device with its own pair → that pair.
        let s =
            settings_for_special_device(&global, "asio::Focusrite USB ASIO", "Focusrite USB ASIO");
        assert_eq!(s.device_id.as_deref(), Some("asio::Focusrite USB ASIO"));
        assert_eq!(s.device_name.as_deref(), Some("Focusrite USB ASIO"));
        assert_eq!((s.input_channel_l, s.input_channel_r), (Some(2), Some(3)));

        // A device nobody chose a pair for → default routing, not 16/17.
        let s = settings_for_special_device(&global, "Rode NT-USB", "Rode NT-USB");
        assert_eq!((s.input_channel_l, s.input_channel_r), (None, None));

        // Everything that is not the device stays the global settings'.
        assert_eq!(
            Settings {
                device_id: global.device_id.clone(),
                device_name: global.device_name.clone(),
                input_channel_l: global.input_channel_l,
                input_channel_r: global.input_channel_r,
                ..s
            },
            global
        );
    }

    #[test]
    fn an_old_profile_without_a_channel_map_does_not_leak_its_pair_to_another_device() {
        use crate::settings::Settings;
        // An empty map keeps the flat pair (profiles older than the map) —
        // `validate` alone would hand the mixer's 16/17 to the USB mic.
        let global = Settings {
            device_id: Some("Behringer X32".into()),
            device_name: Some("Behringer X32".into()),
            input_channel_l: Some(16),
            input_channel_r: Some(17),
            ..Settings::default()
        }
        .validated();
        let other = settings_for_special_device(&global, "Rode NT-USB", "Rode NT-USB");
        assert_eq!((other.input_channel_l, other.input_channel_r), (None, None));

        // …while naming the SAME device the global settings use changes nothing.
        let same = settings_for_special_device(&global, "Behringer X32", "Behringer X32");
        assert_eq!(same, global);
    }
}
