//! The main window's close button — the thin shell over
//! [`sundayrec_core::window`].
//!
//! ## What changed
//!
//! There was no `on_window_event` handler at all. Closing the last window
//! produced `RunEvent::ExitRequested`, and that handler stops the recorder — so
//! a volunteer who closed the window mid-sermon lost the rest of the service.
//! Now a close request during a live (or finalising) session hides the window
//! instead: `api.prevent_close()` + `window.hide()`, with the capture untouched
//! and the app still in the menubar / system tray.
//!
//! Outside a session nothing changed: the close goes through, the last window
//! closing still raises `ExitRequested`, and that handler still stops the
//! sidecars exactly as before.
//!
//! ## …and the quit, which used to walk straight past all of it
//!
//! Cmd+Q, the app menu's Quit and the tray's «Avslutt» are `ExitRequested`, not
//! `CloseRequested`, so the close rule never saw them: one keystroke ended a
//! service. [`request_quit`] is the seam that fixes it — refuse once, confirm,
//! then WAIT for the file — and it is called from three places, all of them
//! user-initiated:
//!
//! * `lib.rs`'s `RunEvent::ExitRequested` with `code: None`,
//! * `crate::tray`'s `TrayAction::Quit`,
//! * `crate::menu`'s Quit item (macOS — see that module for why the app menu
//!   had to be rebuilt for Cmd+Q to be interceptable at all).
//!
//! ## ⚠️ GUI-UNVERIFIED
//!
//! ## Why the decision is not in the closure
//!
//! A `tauri::Window` cannot be constructed in `cargo test`, so anything written
//! inline in the handler is unreachable by every gate the repo has. The rule
//! therefore lives in the pure, exhaustively-matched
//! [`sundayrec_core::window::close_action`] and this file is deliberately dumb:
//! ask, then hide or stand aside.
//!
//! The `prevent_close`/`hide` pair, the Dock-reopen path, and every quit path
//! below need a real desktop session; they are compiled and reasoned about,
//! never clicked headless. The decisions and the notification wording above them
//! are unit-tested, and `docs/APP-SHELL.md` carries the rig steps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Manager, Window, WindowEvent};

use sundayrec_core::timeouts::RecorderTimeouts;
use sundayrec_core::tray::TrayLang;
use sundayrec_core::window::{
    close_action, hidden_notice, quit_action, quit_notice, wait_outcome, CloseAction, HideReason,
    QuitAction, QuitNotice, QuitWait, TraySpot, QUIT_REPEAT_FLOOR_MS,
};

/// The label of the one window `tauri.conf.json` defines. A close request on any
/// other window (none exist today) is left alone.
pub const MAIN_LABEL: &str = "main";

/// One "the window is only hidden" notification per hide. Set while the window
/// is hidden and cleared by [`note_window_shown`] when it comes back, so a
/// volunteer who hides, reopens and hides again is told again — and a platform
/// that delivers `CloseRequested` twice for one click is not.
static NOTICE_PENDING: AtomicBool = AtomicBool::new(false);

/// The `on_window_event` handler. Registered on the builder in `lib.rs`.
pub fn on_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN_LABEL {
        return;
    }
    let app = window.app_handle();
    let state = app
        .state::<crate::recorder::engine::RecorderEngine>()
        .current_state();
    let reason = match close_action(state) {
        // Nothing is running: close means quit, exactly as it always did.
        CloseAction::Exit => return,
        CloseAction::Hide(reason) => reason,
    };

    // Hide FIRST, and only claim the close if the hide actually worked. The
    // other order can leave a window that refuses to close AND refuses to
    // disappear — a wedged app is worse than the bug being fixed.
    if let Err(e) = window.hide() {
        tracing::error!(
            ?state,
            "close during a session: hiding the window failed ({e}) — letting the close through"
        );
        return;
    }
    api.prevent_close();
    tracing::info!(
        ?state,
        "close during a session: window hidden, the recording keeps running"
    );

    if !NOTICE_PENDING.swap(true, Ordering::SeqCst) {
        notify_hidden(app.clone(), reason);
    }
}

/// Re-arm the hide notification. Called from every path that brings the window
/// back (the tray's "Åpne SundayRec", a second launch, the macOS Dock icon), so
/// the next hide explains itself again.
pub fn note_window_shown() {
    NOTICE_PENDING.store(false, Ordering::SeqCst);
}

/// Show + focus the main window. THE one way back from a hidden window, so every
/// caller also re-arms the notification.
pub fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window(MAIN_LABEL) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        note_window_shown();
    }
}

/// Fire the OS notification that says the window is hidden, not closed.
///
/// Deliberately NOT gated by the `notifyStart`/`notifyStop` comfort toggles —
/// see [`sundayrec_core::window::hidden_notice`]. Async because the UI language
/// lives in the settings row and the event-loop callback must not block on the
/// database.
fn notify_hidden(app: tauri::AppHandle, reason: HideReason) {
    tauri::async_runtime::spawn(async move {
        let (title, body) = hidden_notice(reason, ui_lang(&app).await, tray_spot());
        crate::notify::native(&app, &title, &body);
    });
}

/// Where this platform parks a background app's icon. macOS says menu bar,
/// Windows/Linux say notification area.
///
/// A build without the `tray` feature has no icon anywhere — but `tray` is in
/// `default` AND in both release feature lists (docs/SMOKE-TEST.md §"Features"),
/// so that is a CI compile lane, never something a volunteer runs. Such a build
/// still hides rather than killing the capture (the recording matters more than
/// the sentence being exact), and the window is still reachable by launching
/// SundayRec again — the single-instance plugin focuses the running one.
fn tray_spot() -> TraySpot {
    if cfg!(target_os = "macos") {
        TraySpot::Menubar
    } else {
        TraySpot::SystemTray
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Quit — Cmd+Q, the app menu's Quit, the tray's «Avslutt»
// ─────────────────────────────────────────────────────────────────────────────

/// When this process last refused a quit, or `None` if it never has. Read as an
/// age and handed to the pure [`quit_action`]; the policy — how fresh is fresh —
/// lives there, not here.
static LAST_REFUSAL: Mutex<Option<Instant>> = Mutex::new(None);

/// Set while a quit is waiting for the finalisation. The volunteer's next press
/// is then not a new decision but an override: die now.
static WAITING: AtomicBool = AtomicBool::new(false);

/// How often the wait re-reads the recorder's state. Short enough that the app
/// disappears the moment the file lands (a stop that finalises in 2 s must not
/// feel like 10), cheap enough to be free over the cap.
const WAIT_POLL: Duration = Duration::from_millis(250);

/// What the caller must do after [`request_quit`] has had its say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitVerdict {
    /// Nothing was intercepted — let the quit happen the way this caller
    /// normally would (stand aside in `ExitRequested`, `app.exit(0)` in the
    /// tray).
    Proceed,
    /// Handled here: the quit was refused, or accepted and now waiting for the
    /// recording's file. The caller must NOT exit.
    Handled,
}

/// THE quit seam. Every *user-initiated* quit goes through here — Cmd+Q and the
/// app menu's Quit (`crate::menu`), the tray's «Avslutt» (`crate::tray`), and
/// the last window closing while nothing is recording (`RunEvent::ExitRequested`
/// with `code: None`).
///
/// ## ⚠️ What must NOT come through here
///
/// A *programmatic* `app.exit(code)` — the updater's relaunch, the tray path's
/// own second step, and the wait below — comes back as
/// `RunEvent::ExitRequested { code: Some(code) }` (verified in the tauri 2.11
/// source: `RunEvent::ExitRequested`'s `code` is documented `None` for user
/// interaction and `Some` for `AppHandle::exit`/`restart`, and
/// `tauri-runtime-wry` raises `code: None` only from the last-window-destroyed
/// path while `Message::RequestExit` carries `Some(code)`). Asking the policy
/// again on the way out would refuse our own exit, and the app could never die.
/// `lib.rs` therefore lets every `code.is_some()` straight through.
pub fn request_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> QuitVerdict {
    // Already waiting → this press is the volunteer insisting. Give up the file
    // and die: an app that ignores the second Cmd+Q is the failure mode the
    // whole wait exists to avoid becoming.
    if WAITING.load(Ordering::SeqCst) {
        tracing::warn!("quit pressed again while waiting for the recording — exiting now");
        return QuitVerdict::Proceed;
    }

    let state = app
        .state::<crate::recorder::engine::RecorderEngine>()
        .current_state();
    let age = refusal_age_ms();
    let action = quit_action(state, age);
    tracing::info!(?state, ?action, refusal_age_ms = ?age, "quit requested");

    match action {
        // Nothing running: quit exactly as it always did.
        QuitAction::ExitNow => QuitVerdict::Proceed,
        QuitAction::Refuse => {
            // A press inside the floor is a key repeat or a doubled platform
            // event (see `QUIT_REPEAT_FLOOR_MS`). It still re-stamps — so a held
            // Cmd+Q keeps pushing the confirmation out of reach — but it must
            // not fire a notification per repeat.
            let repeat = age.is_some_and(|age| age < QUIT_REPEAT_FLOOR_MS);
            *lock_recover(&LAST_REFUSAL) = Some(Instant::now());
            if !repeat {
                notify_quit(app, QuitNotice::Refused);
            }
            QuitVerdict::Handled
        }
        QuitAction::StopThenWait => {
            // The confirmation. Stop the capture the graceful way — the
            // supervisor finalises the container, delivers the file and writes
            // the history row — and only then let the process die.
            app.state::<crate::recorder::engine::RecorderEngine>()
                .stop();
            notify_quit(app, QuitNotice::Waiting);
            arm_wait(app)
        }
        // The stop is already in flight; all that is missing is the patience.
        QuitAction::WaitOnly => {
            notify_quit(app, QuitNotice::Waiting);
            arm_wait(app)
        }
    }
}

/// Arm the bounded wait for the finalisation, which ends in `app.exit(0)`
/// whichever way it goes.
///
/// Returns [`QuitVerdict::Proceed`] when no wait could be armed — the caller
/// must then quit immediately rather than leave an app that cannot die.
fn arm_wait<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> QuitVerdict {
    if WAITING.swap(true, Ordering::SeqCst) {
        // A wait was armed between the load in `request_quit` and here. One
        // waiter is enough; this press becomes the override.
        return QuitVerdict::Proceed;
    }
    let app = app.clone();
    let cap = Duration::from_millis(RecorderTimeouts::QUIT_WAIT_CAP_MS);
    tauri::async_runtime::spawn(async move {
        let outcome = tokio::select! {
            settled = poll_until_settled(app.clone(), RecorderTimeouts::QUIT_WAIT_CAP_MS) => settled,
            // The cap is the promise that a wedged finalise cannot produce an
            // app nobody can quit. It is derived from the supervisor's own
            // last-resort abort, so it never fires on a merely slow service —
            // see `RecorderTimeouts::QUIT_WAIT_CAP_MS`.
            () = tokio::time::sleep(cap) => QuitWait::Capped,
        };
        match outcome {
            QuitWait::Capped => tracing::error!(
                cap_ms = RecorderTimeouts::QUIT_WAIT_CAP_MS,
                "quit waited out the cap without the recorder reaching rest — exiting anyway"
            ),
            _ => tracing::info!("recording finalised — quitting"),
        }
        // BOTH arms end here. This is a programmatic exit, so it comes back as
        // `ExitRequested { code: Some(0) }` and passes straight through the
        // policy to the ordinary sidecar cleanup.
        app.exit(0);
    });
    QuitVerdict::Handled
}

/// Poll the recorder until the file is safe (or the cap has run out — belt and
/// braces next to the `select!`'s own timer, so neither alone can hang a quit).
async fn poll_until_settled<R: tauri::Runtime>(app: tauri::AppHandle<R>, cap_ms: u64) -> QuitWait {
    let started = Instant::now();
    loop {
        // The `State` guard is a statement temporary on purpose: it must not be
        // held across the await below.
        let state = app
            .state::<crate::recorder::engine::RecorderEngine>()
            .current_state();
        match wait_outcome(state, elapsed_ms(started), cap_ms) {
            QuitWait::KeepWaiting => {}
            settled => return settled,
        }
        tokio::time::sleep(WAIT_POLL).await;
    }
}

/// How long ago this process last refused a quit. `None` when it never has, and
/// the value is only ever *compared* — the freshness rule itself is
/// [`sundayrec_core::window::QUIT_CONFIRM_WINDOW_MS`].
fn refusal_age_ms() -> Option<u64> {
    lock_recover(&LAST_REFUSAL).map(elapsed_ms)
}

/// Milliseconds since `since`, saturated. `Instant` differences cannot be
/// negative and a `u128` that overflows `u64` is 584 million years of uptime, so
/// the saturation is a formality — but an `as` cast that silently wraps is
/// exactly how a "10 second window" becomes "always fresh".
fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A poisoned lock here means another thread panicked while holding it; the
/// value is still a plain `Option<Instant>` and nothing about it can be
/// half-written. Recovering beats poisoning the quit path.
fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Fire the quit notification. Same channel, and the same deliberate exemption
/// from the `notifyStart`/`notifyStop` toggles, as [`notify_hidden`].
fn notify_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>, notice: QuitNotice) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (title, body) = quit_notice(notice, ui_lang(&app).await);
        crate::notify::native(&app, &title, &body);
    });
}

/// The volunteer's UI language, from the settings row. Async because the
/// event-loop callback must never block on the database; `TrayLang::No` is the
/// fallback the tray uses for the same lookup.
async fn ui_lang<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> TrayLang {
    match app.try_state::<crate::db::Db>() {
        Some(db) => TrayLang::from_code(
            crate::settings::load(&db.pool)
                .await
                .unwrap_or_default()
                .language
                .as_deref(),
        ),
        None => TrayLang::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_flag_fires_once_per_hide_and_re_arms_on_show() {
        note_window_shown();
        // First hide claims the notification…
        assert!(!NOTICE_PENDING.swap(true, Ordering::SeqCst));
        // …a repeated CloseRequested for the same hidden window does not.
        assert!(NOTICE_PENDING.swap(true, Ordering::SeqCst));
        // Coming back re-arms it, so the NEXT hide explains itself again.
        note_window_shown();
        assert!(!NOTICE_PENDING.swap(true, Ordering::SeqCst));
        note_window_shown();
    }

    #[test]
    fn the_tray_spot_matches_the_platform_that_has_that_word() {
        let spot = tray_spot();
        if cfg!(target_os = "macos") {
            assert_eq!(spot, TraySpot::Menubar);
        } else {
            assert_eq!(spot, TraySpot::SystemTray);
        }
    }

    #[test]
    fn the_refusal_stamp_is_an_age_the_pure_rule_can_judge() {
        use sundayrec_core::recorder::RecorderState;
        use sundayrec_core::window::QUIT_CONFIRM_WINDOW_MS;

        *lock_recover(&LAST_REFUSAL) = None;
        assert_eq!(refusal_age_ms(), None, "no press yet");
        assert_eq!(
            quit_action(RecorderState::Recording, refusal_age_ms()),
            QuitAction::Refuse,
            "the first Cmd+Q mid-service must be refused"
        );

        *lock_recover(&LAST_REFUSAL) = Some(Instant::now());
        let age = refusal_age_ms().expect("a stamp was just taken");
        assert!(age < 1_000, "a stamp taken now reads {age} ms old");
        // A stamp taken THIS instant is still inside the repeat floor, so the
        // rule reads it as one keystroke arriving twice, not as an answer.
        assert_eq!(
            quit_action(RecorderState::Recording, refusal_age_ms()),
            QuitAction::Refuse,
            "a press in the same millisecond is a repeat"
        );

        // A second, deliberate press — past the floor, inside the window.
        *lock_recover(&LAST_REFUSAL) = stamped_ms_ago(1_000);
        assert_eq!(
            quit_action(RecorderState::Recording, refusal_age_ms()),
            QuitAction::StopThenWait,
            "the second press inside the window is the confirmation"
        );

        // …and the same stamp, once stale, is not a confirmation any more: the
        // Cmd+Q ten minutes later starts the conversation over.
        *lock_recover(&LAST_REFUSAL) = stamped_ms_ago(QUIT_CONFIRM_WINDOW_MS + 1);
        assert_eq!(
            quit_action(RecorderState::Recording, refusal_age_ms()),
            QuitAction::Refuse
        );
        *lock_recover(&LAST_REFUSAL) = None;
    }

    /// A refusal stamp `ms` milliseconds in the past. The test process has been
    /// up for at least that long by the time this runs (the runner starts the
    /// clock before it starts the tests), so the subtraction is defined.
    #[cfg(test)]
    fn stamped_ms_ago(ms: u64) -> Option<Instant> {
        Some(
            Instant::now()
                .checked_sub(Duration::from_millis(ms))
                .expect("the process is older than the window under test"),
        )
    }

    #[test]
    fn a_repeated_press_re_stamps_but_does_not_re_notify() {
        // Both halves of the anti-repeat guard, as the shell implements them:
        // the stamp moves (so the confirmation stays out of reach for as long as
        // the key is held) while the notification does not fire again.
        let age_of_a_repeat = Some(QUIT_REPEAT_FLOOR_MS - 1);
        assert!(age_of_a_repeat.is_some_and(|age| age < QUIT_REPEAT_FLOOR_MS));
        let age_of_a_decision = Some(QUIT_REPEAT_FLOOR_MS);
        assert!(!age_of_a_decision.is_some_and(|age| age < QUIT_REPEAT_FLOOR_MS));
        // A first press has no prior age at all, so it always notifies.
        assert!(!None::<u64>.is_some_and(|age| age < QUIT_REPEAT_FLOOR_MS));
    }

    #[test]
    fn a_second_quit_while_waiting_is_an_override_not_a_new_decision() {
        // The contract `request_quit` and `arm_wait` share: while a wait is
        // armed, the next press must not consult the policy at all (it would
        // say `WaitOnly` and arm a SECOND waiter), it must let the app die.
        WAITING.store(false, Ordering::SeqCst);
        assert!(!WAITING.swap(true, Ordering::SeqCst), "nothing armed yet");
        assert!(
            WAITING.load(Ordering::SeqCst),
            "request_quit short-circuits"
        );
        assert!(
            WAITING.swap(true, Ordering::SeqCst),
            "arm_wait must refuse to stack a second waiter"
        );
        WAITING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn the_wait_polls_often_enough_to_feel_instant_and_far_inside_its_cap() {
        // A stop that finalises in two seconds must not feel like ten…
        assert!(WAIT_POLL <= Duration::from_millis(500), "{WAIT_POLL:?}");
        // …and the poll must be orders of magnitude below the cap, or the cap
        // is not a backstop but a schedule.
        assert!(
            u64::try_from(WAIT_POLL.as_millis()).unwrap() * 100
                < RecorderTimeouts::QUIT_WAIT_CAP_MS
        );
    }

    #[test]
    fn an_absurd_uptime_cannot_make_a_stale_refusal_look_fresh() {
        // `as u64` on a `u128` wraps silently, and a wrapped age below the
        // window would turn every quit into an unconfirmed stop. Saturating is
        // the safe direction: too old, never too fresh.
        let now = Instant::now();
        assert!(elapsed_ms(now) < 1_000);
        assert_eq!(
            u64::try_from(u128::from(u64::MAX) + 1).unwrap_or(u64::MAX),
            u64::MAX
        );
    }
}
