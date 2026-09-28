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

use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use ts_rs::TS;

use sundayrec_core::alerts::AlertText;
use sundayrec_core::lang::Lang;
use sundayrec_core::schedule::{
    active_within, capped_supervisor_sleep_ms, late_start_choice, missed_recordings,
    next_recording, prune_specials, scheduled_max_minutes, supervisor_should_fire, upcoming_dates,
    upcoming_events, CoveredWindow, ScheduledEvent, ScheduledEventKind, TriggerKind,
    MISSED_WINDOW_MS,
};
use sundayrec_core::settings::Settings;
use sundayrec_core::wake::{background_wake_log_action, should_block, wake_failure_notice_key};

use crate::db::Db;
use crate::error::AppResult;
use crate::notify::APP_TITLE;
use crate::power::KeepAwake;
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
/// Emitted when a scheduled recording could not be started or prepared —
/// payload is the stable failure code. The native notification is said once;
/// this is what keeps the menu-bar icon amber afterwards.
pub const FAILURE_EVENT: &str = "scheduler://failure";

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
                title: Some(AlertText::SchedulerTaskTitle),
                body: AlertText::SchedulerTaskBody,
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

/// Open or close the supervisor's keep-awake window for this pass (F2-W5).
///
/// One line, but its own function for two reasons: the supervisor cannot be
/// unit-tested (it needs an `AppHandle` and a db pool), and a copy of this
/// expression written in a test would be free to drift from the one that ships.
/// The tests below call exactly this.
///
/// The decision is [`should_block`] — "is any upcoming start within
/// [`BLOCKER_SOON_MS`](sundayrec_core::wake::BLOCKER_SOON_MS)" — and it is
/// re-asked every pass, so the window closes on its own when the start passes
/// without one (the app was hidden, the slot was deleted, the machine came back
/// late) rather than needing a matching release somewhere.
fn drive_keep_awake(keep: &mut KeepAwake, upcoming: &[NaiveDateTime], now: NaiveDateTime) {
    keep.set(should_block(upcoming, now));
}

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
    // F2-W5: the supervisor's own keep-awake block, opened and closed by
    // [`drive_keep_awake`] once per pass. It lives OUTSIDE the loop because it
    // has to survive from one pass to the next — a block re-taken every pass
    // would be a stack, and one dropped at the end of every pass would leave
    // the machine free to sleep between two ticks. If this task dies, its
    // `Drop` releases and the re-spawned supervisor re-opens the window on its
    // first pass.
    let mut keep_awake = KeepAwake::new(crate::power::blocker(), "scheduled recording is due");
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

        let upcoming = upcoming_dates(settings.active_slots(), &kept, now, WAKE_HORIZON_DAYS);

        // F2-W5: keep the machine awake while a start is imminent. Deliberately
        // NOT inside the `wake_from_sleep` branch below — the two answer
        // different questions. Waking a sleeping machine is opt-in and can need
        // an admin prompt; *not falling asleep* in the half hour before a start
        // the operator has scheduled is unconditional, and it is the leg the
        // wake mechanism itself depends on: a Windows box our timer resumes at
        // T−10 min is subject to the 2-minute unattended-sleep timeout, so
        // without this it can be asleep again before the recording is due.
        drive_keep_awake(&mut keep_awake, &upcoming, now);

        // Schedule OS wake-from-sleep timers for upcoming recordings (Fase 5.2).
        // Non-admin (no prompt) from the supervisor — the WakeEngine dedups so an
        // unchanged schedule is a cheap no-op. A user-initiated reschedule (which
        // may prompt for admin) goes through the `wake_reschedule` command.
        if settings.wake_from_sleep {
            if let Some(wake) = app.try_state::<crate::wake::WakeEngine>() {
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
                // …and the operator, once per launch per kind. The log answers
                // "was the wake ever armed?" for whoever debugs; this is the
                // volunteer's only chance to hear it before the service.
                if let Some(key) = wake_failure_notice_key(res.ok, res.reason.as_deref()) {
                    if first_wake_notice(key) {
                        notify_user(
                            &app,
                            APP_TITLE,
                            &AlertText::WakeNotArmed.text(lang_of(&settings)),
                        );
                    }
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
                        APP_TITLE,
                        &AlertText::ScheduledSkippedBusy.text(lang_of(settings)),
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
                                notify_user(
                                    app,
                                    APP_TITLE,
                                    &AlertText::ScheduledStarted.text(lang_of(settings)),
                                );
                            }
                        }
                        // A scheduled recording that does not start is the single
                        // worst thing this app can do quietly: nobody is watching
                        // the screen at 11:00, and the service is not repeatable.
                        // All three go through `crate::notify::dispatch_failure`,
                        // which no setting can silence.
                        Ok(Err(e)) => {
                            tracing::error!("scheduler: scheduled start failed: {e}");
                            dispatch_scheduler_failure(
                                app,
                                "scheduled_start_failed",
                                AlertText::ScheduledStartFailed
                                    .fill(lang_of(settings), &[("detail", &e.to_string())]),
                            );
                        }
                        Err(_) => {
                            tracing::error!("scheduler: scheduled start TIMED OUT after 30s");
                            dispatch_scheduler_failure(
                                app,
                                "scheduled_start_timeout",
                                AlertText::ScheduledStartTimeout.text(lang_of(settings)),
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("scheduler: could not build opts: {e}");
                    dispatch_scheduler_failure(
                        app,
                        "scheduled_prepare_failed",
                        AlertText::ScheduledPrepareFailed
                            .fill(lang_of(settings), &[("detail", &e.to_string())]),
                    );
                }
            }
        }
        ScheduledEventKind::Stop => {
            let engine = app.state::<RecorderEngine>();
            // Read BEFORE the stop: a scheduled stop with nothing recording
            // (the start failed, or someone already pressed Stop) used to say
            // «Planlagt opptak avsluttet» about a recording that never ran.
            let was_active = engine.current_state().is_active();
            engine.stop();
            tracing::info!(was_active, "scheduler: stop fired");
            // R3-H: «Varsel på PC når opptak avsluttes». Fires when the
            // scheduled stop is DISPATCHED (finalisation continues in the
            // engine); a stop that later fails to finalise reaches the operator
            // through the failure dispatch, which is never gated.
            if stopped_notice_due(was_active, settings) {
                notify_user(
                    app,
                    APP_TITLE,
                    &AlertText::ScheduledStopped.text(lang_of(settings)),
                );
            }
        }
        ScheduledEventKind::Reminder => {
            let body = AlertText::Reminder.fill(
                lang_of(settings),
                &[("min", &settings.reminder_minutes.to_string())],
            );
            // ALWAYS fires (should_notify pins Reminder on — not governed by
            // notify_start/notify_stop): the reminder has its own opt-in,
            // `reminder_minutes` = 0 means the event is never scheduled at all.
            if should_notify(SchedulerNotice::Reminder, settings) {
                notify_user(app, APP_TITLE, &body);
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
            // F2-I18N-R2: BOTH halves are localized now. The finding carries a
            // `PreflightCode`, and the code names its own `AlertText` arm — so
            // the notification is written in the volunteer's language rather
            // than in the engine's. `message` (English) remains the reserve
            // for a finding with no code, which is only ever one the SHELL
            // built; the scheduler never sees those, so in practice this is
            // the total-function branch and not a fallback anybody hits.
            let lang = lang_of(settings);
            let body = match first.code {
                Some(code) => {
                    let vars: Vec<(&str, &str)> = code
                        .alert()
                        .params()
                        .iter()
                        .map(|p| (*p, first.params.get(*p).map(String::as_str).unwrap_or("?")))
                        .collect();
                    code.alert().fill(lang, &vars)
                }
                None => first.message.clone(),
            };
            notify_user(app, &AlertText::PreflightTitle.text(lang), &body);
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
            .msg(format!("The audio device \"{name}\" is not connected."))
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
                }
                if let Err(e) = late {
                    tracing::error!("scheduler: late-start of missed recording failed: {e}");
                    // The recovery attempt for an already-missed recording just
                    // failed too — the operator hears it natively.
                    dispatch_scheduler_failure(
                        app,
                        "scheduled_late_start_failed",
                        AlertText::ScheduledLateStartFailed
                            .fill(lang_of(&settings), &[("detail", &e.to_string())]),
                    );
                }
            }
            Err(e) => {
                tracing::error!("scheduler: could not build opts for late-start: {e}");
                // This trigger is in `triggered_keys`, so the missed report
                // below will NOT claim it — without this dispatch a late start
                // that could not even be prepared was said nowhere at all.
                dispatch_scheduler_failure(
                    app,
                    "scheduled_late_start_failed",
                    AlertText::ScheduledLateStartFailed
                        .fill(lang_of(&settings), &[("detail", &e.to_string())]),
                );
            }
        }
    }

    // History start times → local naive, for the dedup window.
    let history = recording_history_local(pool).await;
    // …and the recordings the database does not know about YET: a crash leaves a
    // session manifest behind, and startup recovery is still concatenating it
    // into a history row while this runs. Without these windows the sweep
    // reports a service that is being salvaged one task over as never recorded
    // (F1 finding A10) — which, with the missed dispatch behind it, is a desktop
    // notification saying so.
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

/// Tell the operator about the Sundays that were not recorded.
///
/// ## The hole this closes
///
/// [`check_missed`] used to emit an event to a renderer that may not be running
/// and stop there. A church whose machine slept through Sunday morning learned
/// about it when somebody asked for the recording. Now the same sweep raises a
/// native notification.
///
/// ## Once per occurrence, durably
///
/// [`check_missed`] runs at startup AND after every wake, so the same Sunday is
/// rediscovered every time the app launches for as long as it stays inside the
/// 24-hour window. The `notify_seen` ledger ([`crate::notify::seen`]) is what
/// makes that one notification instead of five, and it is a TABLE rather than a
/// flag in RAM for exactly that reason: the repeats are separated by restarts,
/// which is precisely what RAM does not survive.
///
/// The ledger is stamped AFTER the notification: failing to stamp it costs one
/// possible repeat on the next launch, which is a great deal better than a
/// stamped Sunday nobody was ever told about.
async fn report_missed(app: &AppHandle, pool: &SqlitePool, missed: &[MissedRecordingInfo]) {
    use sundayrec_core::notify::SeenScope;

    let settings = settings::load(pool).await.unwrap_or_default();
    let lang = lang_of(&settings);
    let now = crate::util::now_ms();

    let fresh = unreported_missed(pool, missed, now).await;
    if fresh.is_empty() {
        tracing::debug!("scheduler: every missed occurrence had already been reported");
        return;
    }

    let message = missed_summary(&fresh, lang);
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
        ),
    );

    for slot in &fresh {
        if let Err(e) =
            crate::notify::seen::seen_mark(pool, SeenScope::Missed, &slot.seen_key(), now).await
        {
            // The alert HAS gone out; failing to record that means one possible
            // repeat on the next launch, which is a great deal better than
            // refusing to send it in the first place.
            tracing::warn!("scheduler: could not stamp the missed ledger: {e}");
        }
    }
}

/// The occurrences that have NOT been reported yet, oldest first.
///
/// Separate from [`report_missed`] because this half is the whole once-guarantee
/// and the other half needs an `AppHandle` — the filter can be run twice against
/// one ledger in a test, which is exactly the sequence a machine that restarts
/// twice on a Sunday afternoon performs.
async fn unreported_missed(
    pool: &SqlitePool,
    missed: &[MissedRecordingInfo],
    now_ms: i64,
) -> Vec<crate::notify::MissedSlot> {
    use sundayrec_core::notify::{seen_decision, SeenScope};

    let mut fresh = Vec::new();
    for m in missed {
        let slot = crate::notify::MissedSlot {
            at: m.at.clone(),
            label: m.label.clone(),
        };
        let last = crate::notify::seen::seen_get(pool, SeenScope::Missed, &slot.seen_key())
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
    // OLDEST FIRST, and this sort is load-bearing: `missed_recordings` walks the
    // weekly slots and THEN the dated specials, so its output is in settings
    // order, not clock order — and [`missed_summary`] headlines `missed[0]` as
    // the oldest ("…Det eldste: …"), which would then name whichever slot
    // happened to be typed in first.
    //
    // The `at` strings sort lexicographically because `fmt_dt` is
    // `%Y-%m-%dT%H:%M:%S` — fixed-width, most significant first. That is a
    // property of the format, not a coincidence, and the tests pin it.
    fresh.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.label.cmp(&b.label)));
    fresh
}

/// The one sentence the native notification shows.
///
/// F1 A8: this was the last Norwegian-only sentence on the missed path, which
/// made it the worst one. A church whose machine slept through Sunday morning
/// gets exactly this line on the desktop; a Polish volunteer got it in
/// Norwegian. The slot LABEL inside it stays as the schedule wrote
/// it — see [`sundayrec_core::alerts`]'s header: it is hashed into the durable
/// `notify_seen` key, so translating it would re-alert the same Sunday every
/// time somebody changes language.
fn missed_summary(missed: &[crate::notify::MissedSlot], lang: Lang) -> String {
    match missed {
        [one] => AlertText::MissedOne.fill(lang, &[("label", &one.label), ("at", &one.at)]),
        many => AlertText::MissedMany.fill(
            lang,
            &[
                ("count", &many.len().to_string()),
                ("label", &many[0].label),
                ("at", &many[0].at),
            ],
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

/// Whether the «Planlagt opptak avsluttet» notice is due for a scheduled stop.
///
/// Only when something was actually recording when the stop fired — and then
/// still only if the operator's `notify_stop` toggle allows it.
fn stopped_notice_due(was_active: bool, settings: &Settings) -> bool {
    was_active && should_notify(SchedulerNotice::StoppedScheduled, settings)
}

/// The wake-failure kinds already told to the operator in this process.
static WAKE_NOTICES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// `true` the first time `key` is seen in this process — the once-per-launch
/// gate for [`AlertText::WakeNotArmed`]. The supervisor re-runs on every
/// settings change and timer, and a permission problem does not go away
/// between passes.
fn first_wake_notice(key: &'static str) -> bool {
    let mut seen = lock_recover(&WAKE_NOTICES);
    if seen.contains(&key) {
        false
    } else {
        seen.push(key);
        true
    }
}

/// A scheduled recording did not happen. Routes the sentence through the one
/// failure dispatch, which shows it as a native notification no setting can
/// silence.
///
/// `code` is the stable machine code, logged with the dispatch; `message` is
/// the localized sentence, passed through verbatim.
fn dispatch_scheduler_failure(app: &AppHandle, code: &str, message: String) {
    if let Err(e) = app.emit(FAILURE_EVENT, code) {
        tracing::warn!("scheduler: could not emit {FAILURE_EVENT}: {e}");
    }
    crate::notify::dispatch_failure(
        app,
        crate::notify::FailureCtx::now(
            code,
            message,
            sundayrec_core::notify::FailureSource::Scheduler,
        ),
    );
}

/// The volunteer's language, from the settings this pass already loaded.
///
/// Every scheduler site that says something to a person goes through here — and
/// through the SETTINGS, not through [`crate::ui_lang`]'s cache: this module
/// always has a freshly-loaded `Settings` in hand, so it can use the source
/// rather than the cache. (The recorder's capture loop cannot, which is what
/// the cache exists for.)
fn lang_of(settings: &Settings) -> Lang {
    Lang::from_code(settings.language.as_deref())
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
    fn a_stop_with_nothing_recording_is_not_announced() {
        let on = Settings::default();
        assert!(stopped_notice_due(true, &on));
        assert!(
            !stopped_notice_due(false, &on),
            "«Planlagt opptak avsluttet» about a recording that never ran"
        );
        let off = Settings {
            notify_stop: false,
            ..Settings::default()
        };
        assert!(!stopped_notice_due(true, &off), "the toggle still decides");
    }

    #[test]
    fn a_wake_failure_is_told_once_per_kind() {
        // Process-wide state: use keys no other test touches.
        assert!(first_wake_notice("test-kind-a"));
        assert!(!first_wake_notice("test-kind-a"));
        assert!(first_wake_notice("test-kind-b"));
    }

    #[test]
    fn fmt_dt_is_zoneless_local_iso() {
        let dt = NaiveDateTime::parse_from_str("2026-06-07 11:00", "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(fmt_dt(dt), "2026-06-07T11:00:00");
    }

    /// The reminder moved to `sundayrec_core::alerts` (F1 A8), which pins the
    /// seven wordings and their `{min}` placeholder. What is still THIS
    /// module's to prove is the seam: that `settings.language` reaches the
    /// catalog, and that the minutes reach the placeholder. A `lang_of` that
    /// returned a constant would pass the core's tests and fail here.
    #[test]
    fn the_reminder_speaks_the_settings_language() {
        let en = Settings {
            language: Some("en".into()),
            reminder_minutes: 15,
            ..Settings::default()
        };
        assert_eq!(
            AlertText::Reminder.fill(lang_of(&en), &[("min", &en.reminder_minutes.to_string())]),
            "Recording starts in 15 minutes"
        );

        let no = Settings {
            language: Some("no".into()),
            reminder_minutes: 10,
            ..Settings::default()
        };
        assert_eq!(
            AlertText::Reminder.fill(lang_of(&no), &[("min", &no.reminder_minutes.to_string())]),
            "Opptak starter om 10 minutter"
        );

        // Unknown language, and "follow the OS" (None) → Norwegian.
        for code in [Some("xx".to_string()), None] {
            let s = Settings {
                language: code,
                reminder_minutes: 5,
                ..Settings::default()
            };
            assert_eq!(
                AlertText::Reminder.fill(lang_of(&s), &[("min", &s.reminder_minutes.to_string())]),
                "Opptak starter om 5 minutter"
            );
        }
    }

    /// Every scheduler sentence goes through `lang_of`, so a settings language
    /// the catalog knows must produce a NON-Norwegian sentence. This is the
    /// test that would fail if somebody re-hardcoded a literal at a call site:
    /// the Polish church in finding A8 would be back to Norwegian alerts.
    #[test]
    fn a_polish_church_gets_polish_scheduler_alerts() {
        let pl = Settings {
            language: Some("pl".into()),
            ..Settings::default()
        };
        assert_eq!(lang_of(&pl), Lang::Pl);
        for a in [
            AlertText::ScheduledSkippedBusy,
            AlertText::ScheduledStarted,
            AlertText::ScheduledStopped,
            AlertText::ScheduledStartTimeout,
            AlertText::PreflightTitle,
            AlertText::SchedulerTaskTitle,
            AlertText::SchedulerTaskBody,
        ] {
            assert_ne!(
                a.text(lang_of(&pl)),
                a.text(Lang::No),
                "{a:?} came out Norwegian for a Polish church"
            );
        }
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

    // ── F2-W5: the keep-awake window ────────────────────────────────────────
    //
    // Driven through `drive_keep_awake` — the very expression the supervisor
    // runs — over a counting fake blocker, with `now` injected. What the OS
    // does with the block is riggpunkt (w5); what is asserted here is that the
    // window opens once, stays one block wide, and closes.

    /// The supervisor's own `upcoming_dates(...)` call, for a Sunday-11:00
    /// church, evaluated at `now`.
    fn upcoming_at(now: NaiveDateTime) -> Vec<NaiveDateTime> {
        upcoming_dates(&[sunday_slot()], &[], now, WAKE_HORIZON_DAYS)
    }

    fn keep_awake_for(fake: &Arc<crate::power::FakeBlocker>) -> KeepAwake {
        KeepAwake::new(
            Arc::clone(fake) as Arc<dyn crate::power::PowerBlocker>,
            "scheduled recording is due",
        )
    }

    #[test]
    fn a_start_five_minutes_out_opens_the_keep_awake_window() {
        // The gap this whole finding is about: the machine has been woken (or
        // never slept), the recording is minutes away, and nothing is holding a
        // power request because no ffmpeg has started yet.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        let now = dt("2026-06-07 10:55");
        drive_keep_awake(&mut keep, &upcoming_at(now), now);
        assert!(keep.is_held());
        assert_eq!(fake.active(), 1);
        assert_eq!(
            fake.reasons(),
            vec!["scheduled recording is due"],
            "the log line has to say which owner is keeping the machine up"
        );
    }

    #[test]
    fn a_start_beyond_the_window_holds_nothing() {
        // BLOCKER_SOON_MS is 30 minutes; 50 minutes out the machine is free to
        // sleep — the wake timer, not this block, is what brings it back.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        let now = dt("2026-06-07 10:10");
        drive_keep_awake(&mut keep, &upcoming_at(now), now);
        assert!(!keep.is_held());
        assert_eq!(fake.acquired(), 0);
    }

    #[test]
    fn two_ticks_inside_the_window_still_hold_exactly_one_block() {
        // The supervisor re-evaluates every MAX_SUPERVISOR_SLEEP_MS (5 min) and
        // on every settings save, so one 30-minute window is at least six
        // passes. Six blocks would be six OS assertions / six holder threads,
        // released to the wrong depth.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        for t in ["2026-06-07 10:35", "2026-06-07 10:40", "2026-06-07 10:55"] {
            let now = dt(t);
            drive_keep_awake(&mut keep, &upcoming_at(now), now);
        }
        assert!(keep.is_held());
        assert_eq!(fake.acquired(), 1, "three passes, one block");
        assert_eq!(fake.active(), 1);
    }

    #[test]
    fn the_window_closes_once_the_start_has_passed() {
        // A start that came and went (fired, or missed) must not leave the
        // machine pinned awake — the next Sunday is 7 days out, which is well
        // outside the window.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        let before = dt("2026-06-07 10:55");
        drive_keep_awake(&mut keep, &upcoming_at(before), before);
        assert!(keep.is_held());

        let after = dt("2026-06-07 11:05");
        drive_keep_awake(&mut keep, &upcoming_at(after), after);
        assert!(!keep.is_held(), "the block must not outlive its window");
        assert_eq!(fake.active(), 0);
        assert_eq!(fake.acquired(), 1, "closing must not have re-acquired");
    }

    #[test]
    fn an_empty_schedule_never_opens_the_window() {
        // «Ta opp automatisk» off ⇒ `active_slots()` is empty ⇒ nothing
        // upcoming ⇒ a laptop the volunteer took home still sleeps.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        let now = dt("2026-06-07 10:55");
        drive_keep_awake(&mut keep, &[], now);
        assert!(!keep.is_held());
        assert_eq!(fake.acquired(), 0);
    }

    #[test]
    fn a_dated_special_opens_the_window_too() {
        // Specials go through the same `upcoming_dates` list, so a Christmas
        // Eve service is covered without a second code path.
        let fake = crate::power::FakeBlocker::new();
        let mut keep = keep_awake_for(&fake);
        let now = dt("2026-12-24 15:40");
        let upcoming = upcoming_dates(
            &[],
            &[special("2026-12-24", "16:00", "17:00", "Julaften")],
            now,
            WAKE_HORIZON_DAYS,
        );
        drive_keep_awake(&mut keep, &upcoming, now);
        assert!(keep.is_held());
        assert_eq!(fake.active(), 1);
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
    fn missed_check_looks_back_a_week_and_no_further() {
        // A machine switched off from Sunday to Wednesday used to say nothing:
        // the window was 24 h. It is 7 days now — Sunday 11:00 is reported on
        // Wednesday…
        let wednesday = dt("2026-06-10 09:00");
        let missed = missed_recordings(&[sunday_slot()], &[], wednesday, &[], &[], &HashSet::new());
        assert_eq!(missed.len(), 1, "three days old → still reported");
        // …but an occurrence older than a week is not, so a machine that was
        // off for a month does not open with a month of notifications. The
        // special on the 1st is 9 days old on the 10th.
        let old_special = special("2026-06-01", "19:00", "21:00", "Konsert");
        let missed = missed_recordings(&[], &[old_special], wednesday, &[], &[], &HashSet::new());
        assert!(missed.is_empty(), "older than a week → not reported");
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
            "a manifest on disk IS the recording — reporting it missed is what told \
             a volunteer about a lost service that was being salvaged"
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

    // ── A3: the missed hole ──────────────────────────────────────────────────

    use sundayrec_core::notify::SeenScope;

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
    /// notification. The second sweep here is that second launch.
    #[tokio::test]
    async fn a_missed_sunday_is_reported_once_and_never_twice() {
        let (pool, _d) = temp_pool().await;
        let history = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-06T19:00:00", "Kveldsmesse"),
        ];
        let now = crate::util::now_ms();

        let first = unreported_missed(&pool, &history, now).await;
        assert_eq!(first.len(), 2, "nothing has been said yet");

        // What `report_missed` does after the dispatch returns.
        for slot in &first {
            crate::notify::seen::seen_mark(&pool, SeenScope::Missed, &slot.seen_key(), now)
                .await
                .unwrap();
        }

        // The next launch, minutes later — and a year later, because "once" for
        // an occurrence is a full stop, not a window.
        for later in [now + 60_000, now + 365 * 24 * 60 * 60 * 1_000] {
            assert!(
                unreported_missed(&pool, &history, later).await.is_empty(),
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
        for slot in unreported_missed(&pool, &first, now).await {
            crate::notify::seen::seen_mark(&pool, SeenScope::Missed, &slot.seen_key(), now)
                .await
                .unwrap();
        }

        let both = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-13T11:00:00", "Ukentlig opptak (11:00–13:00)"),
        ];
        let fresh = unreported_missed(&pool, &both, now).await;
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].at, "2026-09-13T11:00:00");
    }

    /// The sweep is handed back OLDEST FIRST, whatever order the settings put
    /// the slots in.
    ///
    /// `missed_recordings` walks the weekly slots and then the dated specials,
    /// so a special that happened on Saturday evening arrives after a slot that
    /// was missed on Sunday morning. The summary headlines the first element as
    /// the oldest, so an unsorted list would mis-name it.
    #[tokio::test]
    async fn the_sweep_is_handed_over_oldest_first() {
        let (pool, _d) = temp_pool().await;
        let settings_order = [
            info("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)"),
            info("2026-09-05T19:00:00", "Konsert"),
        ];
        let fresh = unreported_missed(&pool, &settings_order, crate::util::now_ms()).await;
        assert_eq!(
            fresh.iter().map(|s| s.at.as_str()).collect::<Vec<_>>(),
            vec!["2026-09-05T19:00:00", "2026-09-06T11:00:00"],
            "the summary headlines the first element as the oldest"
        );
    }

    /// The sentence the native notification shows, singular and plural.
    #[test]
    fn the_missed_summary_counts_what_it_names() {
        let one = crate::notify::MissedSlot {
            at: "2026-09-06T11:00:00".into(),
            label: "Ukentlig opptak (11:00–13:00)".into(),
        };
        let many = vec![one.clone(), one.clone(), one.clone()];
        let s1 = missed_summary(std::slice::from_ref(&one), Lang::No);
        assert!(s1.contains("Ukentlig opptak") && s1.contains("2026-09-06T11:00:00"));
        assert!(
            !s1.starts_with('1') && !s1.contains("eldste"),
            "a single occurrence is named, not counted and not ranked: {s1}"
        );
        let s3 = missed_summary(&many, Lang::No);
        assert!(s3.starts_with('3'), "{s3}");
        assert!(s3.contains("eldste"), "the headline names the oldest: {s3}");

        // …and the SAME two shapes in the volunteer's own language (F1 A8).
        // The Sunday a church lost is the one sentence that must never arrive
        // in a language nobody in the building reads.
        let p1 = missed_summary(std::slice::from_ref(&one), Lang::Pl);
        assert!(
            p1.starts_with("Zaplanowane nagranie nie zostało wykonane:"),
            "{p1}"
        );
        let p3 = missed_summary(&many, Lang::Pl);
        assert!(p3.starts_with("Nie wykonano 3 "), "{p3}");
        // The slot LABEL is deliberately untranslated in both: it is hashed
        // into the durable `notify_seen` key, and a key that moves with the
        // language re-alerts the same Sunday. See `sundayrec_core::alerts`.
        assert!(p1.contains("Ukentlig opptak (11:00–13:00)"), "{p1}");
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
    /// salvaged one task over would go out as a desktop notification telling a
    /// volunteer it was never recorded — and, because the
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
        // the notification that goes out.
        let unfiltered = sweep(&[]);
        assert_eq!(unfiltered.len(), 1, "precondition: a missed candidate");
        let would_send = unreported_missed(&pool, &unfiltered, crate::util::now_ms()).await;
        assert_eq!(
            would_send.len(),
            1,
            "precondition: nothing has stamped this occurrence, so it IS fresh news"
        );
        let key = would_send[0].seen_key();
        // `unreported_missed` only reads; the stamping is `report_missed`'s, and
        // that is exactly what must not happen below.
        assert!(
            crate::notify::seen::seen_get(&pool, SeenScope::Missed, &key)
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
            crate::notify::seen::seen_get(&pool, SeenScope::Missed, &key)
                .await
                .unwrap()
                .is_none(),
            "a filtered occurrence must not be recorded as reported — a stamp \
             here is permanent, and would silence a genuine bom for good"
        );
    }
}
