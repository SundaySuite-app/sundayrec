//! What the main window's close button is allowed to do — pure, GUI-free.
//!
//! ## The bug this module exists to close
//!
//! Until now `src-tauri/src/lib.rs` had no `on_window_event` handler at all.
//! Closing the last window therefore produced `RunEvent::ExitRequested`, whose
//! handler calls `RecorderEngine::stop()` — so a volunteer who closed the window
//! mid-sermon lost the rest of the service. `docs/APP-SHELL.md` documented the
//! consequence: the overlay's "you can close the window" hint had to be REMOVED
//! because it was a lie.
//!
//! The decision itself is one line of policy, so it lives here rather than
//! inline in a Tauri closure: closures cannot be unit-tested (a `Window` cannot
//! be built in `cargo test`), and an untested closure is exactly where a
//! "recording is live" predicate quietly drifts out of step with the recorder's
//! own state machine.
//!
//! ## The policy
//!
//! * A live session (`Preparing`/`Recording`/`Reconnecting`) → **hide**. The
//!   capture keeps running and the app stays in the menubar/system tray.
//! * `Stopping` → **hide** as well. That state is NOT "almost idle": the
//!   supervisor emits it *before* `finalize_pending`, so the whole concat +
//!   delivery transcode + history write happens inside it, and a 60–90 minute
//!   service's transcode alone runs minutes. Exiting there is the one moment
//!   that can still destroy an otherwise-complete recording. (The tray's
//!   `state_is_live` draws the same line, for the same reason.)
//! * Everything else (`Idle`/`Stopped`/`Failed`) → **exit**, byte-for-byte the
//!   behaviour that shipped before: close means quit.
//!
//! ## …and the quit that used to walk straight past it
//!
//! Closing the window was only half the door. Cmd+Q, the app menu's Quit and the
//! tray's «Avslutt» are `ExitRequested`, not `CloseRequested`, so they never
//! reached [`close_action`] at all — a single keystroke mid-sermon called
//! `RecorderEngine::stop()` and let the process die on top of it. `stop()` does
//! not block: it signals the supervisor and returns, so the recording's rescue
//! depended on the next launch's recovery scan rather than on a finalised file.
//!
//! [`quit_action`] closes that half:
//!
//! * live (`Preparing`/`Recording`/`Reconnecting`) → **refuse** the first press,
//!   say so, and remember when. A second press between [`QUIT_REPEAT_FLOOR_MS`]
//!   and [`QUIT_CONFIRM_WINDOW_MS`] later means it: stop, then **wait** for the
//!   file. (Sooner than the floor is a key repeat, not an answer.)
//! * `Stopping` → **wait**, with no second press demanded — the volunteer asked
//!   for the stop already.
//! * `Idle`/`Stopped`/`Failed` → **quit now**, exactly as before.
//!
//! A native confirmation dialog was considered and rejected (see
//! `docs/APP-SHELL.md`): the blocking dialogs must not be called from the run
//! loop, and the non-blocking one turns a failed dialog into an app that cannot
//! be quit. A refusal + a notification degrades the other way — worst case the
//! volunteer sees no notice and presses again, which is the outcome they wanted.
//!
//! ## …and the updater's restart, which walked past BOTH
//!
//! [`relaunch_plan`] is the third rule in this file for the same reason the
//! second one exists: `update::relaunch` was neither a close nor a quit, so it
//! reached neither guard — and «Start på nytt og installer» during a service
//! stopped the capture and killed the process on top of it, with one click. It
//! draws the same line as the other two (a test asserts all three agree, state
//! for state); what it cannot do is refuse afterwards, because tauri's
//! `prevent_exit` is a no-op for a restart. The wait therefore has to come
//! first.

use crate::recorder::RecorderState;
use crate::tray::TrayLang;

/// What the shell should do with a close request on the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Keep the process (and the capture) alive; hide the window instead.
    Hide(HideReason),
    /// Let the close through — the app quits, exactly as it did before.
    Exit,
}

/// Why the window was hidden rather than closed. Picks the notification's
/// wording so a hide during finalisation does not claim the service is still
/// being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideReason {
    /// A capture is live.
    Recording,
    /// The capture has stopped and the file is being finalised/written.
    Finishing,
}

/// Where the app's background icon lives, so the notification can tell the
/// volunteer where to click. macOS calls it the menu bar; Windows/Linux call it
/// the notification area / system tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraySpot {
    Menubar,
    SystemTray,
}

/// THE decision. One caller (the `CloseRequested` handler in
/// `src-tauri/src/window.rs`), so this function is the only place the rule
/// exists.
pub fn close_action(state: RecorderState) -> CloseAction {
    match state {
        RecorderState::Preparing | RecorderState::Recording | RecorderState::Reconnecting => {
            CloseAction::Hide(HideReason::Recording)
        }
        RecorderState::Stopping => CloseAction::Hide(HideReason::Finishing),
        RecorderState::Idle | RecorderState::Stopped | RecorderState::Failed => CloseAction::Exit,
    }
}

/// The localised `(title, body)` of the OS notification fired when the window
/// was hidden instead of closed.
///
/// Deliberately NOT gated by the `notifyStart`/`notifyStop` comfort toggles:
/// those silence "the thing you asked for happened" notices. This one says "the
/// thing you just did did not do what you expected, and something you cannot
/// afford to lose is still running" — the same class as the failure
/// notifications, which ignore the toggles too (see
/// `scheduler::should_notify`'s `failure_notices_ignore_the_toggles`).
pub fn hidden_notice(reason: HideReason, lang: TrayLang, spot: TraySpot) -> (String, String) {
    let title = match (reason, lang) {
        (HideReason::Recording, TrayLang::No) => "SundayRec tar fortsatt opp",
        (HideReason::Recording, TrayLang::En) => "SundayRec is still recording",
        (HideReason::Recording, TrayLang::De) => "SundayRec nimmt weiterhin auf",
        (HideReason::Recording, TrayLang::Sv) => "SundayRec spelar fortfarande in",
        (HideReason::Recording, TrayLang::Da) => "SundayRec optager stadig",
        (HideReason::Recording, TrayLang::Pl) => "SundayRec nadal nagrywa",
        (HideReason::Recording, TrayLang::Fr) => "SundayRec enregistre toujours",
        (HideReason::Finishing, TrayLang::No) => "SundayRec lagrer opptaket",
        (HideReason::Finishing, TrayLang::En) => "SundayRec is saving the recording",
        (HideReason::Finishing, TrayLang::De) => "SundayRec speichert die Aufnahme",
        (HideReason::Finishing, TrayLang::Sv) => "SundayRec sparar inspelningen",
        (HideReason::Finishing, TrayLang::Da) => "SundayRec gemmer optagelsen",
        (HideReason::Finishing, TrayLang::Pl) => "SundayRec zapisuje nagranie",
        (HideReason::Finishing, TrayLang::Fr) => "SundayRec enregistre le fichier",
    };
    let body = match lang {
        TrayLang::No => "Vinduet er skjult, ikke lukket. Hent det tilbake fra {spot}.",
        TrayLang::En => "The window is hidden, not closed. Bring it back from the {spot}.",
        TrayLang::De => {
            "Das Fenster ist ausgeblendet, nicht geschlossen. Hol es über {spot} zurück."
        }
        TrayLang::Sv => "Fönstret är dolt, inte stängt. Hämta tillbaka det från {spot}.",
        TrayLang::Da => "Vinduet er skjult, ikke lukket. Hent det tilbage fra {spot}.",
        TrayLang::Pl => "Okno jest ukryte, a nie zamknięte. Przywróć je z {spot}.",
        TrayLang::Fr => "La fenêtre est masquée, pas fermée. Rouvrez-la depuis {spot}.",
    };
    (
        title.to_string(),
        body.replace("{spot}", spot_noun(spot, lang)),
    )
}

/// The localised name of the place the app's icon sits, in the grammatical form
/// the `{spot}` slot above needs ("from the …").
fn spot_noun(spot: TraySpot, lang: TrayLang) -> &'static str {
    match (spot, lang) {
        (TraySpot::Menubar, TrayLang::No) => "menylinja",
        (TraySpot::Menubar, TrayLang::En) => "menu bar",
        (TraySpot::Menubar, TrayLang::De) => "die Menüleiste",
        (TraySpot::Menubar, TrayLang::Sv) => "menyraden",
        (TraySpot::Menubar, TrayLang::Da) => "menulinjen",
        (TraySpot::Menubar, TrayLang::Pl) => "paska menu",
        (TraySpot::Menubar, TrayLang::Fr) => "la barre de menus",
        (TraySpot::SystemTray, TrayLang::No) => "systemstatusfeltet",
        (TraySpot::SystemTray, TrayLang::En) => "system tray",
        (TraySpot::SystemTray, TrayLang::De) => "den Infobereich",
        (TraySpot::SystemTray, TrayLang::Sv) => "aktivitetsfältet",
        (TraySpot::SystemTray, TrayLang::Da) => "proceslinjen",
        (TraySpot::SystemTray, TrayLang::Pl) => "zasobnika systemowego",
        (TraySpot::SystemTray, TrayLang::Fr) => "la zone de notification",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Quit (Cmd+Q, the app menu, the tray's «Avslutt»)
// ─────────────────────────────────────────────────────────────────────────────

/// How long the first, refused quit stays "fresh": press Quit again inside this
/// window and the second press is taken as the confirmation, stops the capture
/// and quits.
///
/// Ten seconds is the whole design. Long enough that a volunteer who read the
/// notification can act on it without hurrying, short enough that the *next*
/// Cmd+Q — minutes later, for an unrelated reason — is refused again rather than
/// silently ending a service because of a keypress nobody remembers. It is also
/// why the confirmation needs no dialog: a dialog that fails to appear leaves an
/// app that cannot be quit at all (the reason `docs/APP-SHELL.md` recorded this
/// as a restanse instead of shipping `tauri-plugin-dialog` inside the run loop),
/// while a refusal that fails to notify still leaves an app that quits on the
/// second press.
///
/// ⚠️ The refusal text spells this number out in seconds ({seconds} → 10). Polish
/// takes the genitive plural ("10 sekund") for 10; a value of 2–4 s would need
/// "sekundy" instead, so `the_refusal_wording_is_pinned_to_ten_seconds` fails if
/// anyone retunes this without revisiting the wording.
pub const QUIT_CONFIRM_WINDOW_MS: u64 = 10_000;

/// The other edge of the same window: a press this soon after the last one is
/// not a second decision.
///
/// Two ways one keystroke can arrive as two. A held Cmd+Q auto-repeats — macOS's
/// fastest repeat *rate* is a few tens of milliseconds once the initial delay
/// (≥120 ms) has passed — and a platform can deliver one menu activation twice
/// (the reason `NOTICE_PENDING` exists on the close side). Without a floor,
/// either turns "refuse the first press" into "refuse, then instantly confirm",
/// which is the exact outcome this whole rule exists to prevent, reached by
/// leaning on a key.
///
/// 300 ms sits above the repeat rate and below a deliberate double-tap. It
/// degrades safely in the one case it gets wrong: an unusually fast intentional
/// double-press is refused again, and the volunteer presses once more.
pub const QUIT_REPEAT_FLOOR_MS: u64 = 300;

/// What the shell should do with a *user-initiated* quit request (Cmd+Q, the app
/// menu's Quit, the tray's «Avslutt»). A programmatic `app.exit(code)` is NOT
/// this — see the shell for why that distinction is load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAction {
    /// A capture is live and no fresh refusal stands: refuse this quit, tell the
    /// volunteer, and remember when. The recording is untouched.
    Refuse,
    /// The volunteer confirmed (a second press inside
    /// [`QUIT_CONFIRM_WINDOW_MS`]): stop the capture, then wait for the file to
    /// land before the process dies.
    StopThenWait,
    /// Nothing to stop — the stop is already in flight — but the file is still
    /// being written: wait for it, then quit. No second press is demanded here;
    /// the volunteer already asked for the stop, and asking twice for something
    /// they did not request is just noise.
    WaitOnly,
    /// Nothing is running: quit immediately, byte-for-byte as before.
    ExitNow,
}

/// THE quit decision. Pure, exhaustively matched (no `_` arm), so a new
/// [`RecorderState`] forces a choice here rather than defaulting into "kill the
/// service".
///
/// `prior_refusal_age_ms` is how long ago this process last refused a quit, or
/// `None` when it never has. The shell measures it; the policy lives here.
pub fn quit_action(state: RecorderState, prior_refusal_age_ms: Option<u64>) -> QuitAction {
    match state {
        // Live capture. The first press is a question, the second is an answer.
        RecorderState::Preparing | RecorderState::Recording | RecorderState::Reconnecting => {
            match prior_refusal_age_ms {
                // Too soon to be a decision — a key repeat or a doubled event.
                // Refusing again (and, in the shell, re-stamping) means a held
                // Cmd+Q can never confirm itself, however long it is held.
                Some(age) if age < QUIT_REPEAT_FLOOR_MS => QuitAction::Refuse,
                Some(age) if age < QUIT_CONFIRM_WINDOW_MS => QuitAction::StopThenWait,
                _ => QuitAction::Refuse,
            }
        }
        // `Stopping` is emitted BEFORE `finalize_pending`, so concat + the
        // delivery transcode + the history row all happen inside it — minutes,
        // for a 60–90 minute service. Quitting here is the one moment that can
        // still destroy an otherwise-complete recording, so the quit is honoured
        // but the process waits for the file.
        RecorderState::Stopping => QuitAction::WaitOnly,
        RecorderState::Idle | RecorderState::Stopped | RecorderState::Failed => QuitAction::ExitNow,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The update's relaunch — the fourth way out of the process
// ─────────────────────────────────────────────────────────────────────────────

/// What a staged-update **relaunch** must do before the process may be replaced.
///
/// ## The hole this closes
///
/// [`quit_action`] guards Cmd+Q, the app menu and the tray. It never saw the
/// updater: `update::relaunch` called `RecorderEngine::stop()` and then killed
/// the process itself, so «Start på nytt og installer» mid-service was the
/// pre-guard outcome reached with one click. `stop()` does not block — it
/// signals the supervisor and returns — so the concat, the delivery transcode
/// and the history row all died with the process, and the recording's rescue
/// fell back to the next launch's recovery scan.
///
/// ## Why the wait cannot be bolted on where the quit's is
///
/// The quit's wait works by REFUSING the exit (`api.prevent_exit()` in the
/// `ExitRequested` handler) and finishing later. That door is nailed shut for a
/// restart: tauri's `ExitRequestApi::prevent_exit` is documented — and
/// implemented — as a no-op when the code is `RESTART_EXIT_CODE`
/// (`if self.code != Some(RESTART_EXIT_CODE)`, tauri 2.11.5 `src/app.rs`), and
/// `AppHandle::restart()` called on the main thread skips the event entirely.
/// So the relaunch cannot be taken back once asked for: the wait must happen
/// BEFORE tauri is asked to restart at all. That is what this type sequences —
/// [`RelaunchPlan::waits_for_the_file`] is the shell's instruction to hold the
/// restart, not to undo it.
///
/// ## Why there is no `Refuse` arm
///
/// The quit refuses the first press because a quit is one keystroke. Reaching a
/// relaunch takes a check, a download and a deliberate "restart & install", and
/// a refusal that the shell cannot explain (the update panel's own text belongs
/// to the renderer) would read as a dead button. The recording is not sacrificed
/// by that choice: [`RelaunchPlan::StopThenWait`] stops it the graceful way and
/// the file lands before the restart. A confirm step, if the renderer ever wants
/// one, belongs in the panel that knows what it is asking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaunchPlan {
    /// Nothing is running: restart now, exactly as the updater always did.
    Now,
    /// A capture is live: stop it the graceful way, then wait for the file
    /// before restarting.
    StopThenWait,
    /// The stop is already in flight (`Stopping` — i.e. inside `finalize_pending`,
    /// where the concat and the delivery transcode live): nothing to stop, but
    /// the restart still waits for the file.
    WaitOnly,
}

impl RelaunchPlan {
    /// Whether the shell must call `RecorderEngine::stop()` first.
    ///
    /// Read by the shell so the sequence is this tested function's answer rather
    /// than a second copy of the rule in an untestable Tauri closure.
    pub fn stops_the_capture(self) -> bool {
        matches!(self, Self::StopThenWait)
    }

    /// Whether the restart must wait for the recorder to reach rest first.
    ///
    /// `false` is the ONLY way a restart happens immediately, so this is the
    /// predicate that decides whether a service survives the update.
    pub fn waits_for_the_file(self) -> bool {
        matches!(self, Self::StopThenWait | Self::WaitOnly)
    }
}

/// THE relaunch decision. Pure, exhaustively matched (no `_` arm), so a new
/// [`RecorderState`] forces a choice here rather than defaulting into "restart
/// on top of a live service".
///
/// The state split is [`quit_action`]'s, minus the refuse/confirm dance: live
/// stops-then-waits, finalising waits, at rest restarts.
pub fn relaunch_plan(state: RecorderState) -> RelaunchPlan {
    match state {
        RecorderState::Preparing | RecorderState::Recording | RecorderState::Reconnecting => {
            RelaunchPlan::StopThenWait
        }
        RecorderState::Stopping => RelaunchPlan::WaitOnly,
        RecorderState::Idle | RecorderState::Stopped | RecorderState::Failed => RelaunchPlan::Now,
    }
}

/// Which of the two quit notifications to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitNotice {
    /// [`QuitAction::Refuse`] — "I did not quit, and here is how to insist."
    Refused,
    /// [`QuitAction::StopThenWait`] / [`QuitAction::WaitOnly`] — "I am quitting,
    /// but not until the file is safe."
    Waiting,
}

/// One observation during the bounded wait for the finalisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitWait {
    /// The file is not safe yet and the cap has not been reached.
    KeepWaiting,
    /// The recorder reached rest — the file is written, quit now.
    Settled,
    /// The cap ran out first. Quit anyway: an app that cannot be quit is worse
    /// than a finalisation nobody is waiting for any more.
    Capped,
}

/// Pure: what one poll of the recorder's state means for a quit that is waiting
/// for the file, given how long it has been waiting and the cap it may not
/// exceed.
///
/// `Settled` is checked FIRST, so a file that lands in the same tick the cap
/// expires is reported as saved rather than as a timeout — the honest reading,
/// and the one the log line has to get right for the next bug report.
pub fn wait_outcome(state: RecorderState, elapsed_ms: u64, cap_ms: u64) -> QuitWait {
    let settled = match state {
        // Terminal: the supervisor is done, the history row is written.
        RecorderState::Stopped | RecorderState::Failed => true,
        // Unreachable in practice (nothing transitions back to `Idle`), but a
        // never-started engine has nothing to wait for either — and the
        // alternative reading would hang the quit for the whole cap.
        RecorderState::Idle => true,
        RecorderState::Preparing
        | RecorderState::Recording
        | RecorderState::Reconnecting
        | RecorderState::Stopping => false,
    };
    if settled {
        QuitWait::Settled
    } else if elapsed_ms >= cap_ms {
        QuitWait::Capped
    } else {
        QuitWait::KeepWaiting
    }
}

/// The localised `(title, body)` of the OS notification a quit fires.
///
/// Same channel and the same deliberate exemption as [`hidden_notice`]: neither
/// notice is gated by the `notifyStart`/`notifyStop` comfort toggles, because
/// both say "what you just did did not do what you expected, and something you
/// cannot afford to lose is at stake".
pub fn quit_notice(notice: QuitNotice, lang: TrayLang) -> (String, String) {
    let (title, body) = match (notice, lang) {
        (QuitNotice::Refused, TrayLang::No) => (
            "SundayRec tar opp",
            "Trykk Avslutt igjen innen {seconds} sekunder for å stoppe opptaket og avslutte.",
        ),
        (QuitNotice::Refused, TrayLang::En) => (
            "SundayRec is recording",
            "Press Quit again within {seconds} seconds to stop the recording and quit.",
        ),
        (QuitNotice::Refused, TrayLang::De) => (
            "SundayRec nimmt auf",
            "Drücke innerhalb von {seconds} Sekunden erneut auf Beenden, um die Aufnahme zu stoppen und das Programm zu schließen.",
        ),
        (QuitNotice::Refused, TrayLang::Sv) => (
            "SundayRec spelar in",
            "Tryck Avsluta igen inom {seconds} sekunder för att stoppa inspelningen och avsluta.",
        ),
        (QuitNotice::Refused, TrayLang::Da) => (
            "SundayRec optager",
            "Tryk Afslut igen inden for {seconds} sekunder for at stoppe optagelsen og afslutte.",
        ),
        (QuitNotice::Refused, TrayLang::Pl) => (
            "SundayRec nagrywa",
            "Naciśnij Zakończ ponownie w ciągu {seconds} sekund, aby zatrzymać nagrywanie i zamknąć aplikację.",
        ),
        (QuitNotice::Refused, TrayLang::Fr) => (
            "SundayRec enregistre",
            "Appuyez de nouveau sur Quitter dans les {seconds} secondes pour arrêter l'enregistrement et quitter.",
        ),
        (QuitNotice::Waiting, TrayLang::No) => (
            "Lagrer opptaket",
            "SundayRec avslutter når fila er trygg. Ett trykk til avslutter med én gang.",
        ),
        (QuitNotice::Waiting, TrayLang::En) => (
            "Saving the recording",
            "SundayRec quits once the file is safe. One more press quits immediately.",
        ),
        (QuitNotice::Waiting, TrayLang::De) => (
            "Aufnahme wird gespeichert",
            "SundayRec beendet sich, sobald die Datei sicher ist. Noch einmal drücken beendet sofort.",
        ),
        (QuitNotice::Waiting, TrayLang::Sv) => (
            "Sparar inspelningen",
            "SundayRec avslutas när filen är trygg. Ett tryck till avslutar direkt.",
        ),
        (QuitNotice::Waiting, TrayLang::Da) => (
            "Gemmer optagelsen",
            "SundayRec afslutter, når filen er sikker. Endnu et tryk afslutter med det samme.",
        ),
        (QuitNotice::Waiting, TrayLang::Pl) => (
            "Zapisywanie nagrania",
            "SundayRec zamknie się, gdy plik będzie bezpieczny. Kolejne naciśnięcie zamyka natychmiast.",
        ),
        (QuitNotice::Waiting, TrayLang::Fr) => (
            "Sauvegarde de l'enregistrement",
            "SundayRec se ferme une fois le fichier en sécurité. Une pression de plus quitte immédiatement.",
        ),
    };
    (
        title.to_string(),
        // The number in the text IS the constant — a hard-coded "10" is exactly
        // how a retuned window and its own explanation drift apart.
        body.replace("{seconds}", &(QUIT_CONFIRM_WINDOW_MS / 1000).to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_capture_hides_the_window_instead_of_quitting() {
        // THE regression this module exists for: every state in which ffmpeg is
        // holding the device must survive the close button. Before this, each of
        // these ended the service's recording.
        for state in [
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
        ] {
            assert_eq!(
                close_action(state),
                CloseAction::Hide(HideReason::Recording),
                "{state:?} must hide, never exit"
            );
        }
    }

    #[test]
    fn finalising_hides_too_because_stopping_is_where_the_file_is_written() {
        // `Stopping` is emitted BEFORE finalize_pending: concat + the delivery
        // transcode + the history row all happen inside it, and a 90-minute
        // service's transcode runs minutes. Exiting here loses a recording that
        // was otherwise complete.
        assert_eq!(
            close_action(RecorderState::Stopping),
            CloseAction::Hide(HideReason::Finishing)
        );
    }

    #[test]
    fn without_a_recording_close_still_means_quit() {
        // No behaviour change outside a session — the volunteer who is done must
        // still be able to quit with the close button.
        for state in [
            RecorderState::Idle,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            assert_eq!(
                close_action(state),
                CloseAction::Exit,
                "{state:?} must close as it always did"
            );
        }
    }

    #[test]
    fn every_recorder_state_is_decided_exactly_once() {
        // A new `RecorderState` variant must force a decision here rather than
        // silently landing in a catch-all that quits mid-service. `close_action`
        // matches exhaustively with no `_` arm, so this is really a guard that
        // nobody adds one; the count is the tripwire if they do.
        let all = [
            RecorderState::Idle,
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
            RecorderState::Stopped,
            RecorderState::Failed,
        ];
        let hides = all
            .iter()
            .filter(|s| close_action(**s) != CloseAction::Exit);
        assert_eq!(
            hides.count(),
            4,
            "live + finalising states hide, the rest exit"
        );
    }

    #[test]
    fn the_hide_notice_is_localised_in_all_seven_languages() {
        let langs = [
            TrayLang::No,
            TrayLang::En,
            TrayLang::De,
            TrayLang::Sv,
            TrayLang::Da,
            TrayLang::Pl,
            TrayLang::Fr,
        ];
        let mut titles = Vec::new();
        for lang in langs {
            for spot in [TraySpot::Menubar, TraySpot::SystemTray] {
                let (title, body) = hidden_notice(HideReason::Recording, lang, spot);
                assert!(!title.is_empty(), "{lang:?} title");
                // The placeholder must always be substituted — a literal
                // "{spot}" on screen is the classic template-leak.
                assert!(
                    !body.contains("{spot}"),
                    "{lang:?}/{spot:?} left the slot in"
                );
                assert!(body.len() > 20, "{lang:?} body looks truncated");
            }
            titles.push(hidden_notice(HideReason::Recording, lang, TraySpot::Menubar).0);
        }
        // Seven distinct titles: a copy-paste that leaves two languages sharing
        // the Norwegian string is the failure this catches.
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), 7);
    }

    #[test]
    fn finalising_does_not_claim_the_service_is_still_being_recorded() {
        let (recording, _) = hidden_notice(HideReason::Recording, TrayLang::No, TraySpot::Menubar);
        let (finishing, _) = hidden_notice(HideReason::Finishing, TrayLang::No, TraySpot::Menubar);
        assert_ne!(recording, finishing);
        assert_eq!(recording, "SundayRec tar fortsatt opp");
        assert_eq!(finishing, "SundayRec lagrer opptaket");
    }

    #[test]
    fn the_notice_names_the_right_place_per_platform() {
        let (_, mac) = hidden_notice(HideReason::Recording, TrayLang::No, TraySpot::Menubar);
        let (_, win) = hidden_notice(HideReason::Recording, TrayLang::No, TraySpot::SystemTray);
        assert!(mac.contains("menylinja"), "{mac}");
        assert!(win.contains("systemstatusfeltet"), "{win}");
        let (_, mac_en) = hidden_notice(HideReason::Recording, TrayLang::En, TraySpot::Menubar);
        assert!(mac_en.contains("menu bar"), "{mac_en}");
    }

    // ─────────────────────────────────────────────────────────────────────
    //   Quit
    // ─────────────────────────────────────────────────────────────────────

    /// Every state, both with and without a fresh refusal standing. This IS the
    /// table from the design — written out so a future edit has to change the
    /// table, not just the code.
    #[test]
    fn the_quit_table_is_exactly_the_policy() {
        use QuitAction::*;
        use RecorderState::*;
        //  state         no prior      stale (11 s)  fresh (1 s)
        // (the fourth column, "too soon to be a decision", is asserted below)
        let table = [
            (Idle, ExitNow, ExitNow, ExitNow),
            (Preparing, Refuse, Refuse, StopThenWait),
            (Recording, Refuse, Refuse, StopThenWait),
            (Reconnecting, Refuse, Refuse, StopThenWait),
            (Stopping, WaitOnly, WaitOnly, WaitOnly),
            (Stopped, ExitNow, ExitNow, ExitNow),
            (Failed, ExitNow, ExitNow, ExitNow),
        ];
        for (state, none, stale, fresh) in table {
            assert_eq!(quit_action(state, None), none, "{state:?} / no prior press");
            assert_eq!(
                quit_action(state, Some(11_000)),
                stale,
                "{state:?} / an 11 s old refusal is NOT a confirmation"
            );
            assert_eq!(
                quit_action(state, Some(1_000)),
                fresh,
                "{state:?} / a 1 s old refusal IS the confirmation"
            );
        }
    }

    #[test]
    fn the_first_quit_during_a_live_capture_is_always_refused() {
        // THE regression this exists for: one keystroke must never end a
        // service. Removing the `Refuse` arm turns this red.
        for state in [
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
        ] {
            assert_eq!(
                quit_action(state, None),
                QuitAction::Refuse,
                "{state:?}: the first press must ask, not act"
            );
        }
    }

    #[test]
    fn a_held_cmd_q_can_never_confirm_itself() {
        // The failure this closes: press-and-hold auto-repeats, the repeats
        // arrive milliseconds apart, and press #2 lands inside the confirmation
        // window — so leaning on the key would stop the service. Below the floor
        // every press is a refusal, forever.
        for age in [0, 1, 15, 120, QUIT_REPEAT_FLOOR_MS - 1] {
            assert_eq!(
                quit_action(RecorderState::Recording, Some(age)),
                QuitAction::Refuse,
                "{age} ms after the last press is a repeat, not an answer"
            );
        }
        // …and the floor is the exact edge: one millisecond later IS an answer.
        assert_eq!(
            quit_action(RecorderState::Recording, Some(QUIT_REPEAT_FLOOR_MS)),
            QuitAction::StopThenWait
        );
        // The two edges must not cross, or there is no window left at all.
        const _: () = assert!(QUIT_REPEAT_FLOOR_MS < QUIT_CONFIRM_WINDOW_MS);
    }

    #[test]
    fn the_confirmation_window_is_exact_at_its_edge() {
        // 9 999 ms still counts, 10 000 does not, 10 001 certainly does not.
        // Setting QUIT_CONFIRM_WINDOW_MS to 0 (or to `<=`) turns this red.
        assert_eq!(
            quit_action(RecorderState::Recording, Some(QUIT_CONFIRM_WINDOW_MS - 1)),
            QuitAction::StopThenWait
        );
        assert_eq!(
            quit_action(RecorderState::Recording, Some(QUIT_CONFIRM_WINDOW_MS)),
            QuitAction::Refuse
        );
        assert_eq!(
            quit_action(RecorderState::Recording, Some(QUIT_CONFIRM_WINDOW_MS + 1)),
            QuitAction::Refuse
        );
        assert_eq!(
            quit_action(RecorderState::Recording, Some(9_999)),
            QuitAction::StopThenWait
        );
        assert_eq!(
            quit_action(RecorderState::Recording, Some(10_001)),
            QuitAction::Refuse
        );
    }

    #[test]
    fn finalising_never_demands_a_second_press() {
        // The volunteer already asked for the stop. Asking them to confirm
        // something they did not request is noise — but the file still has to
        // land before the process dies.
        for age in [None, Some(0), Some(1_000), Some(u64::MAX)] {
            assert_eq!(
                quit_action(RecorderState::Stopping, age),
                QuitAction::WaitOnly,
                "age {age:?}"
            );
        }
    }

    #[test]
    fn quitting_without_a_recording_is_untouched() {
        // No behaviour change for the volunteer who is done: Cmd+Q still quits
        // on the first press, whatever the refusal history says.
        for state in [
            RecorderState::Idle,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            for age in [None, Some(0), Some(500), Some(60_000)] {
                assert_eq!(
                    quit_action(state, age),
                    QuitAction::ExitNow,
                    "{state:?} / {age:?} must quit as it always did"
                );
            }
        }
    }

    #[test]
    fn every_recorder_state_is_decided_exactly_once_by_the_quit_rule() {
        // Same tripwire as `close_action`'s: `quit_action` matches exhaustively
        // with no `_` arm, so a new state cannot silently land on "quit mid
        // service". The counts are what breaks if someone adds one.
        let all = [
            RecorderState::Idle,
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
            RecorderState::Stopped,
            RecorderState::Failed,
        ];
        let refused = all
            .iter()
            .filter(|s| quit_action(**s, None) == QuitAction::Refuse)
            .count();
        let waits = all
            .iter()
            .filter(|s| quit_action(**s, None) == QuitAction::WaitOnly)
            .count();
        let exits = all
            .iter()
            .filter(|s| quit_action(**s, None) == QuitAction::ExitNow)
            .count();
        assert_eq!((refused, waits, exits), (3, 1, 3));
        assert_eq!(refused + waits + exits, all.len());
    }

    #[test]
    fn the_close_rule_and_the_quit_rule_draw_the_same_line() {
        // Two policies, one truth: every state the close button protects is a
        // state the quit must not walk straight through. If these ever disagree,
        // one of the two doors loses a service.
        for state in [
            RecorderState::Idle,
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            let close_protects = close_action(state) != CloseAction::Exit;
            let quit_protects = quit_action(state, None) != QuitAction::ExitNow;
            assert_eq!(close_protects, quit_protects, "{state:?}");
        }
    }

    #[test]
    fn the_quit_notices_are_localised_in_all_seven_languages() {
        let langs = [
            TrayLang::No,
            TrayLang::En,
            TrayLang::De,
            TrayLang::Sv,
            TrayLang::Da,
            TrayLang::Pl,
            TrayLang::Fr,
        ];
        for notice in [QuitNotice::Refused, QuitNotice::Waiting] {
            let mut titles = Vec::new();
            for lang in langs {
                let (title, body) = quit_notice(notice, lang);
                assert!(!title.is_empty(), "{notice:?}/{lang:?} title");
                assert!(body.len() > 20, "{notice:?}/{lang:?} body looks truncated");
                // A literal "{seconds}" on screen is the classic template leak.
                assert!(
                    !body.contains("{seconds}"),
                    "{notice:?}/{lang:?} left the slot in"
                );
                titles.push(title);
            }
            // Seven distinct titles: a copy-paste that leaves two languages
            // sharing the Norwegian string is what this catches.
            titles.sort();
            titles.dedup();
            assert_eq!(titles.len(), 7, "{notice:?}");
        }
    }

    #[test]
    fn the_refusal_says_how_long_the_volunteer_has_and_the_wait_does_not() {
        // The refusal has to name the window — "press again" without "within
        // ten seconds" is an instruction the volunteer cannot follow.
        for lang in [
            TrayLang::No,
            TrayLang::En,
            TrayLang::De,
            TrayLang::Sv,
            TrayLang::Da,
            TrayLang::Pl,
            TrayLang::Fr,
        ] {
            let (_, refused) = quit_notice(QuitNotice::Refused, lang);
            assert!(refused.contains("10"), "{lang:?}: {refused}");
            let (_, waiting) = quit_notice(QuitNotice::Waiting, lang);
            assert!(
                !waiting.contains("10"),
                "{lang:?}: the wait has no deadline to offer: {waiting}"
            );
        }
    }

    #[test]
    fn the_refusal_wording_is_pinned_to_ten_seconds() {
        // The seconds slot is filled from the constant, so the number can never
        // contradict the rule — but the surrounding grammar is not translated
        // per value: Polish "10 sekund" is the genitive plural, and 2–4 s would
        // need "sekundy". Retuning the window means revisiting the seven texts.
        assert_eq!(QUIT_CONFIRM_WINDOW_MS, 10_000);
        assert_eq!(
            QUIT_CONFIRM_WINDOW_MS % 1000,
            0,
            "a fractional second has no wording"
        );
        let (_, pl) = quit_notice(QuitNotice::Refused, TrayLang::Pl);
        assert!(pl.contains("10 sekund"), "{pl}");
        let (_, no) = quit_notice(QuitNotice::Refused, TrayLang::No);
        assert!(no.contains("10 sekunder"), "{no}");
    }

    #[test]
    fn the_refusal_and_the_wait_never_say_the_same_thing() {
        // "I did not quit" and "I am quitting" are opposite messages; a shared
        // string would be the cruellest possible bug here.
        for lang in [
            TrayLang::No,
            TrayLang::En,
            TrayLang::De,
            TrayLang::Sv,
            TrayLang::Da,
            TrayLang::Pl,
            TrayLang::Fr,
        ] {
            assert_ne!(
                quit_notice(QuitNotice::Refused, lang),
                quit_notice(QuitNotice::Waiting, lang),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn the_wait_ends_when_the_recorder_reaches_rest() {
        for state in [
            RecorderState::Stopped,
            RecorderState::Failed,
            RecorderState::Idle,
        ] {
            assert_eq!(
                wait_outcome(state, 0, 1_000),
                QuitWait::Settled,
                "{state:?} has nothing left to write"
            );
        }
    }

    #[test]
    fn the_wait_keeps_waiting_while_the_file_is_still_being_written() {
        for state in [
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
        ] {
            assert_eq!(
                wait_outcome(state, 0, 1_000),
                QuitWait::KeepWaiting,
                "{state:?}"
            );
        }
    }

    #[test]
    fn the_wait_is_capped_so_the_app_can_always_die() {
        // The cap is the promise that a wedged finalise cannot produce an app
        // nobody can quit.
        assert_eq!(
            wait_outcome(RecorderState::Stopping, 1_000, 1_000),
            QuitWait::Capped
        );
        assert_eq!(
            wait_outcome(RecorderState::Stopping, 999, 1_000),
            QuitWait::KeepWaiting
        );
        // …and a file that lands in the very tick the cap expires is reported as
        // saved, not as a timeout.
        assert_eq!(
            wait_outcome(RecorderState::Stopped, u64::MAX, 1_000),
            QuitWait::Settled
        );
    }

    // ── The updater's relaunch (the fourth way out of the process) ───────────

    #[test]
    fn a_live_capture_is_stopped_and_waited_out_before_the_update_restarts() {
        // THE regression: `update::relaunch` used to call `RecorderEngine::stop()`
        // and kill the process on top of it. `stop()` only signals the
        // supervisor, so the concat + delivery transcode + history row died with
        // the process — the pre-guard outcome, reached with one click on
        // «Start på nytt og installer».
        for state in [
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
        ] {
            let plan = relaunch_plan(state);
            assert_eq!(plan, RelaunchPlan::StopThenWait, "{state:?}");
            assert!(plan.stops_the_capture(), "{state:?} must stop gracefully");
            assert!(
                plan.waits_for_the_file(),
                "{state:?} must hold the restart until the file lands"
            );
        }
    }

    #[test]
    fn finalising_waits_for_the_file_without_a_second_stop() {
        // `Stopping` is emitted BEFORE `finalize_pending`, so the transcode of a
        // 90-minute service happens inside it. Nothing left to stop; everything
        // left to lose.
        let plan = relaunch_plan(RecorderState::Stopping);
        assert_eq!(plan, RelaunchPlan::WaitOnly);
        assert!(!plan.stops_the_capture());
        assert!(plan.waits_for_the_file());
    }

    #[test]
    fn at_rest_the_update_restarts_immediately_exactly_as_before() {
        // No behaviour change outside a session: the whole point of an update
        // panel is that pressing restart restarts.
        for state in [
            RecorderState::Idle,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            let plan = relaunch_plan(state);
            assert_eq!(plan, RelaunchPlan::Now, "{state:?}");
            assert!(!plan.stops_the_capture(), "{state:?}");
            assert!(!plan.waits_for_the_file(), "{state:?}");
        }
    }

    #[test]
    fn no_state_that_hides_the_window_lets_the_update_restart_straight_through() {
        // The three doors out of the process must draw the SAME line, or the one
        // that draws it differently is the one that loses the service.
        // `close_action` hides, `quit_action` refuses-or-waits, and the relaunch
        // waits — for exactly the same states.
        for state in [
            RecorderState::Idle,
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            assert_eq!(
                relaunch_plan(state).waits_for_the_file(),
                close_action(state) != CloseAction::Exit,
                "{state:?}: the relaunch and the close button disagree about \
                 whether something is at stake"
            );
            assert_eq!(
                relaunch_plan(state).waits_for_the_file(),
                quit_action(state, None) != QuitAction::ExitNow,
                "{state:?}: the relaunch and the quit disagree"
            );
        }
    }

    #[test]
    fn the_relaunch_never_restarts_before_it_waits() {
        // The sequence, as a rule rather than as a comment: `Now` is the only
        // plan the shell may act on synchronously, and it is the only one that
        // does not wait. Anything else must reach the restart THROUGH the wait —
        // which matters because tauri cannot take a restart back
        // (`prevent_exit` is a no-op for `RESTART_EXIT_CODE`).
        for state in [
            RecorderState::Idle,
            RecorderState::Preparing,
            RecorderState::Recording,
            RecorderState::Reconnecting,
            RecorderState::Stopping,
            RecorderState::Stopped,
            RecorderState::Failed,
        ] {
            let plan = relaunch_plan(state);
            assert_eq!(
                plan == RelaunchPlan::Now,
                !plan.waits_for_the_file(),
                "{state:?}"
            );
            // A stop is never ordered without the wait that collects its file.
            assert!(
                !plan.stops_the_capture() || plan.waits_for_the_file(),
                "{state:?}: stopping without waiting is the old bug"
            );
        }
    }
}
