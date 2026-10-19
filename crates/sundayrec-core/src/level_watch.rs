//! D3 — «er mikseren dempet?»: the pure state machine behind the weak-level
//! warning during a take.
//!
//! The native engine measures the short-term loudness (BS.1770, 3 s window) of
//! the routed f32 stream on the writer thread. This module decides, from those
//! readings and an INJECTED clock, whether the take has been so quiet for so
//! long that the operator should hear about it:
//!
//!   below [`WEAK_LUFS`] (−40 LUFS) for MORE than [`WEAK_AFTER`] (5 min).
//!
//! No clock, no thread, no sleeping in here: the caller passes `at`, the time
//! since the take began (any monotonic origin), with every reading. The tests
//! run five «minutes» in microseconds.
//!
//! ## What counts as weak
//!
//! - A reading below −40 LUFS is weak. That includes digital silence: the
//!   meter reports −∞ there and anything under its own −70 LUFS absolute gate
//!   is nothing but a quieter version of the same fact. [`clamp_lufs`] maps
//!   −∞ to a finite [`SILENCE_FLOOR_LUFS`] so the value survives JSON (which
//!   cannot carry infinity) and the overlay can print it.
//! - `NaN` and «no reading yet» are NOT readings: they neither start nor reset
//!   the clock. (The first 3 s of a take have no short-term value, and a
//!   reconnect gap has none either.)
//! - Any reading at or above −40 LUFS resets the clock: the signal is back, so
//!   the five minutes start over. A prayer or a minute of silence inside a
//!   normal service never gets near the line.
//!
//! ## Silence wins
//!
//! A take that is silent for over five minutes also trips the silence warning
//! («Stillhet oppdaget»). Telling the operator the same fact twice is noise, so
//! once the silence warning has been given in this take the weak-level warning
//! is held back for the rest of it ([`WeakLevelWatch::note_silence_alerted`]):
//!
//! - silent > 5 min  ⇒ only the silence warning;
//! - weak but not silent > 5 min ⇒ only the weak-level warning;
//! - weak warning first, silence later ⇒ both were said, in that order (the
//!   weak one was already true and useful when it was given; the silence
//!   warning is a different, stronger fact and is never suppressed);
//! - silence warning first, weak later in the same take ⇒ no weak warning,
//!   even if the sound came back and then went weak again: the silence warning
//!   already told the operator to look at the mixer.
//!
//! ## Once per take
//!
//! The warning is due ONCE. [`WeakLevelWatch::observe`] keeps answering `true`
//! until the caller says it was raised ([`WeakLevelWatch::mark_alerted`]); after
//! that it stays quiet for the rest of the take, however the level moves, until
//! [`WeakLevelWatch::reset`] (the next take).

use std::time::Duration;

/// The short-term loudness below which a take counts as «very weak», in LUFS.
/// A spoken sermon through a healthy mixer lands around −23…−16; −40 is a
/// microphone that is there but turned almost off.
pub const WEAK_LUFS: f64 = -40.0;

/// How long the level must stay under [`WEAK_LUFS`] before the warning is due.
/// «Mer enn 5 minutter»: exactly five is not enough.
pub const WEAK_AFTER: Duration = Duration::from_secs(5 * 60);

/// What digital silence (and anything below it) is reported as, so a loudness
/// value is always a finite number on the wire.
pub const SILENCE_FLOOR_LUFS: f64 = -120.0;

/// A reading made safe to store and send: `NaN` is no reading (`None`), −∞ and
/// anything under the floor become [`SILENCE_FLOOR_LUFS`], and +∞ is not a
/// level a meter can produce (`None`).
pub fn clamp_lufs(lufs: f64) -> Option<f64> {
    if lufs.is_nan() || lufs == f64::INFINITY {
        None
    } else {
        Some(lufs.max(SILENCE_FLOOR_LUFS))
    }
}

/// Whether `lufs` is below the weak line. −∞ (digital silence) is weak; so is
/// everything under the meter's −70 LUFS gate.
pub fn is_weak(lufs: f64) -> bool {
    lufs < WEAK_LUFS
}

/// The weak-level decision for one take.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WeakLevelWatch {
    /// `at` of the first weak reading of the current weak stretch.
    weak_since: Option<Duration>,
    /// The warning for this take was raised; say nothing more.
    alerted: bool,
    /// The silence warning was given in this take: hold this one back.
    silence_told: bool,
}

impl WeakLevelWatch {
    /// A watch with nothing seen (usable in a `static`).
    pub const fn new() -> Self {
        Self {
            weak_since: None,
            alerted: false,
            silence_told: false,
        }
    }

    /// A new take: forget the stretch and re-arm.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One short-term loudness reading, `at` into the take. `true` while the
    /// warning is due and has not been raised for this take.
    pub fn observe(&mut self, at: Duration, lufs: Option<f64>) -> bool {
        let Some(lufs) = lufs.filter(|l| !l.is_nan()) else {
            // No reading: neither start nor reset anything.
            return self.due(at);
        };
        if is_weak(lufs) {
            // `at` going backwards (a clock hiccup) must not make the stretch
            // longer than it was: the start only moves forward from here.
            self.weak_since.get_or_insert(at);
        } else {
            self.weak_since = None;
        }
        self.due(at)
    }

    fn due(&self, at: Duration) -> bool {
        match self.weak_since {
            Some(since) if !self.alerted && !self.silence_told => {
                at.saturating_sub(since) > WEAK_AFTER
            }
            _ => false,
        }
    }

    /// The silence warning has been given: this take will not also say «svakt».
    pub fn note_silence_alerted(&mut self) {
        self.silence_told = true;
    }

    /// The warning was raised; do not say it again this take.
    pub fn mark_alerted(&mut self) {
        self.alerted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    /// Feed one reading per second from `from` to `to` (inclusive); the number
    /// of times it said «due».
    fn feed(w: &mut WeakLevelWatch, from: u64, to: u64, lufs: f64) -> Vec<u64> {
        (from..=to)
            .filter(|&t| w.observe(s(t), Some(lufs)))
            .collect()
    }

    #[test]
    fn a_muted_mixer_for_over_five_minutes_warns_exactly_once() {
        let mut w = WeakLevelWatch::new();
        // First reading at 3 s (the meter's first short-term value).
        let mut due_at = vec![];
        for t in 3..=1200u64 {
            if w.observe(s(t), Some(-52.0)) {
                due_at.push(t);
                // The caller raises it and says so.
                w.mark_alerted();
            }
        }
        // Weak since 3 s → due strictly after 3 + 300 s.
        assert_eq!(due_at, vec![304], "once, one second past five minutes");
    }

    #[test]
    fn exactly_five_minutes_is_not_more_than_five_minutes() {
        let mut w = WeakLevelWatch::new();
        assert!(!w.observe(s(0), Some(-50.0)));
        assert!(!w.observe(s(300), Some(-50.0)), "5:00 exactly: not yet");
        assert!(w.observe(s(301), Some(-50.0)), "5:01: due");
    }

    #[test]
    fn it_stays_due_until_raised_and_quiet_after() {
        let mut w = WeakLevelWatch::new();
        w.observe(s(0), Some(-60.0));
        assert!(w.observe(s(400), Some(-60.0)));
        assert!(w.observe(s(401), Some(-60.0)), "not raised yet: still due");
        w.mark_alerted();
        assert!(!w.observe(s(402), Some(-60.0)));
        // Level recovers, then drops again for another long stretch: the take
        // has already been told. Once per take.
        assert!(!w.observe(s(500), Some(-20.0)));
        assert!(feed(&mut w, 501, 1500, -60.0).is_empty());
    }

    #[test]
    fn a_normal_level_never_warns() {
        let mut w = WeakLevelWatch::new();
        // A sermon at −23 LUFS for two hours.
        assert!(feed(&mut w, 3, 7200, -23.0).is_empty());
        // Right at the line is not under it.
        let mut w = WeakLevelWatch::new();
        assert!(feed(&mut w, 0, 3600, WEAK_LUFS).is_empty());
    }

    #[test]
    fn short_quiet_stretches_do_not_warn() {
        // Prayer, silence, a pause for the offering: under five minutes each,
        // with speech or music in between.
        let mut w = WeakLevelWatch::new();
        let mut t = 3;
        for _ in 0..10 {
            // 4 min 50 s of quiet…
            assert!(feed(&mut w, t, t + 289, -55.0).is_empty());
            t += 290;
            // …then 10 s of a normal level resets the stretch.
            assert!(feed(&mut w, t, t + 9, -22.0).is_empty());
            t += 10;
        }
    }

    #[test]
    fn digital_silence_counts_as_weak() {
        let mut w = WeakLevelWatch::new();
        // −∞ straight from the meter.
        assert!(!w.observe(s(0), Some(f64::NEG_INFINITY)));
        assert!(w.observe(s(301), Some(f64::NEG_INFINITY)));
        // …and the clamped, wire-safe form is just as weak.
        let wire = clamp_lufs(f64::NEG_INFINITY).unwrap();
        assert_eq!(wire, SILENCE_FLOOR_LUFS);
        assert!(is_weak(wire));
        // Below the −70 LUFS absolute gate is weak, too.
        assert!(is_weak(-71.0));
    }

    #[test]
    fn no_reading_neither_starts_nor_resets_the_clock() {
        let mut w = WeakLevelWatch::new();
        assert!(!w.observe(s(0), None), "nothing measured yet");
        assert!(!w.observe(s(100), Some(f64::NAN)), "NaN is not a reading");
        w.observe(s(200), Some(-50.0));
        // A reconnect gap with no readings does not reset the stretch…
        assert!(!w.observe(s(300), None));
        // …so five minutes after the first weak reading it is due.
        assert!(w.observe(s(501), None));
    }

    /// Seconds at which the warning was due, raising it (as the caller does)
    /// the first time.
    fn raised(w: &mut WeakLevelWatch, from: u64, to: u64, lufs: f64) -> Vec<u64> {
        let mut v = vec![];
        for t in from..=to {
            if w.observe(s(t), Some(lufs)) {
                v.push(t);
                w.mark_alerted();
            }
        }
        v
    }

    #[test]
    fn silent_over_five_minutes_gives_only_the_silence_warning() {
        let mut w = WeakLevelWatch::new();
        // Digital silence from the start. The silence warning fires first (its
        // own timer is shorter) and the engine tells us.
        assert!(!w.observe(s(3), Some(f64::NEG_INFINITY)));
        w.note_silence_alerted();
        assert!(feed(&mut w, 4, 1800, f64::NEG_INFINITY).is_empty());
    }

    #[test]
    fn weak_but_not_silent_gives_the_weak_warning() {
        let mut w = WeakLevelWatch::new();
        // No silence warning (the signal is quiet, not gone).
        assert_eq!(raised(&mut w, 3, 1800, -55.0), vec![304]);
    }

    #[test]
    fn weak_first_then_silence_says_both_in_that_order() {
        let mut w = WeakLevelWatch::new();
        assert_eq!(raised(&mut w, 3, 400, -55.0), vec![304]);
        // Later the mixer is cut completely: the silence warning is the
        // engine's, unaffected; the weak one stays quiet (already said).
        w.note_silence_alerted();
        assert!(feed(&mut w, 401, 1800, f64::NEG_INFINITY).is_empty());
    }

    #[test]
    fn silence_told_holds_weak_back_for_the_rest_of_the_take_and_reset_frees_it() {
        let mut w = WeakLevelWatch::new();
        w.note_silence_alerted();
        assert!(feed(&mut w, 0, 100, -20.0).is_empty());
        assert!(
            feed(&mut w, 101, 1000, -55.0).is_empty(),
            "silence came first"
        );
        w.reset();
        assert_eq!(raised(&mut w, 0, 400, -55.0), vec![301]);
    }

    #[test]
    fn a_new_take_rearms_the_watch() {
        let mut w = WeakLevelWatch::new();
        w.observe(s(0), Some(-50.0));
        assert!(w.observe(s(301), Some(-50.0)));
        w.mark_alerted();
        w.reset();
        assert!(!w.observe(s(0), Some(-50.0)));
        assert!(
            w.observe(s(301), Some(-50.0)),
            "told again in the next take"
        );
    }

    #[test]
    fn a_clock_that_steps_back_does_not_extend_or_fake_a_stretch() {
        let mut w = WeakLevelWatch::new();
        w.observe(s(100), Some(-50.0));
        assert!(
            !w.observe(s(50), Some(-50.0)),
            "earlier than the start: saturates to 0"
        );
        assert!(w.observe(s(401), Some(-50.0)));
    }

    #[test]
    fn clamp_makes_every_reading_wire_safe() {
        assert_eq!(clamp_lufs(-23.4), Some(-23.4));
        assert_eq!(clamp_lufs(f64::NEG_INFINITY), Some(SILENCE_FLOOR_LUFS));
        assert_eq!(clamp_lufs(-500.0), Some(SILENCE_FLOOR_LUFS));
        assert_eq!(clamp_lufs(f64::NAN), None);
        assert_eq!(clamp_lufs(f64::INFINITY), None);
    }
}
