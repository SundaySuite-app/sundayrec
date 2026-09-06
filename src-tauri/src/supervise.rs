//! One supervisor for every long-lived background task (E2.2).
//!
//! ## Why this exists
//!
//! The scheduler already had this pattern, and it was the ONLY task in the app
//! that had it: an outer loop that re-spawns the inner task if its handle ever
//! resolves, counts *rapid* re-deaths, backs off, and escalates once. It was
//! written there because a silently-dead scheduler misses every future
//! recording — which for a church recorder is the worst possible failure.
//!
//! But it is not the only task with that property. A dead trash sweep means
//! "delete" becomes a leak with a nice name (and the since-removed cloud
//! worker and review-reminder tick had the same shape). Each of those was a
//! bare `spawn` whose `JoinHandle` was dropped on the floor — a panic inside
//! them left no log line, no record, and no replacement.
//!
//! So the scheduler's pattern is extracted here VERBATIM (same 300 s healthy
//! threshold, same 5 s → 30 s backoff, same escalate-once-at-three) and the
//! scheduler now calls it, so there is exactly one implementation to reason
//! about and one place a future fix lands.
//!
//! ## What is deliberately NOT supervised
//!
//! Restarting is only correct for a task that is *supposed* to run forever.
//! One long-running task in this app is session-scoped — it owns a device
//! or a process for exactly as long as the user asked, and its handle is held
//! by an engine so `stop()` can abort it:
//!
//!   - the **recorder supervisor** (`recorder::engine`): re-spawning it after a
//!     stop would start recording again.
//!
//! and one is per-recording by design:
//!
//!   - the **low-disk poller** (`notify::disk`): it exits when the take ends,
//!     which is its contract; a "restart" would poll forever after the
//!     recording stopped.
//!
//! Those keep their bare spawn. The poller instead gets
//! [`crate::crash::watch_handle`], because a panic there silently disables
//! low-disk warnings for the rest of the process and nothing would have said so.

use std::future::Future;
use std::time::Duration;

use sundayrec_core::alerts::AlertText;
use tauri::AppHandle;

/// A supervisor that ran at least this long before dying was a one-off, not a
/// persistent fault — the rapid counter resets. Lifted unchanged from the
/// scheduler.
const HEALTHY_RUN: Duration = Duration::from_secs(300);

/// Delay before re-spawning while the fault still looks transient.
const QUICK_RETRY: Duration = Duration::from_secs(5);

/// Delay once the fault is established, so a permanently-broken task cannot
/// spin the CPU (or the log) forever.
const BACKOFF: Duration = Duration::from_secs(30);

/// The rapid-restart count at which the operator is told, exactly once.
const ESCALATE_AT: u32 = 3;

/// The native notification a task fires at [`ESCALATE_AT`] rapid restarts.
///
/// Required, not optional, at every call site: a task that has died three times
/// in fifteen seconds is a real fault the operator can act on (restart the app,
/// run Diagnose), and deciding per task what they are told is a decision worth
/// making deliberately rather than defaulting.
///
/// F1 A8: the two `&'static str` fields were Norwegian sentences, so a Polish
/// or German church was told its scheduler had died in a language it had not
/// chosen. They are [`AlertText`] keys now, resolved to a sentence at FIRE
/// time — not at spawn time — through [`crate::ui_lang`]. Spawn time is
/// process start, before the renderer has pushed a language change; fire time
/// is the moment the person is actually being told something.
#[derive(Debug, Clone, Copy)]
pub struct TaskAlert {
    /// The notification title, or `None` for the bare app name. Three of the
    /// five call sites want exactly that (both telemetry tasks and the relay
    /// pump): "SundayRec" is a product name, and translating it seven ways
    /// would be inventing work.
    pub title: Option<AlertText>,
    pub body: AlertText,
}

/// What to do after a supervised task's handle resolved. Pure — the whole
/// policy of [`supervise_loop`] in one function, so the arithmetic is provable
/// without spawning anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RestartDecision {
    /// The new consecutive-rapid-restart count.
    pub rapid_restarts: u32,
    /// How long to wait before re-spawning.
    pub delay: Duration,
    /// Whether to fire the native alert (true exactly once, at the threshold).
    pub notify: bool,
}

/// Decide the restart policy from the previous rapid count and how long the
/// task that just died had been running.
pub(crate) fn restart_decision(previous_rapid: u32, ran_for: Duration) -> RestartDecision {
    let rapid_restarts = if ran_for >= HEALTHY_RUN {
        0
    } else {
        previous_rapid.saturating_add(1)
    };
    RestartDecision {
        rapid_restarts,
        delay: if rapid_restarts >= ESCALATE_AT {
            BACKOFF
        } else {
            QUICK_RETRY
        },
        // Escalate ONCE, at the threshold — then back off quietly. Notifying on
        // every restart past three would turn one fault into a notification
        // storm during a service.
        notify: rapid_restarts == ESCALATE_AT,
    }
}

/// Spawn `factory()` and keep it running for the process lifetime.
///
/// `factory` is called once per (re)start, so it must clone whatever the task
/// body needs — an `AppHandle`, an `Arc<Notify>`, a pool. If the spawned future
/// ever resolves (a panic unwinds the task and its handle resolves; so does an
/// unexpected `return`), it is re-spawned after [`restart_decision`]'s delay.
///
/// Every restart is logged AND written to the crash ring's `task_restart`
/// records, so E2.5's diagnose report and E3's telemetry can say "this task has
/// died eleven times since Thursday" instead of the operator noticing that
/// backups stopped a month ago.
pub fn supervised_spawn<F, Fut>(app: AppHandle, name: &'static str, alert: TaskAlert, factory: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(supervise_loop(
        name,
        move || {
            // Resolved HERE, when the alert actually fires: the volunteer may
            // have picked their language long after this task was spawned.
            let lang = crate::ui_lang::current();
            let title = alert
                .title
                .map(|t| t.text(lang))
                .unwrap_or_else(|| crate::notify::APP_TITLE.to_string());
            crate::notify::native(&app, &title, &alert.body.text(lang));
        },
        factory,
    ));
}

/// The supervisor body, with the two impure legs (the native notification, the
/// clock) parameterised so the loop itself is testable without an `AppHandle`.
///
/// `on_alert` is invoked at most once per fault episode — see
/// [`restart_decision`].
async fn supervise_loop<F, Fut, A>(name: &'static str, on_alert: A, factory: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    A: Fn() + Send + 'static,
{
    let mut rapid_restarts: u32 = 0;
    loop {
        let started = tokio::time::Instant::now();
        // `tokio::spawn`, not `tauri::async_runtime::spawn`: we are already
        // INSIDE a runtime task here (the outer spawn entered it), so the
        // "must be called from the context of a Tokio runtime" panic cannot
        // apply — and tokio's `JoinError`
        // tells us directly whether the task PANICKED or merely returned.
        let handle = tokio::spawn(factory());
        let outcome = handle.await;
        let panicked = outcome.as_ref().err().is_some_and(|e| e.is_panic());
        let ran_for = started.elapsed();

        let decision = restart_decision(rapid_restarts, ran_for);
        rapid_restarts = decision.rapid_restarts;

        let detail = match &outcome {
            Ok(()) => format!(
                "task returned unexpectedly after {:.0}s (restart #{rapid_restarts})",
                ran_for.as_secs_f64()
            ),
            Err(e) => format!(
                "task ended after {:.0}s: {e} (restart #{rapid_restarts})",
                ran_for.as_secs_f64()
            ),
        };
        tracing::error!(
            task = name,
            panicked,
            rapid_restarts,
            ran_for_s = ran_for.as_secs(),
            "supervised task ENDED unexpectedly — restarting: {detail}"
        );
        crate::crash::record_task_restart(name, &detail);

        if decision.notify {
            on_alert();
        }
        tokio::time::sleep(decision.delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // ── the pure decision ────────────────────────────────────────────────────

    #[test]
    fn the_restart_table_matches_the_schedulers_original_policy() {
        let quick = Duration::from_secs(1);
        let healthy = HEALTHY_RUN;
        // (previous rapid, ran_for) → (new rapid, delay, notify)
        let cases: &[(u32, Duration, u32, Duration, bool)] = &[
            // A first quick death: count 1, retry soon, say nothing.
            (0, quick, 1, QUICK_RETRY, false),
            (1, quick, 2, QUICK_RETRY, false),
            // The third is the fault — escalate ONCE and start backing off.
            (2, quick, 3, BACKOFF, true),
            // …and stay quiet afterwards, however long it keeps failing.
            (3, quick, 4, BACKOFF, false),
            (99, quick, 100, BACKOFF, false),
            // A task that ran healthily then died is a one-off: reset.
            (0, healthy, 0, QUICK_RETRY, false),
            (2, healthy, 0, QUICK_RETRY, false),
            (99, healthy, 0, QUICK_RETRY, false),
            // The boundary is inclusive, exactly as the scheduler had it.
            (2, healthy - Duration::from_millis(1), 3, BACKOFF, true),
            (2, healthy + Duration::from_secs(1), 0, QUICK_RETRY, false),
        ];
        for &(prev, ran, rapid, delay, notify) in cases {
            assert_eq!(
                restart_decision(prev, ran),
                RestartDecision {
                    rapid_restarts: rapid,
                    delay,
                    notify
                },
                "prev={prev} ran_for={ran:?}"
            );
        }
    }

    #[test]
    fn the_counter_cannot_overflow_into_a_silent_reset() {
        // `u32::MAX + 1` wrapping to 0 would silently re-arm the alert and drop
        // the backoff on a task that has failed four billion times.
        let d = restart_decision(u32::MAX, Duration::from_secs(1));
        assert_eq!(d.rapid_restarts, u32::MAX);
        assert_eq!(d.delay, BACKOFF);
        assert!(!d.notify);
    }

    // ── the loop ─────────────────────────────────────────────────────────────

    /// Drive `supervise_loop` for `virtual_secs` of paused time and return
    /// (runs started, alerts fired).
    async fn drive<F, Fut>(virtual_secs: u64, factory: F) -> (u32, u32)
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let alerts = Arc::new(AtomicU32::new(0));
        let runs = Arc::new(AtomicU32::new(0));
        let a = Arc::clone(&alerts);
        let r = Arc::clone(&runs);
        let task = tokio::spawn(supervise_loop(
            "test::supervised",
            move || {
                a.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                r.fetch_add(1, Ordering::SeqCst);
                factory()
            },
        ));
        tokio::time::sleep(Duration::from_secs(virtual_secs)).await;
        task.abort();
        (runs.load(Ordering::SeqCst), alerts.load(Ordering::SeqCst))
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_that_panics_is_respawned_and_the_operator_is_told_exactly_once() {
        // A task that dies instantly, forever: the loop must keep replacing it
        // (that is the whole point) and must alert once, not on every restart.
        let (runs, alerts) = drive(600, || async {
            panic!("supervised task test panic");
        })
        .await;
        assert!(
            runs >= 5,
            "the loop must keep re-spawning a dying task (ran {runs}×)"
        );
        assert_eq!(alerts, 1, "escalate once at the threshold, then stay quiet");
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_that_returns_without_panicking_is_also_respawned() {
        // A supervisor that `return`s is just as dead as one that panicked —
        // the scheduler's original comment says so, and it is the case that
        // actually happened (an early `return` inside the loop body).
        let (runs, _) = drive(600, || async {}).await;
        assert!(
            runs >= 5,
            "a clean early return must restart too (ran {runs}×)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_healthy_run_resets_the_counter_so_the_alert_never_fires() {
        // panic, panic, HEALTHY, panic, panic — four deaths, but never three in
        // a row, so the operator is not told. Without the reset this reaches
        // rapid=4 and fires. That is the behavioural proof of the reset.
        let runs = Arc::new(AtomicU32::new(0));
        let r = Arc::clone(&runs);
        let (_, alerts) = drive(1200, move || {
            let n = r.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 | 1 | 3 | 4 => panic!("quick death"),
                    2 => tokio::time::sleep(HEALTHY_RUN + Duration::from_secs(60)).await,
                    // Park forever so nothing after the scenario counts.
                    _ => std::future::pending::<()>().await,
                }
            }
        })
        .await;
        assert_eq!(
            alerts, 0,
            "a healthy run between fault bursts must reset the rapid counter"
        );
        assert!(runs.load(Ordering::SeqCst) >= 5);
    }
}
