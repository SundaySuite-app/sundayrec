//! Silence-watcher decision state machine.
//!
//! Ported from the Electron `recorder-utils.ts` `createSilenceWatcher`. The
//! Electron version owned real `setTimeout` handles; here we model only the
//! *decisions* — which timers to arm or cancel — so the logic is fully
//! deterministic and unit-testable. The `src-tauri` layer translates the
//! emitted [`SilenceAction`]s into actual tokio timers.
//!
//! Behaviour (mirrors the inline logic shared by the recorders so the unified
//! pipeline reacts to silence identically — the unified path previously had NO
//! silence handling at all, so a muted mixer recorded silently):
//!
//!   • `silence_start` → arm a stop timer (only when stop-on-silence is on, and
//!     only if not already armed) AND a warning timer (always, once per
//!     stretch). The warning fires once per silent stretch.
//!   • `silence_end`   → cancel both timers and re-arm the warning so the next
//!     stretch can warn again.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The two events we extract from ffmpeg's `silencedetect` stderr output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SilenceEvent.ts")]
#[serde(rename_all = "snake_case")]
pub enum SilenceEvent {
    /// A `silence_start` marker — a silent stretch has begun.
    Start,
    /// A `silence_end` marker — the silent stretch has ended.
    End,
}

impl SilenceEvent {
    /// Classify a chunk of ffmpeg stderr. Returns `None` if the chunk contains
    /// neither marker. `silence_end` takes precedence if a chunk somehow
    /// contains both (the stretch has resolved).
    pub fn from_stderr(chunk: &str) -> Option<SilenceEvent> {
        if chunk.contains("silence_end") {
            Some(SilenceEvent::End)
        } else if chunk.contains("silence_start") {
            Some(SilenceEvent::Start)
        } else {
            None
        }
    }
}

/// A timer instruction the host layer should act on. Arming an already-armed
/// timer or cancelling a non-existent one is never emitted — the watcher
/// tracks that itself, so the host can act on these naively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SilenceAction.ts")]
#[serde(rename_all = "snake_case")]
pub enum SilenceAction {
    /// Start the stop-on-silence timer (fires `onStopSilence` after the
    /// configured silence timeout).
    ArmStop,
    /// Start the warning timer (fires `onWarning` once after the warn delay).
    ArmWarn,
    /// Cancel the stop timer (silence ended before it fired).
    CancelStop,
    /// Cancel the warning timer (silence ended before it fired).
    CancelWarn,
}

/// Deterministic silence-watcher. Feed it [`SilenceEvent`]s and it returns the
/// [`SilenceAction`]s the host should perform.
#[derive(Debug, Clone)]
pub struct SilenceWatcher {
    stop_on_silence: bool,
    /// Is the stop timer currently armed?
    stop_armed: bool,
    /// Is the warn timer currently armed?
    warn_armed: bool,
    /// Has the warning already fired during the *current* silent stretch?
    /// Reset on `silence_end` so the next stretch can warn again.
    warn_fired: bool,
}

impl SilenceWatcher {
    /// Create a watcher. `stop_on_silence` gates whether `ArmStop` is ever
    /// emitted; the warning path is always active.
    pub fn new(stop_on_silence: bool) -> Self {
        Self {
            stop_on_silence,
            stop_armed: false,
            warn_armed: false,
            warn_fired: false,
        }
    }

    /// Feed an event; returns the actions to perform (possibly empty).
    pub fn feed(&mut self, event: SilenceEvent) -> Vec<SilenceAction> {
        let mut actions = Vec::new();
        match event {
            SilenceEvent::Start => {
                if self.stop_on_silence && !self.stop_armed {
                    self.stop_armed = true;
                    actions.push(SilenceAction::ArmStop);
                }
                if !self.warn_armed && !self.warn_fired {
                    self.warn_armed = true;
                    actions.push(SilenceAction::ArmWarn);
                }
            }
            SilenceEvent::End => {
                if self.stop_armed {
                    self.stop_armed = false;
                    actions.push(SilenceAction::CancelStop);
                }
                if self.warn_armed {
                    self.warn_armed = false;
                    actions.push(SilenceAction::CancelWarn);
                }
                // Re-arm so the next silent stretch can warn again.
                self.warn_fired = false;
            }
        }
        actions
    }

    /// Mark that the warning timer has fired. The host calls this when its warn
    /// timer elapses; it disarms the timer and latches `warn_fired` so the same
    /// stretch won't warn twice.
    pub fn on_warn_fired(&mut self) {
        self.warn_armed = false;
        self.warn_fired = true;
    }

    /// Mark that the stop timer has fired (disarms it). The host stops the
    /// recording separately.
    pub fn on_stop_fired(&mut self) {
        self.stop_armed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stderr_markers() {
        assert_eq!(
            SilenceEvent::from_stderr("[silencedetect] silence_start: 12.3"),
            Some(SilenceEvent::Start)
        );
        assert_eq!(
            SilenceEvent::from_stderr("[silencedetect] silence_end: 18.0 | silence_duration: 5.7"),
            Some(SilenceEvent::End)
        );
        assert_eq!(SilenceEvent::from_stderr("size=  1024kB time=..."), None);
    }

    #[test]
    fn start_arms_warn_and_stop_when_stop_on() {
        let mut w = SilenceWatcher::new(true);
        let actions = w.feed(SilenceEvent::Start);
        assert_eq!(
            actions,
            vec![SilenceAction::ArmStop, SilenceAction::ArmWarn]
        );
    }

    #[test]
    fn start_arms_only_warn_when_stop_off() {
        let mut w = SilenceWatcher::new(false);
        let actions = w.feed(SilenceEvent::Start);
        assert_eq!(actions, vec![SilenceAction::ArmWarn]);
    }

    #[test]
    fn repeated_start_does_not_rearm() {
        let mut w = SilenceWatcher::new(true);
        let _ = w.feed(SilenceEvent::Start);
        // A second silence_start within the same stretch arms nothing new.
        let actions = w.feed(SilenceEvent::Start);
        assert!(actions.is_empty());
    }

    #[test]
    fn end_cancels_both_and_rearms() {
        let mut w = SilenceWatcher::new(true);
        let _ = w.feed(SilenceEvent::Start);
        let end = w.feed(SilenceEvent::End);
        assert_eq!(
            end,
            vec![SilenceAction::CancelStop, SilenceAction::CancelWarn]
        );
        // Next stretch arms fresh timers again.
        let restart = w.feed(SilenceEvent::Start);
        assert_eq!(
            restart,
            vec![SilenceAction::ArmStop, SilenceAction::ArmWarn]
        );
    }

    #[test]
    fn warn_fires_only_once_per_stretch() {
        let mut w = SilenceWatcher::new(false);
        let first = w.feed(SilenceEvent::Start);
        assert_eq!(first, vec![SilenceAction::ArmWarn]);
        // Host's warn timer elapses.
        w.on_warn_fired();
        // Another silence_start in the SAME stretch must not re-arm the warn.
        let again = w.feed(SilenceEvent::Start);
        assert!(again.is_empty());
    }

    #[test]
    fn warn_rearms_after_end_even_if_already_fired() {
        let mut w = SilenceWatcher::new(false);
        let _ = w.feed(SilenceEvent::Start);
        w.on_warn_fired();
        // Stretch ends — nothing armed to cancel, but warn_fired resets.
        let end = w.feed(SilenceEvent::End);
        assert!(end.is_empty());
        // New stretch can warn again.
        let next = w.feed(SilenceEvent::Start);
        assert_eq!(next, vec![SilenceAction::ArmWarn]);
    }

    #[test]
    fn end_with_nothing_armed_is_noop() {
        let mut w = SilenceWatcher::new(true);
        assert!(w.feed(SilenceEvent::End).is_empty());
    }

    #[test]
    fn stop_fired_disarms_stop_so_end_does_not_cancel_it() {
        let mut w = SilenceWatcher::new(true);
        let _ = w.feed(SilenceEvent::Start);
        w.on_stop_fired();
        // Stop already fired; end only cancels the warn timer.
        let end = w.feed(SilenceEvent::End);
        assert_eq!(end, vec![SilenceAction::CancelWarn]);
    }
}
