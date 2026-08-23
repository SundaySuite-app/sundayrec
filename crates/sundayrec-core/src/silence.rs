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
#[ts(export, export_to = "SilenceEvent.ts")]
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
#[ts(export, export_to = "SilenceAction.ts")]
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

/// The detection threshold in dBFS, mirroring
/// [`crate::ffmpeg::build_silence_detect_filter`] exactly: the user's clamped
/// value when stop-on-silence is on, else a fixed permissive −55 dB — detection
/// always runs because the warning path always needs markers.
pub fn silence_threshold_db(stop_on_silence: bool, silence_threshold_db: Option<i32>) -> f32 {
    if stop_on_silence {
        silence_threshold_db.unwrap_or(-50).clamp(-70, -10) as f32
    } else {
        -55.0
    }
}

/// How long a stretch must stay below threshold before `Start` is emitted —
/// parity with the ffmpeg filter's `duration=1` (debounces inter-sentence gaps).
pub const SILENCE_DEBOUNCE_SECS: f64 = 1.0;

/// In-process replacement for ffmpeg's `silencedetect` filter: the native
/// capture writer feeds it one (block peak, frame count) pair per drained
/// block, and it emits the same debounced [`SilenceEvent`]s the stderr parser
/// used to — so the downstream [`SilenceWatcher`] and host timers are
/// unchanged. Deterministic, no clocks: time is counted in frames.
#[derive(Debug, Clone)]
pub struct SilenceDetector {
    threshold_linear: f32,
    debounce_frames: u64,
    silent_frames: u64,
    /// Has `Start` been emitted for the current silent stretch?
    in_silence: bool,
}

impl SilenceDetector {
    /// `threshold_db` from [`silence_threshold_db`]; `sample_rate` in Hz (the
    /// session's pinned rate — frames are counted at this rate).
    pub fn new(threshold_db: f32, sample_rate: u32) -> Self {
        Self {
            threshold_linear: 10.0_f32.powf(threshold_db / 20.0),
            debounce_frames: ((sample_rate.max(1) as f64) * SILENCE_DEBOUNCE_SECS) as u64,
            silent_frames: 0,
            in_silence: false,
        }
    }

    /// Feed one block: its max |sample| (linear) and its length in frames.
    /// Returns `Start` once a stretch has stayed silent for the debounce
    /// window, `End` on the first loud block after a `Start`, else `None`.
    /// A sub-debounce dip just resets the counter — no event (that's the
    /// hysteresis `duration=1` provides in ffmpeg).
    pub fn feed(&mut self, block_peak_linear: f32, frames: u64) -> Option<SilenceEvent> {
        // A NaN peak counts as silent (defensive: never lets a broken block
        // suppress a legitimate silence alarm).
        let silent = !block_peak_linear.is_finite() || block_peak_linear < self.threshold_linear;
        if silent {
            self.silent_frames = self.silent_frames.saturating_add(frames);
            if !self.in_silence && self.silent_frames >= self.debounce_frames {
                self.in_silence = true;
                return Some(SilenceEvent::Start);
            }
            None
        } else {
            self.silent_frames = 0;
            if self.in_silence {
                self.in_silence = false;
                return Some(SilenceEvent::End);
            }
            None
        }
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

    // ── SilenceDetector (in-process silencedetect parity) ────────────────────

    #[test]
    fn threshold_rules_mirror_ffmpeg_filter_builder() {
        // Mirrors the table in ffmpeg.rs: default, clamp both ends, off → −55.
        assert_eq!(silence_threshold_db(true, None), -50.0);
        assert_eq!(silence_threshold_db(true, Some(-60)), -60.0);
        assert_eq!(silence_threshold_db(true, Some(-100)), -70.0);
        assert_eq!(silence_threshold_db(true, Some(0)), -10.0);
        assert_eq!(silence_threshold_db(false, Some(-30)), -55.0);
        assert_eq!(silence_threshold_db(false, None), -55.0);
    }

    /// 100-frame blocks at rate 1000 → debounce is exactly 10 blocks.
    fn det() -> SilenceDetector {
        SilenceDetector::new(-50.0, 1000)
    }
    const QUIET: f32 = 0.001; // −60 dBFS, below −50
    const LOUD: f32 = 0.1; // −20 dBFS, above −50

    #[test]
    fn start_emitted_exactly_at_debounce_boundary() {
        let mut d = det();
        for _ in 0..9 {
            assert_eq!(d.feed(QUIET, 100), None);
        }
        // The 10th silent block completes 1.0 s → Start.
        assert_eq!(d.feed(QUIET, 100), Some(SilenceEvent::Start));
        // Continued silence emits nothing further.
        assert_eq!(d.feed(QUIET, 100), None);
    }

    #[test]
    fn sub_debounce_dip_emits_nothing() {
        let mut d = det();
        for _ in 0..9 {
            assert_eq!(d.feed(QUIET, 100), None);
        }
        // A loud block before the boundary resets the counter — no event.
        assert_eq!(d.feed(LOUD, 100), None);
        for _ in 0..9 {
            assert_eq!(d.feed(QUIET, 100), None);
        }
        assert_eq!(d.feed(QUIET, 100), Some(SilenceEvent::Start));
    }

    #[test]
    fn end_on_first_loud_block_after_start() {
        let mut d = det();
        for _ in 0..10 {
            d.feed(QUIET, 100);
        }
        assert_eq!(d.feed(LOUD, 100), Some(SilenceEvent::End));
        // And the next stretch debounces afresh.
        for _ in 0..9 {
            assert_eq!(d.feed(QUIET, 100), None);
        }
        assert_eq!(d.feed(QUIET, 100), Some(SilenceEvent::Start));
    }

    #[test]
    fn exactly_at_threshold_counts_as_loud() {
        // silencedetect triggers on strictly-below-noise; at-threshold is loud.
        let mut d = SilenceDetector::new(-20.0, 1000);
        let at = 10.0_f32.powf(-20.0 / 20.0);
        for _ in 0..20 {
            assert_eq!(d.feed(at, 100), None);
        }
    }

    #[test]
    fn nan_peak_counts_as_silent_and_never_panics() {
        let mut d = det();
        for _ in 0..10 {
            d.feed(f32::NAN, 100);
        }
        // NaN blocks accumulated as silence → Start already emitted at block 10.
        assert_eq!(d.feed(LOUD, 100), Some(SilenceEvent::End));
    }

    #[test]
    fn zero_sample_rate_is_defensive_not_divide_by_zero() {
        let mut d = SilenceDetector::new(-50.0, 0);
        // rate clamps to 1 → debounce 1 frame; first silent block starts.
        assert_eq!(d.feed(QUIET, 1), Some(SilenceEvent::Start));
    }
}
