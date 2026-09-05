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

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use ts_rs::TS;

use sundayrec_core::schedule::{
    active_within, capped_supervisor_sleep_ms, late_start_choice, missed_recordings,
    next_recording, prune_specials, scheduled_max_minutes, supervisor_should_fire, upcoming_dates,
    upcoming_events, CoveredWindow, ScheduledEvent, ScheduledEventKind, TriggerKind,
    MISSED_WINDOW_MS,
};
use sundayrec_core::settings::Settings;
use sundayrec_core::wake::background_wake_log_action;

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

/// How many EXPECTED background wake failures (needs-admin / disabled / the
/// prompt dismissed) this process has already reported.
///
/// The supervisor re-runs on every settings change and every timer, so the
/// choice is between one log line per pass and none at all — and "none at all"
/// is what shipped: `permission`, the failure that means this machine will sleep
/// through the service, was filtered to silence. One report per launch, the rest
/// counted. It re-arms on the next start, which is also the next time the
/// answer can have changed.
static QUIET_WAKE_REPORTS: AtomicU32 = AtomicU32::new(0);

/// Emitted whenever the next scheduled start changes — payload is an ISO-like
/// local string (`YYYY-MM-DDTHH:MM:SS`) or `null`. Drives the tray tooltip / UI.
pub const NEXT_EVENT: &str = "scheduler://next";
/// Emitted when [`check_missed`] finds scheduled recordings that never ran.
pub const MISSED_EVENT: &str = "scheduler://missed";

// ─────────────────────────────────────────────────────────────────────────────
//   The scheduled-run marker
// ─────────────────────────────────────────────────────────────────────────────

/// What a scheduled recording leaves behind when it STARTS, so the receipt can
/// describe it after it finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledRun {
    /// The schedule's own name for the occurrence — the weekly slot's times, or
    /// a special's name. Same wording [`missed_recordings`] puts on a missed
    /// one, so the two mails call the same slot the same thing.
    pub slot: String,
    /// When the engine actually accepted the start (local wall clock). The
    /// receipt's "Start", and with the finish time its duration.
    pub started_at: DateTime<Local>,
}

/// Managed state: the ONE scheduled run that is currently in flight, if any.
///
/// ## Why a marker and not a look at the recording
///
/// The receipt exists for the person who was NOT in the building: a scheduled
/// recording ran unattended and they would like to know it worked. A manual one
/// needs no mail — whoever pressed Start watched the app finish. But
/// `recording://finished` says only where the file landed; nothing on that event,
/// and nothing in the recorder at all, remembers which button began the take.
/// Rather than teach the hardware-verified capture path to carry a reporting
/// flag, the scheduler stamps this on its way past and the dispatcher consumes
/// it.
///
/// ## Set once, taken once
///
/// Stamped after a SUCCESSFUL `engine.start` in both branches that have one (the
/// ordinary timer and `check_missed`'s late start), consumed by the finish
/// dispatch, and dropped by a recorder failure — a run that died did not finish.
/// Taking rather than reading is what stops a marker from outliving its run and
/// being claimed by the next manual recording.
#[derive(Debug, Default)]
pub struct ScheduledRunMarker(Mutex<Option<ScheduledRun>>);

impl ScheduledRunMarker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember that THIS run was scheduled. Overwrites any stale marker: a new
    /// start means the previous run is over one way or another, and the fresher
    /// fact is the true one.
    pub fn set(&self, run: ScheduledRun) {
        *lock_recover(&self.0) = Some(run);
    }

    /// Consume the marker, if there is one.
    pub fn take(&self) -> Option<ScheduledRun> {
        lock_recover(&self.0).take()
    }
}

/// The schedule's own name for a trigger, in the wording
/// [`sundayrec_core::schedule::missed_recordings`] uses for a missed one.
///
/// Stated once here so the receipt and the missed alert cannot name the same
/// Sunday two different ways — which is exactly the confusion a volunteer
/// comparing two mails would have to resolve on their own.
fn trigger_label(settings: &Settings, kind: TriggerKind) -> String {
    match kind {
        TriggerKind::Slot(i) => settings
            .active_slots()
            .get(i)
            .map(|s| format!("Ukentlig opptak ({}–{})", s.start, s.stop))
            .unwrap_or_else(|| "Ukentlig opptak".to_string()),
        TriggerKind::Special(i) => settings
            .special_recordings
            .get(i)
            .map(|s| s.name.trim())
            .filter(|n| !n.is_empty())
            .unwrap_or("Spesialopptak")
            .to_string(),
    }
}

/// Stamp the marker after a scheduled start the engine accepted.
///
/// A missing marker state is not an error worth a log line per start: the only
/// way here without one is a build that forgot to manage it, which the receipt
/// test would have caught long before a church saw it.
fn mark_scheduled_run(app: &AppHandle, settings: &Settings, kind: TriggerKind) {
    if let Some(marker) = app.try_state::<ScheduledRunMarker>() {
        marker.set(ScheduledRun {
            slot: trigger_label(settings, kind),
            started_at: Local::now(),
        });
    }
}

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
                // Best-effort from the supervisor (non-admin, no prompt) — but
                // "best-effort" used to mean `permission`/`disabled`/`cancelled`
                // were filtered to SILENCE, and `permission` is the failure that
                // matters most: this pass may not prompt, the interactive
                // `wake_reschedule` is the only path that can, and if nobody
                // presses it the machine sleeps through the service. Filtered to
                // nothing, the first evidence was a missing recording.
                //
                // The supervisor re-runs on every settings change and every
                // timer, so the expected failures are reported ONCE per launch
                // and silently counted after that
                // (`background_wake_log_action`). A real failure still logs
                // every time, and never spends that one report.
                let quiet_so_far = QUIET_WAKE_REPORTS.load(Ordering::Relaxed);
                let action =
                    background_wake_log_action(res.ok, res.reason.as_deref(), quiet_so_far);
                if action.logs() {
                    tracing::warn!(
                        reason = ?res.reason,
                        message = ?res.message,
                        "scheduler: background wake reschedule failed — the machine may not \
                         wake for the next recording. An admin-capable retry is the «Registrer \
                         vekkinger» button (`wake_reschedule`); further notices of this kind \
                         are counted, not logged, until the next launch"
                    );
                }
                if action.counts() {
                    QUIET_WAKE_REPORTS.fetch_add(1, Ordering::Relaxed);
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
                            // …and the marker the receipt is gated on. Only after
                            // a start the engine ACCEPTED: a receipt for a
                            // recording that never began would be the worst kind
                            // of mail this app could send.
                            mark_scheduled_run(app, settings, ev.source);
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

/// On-demand missed-recording check (call at startup / resume). Late-starts ONE
/// slot/special currently inside the 60-min window, then returns + emits the
/// older occurrences that were missed. See the module header for the
/// persistence gap.
///
/// The two halves pull in opposite directions after a crash, and both are right:
/// late-starting a service already in progress is WANTED (the second half of the
/// sermon is better than nothing), while reporting that same service as missed is
/// the bug — the recording exists, in fragments the recovery task is still
/// finalising. `covered_windows_local` is what tells the second half about work
/// the first half's own subsystem has not finished writing down.
pub async fn check_missed(
    app: &AppHandle,
    pool: &SqlitePool,
) -> AppResult<Vec<MissedRecordingInfo>> {
    let settings = settings::load(pool).await.unwrap_or_default();
    let now = Local::now().naive_local();
    let specials = &settings.special_recordings;

    // Late-start what is active right now. EVERY active trigger counts as
    // handled by this pass (its key goes into the dedup set below, so the missed
    // report does not also claim it), but at most ONE may reach the recorder:
    // `RecorderEngine::start` stops whatever is running before it starts, so a
    // second start does not add a recording — it kills the one that began
    // 200 ms ago. `late_start_choice` makes "one pass, one start" a property of
    // the core rather than a discipline this loop had to keep, and it is handed
    // an engine reading taken one statement earlier. The old code read the
    // engine ONCE, above a `for`, and every iteration after the first acted on a
    // fact the first had already invalidated (F1 finding A4).
    let triggers = active_within(settings.active_slots(), specials, now, MISSED_WINDOW_MS);
    let triggered_keys: std::collections::HashSet<String> =
        triggers.iter().map(|t| t.key.clone()).collect();
    // `is_active()`, the same predicate `fire()` uses — not "anything but
    // `Idle`". `Idle` is the never-yet-started engine; a machine that recorded
    // this morning sits in `Stopped` forever after, and the old test therefore
    // switched the late-start net OFF for the rest of the day. That included the
    // post-oversleep pass, which is exactly when an evening service that passed
    // while the lid was shut needs it.
    let busy = app.state::<RecorderEngine>().current_state().is_active();
    if let Some(t) = late_start_choice(&triggers, busy) {
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
                    // The SECOND branch that starts a scheduled recording, and
                    // the one that is easy to forget: a late start is still a
                    // start nobody watched, so it earns the same receipt.
                    mark_scheduled_run(app, &settings, t.kind);
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
    // …and the recordings the database does not know about YET: a crash leaves a
    // session manifest behind, and startup recovery is still concatenating it
    // into a history row while this runs. Without these windows the sweep
    // reports a service that is being salvaged one task over as never recorded
    // (F1 finding A10) — which, with the missed dispatch behind it, is a desktop
    // notification and an e-mail saying so.
    let covered = covered_windows_local(crate::recorder::recovery::pending_windows(app));
    let missed = missed_recordings(
        settings.active_slots(),
        specials,
        now,
        &history,
        &covered,
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
        report_missed(app, pool, &out).await;
    }
    Ok(out)
}

/// An epoch-ms instant in the local-wall frame the core compares in.
fn local_naive(ms: u64) -> Option<NaiveDateTime> {
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|dt| dt.naive_local())
}

/// Unfinalised crash-recovery manifests → the local-wall windows
/// [`missed_recordings`] treats as evidence that something DID record.
///
/// The same conversion [`recording_history_local`] performs on stored history,
/// for the same reason: the decision core is tz-free by construction, and this
/// is the seam where wall time is chosen.
///
/// A pair that cannot be represented at all (a nonsense timestamp in a manifest
/// somebody hand-edited) is dropped rather than guessed. Dropping one costs a
/// missed-report that may be false — exactly where the app already was — while
/// guessing could silence a genuine one.
pub(crate) fn covered_windows_local(pending: Vec<(u64, u64)>) -> Vec<CoveredWindow> {
    pending
        .into_iter()
        .filter_map(|(start_ms, last_seen_ms)| {
            Some(CoveredWindow {
                start: local_naive(start_ms)?,
                last_seen: local_naive(last_seen_ms)?,
            })
        })
        .collect()
}

/// Tell somebody about the Sundays that were not recorded.
///
/// ## The hole this closes
///
/// `settings.rs` has promised an e-mail "when a recording fails / a scheduled
/// one is missed" since the Electron port. The first half was wired in P; the
/// second half sent NOTHING — [`check_missed`] emitted an event to a renderer
/// that may not be running and stopped there. A church whose machine slept
/// through Sunday morning learned about it when somebody asked for the
/// recording.
///
/// ## Once per occurrence, durably
///
/// [`check_missed`] runs at startup AND after every wake, so the same Sunday is
/// rediscovered every time the app launches for as long as it stays inside the
/// 24-hour window. The `notify_seen` ledger is what makes that one alert instead
/// of five, and it is a TABLE rather than the in-memory `AlertGate` for exactly
/// that reason: the repeats are separated by restarts, which is precisely what
/// RAM does not survive.
///
/// The ledger is stamped AFTER the dispatch, not before: the relay leg reads the
/// same rows to decide whether it has already sent this, and a row written first
/// would suppress the very mail this call is making. Both writes are in one
/// task, so there is no window between them for a second observer to slip
/// through — and the outbox's unique dedup key holds even if there were.
///
/// ⚠️ **The native notification now fires too, and so does SMTP where it is
/// configured.** That is a behaviour change for existing users, not just for
/// relay subscribers: a missed service used to be silent on every channel. It is
/// deliberate — an unrecorded service is exactly the news an operator standing
/// at the machine can still act on (the next slot is in the schedule too) — and
/// it is the one part of this feature that needed the owner's word before it
/// merged.
async fn report_missed(app: &AppHandle, pool: &SqlitePool, missed: &[MissedRecordingInfo]) {
    use sundayrec_core::relay::SeenScope;

    let settings = settings::load(pool).await.unwrap_or_default();
    let lang = sundayrec_core::email::MailLang::from_code(settings.language.as_deref());
    let fmt = sundayrec_core::notify::alert_date_format(lang);
    let now = crate::util::now_ms();

    let fresh = unreported_missed(pool, missed, fmt, now).await;
    if fresh.is_empty() {
        tracing::debug!("scheduler: every missed occurrence had already been reported");
        return;
    }

    let message = missed_summary(&fresh);
    tracing::warn!(
        count = fresh.len(),
        "scheduler: reporting missed scheduled recording(s)"
    );
    crate::notify::dispatch_failure(
        app,
        crate::notify::FailureCtx::now(
            crate::notify::CODE_SCHEDULED_MISSED,
            message,
            sundayrec_core::notify::FailureSource::Missed,
        )
        .with_missed(fresh.clone()),
    )
    .await;

    for slot in &fresh {
        if let Err(e) =
            crate::notify::relay::store::seen_mark(pool, SeenScope::Missed, &slot.seen_key(), now)
                .await
        {
            // The alert HAS gone out; failing to record that means one possible
            // repeat on the next launch, which is a great deal better than
            // refusing to send it in the first place.
            tracing::warn!("scheduler: could not stamp the missed ledger: {e}");
        }
    }
}

/// The occurrences that have NOT been reported yet, oldest first, each carrying
/// the human date the mail will print.
///
/// Separate from [`report_missed`] because this half is the whole once-guarantee
/// and the other half needs an `AppHandle` — the filter can be run twice against
/// one ledger in a test, which is exactly the sequence a machine that restarts
/// twice on a Sunday afternoon performs.
async fn unreported_missed(
    pool: &SqlitePool,
    missed: &[MissedRecordingInfo],
    date_format: &str,
    now_ms: i64,
) -> Vec<crate::notify::MissedSlot> {
    use sundayrec_core::relay::{seen_decision, SeenScope};

    let mut fresh = Vec::new();
    for m in missed {
        let slot = crate::notify::MissedSlot {
            at: m.at.clone(),
            label: m.label.clone(),
            // The human date, in the mail's language. `at` is the ISO-like local
            // string [`fmt_dt`] produced, so parsing it back cannot fail unless
            // that format changed — in which case the raw string is still true.
            date: NaiveDateTime::parse_from_str(&m.at, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.format(date_format).to_string())
                .unwrap_or_else(|_| m.at.clone()),
        };
        let last = crate::notify::relay::store::seen_get(pool, SeenScope::Missed, &slot.seen_key())
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("scheduler: could not read the missed ledger: {e}");
                None
            });
        if seen_decision(SeenScope::Missed, last, now_ms) {
            continue;
        }
        fresh.push(slot);
    }
    // OLDEST FIRST, and this sort is load-bearing twice over.
    // `missed_recordings` walks the weekly slots and THEN the dated specials, so
    // its output is in settings order, not clock order — and the mail's subject
    // headlines `missed[0]` as the oldest ("…og 2 til"), which would then name
    // whichever slot happened to be typed in first. The outbox's dedup key is
    // built from the same first element, so without this a slot dragged up the
    // settings list would look like a different sweep of the same Sundays.
    //
    // The `at` strings sort lexicographically because `fmt_dt` is
    // `%Y-%m-%dT%H:%M:%S` — fixed-width, most significant first. That is a
    // property of the format, not a coincidence, and the test above pins it.
    fresh.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.label.cmp(&b.label)));
    fresh
}

/// The one Norwegian sentence the native notification shows and the SMTP alert
/// carries. The relay's mail is [`sundayrec_core::email::render_missed`]'s
/// instead — seven languages, one line per occurrence — because it has a
/// subject and a body to fill and this has a toast to fit in.
fn missed_summary(missed: &[crate::notify::MissedSlot]) -> String {
    match missed {
        [one] => format!(
            "Planlagt opptak ble ikke gjort: {} ({}).",
            one.label, one.at
        ),
        many => format!(
            "{} planlagte opptak ble ikke gjort. Det eldste: {} ({}).",
            many.len(),
            many[0].label,
            many[0].at
        ),
    }
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
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &[], &HashSet::new());
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].when, dt("2026-06-07 11:00"));
    }

    #[test]
    fn missed_check_suppressed_when_history_covers_the_occurrence() {
        // A recording within ±30 min of the scheduled start means it DID run.
        let now = dt("2026-06-07 13:00");
        let history = [dt("2026-06-07 11:05")];
        let missed = missed_recordings(&[sunday_slot()], &[], now, &history, &[], &HashSet::new());
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
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &[], &keys);
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
        let missed = missed_recordings(&[sunday_slot()], &[], now, &[], &[], &HashSet::new());
        assert!(missed.is_empty(), "older than 24h → not logged");
    }

    // ── A4: 11:20, a slot AND a special ─────────────────────────────────────

    #[test]
    fn a_slot_and_a_special_at_the_same_time_produce_exactly_one_late_start() {
        // The composition `check_missed` performs, with the engine reading
        // injected: `active_within` in, `late_start_choice` out.
        let now = dt("2026-06-07 11:20");
        let sp = special("2026-06-07", "11:00", "12:00", "Konfirmasjon");
        let triggers = active_within(
            &[sunday_slot()],
            std::slice::from_ref(&sp),
            now,
            MISSED_WINDOW_MS,
        );
        assert_eq!(
            triggers.len(),
            2,
            "precondition: both are inside the window"
        );

        // The engine is idle → ONE start, not two. Two would mean the second
        // `start()` stopping a recording 200 ms old: the church keeps a fragment
        // and a take that begins late.
        let chosen = late_start_choice(&triggers, false);
        assert!(chosen.is_some(), "an idle engine starts the first trigger");
        assert_eq!(chosen.unwrap().kind, TriggerKind::Slot(0));

        // …and once it is running, the pass is over — the reading the shell takes
        // immediately before the start is the one that decides.
        assert!(
            late_start_choice(&triggers, true).is_none(),
            "the second trigger must NOT reach the recorder"
        );

        // BOTH keys still count as handled, so neither occurrence is also
        // reported missed.
        let keys: HashSet<String> = triggers.iter().map(|t| t.key.clone()).collect();
        assert_eq!(keys.len(), 2);
        assert!(missed_recordings(
            &[sunday_slot()],
            std::slice::from_ref(&sp),
            dt("2026-06-07 13:00"),
            &[],
            &[],
            &keys
        )
        .is_empty());
    }

    // ── A10: 11:50, after a crash ───────────────────────────────────────────

    #[test]
    fn a_recovery_manifest_on_disk_stops_the_false_missed_report() {
        use chrono::{Datelike, Duration as ChronoDuration};
        use sundayrec_core::recovery::{DeliverableManifest, SessionManifest};

        // A service that started five hours ago: past the 60-min late-start
        // window, so it is a missed CANDIDATE, and well inside the 24 h log
        // window. Real clock on purpose — this is the seam between the recovery
        // directory's epoch-ms and the core's local-wall frame, and a fixed
        // `dt()` would test neither side of it.
        //
        // Five hours rather than the 90 minutes the scenario actually describes,
        // for one reason: on the autumn DST night a wall-clock time repeats, and
        // `most_recent_occurrence` would resolve the later repeat — turning a
        // 90-minute-old occurrence into a 30-minute-old one and quietly moving it
        // back inside the late-start window. An hour of slack either way cannot
        // change the verdict. (CI runs in UTC and never sees it; a developer's
        // Mac would, once a year, for an hour.)
        let now_local = Local::now();
        let started = now_local - ChronoDuration::hours(5);
        let now = now_local.naive_local();
        let slot = ScheduleSlot {
            days: vec![started.naive_local().weekday().num_days_from_monday()],
            start: started.format("%H:%M").to_string(),
            stop: (started + ChronoDuration::hours(2))
                .format("%H:%M")
                .to_string(),
            max: None,
        };

        // Nothing in history: the crash means the row is still being concatenated.
        assert_eq!(
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                now,
                &[],
                &[],
                &HashSet::new()
            )
            .len(),
            1,
            "precondition: with an empty database this reads as a missed service"
        );

        // The evidence the database does not have: one unfinalised manifest,
        // written exactly as the engine writes it.
        let dir = tempfile::tempdir().unwrap();
        let save = tempfile::tempdir().unwrap();
        let primary = save
            .path()
            .join("gudstjeneste.m4a")
            .to_string_lossy()
            .into_owned();
        let manifest = SessionManifest {
            session_id: "crashed-session".into(),
            device_name: "Soundcraft USB".into(),
            session_start_ms: started.timestamp_millis() as u64,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![DeliverableManifest {
                primary_path: primary.clone(),
                fragments: vec![primary],
                started_at_ms: started.timestamp_millis() as u64,
            }],
        };
        std::fs::write(
            dir.path().join("crashed-session.json"),
            manifest.to_json().unwrap(),
        )
        .unwrap();

        // The exact composition `check_missed` performs, minus the `AppHandle`
        // that only locates the directory.
        let covered =
            covered_windows_local(crate::recorder::recovery::pending_windows_in(dir.path()));
        assert_eq!(covered.len(), 1, "one interrupted session");
        assert!(
            covered[0].last_seen >= covered[0].start,
            "the window spans forward in time"
        );

        assert!(
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                now,
                &[],
                &covered,
                &HashSet::new()
            )
            .is_empty(),
            "a manifest on disk IS the recording — reporting it missed is what sent \
             a volunteer an e-mail about a service that was being salvaged"
        );
    }

    #[test]
    fn an_empty_recovery_directory_covers_nothing() {
        // The ordinary case — nothing has ever crashed — must not accidentally
        // amnesty a genuinely missed service.
        let dir = tempfile::tempdir().unwrap();
        let covered =
            covered_windows_local(crate::recorder::recovery::pending_windows_in(dir.path()));
        assert!(covered.is_empty());
        let now = dt("2026-06-07 13:00");
        assert_eq!(
            missed_recordings(&[sunday_slot()], &[], now, &[], &covered, &HashSet::new()).len(),
            1,
            "no manifest, no excuse"
        );
    }

    // ── A3: the missed hole, and the receipt marker ──────────────────────────

    use sundayrec_core::relay::SeenScope;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn info(at: &str, label: &str) -> MissedRecordingInfo {
        MissedRecordingInfo {
            at: at.into(),
            label: label.into(),
        }
    }

    /// THE test the missed hole is worth: two sweeps over the same history
    /// report the Sunday ONCE.
    ///
    /// `check_missed` runs at startup and after every wake. A machine restarted
    /// three times on a Sunday afternoon rediscovers the same missed slot three
    /// times, and before the ledger existed each rediscovery would have been a
    /// mail. The second sweep here is that second launch.
    #[tokio::test]
    async fn a_missed_sunday_is_reported_once_and_never_twice() {
        let (pool, _d) = temp_pool().await;
        let history = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-06T19:00:00", "Kveldsmesse"),
        ];
        let now = crate::util::now_ms();

        let first = unreported_missed(&pool, &history, "%d.%m.%Y %H:%M", now).await;
        assert_eq!(first.len(), 2, "nothing has been said yet");
        assert_eq!(first[0].date, "06.09.2026 11:00", "localized for the mail");

        // What `report_missed` does after the dispatch returns.
        for slot in &first {
            crate::notify::relay::store::seen_mark(&pool, SeenScope::Missed, &slot.seen_key(), now)
                .await
                .unwrap();
        }

        // The next launch, minutes later — and a year later, because "once" for
        // an occurrence is a full stop, not a window.
        for later in [now + 60_000, now + 365 * 24 * 60 * 60 * 1_000] {
            assert!(
                unreported_missed(&pool, &history, "%d.%m.%Y %H:%M", later)
                    .await
                    .is_empty(),
                "the same Sunday must not be reported again at {later}"
            );
        }
    }

    /// A sweep that finds a NEW occurrence beside a reported one reports only
    /// the new one. The filter is per occurrence, not per sweep — otherwise one
    /// remembered Sunday would silence the next.
    #[tokio::test]
    async fn a_second_missed_occurrence_is_still_news() {
        let (pool, _d) = temp_pool().await;
        let now = crate::util::now_ms();
        let first = [info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)")];
        for slot in unreported_missed(&pool, &first, "%d.%m.%Y %H:%M", now).await {
            crate::notify::relay::store::seen_mark(&pool, SeenScope::Missed, &slot.seen_key(), now)
                .await
                .unwrap();
        }

        let both = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-13T11:00:00", "Ukentlig opptak (11:00–13:00)"),
        ];
        let fresh = unreported_missed(&pool, &both, "%d.%m.%Y %H:%M", now).await;
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].at, "2026-09-13T11:00:00");
    }

    /// The sweep is handed back OLDEST FIRST, whatever order the settings put
    /// the slots in.
    ///
    /// `missed_recordings` walks the weekly slots and then the dated specials,
    /// so a special that happened on Saturday evening arrives after a slot that
    /// was missed on Sunday morning. The mail's subject headlines the first
    /// element as the oldest, and the outbox's dedup key is built from it — so
    /// an unsorted list would both mis-name the mail and make a re-ordered
    /// settings page look like a different sweep.
    #[tokio::test]
    async fn the_sweep_is_handed_over_oldest_first() {
        let (pool, _d) = temp_pool().await;
        let settings_order = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-05T19:00:00", "Konsert"),
        ];
        let fresh = unreported_missed(
            &pool,
            &settings_order,
            "%d.%m.%Y %H:%M",
            crate::util::now_ms(),
        )
        .await;
        assert_eq!(
            fresh.iter().map(|s| s.at.as_str()).collect::<Vec<_>>(),
            vec!["2026-09-05T19:00:00", "2026-09-06T11:00:00"],
            "the mail headlines the first element as the oldest"
        );
    }

    /// The ledger key survives a language change. A volunteer who switches the
    /// app to English must not be told about the same missed Sunday again just
    /// because the printed date now reads `06/09/2026`.
    #[tokio::test]
    async fn switching_language_does_not_re_report_a_missed_sunday() {
        let (pool, _d) = temp_pool().await;
        let now = crate::util::now_ms();
        let history = [info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)")];

        let no = unreported_missed(&pool, &history, "%d.%m.%Y %H:%M", now).await;
        assert_eq!(no[0].date, "06.09.2026 11:00");
        crate::notify::relay::store::seen_mark(&pool, SeenScope::Missed, &no[0].seen_key(), now)
            .await
            .unwrap();

        // Same occurrence, English date format — the key is keyed on `at`.
        let en = unreported_missed(&pool, &history, "%d/%m/%Y %H:%M", now).await;
        assert!(en.is_empty(), "the ledger keys on the machine's timestamp");
    }

    /// The sentence the native notification shows, singular and plural. It is
    /// the summary; the mail's seven-language body is `render_missed`'s.
    #[test]
    fn the_missed_summary_counts_what_it_names() {
        let one = crate::notify::MissedSlot {
            at: "2026-09-06T11:00:00".into(),
            label: "Ukentlig opptak (11:00–13:00)".into(),
            date: "06.09.2026 11:00".into(),
        };
        let many = vec![one.clone(), one.clone(), one.clone()];
        let s1 = missed_summary(std::slice::from_ref(&one));
        assert!(s1.contains("Ukentlig opptak") && s1.contains("2026-09-06T11:00:00"));
        assert!(
            !s1.starts_with('1') && !s1.contains("eldste"),
            "a single occurrence is named, not counted and not ranked: {s1}"
        );
        let s3 = missed_summary(&many);
        assert!(s3.starts_with('3'), "{s3}");
        assert!(s3.contains("eldste"), "the headline names the oldest: {s3}");
    }

    /// The receipt's marker: taken once, and gone.
    ///
    /// This is what makes "a manual recording gets no receipt" true. The next
    /// `recording://finished` after a scheduled run is a DIFFERENT recording —
    /// whoever pressed Start is standing there — and a marker that survived
    /// being read would hand them a mail about it.
    #[test]
    fn a_manual_recording_finds_no_marker_to_claim() {
        let marker = ScheduledRunMarker::new();
        assert!(
            marker.take().is_none(),
            "a manual recording on a fresh process: no receipt"
        );

        let run = ScheduledRun {
            slot: "Ukentlig opptak (11:00–13:00)".into(),
            started_at: Local::now(),
        };
        marker.set(run.clone());
        assert_eq!(marker.take(), Some(run));
        assert!(
            marker.take().is_none(),
            "and the manual recording that follows finds nothing"
        );
    }

    /// A fresh start overwrites a stale marker rather than being refused by it.
    /// A marker that outlived its run (a crash between start and finish) must
    /// not stop the NEXT scheduled recording from earning its receipt.
    #[test]
    fn a_new_scheduled_run_replaces_a_stale_marker() {
        let marker = ScheduledRunMarker::new();
        marker.set(ScheduledRun {
            slot: "gammelt".into(),
            started_at: Local::now(),
        });
        marker.set(ScheduledRun {
            slot: "nytt".into(),
            started_at: Local::now(),
        });
        assert_eq!(marker.take().map(|r| r.slot), Some("nytt".to_string()));
    }

    /// The receipt and the missed alert must call the same slot the same thing
    /// — the wording is `missed_recordings`', mirrored.
    #[test]
    fn a_slot_is_named_the_same_way_in_both_mails() {
        let settings = Settings {
            slots: vec![sunday_slot()],
            special_recordings: vec![special("2026-09-06", "19:00", "21:00", "Konsert")],
            ..Settings::default()
        };
        assert_eq!(
            trigger_label(&settings, TriggerKind::Slot(0)),
            "Ukentlig opptak (11:00–12:00)"
        );
        // …the exact string the core puts on a MISSED occurrence of that slot.
        let missed = missed_recordings(
            settings.active_slots(),
            &[],
            dt("2026-09-06 13:00"),
            &[],
            &[],
            &HashSet::new(),
        );
        assert_eq!(
            missed.first().map(|m| m.label.clone()),
            Some(trigger_label(&settings, TriggerKind::Slot(0)))
        );

        assert_eq!(
            trigger_label(&settings, TriggerKind::Special(0)),
            "Konsert",
            "a special is called what somebody named it"
        );
        let unnamed = Settings {
            special_recordings: vec![special("2026-09-06", "19:00", "21:00", "  ")],
            ..Settings::default()
        };
        assert_eq!(
            trigger_label(&unnamed, TriggerKind::Special(0)),
            "Spesialopptak",
            "and an unnamed one is not an empty line in a mail"
        );
        // An index the settings no longer hold (a slot deleted mid-run) names
        // something rather than panicking.
        assert_eq!(
            trigger_label(&Settings::default(), TriggerKind::Slot(9)),
            "Ukentlig opptak"
        );
    }

    // ── The seam: what A3 reports is what M4 already filtered ────────────────

    /// A crash recovery still in flight is neither dispatched NOR stamped.
    ///
    /// Neither half owns this test. F1-M4 taught `missed_recordings` that an
    /// occurrence overlapping an unfinalised manifest is not missed; A3 gave
    /// whatever survives that filter a dispatch and a durable `notify_seen` row.
    /// What only the two together have is the ORDER — `check_missed` filters,
    /// and `if !out.is_empty()` is the gate the dispatch sits behind, so the
    /// list A3 reports on is the list M4 has already thinned.
    ///
    /// The other order is the whole reason #204 waited for M4. A Sunday being
    /// salvaged one task over would go out as a desktop notification and an
    /// e-mail telling a volunteer it was never recorded — and, because the
    /// ledger row makes "once" a full stop rather than a window, no later
    /// correction could take it back.
    ///
    /// Both halves of the claim are asserted, and the counterfactual first:
    /// without the manifest this occurrence IS fresh news, so an empty ledger at
    /// the end is the filter's doing and not an inert test.
    #[tokio::test]
    async fn a_recovery_in_flight_is_neither_dispatched_nor_stamped() {
        use chrono::{Datelike, Duration as ChronoDuration};
        use sundayrec_core::recovery::{DeliverableManifest, SessionManifest};

        let (pool, _d) = temp_pool().await;

        // Five hours back, on the real clock, for the reason the A10 test states:
        // this is the seam between the recovery directory's epoch-ms and the
        // core's local-wall frame, and on the autumn DST night a nearer wall time
        // would resolve to the later repeat and drift back inside the late-start
        // window.
        let now_local = Local::now();
        let started = now_local - ChronoDuration::hours(5);
        let now = now_local.naive_local();
        let slot = ScheduleSlot {
            days: vec![started.naive_local().weekday().num_days_from_monday()],
            start: started.format("%H:%M").to_string(),
            stop: (started + ChronoDuration::hours(2))
                .format("%H:%M")
                .to_string(),
            max: None,
        };

        // `check_missed`'s own conversion from the core's verdict to the shape
        // `report_missed` consumes.
        let sweep = |covered: &[CoveredWindow]| -> Vec<MissedRecordingInfo> {
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                now,
                &[],
                covered,
                &HashSet::new(),
            )
            .into_iter()
            .map(|m| MissedRecordingInfo {
                at: fmt_dt(m.when),
                label: m.label,
            })
            .collect()
        };

        // COUNTERFACTUAL — the database alone still reads this as a lost service,
        // and A3's filter agrees it has never been reported. Without M4 this is
        // the mail that goes out.
        let unfiltered = sweep(&[]);
        assert_eq!(unfiltered.len(), 1, "precondition: a missed candidate");
        let would_send =
            unreported_missed(&pool, &unfiltered, "%d.%m.%Y %H:%M", crate::util::now_ms()).await;
        assert_eq!(
            would_send.len(),
            1,
            "precondition: nothing has stamped this occurrence, so it IS fresh news"
        );
        let key = would_send[0].seen_key();
        // `unreported_missed` only reads; the stamping is `report_missed`'s, and
        // that is exactly what must not happen below.
        assert!(
            crate::notify::relay::store::seen_get(&pool, SeenScope::Missed, &key)
                .await
                .unwrap()
                .is_none(),
            "reading the ledger must not write to it"
        );

        // The evidence the database does not have yet: one unfinalised manifest,
        // written the way the engine writes it.
        let dir = tempfile::tempdir().unwrap();
        let save = tempfile::tempdir().unwrap();
        let primary = save
            .path()
            .join("gudstjeneste.m4a")
            .to_string_lossy()
            .into_owned();
        let manifest = SessionManifest {
            session_id: "crashed-session".into(),
            device_name: "Soundcraft USB".into(),
            session_start_ms: started.timestamp_millis() as u64,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![DeliverableManifest {
                primary_path: primary.clone(),
                fragments: vec![primary],
                started_at_ms: started.timestamp_millis() as u64,
            }],
        };
        std::fs::write(
            dir.path().join("crashed-session.json"),
            manifest.to_json().unwrap(),
        )
        .unwrap();

        // The composition `check_missed` performs, minus the `AppHandle` that
        // only locates the directory and carries the dispatch.
        let covered =
            covered_windows_local(crate::recorder::recovery::pending_windows_in(dir.path()));
        assert_eq!(covered.len(), 1, "one interrupted session");
        let out = sweep(&covered);

        // NO DISPATCH: `report_missed` sits behind `if !out.is_empty()`, and the
        // gate is shut.
        assert!(
            out.is_empty(),
            "the recovery covers the window, so nothing reaches the dispatch"
        );

        // NO STAMP: the ledger is written only by `report_missed`, one statement
        // after the dispatch it never made. The occurrence stays reportable, so
        // if the recovery later fails for real, that news can still be sent.
        assert!(
            crate::notify::relay::store::seen_get(&pool, SeenScope::Missed, &key)
                .await
                .unwrap()
                .is_none(),
            "a filtered occurrence must not be recorded as reported — a stamp \
             here is permanent, and would silence a genuine bom for good"
        );
    }
}
