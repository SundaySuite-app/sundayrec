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
}
