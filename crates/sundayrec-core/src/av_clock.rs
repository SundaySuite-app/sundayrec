//! The audio clock (C2, docs/VIDEOMOTOR.md §C2): which audio sample a host-clock
//! instant belongs to.
//!
//! **The audio clock is the boss.** A recording's timeline is the WAV the
//! native audio engine writes, counted in samples. Camera frames arrive stamped
//! on the *host* clock ([`camera::Frame::host_time_ns`] in `src-tauri`:
//! `mach_absolute_time` on macOS, `QueryPerformanceCounter` on Windows). The
//! sound card's crystal and the host clock drift 20–50 ppm apart — 50 ppm is
//! 0.27 s over 90 minutes, which is lip-sync visibly gone — so a frame's place
//! in the recording cannot be "host time × nominal rate". It is read off a line
//! fitted to what the audio engine actually reports.
//!
//! # Input
//!
//! [`AudioClock::observe`] takes pairs of *(sample index, host time in ns)* —
//! one per audio callback: the index of a sample in the recording, and the host
//! time at which **that sample** was captured (cpal's capture timestamp on the
//! host clock, not `Instant::now()` inside the callback: the callback runs a
//! buffer later and with scheduler jitter). Sample indices must increase; a
//! pair that does not is ignored and counted.
//!
//! # The fit
//!
//! Host time is fitted as a straight line in sample index (ordinary least
//! squares — the noise is in the timestamps, the sample count is exact) over a
//! sliding window of the last [`WINDOW_NS`] of host time. The sums are kept
//! incrementally in `i128` relative to the segment's first pair, so they are
//! exact: three hours of callbacks cannot accumulate rounding, and an update is
//! O(1). Until the window spans [`MIN_FIT_SPAN_NS`] the slope is the nominal one
//! (`1e9 / rate` ns per sample) and only the offset is fitted; after that the
//! fitted slope is clamped to nominal ±[`MAX_DRIFT_PPM`], far outside any real
//! crystal but tight enough that a short, noisy window cannot tilt the line.
//!
//! # Jumps: the machine slept
//!
//! When the host clock jumps against the samples — Windows' QPC keeps counting
//! through sleep while no samples are captured, or the audio engine lost a
//! buffer — the old line is wrong from then on. A pair whose host time is more
//! than [`JUMP_THRESHOLD_NS`] off the line becomes a *suspect*. When
//! [`CONFIRM_POINTS`] suspects in a row agree with each other, the jump is real:
//! the fit restarts as a new **segment** seeded with them. A suspect followed by
//! a pair back on the line was a single bad timestamp and is dropped
//! ([`AudioClock::outliers`]). While suspects are pending the clock does not
//! advance ([`AudioClock::newest_sample`] stands still), so nothing downstream
//! decides on a line that may be wrong.
//!
//! The threshold is also the error bound for jumps too small to detect: a step
//! below it is absorbed by the window, and a least-squares line through a step
//! of height `s` is never more than about `s` away from the newer side. At
//! 20 ms that stays inside one 30 fps frame (33 ms), which is the promise C2
//! makes. (macOS's `mach_absolute_time` stops while the machine sleeps, so on a
//! Mac a sleep is usually no jump at all; on Windows it is one.)
//!
//! The last [`MAX_SEGMENTS`] segments are kept: a frame captured just before a
//! jump, delivered after it, is still placed by the line that was true when it
//! was captured ([`AudioClock::sample_at`]).
//!
//! No I/O and no clock here: numbers in, numbers out. The conformer that uses
//! this is [`crate::video_conform`].

use std::collections::VecDeque;

/// How much host time the fit looks back over. Long enough that callback jitter
/// averages out (≈ 700 callbacks at 2048 samples / 48 kHz), short enough that a
/// crystal warming up (drift that changes over minutes) is followed.
pub const WINDOW_NS: u64 = 30_000_000_000;

/// Below this window span the slope is not fitted (nominal slope, fitted
/// offset): a slope from a fraction of a second of jittery timestamps is noise.
pub const MIN_FIT_SPAN_NS: u64 = 2_000_000_000;

/// The fitted slope is clamped to nominal ± this. Real sound cards are within
/// ±100 ppm; this only stops a degenerate window from producing nonsense.
pub const MAX_DRIFT_PPM: f64 = 1_000.0;

/// A pair this far off the line is a suspect — a possible jump. Also the bound
/// on the error a jump too small to detect can cause (see the module header).
pub const JUMP_THRESHOLD_NS: i64 = 20_000_000;

/// How many suspects in a row, agreeing with each other, make a jump real.
pub const CONFIRM_POINTS: usize = 3;

/// Upper bound on pairs in one window (a 1 ms callback period over 30 s is
/// 30 000; this caps memory, not accuracy).
pub const MAX_WINDOW_POINTS: usize = 32_768;

/// How many segments (fits before jumps) are kept for placing late frames.
pub const MAX_SEGMENTS: usize = 4;

/// What [`AudioClock::observe`] did with a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// On the line (or the first pair): taken into the fit.
    Accepted,
    /// Off the line; held until it is known whether the host clock jumped.
    Suspect,
    /// The suspects agreed: the host clock jumped and the fit restarted.
    Reset,
    /// Sample index did not increase; ignored.
    Ignored,
}

/// One fit: everything between two jumps.
#[derive(Clone, Debug)]
struct Segment {
    /// The first pair of the segment; all sums are relative to it.
    sample0: u64,
    host0: u64,
    /// `(sample − sample0, host − host0)` of the pairs in the window.
    points: VecDeque<(i64, i64)>,
    sx: i128,
    sy: i128,
    sxx: i128,
    sxy: i128,
    /// First and newest sample index ever accepted into this segment.
    first_sample: u64,
    last_sample: u64,
    /// The fitted line, `host_rel = mean_y + (sample_rel − mean_x) · slope`.
    mean_x: f64,
    mean_y: f64,
    slope: f64,
}

impl Segment {
    fn new(sample: u64, host: u64, nominal: f64) -> Self {
        let mut s = Segment {
            sample0: sample,
            host0: host,
            points: VecDeque::new(),
            sx: 0,
            sy: 0,
            sxx: 0,
            sxy: 0,
            first_sample: sample,
            last_sample: sample,
            mean_x: 0.0,
            mean_y: 0.0,
            slope: nominal,
        };
        s.push(sample, host, nominal);
        s
    }

    fn push(&mut self, sample: u64, host: u64, nominal: f64) {
        let x = sample.wrapping_sub(self.sample0) as i64;
        let y = host.wrapping_sub(self.host0) as i64;
        self.points.push_back((x, y));
        self.add(x, y, 1);
        self.last_sample = sample;
        // Slide the window: drop pairs older than WINDOW_NS behind the newest.
        while let Some(&(ox, oy)) = self.points.front() {
            let too_old = (y as i128 - oy as i128) > WINDOW_NS as i128;
            if too_old || self.points.len() > MAX_WINDOW_POINTS {
                self.points.pop_front();
                self.add(ox, oy, -1);
            } else {
                break;
            }
        }
        self.refit(nominal);
    }

    fn add(&mut self, x: i64, y: i64, sign: i128) {
        let (x, y) = (x as i128, y as i128);
        self.sx += sign * x;
        self.sy += sign * y;
        self.sxx += sign * x * x;
        self.sxy += sign * x * y;
    }

    fn refit(&mut self, nominal: f64) {
        let n = self.points.len() as i128;
        self.mean_x = self.sx as f64 / n as f64;
        self.mean_y = self.sy as f64 / n as f64;
        let span_ns = match (self.points.front(), self.points.back()) {
            (Some(a), Some(b)) => (b.1 as i128 - a.1 as i128).max(0),
            _ => 0,
        };
        // n·Sxx − Sx² and n·Sxy − Sx·Sy are exact in i128 for any recording
        // shorter than centuries; only the final ratio is floating point.
        let dxx = n * self.sxx - self.sx * self.sx;
        let dxy = n * self.sxy - self.sx * self.sy;
        self.slope = if span_ns < MIN_FIT_SPAN_NS as i128 || dxx <= 0 {
            nominal
        } else {
            let lim = nominal * MAX_DRIFT_PPM * 1e-6;
            (dxy as f64 / dxx as f64).clamp(nominal - lim, nominal + lim)
        };
    }

    /// Host time (absolute ns, as f64) the line gives for `sample`.
    fn host_at(&self, sample: u64) -> f64 {
        let x = sample as f64 - self.sample0 as f64;
        self.host0 as f64 + self.mean_y + (x - self.mean_x) * self.slope
    }

    /// Sample position (fractional) the line gives for `host`.
    fn sample_at(&self, host: u64) -> f64 {
        let y = host as f64 - self.host0 as f64;
        self.sample0 as f64 + self.mean_x + (y - self.mean_y) / self.slope
    }
}

/// Host clock → audio sample position. See the module header.
#[derive(Clone, Debug)]
pub struct AudioClock {
    nominal: f64,
    /// Oldest first; the last one is current.
    segments: VecDeque<Segment>,
    suspects: Vec<(u64, u64)>,
    newest_host: Option<u64>,
    resets: u32,
    outliers: u64,
    ignored: u64,
}

impl AudioClock {
    /// A clock for audio at `sample_rate` Hz. Panics on 0 — a recording always
    /// has a rate.
    pub fn new(sample_rate: u32) -> Self {
        assert!(sample_rate > 0, "sample rate must be positive");
        AudioClock {
            nominal: 1e9 / sample_rate as f64,
            segments: VecDeque::new(),
            suspects: Vec::new(),
            newest_host: None,
            resets: 0,
            outliers: 0,
            ignored: 0,
        }
    }

    /// Feed one *(sample index, host time of that sample)* pair.
    pub fn observe(&mut self, sample_index: u64, host_ns: u64) -> Observed {
        let nominal = self.nominal;
        let newest_seen = self
            .suspects
            .last()
            .map(|s| s.0)
            .or(self.segments.back().map(|s| s.last_sample));
        if newest_seen.is_some_and(|n| sample_index <= n) {
            self.ignored += 1;
            return Observed::Ignored;
        }
        let Some(seg) = self.segments.back_mut() else {
            self.segments
                .push_back(Segment::new(sample_index, host_ns, nominal));
            self.newest_host = Some(host_ns);
            return Observed::Accepted;
        };
        let residual = host_ns as f64 - seg.host_at(sample_index);
        if residual.abs() <= JUMP_THRESHOLD_NS as f64 {
            // Back on the line: any pending suspects were bad timestamps.
            self.outliers += self.suspects.len() as u64;
            self.suspects.clear();
            seg.push(sample_index, host_ns, nominal);
            self.newest_host = Some(self.newest_host.map_or(host_ns, |h| h.max(host_ns)));
            return Observed::Accepted;
        }
        // Off the line. Does it agree with the suspects already held?
        let agrees = self.suspects.first().is_none_or(|&(s0, h0)| {
            let r0 = h0 as f64 - seg.host_at(s0);
            (residual - r0).abs() <= JUMP_THRESHOLD_NS as f64 / 2.0
        });
        if !agrees {
            self.outliers += self.suspects.len() as u64;
            self.suspects.clear();
        }
        self.suspects.push((sample_index, host_ns));
        if self.suspects.len() < CONFIRM_POINTS {
            return Observed::Suspect;
        }
        // Confirmed: the host clock jumped. New segment from the suspects.
        let mut pts = std::mem::take(&mut self.suspects).into_iter();
        let (s0, h0) = pts.next().expect("CONFIRM_POINTS > 0");
        let mut fresh = Segment::new(s0, h0, nominal);
        for (s, h) in pts {
            fresh.push(s, h, nominal);
        }
        self.segments.push_back(fresh);
        while self.segments.len() > MAX_SEGMENTS {
            self.segments.pop_front();
        }
        self.newest_host = Some(host_ns);
        self.resets += 1;
        Observed::Reset
    }

    /// The (fractional) audio sample position of a host-clock instant, or `None`
    /// before the first pair.
    ///
    /// With several segments (jumps), the newest segment whose line puts `host_ns`
    /// inside that segment's stretch of samples wins: from its first sample to
    /// the first sample of the segment after it. (The jump is taken to sit on
    /// the first sample after it — the buffer before it was captured whole,
    /// before the jump.) A frame captured just before a jump but placed after it
    /// therefore still uses the old line. If no segment claims it, the one whose
    /// answer misses its stretch by the least.
    pub fn sample_at(&self, host_ns: u64) -> Option<f64> {
        let n = self.segments.len();
        let mut best: Option<(f64, f64)> = None; // (miss, position)
        for i in (0..n).rev() {
            let seg = &self.segments[i];
            let pos = seg.sample_at(host_ns);
            // The oldest kept segment also owns everything before it (a camera
            // that started before the audio).
            let lo = if i > 0 {
                seg.first_sample as f64
            } else {
                f64::NEG_INFINITY
            };
            let hi = if i + 1 < n {
                self.segments[i + 1].first_sample as f64
            } else {
                f64::INFINITY
            };
            let miss = (lo - pos).max(pos - hi).max(0.0);
            if miss == 0.0 {
                return Some(pos);
            }
            if best.is_none_or(|(m, _)| miss < m) {
                best = Some((miss, pos));
            }
        }
        best.map(|(_, p)| p)
    }

    /// The newest sample index the fit has accepted — how far the recording is
    /// known to have come. Stands still while a possible jump is being
    /// confirmed.
    pub fn newest_sample(&self) -> Option<u64> {
        self.segments.back().map(|s| s.last_sample)
    }

    /// The newest host time accepted into the fit. A frame stamped later than
    /// this cannot be placed safely yet: the host clock may have jumped.
    pub fn newest_host(&self) -> Option<u64> {
        self.newest_host
    }

    /// The current fit's drift of the sound card against the host clock, in
    /// ppm (positive: the host clock counts more ns per sample than nominal).
    /// For diagnostics.
    pub fn drift_ppm(&self) -> Option<f64> {
        self.segments
            .back()
            .map(|s| (s.slope / self.nominal - 1.0) * 1e6)
    }

    /// How many times the fit restarted because the host clock jumped.
    pub fn resets(&self) -> u32 {
        self.resets
    }

    /// Pairs dropped as single bad timestamps.
    pub fn outliers(&self) -> u64 {
        self.outliers
    }

    /// Pairs ignored because their sample index did not increase.
    pub fn ignored(&self) -> u64 {
        self.ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Feed `secs` of callbacks of `buf` samples with the host clock running
    /// `ppm` fast, starting at `start_sample`/`host0`. Returns the next sample.
    fn feed(c: &mut AudioClock, start_sample: u64, host0: u64, secs: u64, ppm: f64) -> u64 {
        let buf = 1024u64;
        let mut s = start_sample;
        while s < start_sample + secs * RATE as u64 {
            let dt = (s - start_sample) as f64 * 1e9 / RATE as f64 * (1.0 + ppm * 1e-6);
            c.observe(s, host0 + dt as u64);
            s += buf;
        }
        s
    }

    #[test]
    fn fits_50_ppm_drift_over_90_minutes_to_well_under_a_millisecond() {
        let mut c = AudioClock::new(RATE);
        let host0 = 5_000_000_000_000;
        let end = feed(&mut c, 0, host0, 90 * 60, 50.0);
        let host_end = host0 + (end as f64 * 1e9 / RATE as f64 * (1.0 + 50e-6)) as u64;
        let pos = c.sample_at(host_end).unwrap();
        // Nominal mapping would be off by 0.27 s (≈ 13 000 samples).
        assert!(
            (pos - end as f64).abs() < 48.0,
            "off by {} samples",
            pos - end as f64
        );
        let ppm = c.drift_ppm().unwrap();
        assert!((ppm - 50.0).abs() < 1.0, "drift {ppm}");
        assert_eq!(c.resets(), 0);
    }

    #[test]
    fn a_single_bad_timestamp_is_an_outlier_not_a_reset() {
        let mut c = AudioClock::new(RATE);
        let next = feed(&mut c, 0, 1_000_000_000, 10, 0.0);
        let host = |s: u64| 1_000_000_000 + s * 1_000_000_000 / RATE as u64;
        assert_eq!(c.observe(next, host(next) + 300_000_000), Observed::Suspect);
        assert_eq!(c.newest_sample(), Some(next - 1024));
        assert_eq!(
            c.observe(next + 1024, host(next + 1024)),
            Observed::Accepted
        );
        assert_eq!(c.resets(), 0);
        assert_eq!(c.outliers(), 1);
    }

    #[test]
    fn a_host_clock_jump_restarts_the_fit_and_old_instants_keep_the_old_line() {
        let mut c = AudioClock::new(RATE);
        let host0 = 1_000_000_000u64;
        let next = feed(&mut c, 0, host0, 60, 30.0);
        let before_jump =
            host0 + (next as f64 * 1e9 / RATE as f64 * (1.0 + 30e-6)) as u64 - 5_000_000;
        // The machine slept an hour: QPC jumped, the sample count did not.
        let slept = 3_600_000_000_000u64;
        let host1 = host0 + (next as f64 * 1e9 / RATE as f64 * (1.0 + 30e-6)) as u64 + slept;
        let mut outcomes = vec![];
        for i in 0..CONFIRM_POINTS as u64 {
            let s = next + i * 1024;
            outcomes.push(c.observe(s, host1 + (i * 1024 * 1_000_000_000) / RATE as u64));
        }
        assert_eq!(outcomes.last(), Some(&Observed::Reset));
        assert_eq!(c.resets(), 1);
        // After the jump: a host instant 1 s past the wake maps 1 s past `next`.
        let after = c.sample_at(host1 + 1_000_000_000).unwrap();
        assert!((after - (next + RATE as u64) as f64).abs() < 5.0, "{after}");
        // A frame captured 5 ms before the sleep still lands just before `next`.
        let old = c.sample_at(before_jump).unwrap();
        assert!((old - (next as f64 - 240.0)).abs() < 5.0, "{old}");
    }

    #[test]
    fn non_increasing_sample_indices_are_ignored() {
        let mut c = AudioClock::new(RATE);
        assert_eq!(c.observe(1000, 1), Observed::Accepted);
        assert_eq!(c.observe(1000, 2), Observed::Ignored);
        assert_eq!(c.observe(10, 3), Observed::Ignored);
        assert_eq!(c.ignored(), 2);
        // One pair: nominal slope through it. 1 ns earlier is 1/20 833 sample earlier.
        let p = c.sample_at(0).unwrap();
        assert!((p - (1000.0 - RATE as f64 / 1e9)).abs() < 1e-9, "{p}");
    }

    #[test]
    fn empty_clock_places_nothing() {
        let c = AudioClock::new(RATE);
        assert_eq!(c.sample_at(123), None);
        assert_eq!(c.newest_sample(), None);
        assert_eq!(c.drift_ppm(), None);
    }
}
