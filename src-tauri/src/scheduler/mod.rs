//! Scheduler engine (Fase 5.1) — the impure timer/trigger shell over the pure
//! [`sundayrec_core::schedule`] decision core.
//!
//! Replaces the Electron `src/main/scheduler.ts` + node-schedule. The *decisions*
//! (which weekday/time fires, the reminder/preflight lead offsets, what counts as
//! "active now", which past occurrences are missed) all live in the core and
//! carry the tests. This module owns only what can't be pure:
//!   - reading `Local::now()` and converting it to the core's `NaiveDateTime`
//!     local-wall frame,
//!   - one supervisor task that enumerates upcoming events
//!     ([`sundayrec_core::schedule::upcoming_events`]), sleeps until the nearest,
//!     fires it, then recomputes — woken early by [`SchedulerEngine::reschedule`]
//!     whenever settings change,
//!   - asking [`crate::recorder::opts::build_opts`] for the [`RecordingOpts`]
//!     of a scheduled start and calling the recorder engine directly (so a
//!     scheduled recording runs even when the window is hidden in the tray) —
//!     the opts composition itself is NOT the scheduler's (v0.15 moved it next
//!     to the engine, so the manual path no longer depends on this module),
//!   - firing native reminder/preflight notifications,
//!   - pruning expired specials and persisting the trimmed list.
//!
//! ## ⚠️ TIMING/HARDWARE-UNVERIFIED
//!
//! The supervisor's wall-clock timing and the recorder hand-off can only be
//! validated on a real run (a clock ticking to a slot time, a mic attached). The
//! logic it delegates to is fully unit-tested; the orchestration here is wired
//! and compiles but has NOT been exercised against a live clock/device. Mac
//! permission prompts (mic/notification) are also a runtime concern.
//!
//! ## Honest gaps (carried to a later Fase-5 slice)
//!
//! - **Missed-recording persistence.** [`sundayrec_core::schedule::missed_recordings`]
//!   decides what was missed, and [`check_missed`] emits it + notifies, but the
//!   current `recording` table has no `status`/`error` column to store a "missed"
//!   row (Electron used a `wakeFailureHistory` ring + a `status` field). Logging
//!   missed/skipped rows waits on that schema. Dedup therefore only considers
//!   real recordings, not previously-logged misses.
//! - **Special device override.** `SpecialRecording.device_id` is a stored id, but
//!   the recorder matches by NAME; mapping id→name needs the device list. Until
//!   then a special uses the global `device_name`.
//! - **Wake-from-sleep.** Actually waking the machine (pmset / SetWaitableTimer) is
//!   Fase 5.2; this slice schedules and fires while the app is running/awake.

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use ts_rs::TS;

use sundayrec_core::schedule::{
    active_within, capped_supervisor_sleep_ms, missed_recordings, next_recording, prune_specials,
    scheduled_max_minutes, supervisor_should_fire, upcoming_dates, upcoming_events, ScheduledEvent,
    ScheduledEventKind, TriggerKind, MISSED_WINDOW_MS,
};
use sundayrec_core::settings::Settings;

use crate::db::Db;
use crate::error::AppResult;
use crate::recorder::engine::RecorderEngine;
use crate::settings;
use crate::util::lock_recover;

/// How far ahead the supervisor enumerates events before sleeping. A weekly slot
/// recurs at most every 7 days, so 8 always captures the next occurrence of
/// every active timer.
const HORIZON_DAYS: i64 = 8;

/// How many days of upcoming starts the status command reports.
const UPCOMING_DAYS: i64 = 14;

/// How many days of upcoming starts wake scheduling considers.
const WAKE_HORIZON_DAYS: i64 = 14;

/// After firing an event the supervisor sleeps this long before recomputing, so
/// a timer that fired a few ms early can't re-select the same event and
/// double-fire it. Harmless at the scheduler's minute granularity.
const FIRE_GUARD: StdDuration = StdDuration::from_secs(1);

/// Emitted whenever the next scheduled start changes — payload is an ISO-like
/// local string (`YYYY-MM-DDTHH:MM:SS`) or `null`. Drives the tray tooltip / UI.
pub const NEXT_EVENT: &str = "scheduler://next";
/// Emitted when [`check_missed`] finds scheduled recordings that never ran.
pub const MISSED_EVENT: &str = "scheduler://missed";

// ─────────────────────────────────────────────────────────────────────────────
//   Engine (Tauri-managed state)
// ─────────────────────────────────────────────────────────────────────────────

/// Managed-state handle for the scheduler supervisor. At most one supervisor
/// task runs; [`reschedule`](Self::reschedule) wakes it to recompute.
pub struct SchedulerEngine {
    /// Wakes the supervisor to recompute (settings changed / manual reschedule).
    notify: Arc<Notify>,
    /// Guards against spawning the supervisor twice.
    started: Mutex<bool>,
    /// Cached nearest future start, for synchronous status reads.
    next: Arc<Mutex<Option<NaiveDateTime>>>,
}

impl Default for SchedulerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerEngine {
    pub fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            started: Mutex::new(false),
            next: Arc::new(Mutex::new(None)),
        }
    }

    /// Spawn the supervisor loop (idempotent). Called once at setup with the app
    /// handle, through which the supervisor reaches the db pool + recorder engine.
    ///
    /// SAFEGUARD: the supervisor runs inside a SUPERVISING wrapper that re-spawns
    /// it if it ever ends — a panic unwinds the inner task and its `JoinHandle`
    /// resolves, so we restart it after a short delay. A silently-dead scheduler
    /// would miss EVERY future recording, which for a church recorder is the worst
    /// possible failure; this makes that self-healing.
    ///
    /// E2.2: that wrapper used to live here, inline, and was the only one in the
    /// app. It now lives in [`crate::supervise`] — same 300 s healthy threshold,
    /// same 5 s → 30 s backoff, same escalate-once-at-three, same wording — so
    /// every other long-lived task gets it too and there is one implementation
    /// to fix rather than seven to remember.
    pub fn start(&self, app: AppHandle) {
        {
            let mut started = lock_recover(&self.started);
            if *started {
                return;
            }
            *started = true;
        }
        let notify = self.notify.clone();
        let next = self.next.clone();
        let sup_app = app.clone();
        crate::supervise::supervised_spawn(
            app,
            "scheduler::supervisor",
            crate::supervise::TaskAlert {
                title: "SundayRec — planlegger-feil",
                body: "Planleggeren har en vedvarende feil og kan gå glipp av planlagte \
                       opptak. Start appen på nytt; vedvarer det, kjør Diagnose under \
                       Innstillinger → Lyd.",
            },
            move || supervisor(sup_app.clone(), notify.clone(), next.clone()),
        );
    }

    /// Wake the supervisor to recompute its timers (e.g. after settings save).
    pub fn reschedule(&self) {
        self.notify.notify_one();
    }

    /// The cached nearest future start, if any.
    pub fn next_recording(&self) -> Option<NaiveDateTime> {
        *lock_recover(&self.next)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Supervisor loop
// ─────────────────────────────────────────────────────────────────────────────

async fn supervisor(
    app: AppHandle,
    notify: Arc<Notify>,
    next_cache: Arc<Mutex<Option<NaiveDateTime>>>,
) {
    // The late-start safety net (`check_missed`) fires once at startup — an
    // app (re)launched at 11:20 for an 11:00 service must still start the
    // recording (Electron recovered up to 60 min in). It ALSO fires after a
    // suspected system sleep / clock jump (see the oversleep check below):
    // `next_occurrence` only looks FORWARD, so a start that passed while the
    // lid was closed would otherwise wait a whole week. The command
    // `scheduler_check_missed` exists but nothing invoked it — the net was
    // built and never wired (found in the 2026-08-04 night sweep).
    let mut startup_missed_check_done = false;
    loop {
        let pool = match app.try_state::<Db>() {
            Some(db) => db.pool.clone(),
            None => {
                // DB not ready yet — wait for a reschedule signal and retry.
                notify.notified().await;
                continue;
            }
        };

        if !startup_missed_check_done {
            startup_missed_check_done = true;
            match check_missed(&app, &pool).await {
                Ok(missed) if !missed.is_empty() => {
                    tracing::info!("scheduler: startup missed-check reported {}", missed.len());
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("scheduler: startup missed-check failed: {e}"),
            }
        }

        let mut settings = settings::load(&pool).await.unwrap_or_default();

        // Prune specials that ended > 7 days ago and persist the trimmed list.
        let now = Local::now().naive_local();
        let (kept, pruned) = prune_specials(&settings.special_recordings, now);
        if pruned > 0 {
            settings.special_recordings = kept.clone();
            if let Err(e) = settings::save(&pool, settings.clone()).await {
                tracing::warn!("scheduler: pruning save failed: {e}");
            }
            tracing::info!("scheduler: pruned {pruned} expired special(s)");
        }

        // Cache + broadcast the next start.
        // `active_slots()` and not `slots`: the level-1 switch «Ta opp
        // automatisk» is a flag now, not an empty list. See
        // `Settings::active_slots`.
        let nxt = next_recording(settings.active_slots(), &kept, now);
        *lock_recover(&next_cache) = nxt;
        let _ = app.emit(NEXT_EVENT, nxt.map(fmt_dt));

        // Schedule OS wake-from-sleep timers for upcoming recordings (Fase 5.2).
        // Non-admin (no prompt) from the supervisor — the WakeEngine dedups so an
        // unchanged schedule is a cheap no-op. A user-initiated reschedule (which
        // may prompt for admin) goes through the `wake_reschedule` command.
        if settings.wake_from_sleep {
            if let Some(wake) = app.try_state::<crate::wake::WakeEngine>() {
                let upcoming =
                    upcoming_dates(settings.active_slots(), &kept, now, WAKE_HORIZON_DAYS);
                let res = wake.reschedule(&upcoming, now, true, false).await;
                // Best-effort from the supervisor (non-admin, no prompt), but a
                // failure that ISN'T just "needs admin"/"disabled" is worth a
                // breadcrumb — a silently un-scheduled wake means a missed record.
                if !res.ok
                    && !matches!(
                        res.reason.as_deref(),
                        Some("permission") | Some("disabled") | Some("cancelled")
                    )
                {
                    tracing::warn!(
                        "scheduler: background wake reschedule failed: {:?} {:?}",
                        res.reason,
                        res.message
                    );
                }
            }
        }

        let events = upcoming_events(
            settings.active_slots(),
            &kept,
            now,
            settings.reminder_minutes,
            HORIZON_DAYS,
        );

        let Some(ev) = events.first().cloned() else {
            // Nothing scheduled ahead — sleep until a reschedule wakes us.
            notify.notified().await;
            continue;
        };

        let wait_ms = (ev.at - Local::now().naive_local())
            .num_milliseconds()
            .max(0) as u64;
        // SAFEGUARD: never `sleep` a multi-day wait in one go — a tokio timer can
        // drift / under-count across macOS system-sleep, and a clock change (NTP /
        // DST) mid-wait would make the recording fire late or never. Cap the sleep
        // so we re-evaluate against the real wall clock at least every few minutes;
        // only FIRE when this sleep covers the WHOLE remaining wait (otherwise it's
        // a periodic re-check → loop + recompute).
        let sleep_ms = capped_supervisor_sleep_ms(wait_ms);
        let fire_now = supervisor_should_fire(wait_ms);

        let slept_from = Local::now().naive_local();
        tokio::select! {
            _ = tokio::time::sleep(StdDuration::from_millis(sleep_ms)) => {
                // Oversleep = the wall clock advanced far beyond the requested
                // sleep → the machine slept (or the clock jumped). A start that
                // passed during that gap is invisible to the forward-only
                // `next_occurrence`, so run the late-start net before the
                // normal recompute.
                let wall_elapsed_ms = (Local::now().naive_local() - slept_from).num_milliseconds();
                if wall_elapsed_ms.saturating_sub(sleep_ms as i64) > 120_000 {
                    tracing::info!(
                        wall_elapsed_ms,
                        sleep_ms,
                        "scheduler: overslept — running the missed-recording net"
                    );
                    if let Err(e) = check_missed(&app, &pool).await {
                        tracing::warn!("scheduler: post-sleep missed-check failed: {e}");
                    }
                }
                if fire_now {
                    fire(&app, &pool, &settings, &kept, &ev).await;
                    tokio::time::sleep(FIRE_GUARD).await;
                }
                // else: periodic re-check — recompute against the fresh clock.
            }
            _ = notify.notified() => {
                // Settings changed — fall through to recompute.
            }
        }
    }
}

/// Perform a single scheduled event.
async fn fire(
    app: &AppHandle,
    pool: &SqlitePool,
    settings: &Settings,
    specials: &[sundayrec_core::schedule::SpecialRecording],
    ev: &ScheduledEvent,
) {
    match ev.kind {
        ScheduledEventKind::Start => {
            let engine = app.state::<RecorderEngine>();
            // SAFEGUARD: never clobber a recording already in progress (a manual
            // take, or an earlier scheduled one still finalising). Skip + tell the
            // user, leaving the running recording untouched.
            if engine.current_state().is_active() {
                tracing::warn!(
                    "scheduler: a recording is already active — skipping the scheduled start"
                );
                // ALWAYS fires — a skipped scheduled start is a problem report:
                // should_notify pins SkippedBusy on regardless of the
                // notify_start/notify_stop comfort toggles.
                if should_notify(SchedulerNotice::SkippedBusy, settings) {
                    notify_user(
                        app,
                        "SundayRec",
                        "Planlagt opptak hoppet over — et opptak pågår allerede.",
                    );
                }
                return;
            }
            let (custom_name, slot_max) = match ev.source {
                // `active_slots()` — the SAME slice `upcoming_events` indexed,
                // so `Slot(i)` keeps meaning what it meant when it was made.
                TriggerKind::Slot(i) => (
                    None,
                    settings
                        .active_slots()
                        .get(i)
                        .and_then(|s| s.max)
                        .unwrap_or(0)
                        .max(0) as u32,
                ),
                TriggerKind::Special(i) => (specials.get(i).map(|s| s.name.clone()), 0u32),
            };
            // SAFEGUARD: a scheduled recording ALWAYS carries a max-duration
            // backstop, so even a missed Stop event can't leave it recording until
            // the disk fills.
            let max_minutes = scheduled_max_minutes(slot_max);
            match crate::recorder::opts::build_opts(
                app,
                settings,
                custom_name.as_deref(),
                max_minutes,
                None,
            ) {
                Ok(opts) => {
                    // SAFEGUARD: bound the start. A stuck device-open must not wedge
                    // the supervisor (which would then miss EVERY later recording).
                    match tokio::time::timeout(
                        StdDuration::from_secs(30),
                        engine.start(app.clone(), Some(pool.clone()), opts, None),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            // SCHEDULED, as opposed to the manual `start_recording`
                            // command: whether churches actually rely on the
                            // scheduler is the single most useful thing this
                            // counter set can answer.
                            crate::telemetry::counters::count(
                                sundayrec_core::telemetry::CounterName::RecordingStartedScheduled,
                            );
                            tracing::info!("scheduler: started scheduled recording");
                            // R3-H: «Varsel på PC når opptak starter» — the
                            // unattended case is exactly when this is useful (a
                            // manual start needs no notification; the operator
                            // just pressed the button). Gated; the FAILURE arms
                            // below are not.
                            if should_notify(SchedulerNotice::StartedScheduled, settings) {
                                notify_user(app, "SundayRec", "Planlagt opptak startet.");
                            }
                        }
                        // A scheduled recording that does not start is the single
                        // worst thing this app can do quietly: nobody is watching
                        // the screen at 11:00, and the service is not repeatable.
                        // These three were native-notification-only — the same
                        // wording now goes out through the full dispatch (native
                        // + e-mail + webhook); see `crate::notify`.
                        Ok(Err(e)) => {
                            tracing::error!("scheduler: scheduled start failed: {e}");
                            dispatch_scheduler_failure(
                                app,
                                "scheduled_start_failed",
                                format!("Planlagt opptak startet ikke: {e}"),
                            )
                            .await;
                        }
                        Err(_) => {
                            tracing::error!("scheduler: scheduled start TIMED OUT after 30s");
                            dispatch_scheduler_failure(
                                app,
                                "scheduled_start_timeout",
                                "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon."
                                    .to_string(),
                            )
                            .await;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("scheduler: could not build opts: {e}");
                    dispatch_scheduler_failure(
                        app,
                        "scheduled_prepare_failed",
                        format!("Planlagt opptak kunne ikke forberedes: {e}"),
                    )
                    .await;
                }
            }
        }
        ScheduledEventKind::Stop => {
            app.state::<RecorderEngine>().stop();
            tracing::info!("scheduler: stop fired");
            // R3-H: «Varsel på PC når opptak avsluttes». Fires when the
            // scheduled stop is DISPATCHED (finalisation continues in the
            // engine); a stop that later fails to finalise reaches the operator
            // through the failure dispatch, which is never gated.
            if should_notify(SchedulerNotice::StoppedScheduled, settings) {
                notify_user(app, "SundayRec", "Planlagt opptak avsluttet.");
            }
        }
        ScheduledEventKind::Reminder => {
            let body = reminder_body(settings.language.as_deref(), settings.reminder_minutes);
            // ALWAYS fires (should_notify pins Reminder on — not governed by
            // notify_start/notify_stop): the reminder has its own opt-in,
            // `reminder_minutes` = 0 means the event is never scheduled at all.
            if should_notify(SchedulerNotice::Reminder, settings) {
                notify_user(app, "SundayRec", &body);
            }
        }
        ScheduledEventKind::Preflight => {
            run_scheduled_preflight(app, pool, settings).await;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Preflight + missed-check
// ─────────────────────────────────────────────────────────────────────────────

async fn run_scheduled_preflight(app: &AppHandle, pool: &SqlitePool, settings: &Settings) {
    use sundayrec_core::preflight::PreflightSeverity;
    let documents = crate::save_folder::documents_dir(app);
    let outcome = crate::preflight::run_preflight_detailed(pool, documents.as_deref()).await;
    let findings = outcome.findings;
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == PreflightSeverity::Error)
        .collect();
    if let Some(first) = errors.first() {
        // ALWAYS fires — a preflight ERROR half an hour before a service is a
        // problem report: should_notify pins PreflightFinding on regardless of
        // the notify_start/notify_stop comfort toggles.
        if should_notify(SchedulerNotice::PreflightFinding, settings) {
            notify_user(app, "SundayRec — sjekk før opptak", &first.message);
        }
    }

    // The preflight card only appears if someone opens the app. A configured
    // mixer that is not plugged in, half an hour before a scheduled recording,
    // is the single most common and most preventable cause of a lost service —
    // so it also goes out as a live warning, carrying the device NAME so the
    // operator knows what to go and find.
    if !outcome.facts.device_present {
        let name = outcome.device_name.unwrap_or_default();
        crate::notify::warn(
            app,
            sundayrec_core::notify::BackendWarning::error(
                sundayrec_core::notify::code::DEVICE_MISSING,
            )
            .msg(format!("Lydenheten «{name}» er ikke tilkoblet."))
            .param("device", name),
        );
    }

    let _ = app.emit("scheduler://preflight", &findings);
}

/// A missed scheduled recording, surfaced to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "MissedRecordingInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct MissedRecordingInfo {
    /// ISO-like local start time the recording was supposed to begin.
    pub at: String,
    /// Human-readable label.
    pub label: String,
}

/// On-demand missed-recording check (call at startup / resume). Late-starts any
/// slot/special currently inside the 60-min window, then returns + emits the
/// older occurrences that were missed. See the module header for the
/// persistence gap.
pub async fn check_missed(
    app: &AppHandle,
    pool: &SqlitePool,
) -> AppResult<Vec<MissedRecordingInfo>> {
    let settings = settings::load(pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let specials = &settings.special_recordings;

    // Late-start anything active right now (unless a recording is already going).
    let recording = !matches!(
        app.state::<RecorderEngine>().current_state(),
        sundayrec_core::recorder::RecorderState::Idle
    );
    let triggers = active_within(settings.active_slots(), specials, now, MISSED_WINDOW_MS);
    let mut triggered_keys = std::collections::HashSet::new();
    for t in &triggers {
        triggered_keys.insert(t.key.clone());
        if recording {
            continue;
        }
        let (custom_name, max_minutes) = match t.kind {
            TriggerKind::Slot(i) => (
                None,
                settings
                    .active_slots()
                    .get(i)
                    .and_then(|s| s.max)
                    .unwrap_or(0)
                    .max(0) as u32,
            ),
            TriggerKind::Special(i) => (specials.get(i).map(|s| s.name.clone()), 0u32),
        };
        match crate::recorder::opts::build_opts(
            app,
            &settings,
            custom_name.as_deref(),
            max_minutes,
            None,
        ) {
            Ok(opts) => {
                let engine = app.state::<RecorderEngine>();
                let late = engine
                    .start(app.clone(), Some(pool.clone()), opts, None)
                    .await;
                if late.is_ok() {
                    crate::telemetry::counters::count(
                        sundayrec_core::telemetry::CounterName::RecordingStartedScheduled,
                    );
                }
                if let Err(e) = late {
                    tracing::error!("scheduler: late-start of missed recording failed: {e}");
                    // The recovery attempt for an already-missed recording just
                    // failed too. Same wording, now on every configured channel.
                    dispatch_scheduler_failure(
                        app,
                        "scheduled_late_start_failed",
                        format!("Forsinket oppstart av planlagt opptak feilet: {e}"),
                    )
                    .await;
                }
            }
            Err(e) => tracing::error!("scheduler: could not build opts for late-start: {e}"),
        }
    }

    // History start times → local naive, for the dedup window.
    let history = recording_history_local(pool).await;
    let missed = missed_recordings(
        settings.active_slots(),
        specials,
        now,
        &history,
        &triggered_keys,
    );
    let out: Vec<MissedRecordingInfo> = missed
        .into_iter()
        .map(|m| MissedRecordingInfo {
            at: fmt_dt(m.when),
            label: m.label,
        })
        .collect();
    if !out.is_empty() {
        let _ = app.emit(MISSED_EVENT, &out);
    }
    Ok(out)
}

/// Recording start times converted to the local-wall `NaiveDateTime` frame the
/// core compares in.
async fn recording_history_local(pool: &SqlitePool) -> Vec<NaiveDateTime> {
    let rows = crate::db::store::list_recordings(pool)
        .await
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            Local
                .timestamp_millis_opt(r.started_at as i64)
                .single()
                .map(|dt| dt.naive_local())
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//   Status (for commands)
// ─────────────────────────────────────────────────────────────────────────────

/// The scheduler snapshot the UI renders: the next start and the next 14 days
/// of starts (ISO-like local strings).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ScheduleStatus.ts")]
#[serde(rename_all = "camelCase")]
pub struct ScheduleStatus {
    pub next: Option<String>,
    pub upcoming: Vec<String>,
}

/// Compute the current [`ScheduleStatus`] from persisted settings.
pub async fn status(pool: &SqlitePool) -> AppResult<ScheduleStatus> {
    let settings = settings::load(pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let next =
        next_recording(settings.active_slots(), &settings.special_recordings, now).map(fmt_dt);
    let upcoming = upcoming_dates(
        settings.active_slots(),
        &settings.special_recordings,
        now,
        UPCOMING_DAYS,
    )
    .into_iter()
    .map(fmt_dt)
    .collect();
    Ok(ScheduleStatus { next, upcoming })
}

// ─────────────────────────────────────────────────────────────────────────────
//   Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format a wall-clock datetime as `YYYY-MM-DDTHH:MM:SS` (no zone — it's already
/// local). The UI parses it with `new Date(...)`, which treats a zone-less
/// string as local time, matching the frame it was produced in.
fn fmt_dt(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Fire a native OS notification. Now a one-line delegation to
/// [`crate::notify::native`]: the helper used to be private here, which is part
/// of why the recorder's failures never produced one — there was nothing shared
/// to call. The reminder + preflight call sites below are unchanged.
fn notify_user(app: &AppHandle, title: &str, body: &str) {
    crate::notify::native(app, title, body);
}

/// The scheduler's notification classes, for [`should_notify`]. Only the two
/// SUCCESS notices are operator-silenceable; everything else is a problem
/// report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerNotice {
    /// «Planlagt opptak startet.» — governed by `notify_start`.
    StartedScheduled,
    /// «Planlagt opptak avsluttet.» — governed by `notify_stop`.
    StoppedScheduled,
    /// A scheduled start was skipped because a recording is already active.
    SkippedBusy,
    /// The pre-service reminder («Opptak starter om N minutter»). Its own gate
    /// is `reminder_minutes` (0 = the event is never scheduled at all).
    Reminder,
    /// A scheduled-preflight ERROR finding («sjekk før opptak»).
    PreflightFinding,
}

/// Whether the operator's «Varsle når opptak starter/stopper» toggles
/// (`notify_start`/`notify_stop` — R3-H, the first thing that ever READ them)
/// allow this notice.
///
/// INVARIANT, pinned by `failure_notices_ignore_the_toggles` below: only the
/// two success notices are gated. Every failure/problem class — the skipped
/// start, preflight findings, and everything routed through
/// [`dispatch_scheduler_failure`]/[`crate::notify::dispatch_failure`] (which
/// deliberately never consults this function) — ALWAYS fires: a failed
/// recording mid-service must never be silenced by a comfort toggle.
fn should_notify(notice: SchedulerNotice, settings: &Settings) -> bool {
    match notice {
        SchedulerNotice::StartedScheduled => settings.notify_start,
        SchedulerNotice::StoppedScheduled => settings.notify_stop,
        SchedulerNotice::SkippedBusy
        | SchedulerNotice::Reminder
        | SchedulerNotice::PreflightFinding => true,
    }
}

/// A scheduled recording did not happen. Routes the SAME sentence the operator
/// used to see as a bare native notification through the full dispatch, so it
/// also reaches the inbox and the chat channel of whoever configured them.
///
/// `code` is the stable machine code (webhook `category`); `message` is the
/// existing Norwegian wording, passed through verbatim — the native leg of the
/// dispatch shows exactly what this site showed before.
async fn dispatch_scheduler_failure(app: &AppHandle, code: &str, message: String) {
    crate::notify::dispatch_failure(
        app,
        crate::notify::FailureCtx::now(
            code,
            message,
            sundayrec_core::notify::FailureSource::Scheduler,
        ),
    )
    .await;
}

/// The localised "recording starts in N minutes" body. Ports the Electron
/// `REMINDER_LABELS` map; unknown languages fall back to Norwegian.
fn reminder_body(lang: Option<&str>, minutes: i32) -> String {
    let tpl = match lang.unwrap_or("no") {
        "en" => "Recording starts in {min} minutes",
        "de" => "Aufnahme beginnt in {min} Minuten",
        "sv" => "Inspelning börjar om {min} minuter",
        "da" => "Optagelse starter om {min} minutter",
        "pl" => "Nagranie rozpocznie się za {min} minut",
        "fr" => "Enregistrement dans {min} minutes",
        _ => "Opptak starter om {min} minutter",
    };
    tpl.replace("{min}", &minutes.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── R3-H: the notify_start/notify_stop gate ──────────────────────────────

    #[test]
    fn the_toggles_silence_exactly_their_own_success_notice() {
        // «Varsle når opptak starter/stopper» OFF → that notice is suppressed.
        // Before R3-H nothing read these settings at all (the toggles saved and
        // changed nothing).
        let mut s = Settings {
            notify_start: false,
            notify_stop: true,
            ..Settings::default()
        };
        assert!(!should_notify(SchedulerNotice::StartedScheduled, &s));
        assert!(should_notify(SchedulerNotice::StoppedScheduled, &s));

        s.notify_start = true;
        s.notify_stop = false;
        assert!(should_notify(SchedulerNotice::StartedScheduled, &s));
        assert!(!should_notify(SchedulerNotice::StoppedScheduled, &s));
    }

    #[test]
    fn failure_notices_ignore_the_toggles() {
        // THE invariant: with BOTH comfort toggles off, every problem-report
        // class still fires. A failed or skipped recording mid-service must
        // never be silenced — the failure dispatch path
        // (dispatch_scheduler_failure → notify::dispatch_failure) never even
        // consults should_notify, and the classes it and the direct sites use
        // are pinned always-on here.
        let s = Settings {
            notify_start: false,
            notify_stop: false,
            ..Settings::default()
        };
        assert!(should_notify(SchedulerNotice::SkippedBusy, &s));
        assert!(should_notify(SchedulerNotice::PreflightFinding, &s));
        assert!(should_notify(SchedulerNotice::Reminder, &s));
    }

    #[test]
    fn fmt_dt_is_zoneless_local_iso() {
        let dt = NaiveDateTime::parse_from_str("2026-06-07 11:00", "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(fmt_dt(dt), "2026-06-07T11:00:00");
    }

    #[test]
    fn reminder_body_localises_and_falls_back() {
        assert_eq!(
            reminder_body(Some("en"), 15),
            "Recording starts in 15 minutes"
        );
        assert_eq!(
            reminder_body(Some("no"), 10),
            "Opptak starter om 10 minutter"
        );
        // Unknown language → Norwegian.
        assert_eq!(reminder_body(Some("xx"), 5), "Opptak starter om 5 minutter");
        assert_eq!(reminder_body(None, 30), "Opptak starter om 30 minutter");
    }

    // ── Scheduler decision contract (time-injected) ─────────────────────────
    //
    // The supervisor (`fire`) + `check_missed` thread these pure core decisions
    // to pick the next start, late-start an active slot/special, and surface what
    // was missed. The supervisor itself needs an `AppHandle` (a live recorder +
    // notifier), so these exercise the SAME decisions the shell threads, with an
    // injected `now` — no clock, no app, no device.
    use std::collections::HashSet;
    use sundayrec_core::schedule::{
        active_within, missed_recordings, next_recording, upcoming_events, ScheduleSlot,
        ScheduledEventKind, SpecialRecording, TriggerKind, MISSED_WINDOW_MS,
    };

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    /// A Sunday 11:00–12:00 weekly slot (weekday 6 = Sunday in the Mon=0 frame).
    fn sunday_slot() -> ScheduleSlot {
        ScheduleSlot {
            days: vec![6],
            start: "11:00".into(),
            stop: "12:00".into(),
            max: None,
        }
    }

    fn special(date: &str, start: &str, stop: &str, name: &str) -> SpecialRecording {
        SpecialRecording {
            id: Some(format!("sp-{date}")),
            date: date.into(),
            name: name.into(),
            start: start.into(),
            stop: stop.into(),
            device_id: None,
        }
    }

    #[test]
    fn next_start_picks_the_nearest_future_occurrence() {
        // 2026-06-03 is a Wednesday at 09:00 → the next Sunday 11:00 start is
        // 2026-06-07 11:00.
        let now = dt("2026-06-03 09:00");
        let next = next_recording(&[sunday_slot()], &[], now).unwrap();
        assert_eq!(fmt_dt(next), "2026-06-07T11:00:00");
    }

    #[test]
    fn late_start_triggers_a_slot_inside_the_missed_window() {
        // 30 min past the Sunday 11:00 start (window is 60 min) → still triggerable.
        let now = dt("2026-06-07 11:30");
        let triggers = active_within(&[sunday_slot()], &[], now, MISSED_WINDOW_MS);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].kind, TriggerKind::Slot(0));
    }

    #[test]
    fn no_late_start_once_past_the_missed_window() {
        // 90 min past start → beyond the 60-min late-start window, so the
        // supervisor would NOT late-start it (it becomes a missed candidate).
        let now = dt("2026-06-07 12:30");
        assert!(active_within(&[sunday_slot()], &[], now, MISSED_WINDOW_MS).is_empty());
    }

    #[test]
    fn missed_check_reports_a_stale_uncovered_occurrence() {
        // 2 h past the Sunday start: outside the late-start window, recent enough
        // to matter, no history covering it, not currently triggered → missed.
        let now = dt("2026-06-07 13:00");
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &HashSet::new());
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].when, dt("2026-06-07 11:00"));
    }

    #[test]
    fn missed_check_suppressed_when_history_covers_the_occurrence() {
        // A recording within ±30 min of the scheduled start means it DID run.
        let now = dt("2026-06-07 13:00");
        let history = [dt("2026-06-07 11:05")];
        let missed = missed_recordings(&[sunday_slot()], &[], now, &history, &HashSet::new());
        assert!(missed.is_empty(), "covered by history → not missed");
    }

    #[test]
    fn missed_check_suppressed_when_already_triggered() {
        // If the supervisor already late-started this occurrence (its key is in the
        // triggered set), it must not ALSO be logged as missed (no double-count).
        let now = dt("2026-06-07 13:00");
        let triggers = active_within(
            &[sunday_slot()],
            &[],
            dt("2026-06-07 11:30"),
            MISSED_WINDOW_MS,
        );
        let keys: HashSet<String> = triggers.into_iter().map(|t| t.key).collect();
        assert!(!keys.is_empty(), "precondition: the slot was triggerable");
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &keys);
        assert!(missed.is_empty(), "already triggered → not missed");
    }

    #[test]
    fn overlapping_slot_and_special_both_late_start() {
        // A weekly slot AND a dated special both start at the same time: the
        // supervisor late-starts each independently (two distinct triggers, two
        // distinct dedup keys).
        let now = dt("2026-06-07 11:20");
        let sp = special("2026-06-07", "11:00", "12:00", "Konfirmasjon");
        let triggers = active_within(
            &[sunday_slot()],
            std::slice::from_ref(&sp),
            now,
            MISSED_WINDOW_MS,
        );
        assert_eq!(triggers.len(), 2, "slot + special both active");
        let kinds: Vec<TriggerKind> = triggers.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TriggerKind::Slot(0)));
        assert!(kinds.contains(&TriggerKind::Special(0)));
        let keys: HashSet<&str> = triggers.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys.len(), 2, "distinct dedup keys");
    }

    #[test]
    fn special_wins_when_it_is_the_nearest_future_start() {
        // A dated special on Wednesday beats the next Sunday slot.
        let now = dt("2026-06-03 08:00");
        let sp = special("2026-06-03", "10:00", "11:00", "Begravelse");
        let next = next_recording(&[sunday_slot()], std::slice::from_ref(&sp), now).unwrap();
        assert_eq!(fmt_dt(next), "2026-06-03T10:00:00");
    }

    #[test]
    fn upcoming_events_emit_a_reminder_lead_before_the_start() {
        // With a 15-min reminder lead the supervisor fires a Reminder event 15 min
        // before the Sunday 11:00 Start.
        let now = dt("2026-06-03 09:00");
        let events = upcoming_events(&[sunday_slot()], &[], now, 15, 8);
        let reminder = events
            .iter()
            .find(|e| e.kind == ScheduledEventKind::Reminder)
            .expect("a reminder event");
        assert_eq!(fmt_dt(reminder.at), "2026-06-07T10:45:00");
        // The Start fires at the slot time itself.
        let start = events
            .iter()
            .find(|e| e.kind == ScheduledEventKind::Start)
            .expect("a start event");
        assert_eq!(fmt_dt(start.at), "2026-06-07T11:00:00");
        // The reminder precedes the start.
        assert!(reminder.at < start.at);
    }

    // ── «Ta opp automatisk» as a FLAG (P1b) ────────────────────────────────
    //
    // Before `auto_record_enabled` the only spelling of "off" was an empty
    // `slots` list, so the UI's switch had to delete the time. These three pin
    // the flag at exactly the composition `status()`, the supervisor and
    // `check_missed` use — `settings.active_slots()` in, the core decision out.

    #[test]
    fn auto_record_off_removes_the_weekly_plan_from_the_next_start() {
        // The scheduler_status journey: a stored Sunday slot, the switch off.
        // `status()` computes exactly this, so `next` comes back null.
        let now = dt("2026-06-03 09:00");
        let mut settings = Settings {
            slots: vec![sunday_slot()],
            ..Settings::default()
        };

        // On (the default) → unchanged behaviour.
        assert!(settings.auto_record_enabled, "a fresh profile is armed");
        let on = next_recording(settings.active_slots(), &settings.special_recordings, now);
        assert_eq!(
            fmt_dt(on.expect("armed → a next start")),
            "2026-06-07T11:00:00"
        );

        // Off → nothing planned, and the TIME IS STILL THERE.
        settings.auto_record_enabled = false;
        assert!(
            next_recording(settings.active_slots(), &settings.special_recordings, now).is_none(),
            "disarmed → no next start"
        );
        assert_eq!(
            settings.slots.len(),
            1,
            "the switch must not delete the plan"
        );
    }

    #[test]
    fn auto_record_off_also_silences_the_late_start_and_the_missed_report() {
        // The half a `next == null` assertion alone would miss: the machine must
        // not late-start a slot it has been told not to plan, and must not report
        // it as missed either — "you missed a recording you switched off" is a
        // warning that teaches people to ignore warnings.
        let now = dt("2026-06-07 11:03");
        let mut settings = Settings {
            slots: vec![sunday_slot()],
            ..Settings::default()
        };
        assert_eq!(
            active_within(settings.active_slots(), &[], now, MISSED_WINDOW_MS).len(),
            1,
            "armed → inside the window"
        );

        settings.auto_record_enabled = false;
        assert!(active_within(settings.active_slots(), &[], now, MISSED_WINDOW_MS).is_empty());
        assert!(missed_recordings(
            settings.active_slots(),
            &[],
            dt("2026-06-07 11:30"),
            &[],
            &HashSet::new()
        )
        .is_empty());
    }

    #[test]
    fn auto_record_off_does_not_cancel_a_dated_special() {
        // A special is a date somebody entered by hand for one concert. The
        // level-1 switch is about the WEEKLY plan; cancelling the concert too
        // would be the switch deleting something it never showed.
        let now = dt("2026-06-03 09:00");
        let settings = Settings {
            auto_record_enabled: false,
            slots: vec![sunday_slot()],
            special_recordings: vec![special("2026-06-05", "19:00", "21:00", "Konsert")],
            ..Settings::default()
        };
        let next = next_recording(settings.active_slots(), &settings.special_recordings, now);
        assert_eq!(
            fmt_dt(next.expect("the special still stands")),
            "2026-06-05T19:00:00"
        );
    }

    #[test]
    fn missed_check_ignores_an_occurrence_older_than_24h() {
        // Last Sunday's slot, checked the FOLLOWING Sunday before its start: older
        // than the 24 h log window → not reported (avoids week-old noise).
        let now = dt("2026-06-14 10:00");
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &HashSet::new());
        assert!(missed.is_empty(), "older than 24h → not logged");
    }
}
