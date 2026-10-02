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
//! ## Honest gaps
//!
//! - **Missed occurrences are not history rows.** [`check_missed`] emits them,
//!   notifies once per occurrence (the `notify_seen` ledger) and, when a wake
//!   was due, logs them to the wake-failure ring — but the `recording` table
//!   has no `status` column, so a missed Sunday never appears in the library.
//! - **Special device override — no hot-plug, no own channel picker.** A special
//!   with its own `device_id` is resolved against the device list at the moment
//!   it STARTS ([`start_settings`]), and records on the global device with a
//!   warning when that device is not there. A device plugged in a minute after
//!   the start is not picked up mid-take, and the special records with the
//!   channel pair the device picker holds for that device (or default routing)
//!   — there is no per-special channel, format or folder.

use std::borrow::Cow;
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
    next_recording, prune_specials, resolve_special_device, scheduled_max_minutes,
    settings_for_special_device, special_device_wanted, supervisor_should_fire, upcoming_dates,
    upcoming_events, CoveredWindow, ScheduledEvent, ScheduledEventKind, SpecialDevice,
    SpecialRecording, TriggerKind, MISSED_WINDOW_MS,
};
use sundayrec_core::settings::Settings;
use sundayrec_core::wake::{background_wake_log_action, should_block, wake_failure_notice_key};

use crate::audio::asio::{AudioBackendKind, TaggedAudioInput};
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
                notify_skipped_busy(app, settings);
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
            // A special with its own device records from it — or, when it is not
            // there, from the global device with a warning. Everything else is
            // `settings` itself, untouched (`start_settings`).
            let Some(rec_settings) = start_settings(app, settings, specials, ev.source).await
            else {
                tracing::warn!(
                    "scheduler: a recording became active while the special's device was \
                     resolved — skipping the scheduled start"
                );
                notify_skipped_busy(app, settings);
                return;
            };
            match crate::recorder::opts::build_opts(
                app,
                &rec_settings,
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
            run_scheduled_preflight(app, pool, settings, specials, ev.source).await;
        }
    }
}

/// «Planlagt opptak hoppet over» — a scheduled start found the recorder busy.
///
/// ALWAYS fires — a skipped scheduled start is a problem report: should_notify
/// pins SkippedBusy on regardless of the notify_start/notify_stop comfort
/// toggles.
fn notify_skipped_busy(app: &AppHandle, settings: &Settings) {
    if should_notify(SchedulerNotice::SkippedBusy, settings) {
        notify_user(
            app,
            APP_TITLE,
            &AlertText::ScheduledSkippedBusy.text(lang_of(settings)),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Special device override
// ─────────────────────────────────────────────────────────────────────────────
//
// A special recording may name its own capture device. The decision is the
// core's (`special_device_wanted` → `resolve_special_device` →
// `settings_for_special_device`); this is the shell around it: enumerate the
// inputs, bounded, only when a special actually asks — and tell the operator
// when it could not have what it asked for.
//
// ## The Sunday invariant
//
// A weekly slot, and a special WITHOUT a device, must record exactly as before:
// no enumeration, no await that does anything, no `validate()`, the SAME
// `Settings` reference handed to `build_opts`. [`choose_device`] answers
// `Global` for them before it touches anything, and the golden tests below
// serialise the composed `RecordingOpts` both ways and compare the bytes.

/// How long a scheduled start waits for the device list before a special gives
/// up on its own device and records on the global one.
///
/// Short on purpose: `fire()` is the supervisor, and a supervisor parked on a
/// wedged driver misses every later recording. Five seconds covers a WASAPI
/// enumeration with room to spare; a cold ASIO sweep (which loads every
/// installed driver) is the one that might not fit, and is a rig item.
const SPECIAL_DEVICE_ENUM_TIMEOUT: StdDuration = StdDuration::from_secs(5);

/// The renderer's picker-id prefix for an ASIO device
/// (`app/state/devices.ts::toDeviceOptions`). A special's `deviceId` is written
/// from that picker, so it is resolved in the same id space — which is also
/// the space `device_channels` is keyed in.
const ASIO_PICKER_PREFIX: &str = "asio::";

/// Enumerated inputs → `(picker id, name)`, the shape the core resolves on.
/// ASIO devices get the picker's prefix; host devices keep their backend id
/// (which is their name).
fn picker_inputs(list: &[TaggedAudioInput]) -> Vec<(String, String)> {
    list.iter()
        .map(|d| {
            let id = if d.backend == AudioBackendKind::Asio {
                format!("{ASIO_PICKER_PREFIX}{}", d.name)
            } else {
                d.id.clone()
            };
            (id, d.name.clone())
        })
        .collect()
}

/// Whether resolving `wanted` needs the ASIO half of the enumeration.
///
/// Only an id the picker gave an ASIO device asks for it. Every other id is a
/// WASAPI/Core Audio device, and a special recording on one must never load
/// every installed ASIO driver at the start of a service (rig item w14) — not
/// even when the device turns out to be missing.
fn wants_asio(wanted: &str) -> bool {
    wanted.starts_with(ASIO_PICKER_PREFIX)
}

/// The device as the operator knows it, for the warning: the picker prefix is
/// an id detail, not part of the name on the box.
fn device_display_name(wanted: &str) -> &str {
    wanted.strip_prefix(ASIO_PICKER_PREFIX).unwrap_or(wanted)
}

/// The real enumeration — BLOCKING, run inside [`choose_device`]'s
/// `spawn_blocking`. The picker's own body (`list_audio_devices`), COM-anchored
/// like every other cpal enumeration, with the ASIO sweep only when the special
/// names an ASIO device.
fn enumerate_special_inputs(wanted: String) -> AppResult<Vec<(String, String)>> {
    let list = crate::commands::audio::enumerate_tagged_inputs(wants_asio(&wanted))?;
    Ok(picker_inputs(&list))
}

/// Why a special's own device was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FallbackReason {
    /// Enumerated, and nothing answers to it.
    Missing,
    /// The enumeration answered with NO inputs at all. On a machine that
    /// manifestly has a microphone that is a probe that failed, not proof
    /// that the special's device is gone (`preflight::device_present` reads an
    /// empty list the same way).
    NothingEnumerated,
    /// The enumeration did not answer within [`SPECIAL_DEVICE_ENUM_TIMEOUT`].
    Timeout,
    /// The enumeration failed (or its task panicked).
    Error(String),
}

/// What a scheduled start records with.
#[derive(Debug)]
enum DeviceChoice {
    /// The global settings, untouched — every weekly slot, and every special
    /// without a device of its own.
    Global,
    /// The special's own device is there: the global settings pointed at it.
    Special(Box<Settings>),
    /// The special asked for a device it cannot have: record on the global
    /// device, and say so.
    Fallback {
        wanted: String,
        reason: FallbackReason,
    },
}

impl DeviceChoice {
    /// The settings `build_opts` composes from. `Global` and `Fallback` hand
    /// back the global settings BY REFERENCE — not a copy, not re-validated.
    fn record_with(self, global: &Settings) -> Cow<'_, Settings> {
        match self {
            DeviceChoice::Global | DeviceChoice::Fallback { .. } => Cow::Borrowed(global),
            DeviceChoice::Special(s) => Cow::Owned(*s),
        }
    }
}

/// THE decision both start paths share — `fire()` and `check_missed`'s late
/// start — with the blocking enumeration injected so the tests can count it.
///
/// Answers `Global` without calling `enumerate` (and without awaiting anything)
/// unless the trigger is a special with a non-blank `device_id`. Only then does
/// it enumerate, on a blocking thread, bounded by `limit`; a timeout leaves
/// that thread to finish on its own — it cannot be cancelled, and it must not
/// hold the supervisor.
async fn choose_device<F>(
    settings: &Settings,
    specials: &[SpecialRecording],
    kind: TriggerKind,
    enumerate: F,
    limit: StdDuration,
) -> DeviceChoice
where
    F: FnOnce(String) -> AppResult<Vec<(String, String)>> + Send + 'static,
{
    let Some(wanted) = special_device_wanted(specials, kind).map(str::to_string) else {
        return DeviceChoice::Global;
    };
    let arg = wanted.clone();
    let inputs = match tokio::time::timeout(
        limit,
        tokio::task::spawn_blocking(move || enumerate(arg)),
    )
    .await
    {
        Ok(Ok(Ok(inputs))) => inputs,
        Ok(Ok(Err(e))) => {
            return DeviceChoice::Fallback {
                wanted,
                reason: FallbackReason::Error(e.to_string()),
            }
        }
        Ok(Err(join)) => {
            return DeviceChoice::Fallback {
                wanted,
                reason: FallbackReason::Error(join.to_string()),
            }
        }
        Err(_) => {
            return DeviceChoice::Fallback {
                wanted,
                reason: FallbackReason::Timeout,
            }
        }
    };
    if inputs.is_empty() {
        return DeviceChoice::Fallback {
            wanted,
            reason: FallbackReason::NothingEnumerated,
        };
    }
    match resolve_special_device(Some(&wanted), &inputs) {
        // Unreachable — `wanted` is non-blank — but total: the global device.
        SpecialDevice::Global => DeviceChoice::Global,
        SpecialDevice::Use { id, name } => {
            DeviceChoice::Special(Box::new(settings_for_special_device(settings, &id, &name)))
        }
        SpecialDevice::Missing { wanted } => DeviceChoice::Fallback {
            wanted,
            reason: FallbackReason::Missing,
        },
    }
}

/// The settings a scheduled start records with — what `fire()` and
/// `check_missed` both call, so the two start paths cannot disagree about the
/// device.
///
/// `None` means "do not start": a recording became active while a special's
/// device was being looked up. Only that path awaits anything, so only that
/// path re-reads the engine — the reading the caller took before is stale the
/// moment an await has passed (F1 finding A4), and `RecorderEngine::start`
/// would stop whatever is running.
///
/// A fallback is said HERE, after that re-read, so a start that is then
/// skipped does not also announce a device switch that never happened.
async fn start_settings<'a>(
    app: &AppHandle,
    settings: &'a Settings,
    specials: &[SpecialRecording],
    kind: TriggerKind,
) -> Option<Cow<'a, Settings>> {
    let choice = choose_device(
        settings,
        specials,
        kind,
        enumerate_special_inputs,
        SPECIAL_DEVICE_ENUM_TIMEOUT,
    )
    .await;
    match plan_start(settings, choice, || {
        app.state::<RecorderEngine>().current_state().is_active()
    }) {
        None => None,
        Some(StartPlan {
            settings: rec,
            own_device,
            fallback,
        }) => {
            if let Some(device) = own_device {
                tracing::info!(
                    device,
                    "scheduler: special recording uses its own audio device"
                );
            }
            if let Some((wanted, reason)) = fallback {
                let device = device_display_name(&wanted);
                tracing::warn!(
                    device,
                    ?reason,
                    "scheduler: the special recording's own audio device is unavailable — \
                     recording on the global device instead"
                );
                // ALWAYS fires (pinned in `should_notify`): the recording runs,
                // but from a device somebody did not choose for it.
                if should_notify(SchedulerNotice::SpecialDeviceFallback, settings) {
                    notify_user(
                        app,
                        APP_TITLE,
                        &AlertText::ScheduledSpecialDeviceFallback
                            .fill(lang_of(settings), &[("device", device)]),
                    );
                }
            }
            Some(rec)
        }
    }
}

/// What [`start_settings`] does with a [`DeviceChoice`] — the pure half, and
/// the ONE function that hands `build_opts` its settings on both start paths.
#[derive(Debug)]
struct StartPlan<'a> {
    /// What the recording is composed from.
    settings: Cow<'a, Settings>,
    /// The special's own device when it is used (for the log).
    own_device: Option<String>,
    /// The device that could not be used, and why (for the warning).
    fallback: Option<(String, FallbackReason)>,
}

/// Turn a device choice into a start plan — `None` when a recording became
/// active while a special's device was being looked up: do not start.
///
/// `engine_active` is the FRESH
/// engine reading, asked for ONLY when a special's device was looked up — the
/// only path that awaited anything, and so the only one whose earlier reading
/// is stale (F1 finding A4). A weekly slot never reads it here, exactly as
/// before.
///
/// This is what the golden tests run: whatever `fire()` and `check_missed`
/// hand `build_opts` comes out of this function and nowhere else.
fn plan_start<'a>(
    settings: &'a Settings,
    choice: DeviceChoice,
    engine_active: impl FnOnce() -> bool,
) -> Option<StartPlan<'a>> {
    let (own_device, fallback) = match &choice {
        DeviceChoice::Global => (None, None),
        DeviceChoice::Special(s) => (s.device_name.clone(), None),
        DeviceChoice::Fallback { wanted, reason } => (None, Some((wanted.clone(), reason.clone()))),
    };
    if !matches!(choice, DeviceChoice::Global) && engine_active() {
        return None;
    }
    Some(StartPlan {
        settings: choice.record_with(settings),
        own_device,
        fallback,
    })
}

/// Who holds an audio input right now. Decides whether the scheduled
/// preflight may enumerate for a special's own device at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MicHolders {
    /// A recording is running or still finalising (`Stopping` holds the
    /// device too — the same reading `start_vu` uses).
    recording: bool,
    /// The pre-roll buffer is running.
    preroll: bool,
    /// The VU meter is running.
    vu: bool,
}

fn mic_holders(app: &AppHandle) -> MicHolders {
    let state = app.state::<RecorderEngine>().current_state();
    MicHolders {
        recording: state.is_active() || state == sundayrec_core::recorder::RecorderState::Stopping,
        preroll: app
            .try_state::<crate::recorder::preroll::PrerollEngine>()
            .is_some_and(|p| p.is_active()),
        vu: app
            .try_state::<crate::audio::vu::VuEngine>()
            .is_some_and(|v| v.is_running()),
    }
}

/// May the preflight enumerate for `wanted` right now?
///
/// The preflight's device check obeys the asymmetry `preflight::device_present`
/// is built on: it may only ever claim ABSENCE it has established, because a
/// false «not connected» half an hour before a service sends a volunteer
/// hunting for a cable that is plugged in. Two states make a special's
/// "missing" unprovable, so the check falls back to the global device:
///
/// - **A recording is running.** Never enumerate under a live take — and a
///   preflight at 11:45 for a 12:15 special lands squarely in the 11:00
///   service.
/// - **The pre-roll or the VU meter holds a device, and the special is ASIO.**
///   asio-sys loads ONE ASIO driver per process: while it holds the global
///   interface's driver, a second ASIO interface enumerates as absent.
fn preflight_may_enumerate(wanted: &str, holders: MicHolders) -> bool {
    if holders.recording {
        return false;
    }
    !(wants_asio(wanted) && (holders.preroll || holders.vu))
}

/// The device the scheduled preflight checks, with who-holds-the-mic and the
/// enumeration injected so the tests can drive both.
///
/// A weekly slot or a special without a device: the settings device, as
/// before — nothing is read. Otherwise [`preflight_may_enumerate`] is asked
/// BEFORE the enumeration (never enumerate while recording) and again AFTER it
/// (a take that started meanwhile makes the answer unprovable too).
async fn preflight_device<F>(
    settings: &Settings,
    specials: &[SpecialRecording],
    kind: TriggerKind,
    holders: impl Fn() -> MicHolders,
    enumerate: F,
    limit: StdDuration,
) -> crate::preflight::PreflightDevice
where
    F: FnOnce(String) -> AppResult<Vec<(String, String)>> + Send + 'static,
{
    use crate::preflight::PreflightDevice;
    let Some(wanted) = special_device_wanted(specials, kind) else {
        return PreflightDevice::Settings;
    };
    if !preflight_may_enumerate(wanted, holders()) {
        return PreflightDevice::Settings;
    }
    let choice = choose_device(settings, specials, kind, enumerate, limit).await;
    if !preflight_may_enumerate(wanted, holders()) {
        return PreflightDevice::Settings;
    }
    preflight_device_for(&choice)
}

/// What the scheduled preflight checks for a trigger's device, from the same
/// [`choose_device`] decision the start will make.
///
/// - `Global` → the settings device, exactly as before.
/// - `Special` → the special's device; it was just enumerated, so it is there.
/// - `Fallback(Missing)` → the special's device, NOT there: the volunteer hears
///   it half an hour early, with the device's name.
/// - `Fallback(NothingEnumerated | Timeout | Error)` → we could not tell. The
///   start falls back to the global device if that repeats, so the global
///   device is what is worth checking — never a "missing" claim we cannot back.
fn preflight_device_for(choice: &DeviceChoice) -> crate::preflight::PreflightDevice {
    use crate::preflight::PreflightDevice;
    match choice {
        DeviceChoice::Global => PreflightDevice::Settings,
        DeviceChoice::Special(s) => PreflightDevice::Resolved {
            name: s.device_name.clone().unwrap_or_default(),
            present: true,
        },
        DeviceChoice::Fallback {
            wanted,
            reason: FallbackReason::Missing,
        } => PreflightDevice::Resolved {
            name: device_display_name(wanted).to_string(),
            present: false,
        },
        DeviceChoice::Fallback { .. } => PreflightDevice::Settings,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Preflight + missed-check
// ─────────────────────────────────────────────────────────────────────────────

async fn run_scheduled_preflight(
    app: &AppHandle,
    pool: &SqlitePool,
    settings: &Settings,
    specials: &[SpecialRecording],
    kind: TriggerKind,
) {
    use sundayrec_core::preflight::PreflightSeverity;
    let documents = crate::save_folder::documents_dir(app);
    // The device the START will use: a special with its own device is checked
    // against that device, through the same decision `fire()` makes — unless
    // that check could only cry wolf (`preflight_may_enumerate`).
    let device = preflight_device(
        settings,
        specials,
        kind,
        || mic_holders(app),
        enumerate_special_inputs,
        SPECIAL_DEVICE_ENUM_TIMEOUT,
    )
    .await;
    // When it is the special's device that is missing, the finding itself says
    // so — `SpecialDeviceMissing`, carrying the device's name — and the
    // notification below and the Record page's card both read it from there.
    let outcome =
        crate::preflight::run_preflight_detailed(pool, documents.as_deref(), device).await;
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
            let body = preflight_notification_body(first, lang);
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

/// The native preflight sentence for the first ERROR finding.
///
/// Every sentence comes from the finding's own code and params — including the
/// one that names a special's device (`PreflightCode::SpecialDeviceMissing`,
/// `{device}`): the generic `DeviceMissing` points at «the device selected in
/// settings», which is the wrong device to go and look for. That is also what
/// makes this sentence and the Record page's card agree: both look the same
/// code up.
fn preflight_notification_body(
    first: &sundayrec_core::preflight::PreflightFinding,
    lang: Lang,
) -> String {
    match first.code {
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
    }
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
        // The same device decision `fire()` makes (`start_settings`). `None`:
        // something started recording while a special's device was looked up —
        // the trigger still counts as handled, and nothing is clobbered.
        match start_settings(app, &settings, specials, t.kind).await {
            None => tracing::warn!(
                "scheduler: a recording became active while the special's device was \
                 resolved — no late start"
            ),
            Some(rec_settings) => match crate::recorder::opts::build_opts(
                app,
                &rec_settings,
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
                        // The recovery attempt for an already-missed recording
                        // just failed too — the operator hears it natively.
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
                    // below will NOT claim it — without this dispatch a late
                    // start that could not even be prepared was said nowhere at
                    // all.
                    dispatch_scheduler_failure(
                        app,
                        "scheduled_late_start_failed",
                        AlertText::ScheduledLateStartFailed
                            .fill(lang_of(&settings), &[("detail", &e.to_string())]),
                    );
                }
            },
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
        .iter()
        .map(|m| MissedRecordingInfo {
            at: fmt_dt(m.when),
            label: m.label.clone(),
        })
        .collect();
    if !out.is_empty() {
        let _ = app.emit(MISSED_EVENT, &out);
        let slots: Vec<crate::notify::MissedSlot> = missed
            .iter()
            .map(crate::notify::MissedSlot::from_missed)
            .collect();
        report_missed(app, pool, &settings, &slots).await;
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
async fn report_missed(
    app: &AppHandle,
    pool: &SqlitePool,
    settings: &Settings,
    missed: &[crate::notify::MissedSlot],
) {
    use sundayrec_core::notify::SeenScope;

    let lang = lang_of(settings);
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

    // «Vekkehistorikk» — the log under Avansert → Test vekking — was written by
    // nothing: `insert_wake_failure` had tests and no caller, so the list was
    // empty on every machine. A missed occurrence belongs in it exactly when a
    // wake was supposed to get the machine up for it; with «Vekk maskinen fra
    // dvale» off, nothing was going to wake it, and the entry («Gikk glipp av en
    // planlagt vekking») would blame the wrong thing. `fresh` is already the
    // once-per-occurrence set, so the log gets one line per missed Sunday.
    if settings.wake_from_sleep {
        for slot in &fresh {
            if let Err(e) =
                crate::db::store::insert_wake_failure(pool, &missed_wake_entry(slot, now)).await
            {
                tracing::warn!("scheduler: could not log the missed wake: {e}");
            }
        }
    }

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
    missed: &[crate::notify::MissedSlot],
    now_ms: i64,
) -> Vec<crate::notify::MissedSlot> {
    use sundayrec_core::notify::{seen_decision, SeenScope};

    let mut fresh = Vec::new();
    for slot in missed {
        let slot = slot.clone();
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
/// F1 A8 made the sentence itself speak the volunteer's language; the two
/// things inside it did not. The slot's name was the canonical Norwegian label
/// (it is hashed into the durable `notify_seen` key, so it cannot follow the
/// language) and the time was the raw `2026-09-06T11:00:00` the ledger stores.
/// Both are now WORDS built beside the key: [`alerts::missed_label`] from the
/// slot's kind, and the tray's own weekday + clock («søn. 11:00») for the time.
/// The key underneath is unchanged — a volunteer who switches language is not
/// told about the same Sunday twice.
///
/// [`alerts::missed_label`]: sundayrec_core::alerts::missed_label
fn missed_summary(missed: &[crate::notify::MissedSlot], lang: Lang) -> String {
    let words = |slot: &crate::notify::MissedSlot| {
        (
            sundayrec_core::alerts::missed_label(&slot.kind, lang),
            sundayrec_core::tray::format_next_label(Some(&slot.at), lang)
                .unwrap_or_else(|| slot.at.clone()),
        )
    };
    match missed {
        [one] => {
            let (label, at) = words(one);
            AlertText::MissedOne.fill(lang, &[("label", &label), ("at", &at)])
        }
        many => {
            let (label, at) = words(&many[0]);
            AlertText::MissedMany.fill(
                lang,
                &[
                    ("count", &many.len().to_string()),
                    ("label", &label),
                    ("at", &at),
                ],
            )
        }
    }
}

/// The «Vekkehistorikk» line for a missed occurrence: stamped `now`, keyed on
/// the time the recording (and so the wake before it) was due. The canonical
/// label goes in, as it does for every other row: the list renders the kind
/// and the time, and telemetry drops the label before anything leaves.
fn missed_wake_entry(
    slot: &crate::notify::MissedSlot,
    now_ms: i64,
) -> sundayrec_core::wake::WakeFailureEntry {
    sundayrec_core::wake::WakeFailureEntry {
        timestamp: now_ms,
        scheduled_at: slot.at.clone(),
        kind: sundayrec_core::wake::WakeFailureKind::Missed,
        label: slot.label.clone(),
        reason: None,
        delta_sec: None,
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
    /// A special recording's own device was unavailable at its start, so it
    /// records on the global device.
    SpecialDeviceFallback,
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
        | SchedulerNotice::PreflightFinding
        | SchedulerNotice::SpecialDeviceFallback => true,
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
        assert!(should_notify(SchedulerNotice::SpecialDeviceFallback, &s));
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

    /// A swept slot as `check_missed` hands it over. The kind only matters to
    /// the sentence; the ledger keys on `at` + `label`.
    fn info(at: &str, label: &str) -> crate::notify::MissedSlot {
        crate::notify::MissedSlot {
            at: at.into(),
            label: label.into(),
            kind: sundayrec_core::schedule::MissedKind::Special {
                name: Some(label.into()),
            },
        }
    }

    fn weekly_sunday() -> crate::notify::MissedSlot {
        crate::notify::MissedSlot {
            at: "2026-09-06T11:00:00".into(),
            label: "Ukentlig opptak (11:00–13:00)".into(),
            kind: sundayrec_core::schedule::MissedKind::Weekly {
                start: "11:00".into(),
                stop: "13:00".into(),
            },
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
        let one = weekly_sunday();
        let many = vec![one.clone(), one.clone(), one.clone()];
        let s1 = missed_summary(std::slice::from_ref(&one), Lang::No);
        assert_eq!(
            s1, "Planlagt opptak ble ikke gjort: Ukentlig opptak (11:00–13:00) (søn. 11:00).",
            "a Norwegian volunteer reads the canonical label, and a time — not an ISO string"
        );
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
        // The slot's NAME and TIME are Polish too now — the words are built
        // from the kind; the Norwegian label stays underneath, as the key.
        assert!(p1.contains("Cotygodniowe nagranie (11:00–13:00)"), "{p1}");
        assert!(p1.contains("niedz. 11:00"), "{p1}");
        assert!(
            !p1.contains("Ukentlig") && !p1.contains("2026-09-06T"),
            "{p1}"
        );
        assert!(p3.contains("Cotygodniowe nagranie"), "{p3}");
        assert_eq!(
            one.seen_key(),
            crate::notify::MissedSlot {
                kind: sundayrec_core::schedule::MissedKind::Special { name: None },
                ..one.clone()
            }
            .seen_key(),
            "the key is at + canonical label — the words never reach it"
        );
    }

    /// «Vekkehistorikk» gets one line per missed occurrence, and only when a
    /// wake was supposed to happen. The row is what `wake_failure_history`
    /// hands the list: kind `missed`, due at the slot's time.
    #[tokio::test]
    async fn a_missed_sunday_lands_in_the_wake_history() {
        let (pool, _d) = temp_pool().await;
        let slot = weekly_sunday();
        crate::db::store::insert_wake_failure(&pool, &missed_wake_entry(&slot, 1_000))
            .await
            .unwrap();
        let rows = crate::db::store::list_wake_failures(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, sundayrec_core::wake::WakeFailureKind::Missed);
        assert_eq!(rows[0].scheduled_at, "2026-09-06T11:00:00");
        assert_eq!(rows[0].timestamp, 1_000);
        assert_eq!(rows[0].reason, None);
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
        let sweep = |covered: &[CoveredWindow]| -> Vec<crate::notify::MissedSlot> {
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                now,
                &[],
                covered,
                &HashSet::new(),
            )
            .iter()
            .map(crate::notify::MissedSlot::from_missed)
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

    // ── Special device override ─────────────────────────────────────────────
    //
    // `choose_device` is the decision `start_settings` makes for BOTH start
    // paths, with the blocking enumeration injected. The engine re-read and the
    // notification around it need an `AppHandle`; the device decision does not.

    use std::sync::atomic::AtomicUsize;

    /// A frozen clock for the composition, so two calls name the same file.
    fn sunday_eleven() -> NaiveDateTime {
        dt("2026-06-07 11:00")
    }

    /// A profile with something for `validate()` and the channel map to bite
    /// on: an out-of-range flat pair (`validate` would clamp it) and a channel
    /// map with no entry for the selected device (`validate` would clear the
    /// pair). If the slot path ever re-validated, or pointed itself at a
    /// device, the composed opts would change — which is what the golden tests
    /// are there to see.
    fn golden_settings(save: &std::path::Path) -> Settings {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "asio::Focusrite USB ASIO".to_string(),
            sundayrec_core::settings::DeviceChannels {
                channel_l: 2,
                channel_r: 3,
            },
        );
        Settings {
            save_folder: Some(save.to_string_lossy().into_owned()),
            device_id: Some("Behringer X32".into()),
            device_name: Some("Behringer X32".into()),
            device_channels: map,
            input_channel_l: Some(99),
            input_channel_r: Some(-5),
            slots: vec![sunday_slot()],
            ..Settings::default()
        }
    }

    /// An enumerator that counts its calls in `calls` and answers with
    /// `inputs`.
    fn counting(
        calls: &Arc<AtomicUsize>,
        inputs: Vec<(String, String)>,
    ) -> impl FnOnce(String) -> AppResult<Vec<(String, String)>> + Send + 'static {
        let seen = Arc::clone(calls);
        move |_wanted: String| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(inputs)
        }
    }

    fn present_inputs() -> Vec<(String, String)> {
        [
            ("asio::Focusrite USB ASIO", "Focusrite USB ASIO"),
            ("Behringer X32", "Behringer X32"),
            ("Rode NT-USB", "Rode NT-USB"),
        ]
        .iter()
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect()
    }

    fn special_on(device: Option<&str>) -> SpecialRecording {
        SpecialRecording {
            device_id: device.map(str::to_string),
            ..special("2026-06-07", "11:00", "12:00", "Konfirmasjon")
        }
    }

    /// What `fire()` composed BEFORE the override existed: `build_opts` over
    /// the global settings, as they were loaded.
    fn old_path_json(
        folder: &std::path::Path,
        settings: &Settings,
        custom_name: Option<&str>,
        max: u32,
    ) -> Vec<u8> {
        let opts = crate::recorder::opts::build_opts_in(
            folder,
            settings,
            custom_name,
            max,
            None,
            sunday_eleven(),
        )
        .expect("old path composes");
        serde_json::to_vec(&opts).unwrap()
    }

    /// The settings a plan records with; panics on `SkipBusy`.
    fn recorded<'a>(plan: Option<StartPlan<'a>>) -> Cow<'a, Settings> {
        plan.expect("expected a start, got a skip").settings
    }

    /// The engine reading a weekly slot (or a special without a device) must
    /// never ask for: those paths did not read it here before the override.
    fn never_read() -> bool {
        panic!("the Global path must not re-read the engine")
    }

    /// What `fire()` composes NOW: `choose_device`, then `plan_start` — the
    /// pure half of `start_settings`, the one function that hands `build_opts`
    /// its settings on both start paths — then `build_opts` over the result.
    async fn new_path_json(
        folder: &std::path::Path,
        settings: &Settings,
        specials: &[SpecialRecording],
        kind: TriggerKind,
        custom_name: Option<&str>,
        max: u32,
        inputs: Vec<(String, String)>,
    ) -> (Vec<u8>, usize) {
        let calls = Arc::new(AtomicUsize::new(0));
        let enumerate = counting(&calls, inputs);
        let choice = choose_device(
            settings,
            specials,
            kind,
            enumerate,
            StdDuration::from_secs(5),
        )
        .await;
        let rec = recorded(plan_start(settings, choice, never_read));
        let opts = crate::recorder::opts::build_opts_in(
            folder,
            &rec,
            custom_name,
            max,
            None,
            sunday_eleven(),
        )
        .expect("new path composes");
        (
            serde_json::to_vec(&opts).unwrap(),
            calls.load(Ordering::SeqCst),
        )
    }

    /// THE Sunday invariant: a weekly slot composes byte-identical opts, and
    /// enumerates nothing — even when a special in the same profile has a
    /// device of its own (Slot(0) and Special(0) are different zeros).
    #[tokio::test]
    async fn golden_a_weekly_slot_records_exactly_as_before() {
        let save = tempfile::tempdir().unwrap();
        let settings = golden_settings(save.path());
        let specials = vec![special_on(Some("Rode NT-USB"))];
        let max = scheduled_max_minutes(0);

        let old = old_path_json(save.path(), &settings, None, max);
        let (new, calls) = new_path_json(
            save.path(),
            &settings,
            &specials,
            TriggerKind::Slot(0),
            None,
            max,
            present_inputs(),
        )
        .await;
        assert_eq!(
            String::from_utf8(new).unwrap(),
            String::from_utf8(old).unwrap(),
            "a weekly slot's RecordingOpts must be byte-identical to the pre-override path"
        );
        assert_eq!(calls, 0, "a weekly slot must never enumerate devices");
    }

    /// …and so does a special WITHOUT a device: `null` (every special the
    /// renderer ever wrote before this change), an empty string, blanks.
    #[tokio::test]
    async fn golden_a_special_without_a_device_records_exactly_as_before() {
        let save = tempfile::tempdir().unwrap();
        let settings = golden_settings(save.path());
        let max = scheduled_max_minutes(0);
        for device in [None, Some(""), Some("   ")] {
            let specials = vec![special_on(device)];
            let old = old_path_json(save.path(), &settings, Some("Konfirmasjon"), max);
            let (new, calls) = new_path_json(
                save.path(),
                &settings,
                &specials,
                TriggerKind::Special(0),
                Some("Konfirmasjon"),
                max,
                present_inputs(),
            )
            .await;
            assert_eq!(
                String::from_utf8(new).unwrap(),
                String::from_utf8(old).unwrap(),
                "device = {device:?}: the opts must be byte-identical"
            );
            assert_eq!(calls, 0, "device = {device:?}: nothing to enumerate for");
        }
    }

    /// A special whose device IS there records from it — the name the engine
    /// opens, and the channel pair the picker holds for THAT device.
    #[tokio::test]
    async fn a_special_with_a_present_device_records_from_it() {
        let save = tempfile::tempdir().unwrap();
        let settings = golden_settings(save.path());
        let specials = vec![special_on(Some("asio::Focusrite USB ASIO"))];
        let calls = Arc::new(AtomicUsize::new(0));
        let enumerate = counting(&calls, present_inputs());
        let choice = choose_device(
            &settings,
            &specials,
            TriggerKind::Special(0),
            enumerate,
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "enumerated exactly once");
        assert!(matches!(choice, DeviceChoice::Special(_)), "{choice:?}");
        let rec = recorded(plan_start(&settings, choice, || false));
        let opts = crate::recorder::opts::build_opts_in(
            save.path(),
            &rec,
            Some("Konfirmasjon"),
            scheduled_max_minutes(0),
            None,
            sunday_eleven(),
        )
        .unwrap();
        assert_eq!(opts.audio_device_name, "Focusrite USB ASIO");
        assert_eq!(
            (opts.input_channel_l, opts.input_channel_r),
            (Some(2), Some(3))
        );

        // Only the device moved: the rest of the opts are the global ones.
        let global = crate::recorder::opts::build_opts_in(
            save.path(),
            &settings,
            Some("Konfirmasjon"),
            scheduled_max_minutes(0),
            None,
            sunday_eleven(),
        )
        .unwrap();
        assert_eq!(opts.output_path, global.output_path);
        assert_eq!(opts.channel_mode, global.channel_mode);
        assert_eq!(opts.manual_max_minutes, global.manual_max_minutes);
        assert_eq!(opts.separate_audio_format, global.separate_audio_format);
    }

    /// The fallback IS the global recording, byte for byte — and it says so.
    #[tokio::test]
    async fn a_missing_special_device_falls_back_to_exactly_the_global_recording() {
        let save = tempfile::tempdir().unwrap();
        let settings = golden_settings(save.path());
        let specials = vec![special_on(Some("Zoom H6"))];
        let max = scheduled_max_minutes(0);
        let calls = Arc::new(AtomicUsize::new(0));
        let enumerate = counting(&calls, present_inputs());
        let choice = choose_device(
            &settings,
            &specials,
            TriggerKind::Special(0),
            enumerate,
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        match &choice {
            DeviceChoice::Fallback { wanted, reason } => {
                assert_eq!(wanted, "Zoom H6");
                assert_eq!(*reason, FallbackReason::Missing);
            }
            other => panic!("expected a fallback, got {other:?}"),
        }
        let plan = plan_start(&settings, choice, || false);
        assert_eq!(
            plan.as_ref()
                .expect("an idle engine must start")
                .fallback
                .as_ref(),
            Some(&("Zoom H6".to_string(), FallbackReason::Missing)),
            "the warning names the device that was not there"
        );
        let rec = recorded(plan);
        assert!(
            matches!(rec, Cow::Borrowed(_)),
            "the global settings, not a copy"
        );
        let new = crate::recorder::opts::build_opts_in(
            save.path(),
            &rec,
            Some("Konfirmasjon"),
            max,
            None,
            sunday_eleven(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&new).unwrap(),
            String::from_utf8(old_path_json(
                save.path(),
                &settings,
                Some("Konfirmasjon"),
                max
            ))
            .unwrap()
        );
    }

    /// A wedged enumeration must not hold the supervisor: past the bound, the
    /// start goes ahead on the global device.
    #[tokio::test]
    async fn a_slow_enumeration_falls_back_instead_of_holding_the_start() {
        let settings = Settings::default();
        let specials = vec![special_on(Some("Rode NT-USB"))];
        let started = std::time::Instant::now();
        let choice = choose_device(
            &settings,
            &specials,
            TriggerKind::Special(0),
            |_wanted: String| {
                std::thread::sleep(StdDuration::from_millis(400));
                Ok(present_inputs())
            },
            StdDuration::from_millis(20),
        )
        .await;
        assert!(
            started.elapsed() < StdDuration::from_millis(350),
            "the bound, not the enumeration, decided when to go on"
        );
        assert!(matches!(
            choice,
            DeviceChoice::Fallback {
                reason: FallbackReason::Timeout,
                ..
            }
        ));
        assert!(matches!(
            recorded(plan_start(&settings, choice, || false)),
            Cow::Borrowed(_)
        ));
    }

    #[tokio::test]
    async fn a_failed_enumeration_falls_back_too() {
        let settings = Settings::default();
        let specials = vec![special_on(Some("Rode NT-USB"))];
        let choice = choose_device(
            &settings,
            &specials,
            TriggerKind::Special(0),
            |_wanted: String| Err(crate::error::AppError::Audio("no host".into())),
            StdDuration::from_secs(5),
        )
        .await;
        assert!(
            matches!(
                &choice,
                DeviceChoice::Fallback {
                    reason: FallbackReason::Error(_),
                    ..
                }
            ),
            "{choice:?}"
        );
    }

    #[test]
    fn picker_ids_follow_the_renderer() {
        // `app/state/devices.ts::toDeviceOptions`: ASIO → `asio::<name>`, host
        // devices their backend id. A special's `deviceId` was written from
        // that picker, so this is the space it is resolved in.
        let tagged = |id: &str, backend| TaggedAudioInput {
            id: id.into(),
            name: id.into(),
            backend,
            input_channels: 2,
            sample_rates: vec![48_000],
            is_default: false,
        };
        let list = vec![
            tagged("Focusrite USB ASIO", AudioBackendKind::Asio),
            tagged("Mikrofon (Realtek)", AudioBackendKind::Wasapi),
            tagged("MacBook Pro Microphone", AudioBackendKind::CoreAudio),
        ];
        assert_eq!(
            picker_inputs(&list),
            vec![
                (
                    "asio::Focusrite USB ASIO".to_string(),
                    "Focusrite USB ASIO".to_string()
                ),
                (
                    "Mikrofon (Realtek)".to_string(),
                    "Mikrofon (Realtek)".to_string()
                ),
                (
                    "MacBook Pro Microphone".to_string(),
                    "MacBook Pro Microphone".to_string()
                ),
            ]
        );
    }

    #[test]
    fn only_an_asio_picker_id_asks_for_the_asio_sweep() {
        // w14: a plain WASAPI device — present or missing — never loads the
        // installed ASIO drivers.
        assert!(wants_asio("asio::Focusrite USB ASIO"));
        assert!(!wants_asio("Mikrofon (Realtek)"));
        assert!(!wants_asio("Focusrite USB ASIO"));
        assert!(!wants_asio(""));
        assert_eq!(
            device_display_name("asio::Focusrite USB ASIO"),
            "Focusrite USB ASIO"
        );
        assert_eq!(device_display_name("Rode NT-USB"), "Rode NT-USB");
    }

    #[test]
    fn the_preflight_checks_the_device_the_start_will_use() {
        use crate::preflight::PreflightDevice;
        assert_eq!(
            preflight_device_for(&DeviceChoice::Global),
            PreflightDevice::Settings
        );
        let special = DeviceChoice::Special(Box::new(Settings {
            device_name: Some("Rode NT-USB".into()),
            ..Settings::default()
        }));
        assert_eq!(
            preflight_device_for(&special),
            PreflightDevice::Resolved {
                name: "Rode NT-USB".into(),
                present: true
            }
        );
        // Missing → said half an hour early, with the name on the box.
        assert_eq!(
            preflight_device_for(&DeviceChoice::Fallback {
                wanted: "asio::Focusrite USB ASIO".into(),
                reason: FallbackReason::Missing,
            }),
            PreflightDevice::Resolved {
                name: "Focusrite USB ASIO".into(),
                present: false
            }
        );
        // Could not tell → no "missing" claim; check what the start falls back to.
        for reason in [
            FallbackReason::NothingEnumerated,
            FallbackReason::Timeout,
            FallbackReason::Error("x".into()),
        ] {
            assert_eq!(
                preflight_device_for(&DeviceChoice::Fallback {
                    wanted: "Rode NT-USB".into(),
                    reason,
                }),
                PreflightDevice::Settings
            );
        }
    }

    #[test]
    fn the_fallback_warning_is_never_silenced_and_speaks_the_settings_language() {
        let pl = Settings {
            language: Some("pl".into()),
            notify_start: false,
            notify_stop: false,
            ..Settings::default()
        };
        assert!(should_notify(SchedulerNotice::SpecialDeviceFallback, &pl));
        let body = AlertText::ScheduledSpecialDeviceFallback.fill(
            lang_of(&pl),
            &[("device", device_display_name("asio::Zoom H6"))],
        );
        assert!(body.contains("„Zoom H6”"), "{body}");
        assert_ne!(
            body,
            AlertText::ScheduledSpecialDeviceFallback.fill(Lang::No, &[("device", "Zoom H6")])
        );
    }

    // ── S1: the special's preflight may only claim absence it established ──

    fn idle() -> MicHolders {
        MicHolders::default()
    }

    #[tokio::test]
    async fn a_live_recording_means_no_special_enumeration_and_no_missing_claim() {
        // 11:45, the 11:00 service is recording, a 12:15 special on its own
        // device is due its preflight. Enumerating now would poke the drivers
        // under the take — and on an ASIO rig could not even see the device.
        use crate::preflight::PreflightDevice;
        let specials = vec![special_on(Some("Zoom H6"))];
        let calls = Arc::new(AtomicUsize::new(0));
        let device = preflight_device(
            &Settings::default(),
            &specials,
            TriggerKind::Special(0),
            || MicHolders {
                recording: true,
                ..MicHolders::default()
            },
            counting(&calls, present_inputs()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "never enumerate while recording"
        );
        assert_eq!(
            device,
            PreflightDevice::Settings,
            "the global check, as before — no «missing» for the special"
        );
    }

    #[tokio::test]
    async fn a_take_that_starts_during_the_enumeration_voids_the_verdict() {
        use crate::preflight::PreflightDevice;
        let specials = vec![special_on(Some("Zoom H6"))];
        let calls = Arc::new(AtomicUsize::new(0));
        let reads = AtomicUsize::new(0);
        let device = preflight_device(
            &Settings::default(),
            &specials,
            TriggerKind::Special(0),
            // Idle before the enumeration, recording after it.
            || MicHolders {
                recording: reads.fetch_add(1, Ordering::SeqCst) > 0,
                ..MicHolders::default()
            },
            counting(&calls, present_inputs()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(device, PreflightDevice::Settings);
    }

    #[tokio::test]
    async fn an_asio_special_is_not_checked_while_another_asio_driver_may_be_held() {
        // asio-sys loads ONE driver per process: with the pre-roll or the VU
        // on the global interface, a second ASIO interface enumerates absent.
        use crate::preflight::PreflightDevice;
        let asio = vec![special_on(Some("asio::Zoom H6 ASIO"))];
        for holders in [
            MicHolders {
                vu: true,
                ..MicHolders::default()
            },
            MicHolders {
                preroll: true,
                ..MicHolders::default()
            },
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let device = preflight_device(
                &Settings::default(),
                &asio,
                TriggerKind::Special(0),
                || holders,
                counting(&calls, present_inputs()),
                StdDuration::from_secs(5),
            )
            .await;
            assert_eq!(calls.load(Ordering::SeqCst), 0, "{holders:?}");
            assert_eq!(device, PreflightDevice::Settings, "{holders:?}");
        }

        // A WASAPI/Core Audio special is unaffected by a running VU: shared
        // mode lists every endpoint, so a miss there IS established.
        let host = vec![special_on(Some("Zoom H6"))];
        let calls = Arc::new(AtomicUsize::new(0));
        let device = preflight_device(
            &Settings::default(),
            &host,
            TriggerKind::Special(0),
            || MicHolders {
                vu: true,
                ..MicHolders::default()
            },
            counting(&calls, present_inputs()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            device,
            PreflightDevice::Resolved {
                name: "Zoom H6".into(),
                present: false
            }
        );
    }

    #[tokio::test]
    async fn an_empty_enumeration_is_not_a_missing_device() {
        use crate::preflight::PreflightDevice;
        let specials = vec![special_on(Some("Rode NT-USB"))];
        let calls = Arc::new(AtomicUsize::new(0));
        let device = preflight_device(
            &Settings::default(),
            &specials,
            TriggerKind::Special(0),
            idle,
            counting(&calls, Vec::new()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(device, PreflightDevice::Settings);

        // The START still falls back — it cannot find the device either — but
        // for the honest reason.
        let choice = choose_device(
            &Settings::default(),
            &specials,
            TriggerKind::Special(0),
            |_wanted: String| Ok(Vec::new()),
            StdDuration::from_secs(5),
        )
        .await;
        assert!(
            matches!(
                choice,
                DeviceChoice::Fallback {
                    reason: FallbackReason::NothingEnumerated,
                    ..
                }
            ),
            "{choice:?}"
        );
    }

    #[tokio::test]
    async fn an_established_miss_with_nobody_on_the_mic_is_said_by_name() {
        use crate::preflight::PreflightDevice;
        let specials = vec![special_on(Some("asio::Zoom H6 ASIO"))];
        let device = preflight_device(
            &Settings::default(),
            &specials,
            TriggerKind::Special(0),
            idle,
            |_wanted: String| Ok(present_inputs()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(
            device,
            PreflightDevice::Resolved {
                name: "Zoom H6 ASIO".into(),
                present: false
            }
        );
    }

    #[tokio::test]
    async fn a_slot_preflight_reads_nothing_new() {
        // The weekly slot's preflight is the settings check, exactly as
        // before: no holders read, no enumeration.
        use crate::preflight::PreflightDevice;
        let calls = Arc::new(AtomicUsize::new(0));
        let device = preflight_device(
            &Settings::default(),
            &[special_on(Some("Zoom H6"))],
            TriggerKind::Slot(0),
            || panic!("a slot's preflight must not read who holds the mic"),
            counting(&calls, present_inputs()),
            StdDuration::from_secs(5),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(device, PreflightDevice::Settings);
    }

    #[test]
    fn the_missing_special_preflight_names_the_device() {
        use sundayrec_core::preflight::{assemble_findings_for, PreflightFacts};
        // Everything fine except the device — the facts the scheduler's run
        // reaches `assemble_findings_for` with.
        let facts = PreflightFacts {
            ffmpeg_missing: false,
            folder_writable: true,
            free_bytes: None,
            video_active: false,
            mic_denied: false,
            cam_denied: false,
            device_present: false,
            save_folder_onedrive: false,
        };
        let special = &assemble_findings_for(facts, Some("Zoom H6"))[0];
        let body = preflight_notification_body(special, Lang::No);
        assert!(body.contains("«Zoom H6»"), "{body}");
        assert!(!body.contains("innstillingene"), "{body}");
        assert_eq!(
            body,
            AlertText::PreflightSpecialDeviceMissing.fill(Lang::No, &[("device", "Zoom H6")])
        );
        // The global device's miss keeps the sentence it always had.
        let global = &assemble_findings_for(facts, None)[0];
        assert_eq!(
            preflight_notification_body(global, Lang::No),
            AlertText::PreflightDeviceMissing.text(Lang::No)
        );
    }

    // ── S2: the plan both start paths take ───────────────────────────────────

    #[test]
    fn a_special_that_found_the_engine_busy_after_its_lookup_does_not_start() {
        let settings = Settings::default();
        let special = DeviceChoice::Special(Box::default());
        assert!(plan_start(&settings, special, || true).is_none());
        let fallback = DeviceChoice::Fallback {
            wanted: "Zoom H6".into(),
            reason: FallbackReason::Missing,
        };
        assert!(plan_start(&settings, fallback, || true).is_none());
        // Global never asks — `never_read` would panic.
        assert!(matches!(
            recorded(plan_start(&settings, DeviceChoice::Global, never_read)),
            Cow::Borrowed(_)
        ));
    }
}
