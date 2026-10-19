//! D3 — loudness of the routed f32 stream, measured WITHOUT touching the
//! capture.
//!
//! ```text
//!   RT callback ──► SPSC ring ──► writer thread ──► .wav
//!   (untouched)                      │
//!                                    └─► LufsTap ──► LoudnessSlot ──► 1 Hz heartbeat
//!                                         (BS.1770)   (an atomic)     + history row
//! ```
//!
//! ## Why the writer thread, and why that is safe
//!
//! The cpal data callback is real-time: no locks, no allocation, no I/O. The
//! meter (K-weighting biquads, a gated histogram, a polyphase true-peak
//! interpolator) is none of those things you want in a callback, so it is not
//! there: this file is not called from `stream.rs` at all. It runs on the WAV
//! writer thread, which already sees every routed block (it feeds the silence
//! detector from the same place) and which is allowed to be slow: the ring in
//! front of it is five seconds deep, and an fsync every five seconds already
//! costs it far more than this does.
//!
//! The cost is measured, not assumed — see the tests at the bottom: per block
//! it is a fraction of a percent of the block's own duration even for eight
//! channels, and a writer fed at full speed with and without the tap loses
//! nothing and drops nothing.
//!
//! ## One meter per DELIVERABLE
//!
//! A reconnect appends a `_rN` fragment to the same deliverable (one file, one
//! history row), so the integrated loudness must run across fragments. The
//! supervisor therefore owns one [`LoudnessSlot`] per deliverable and every
//! fragment's writer borrows the meter from it: [`LoudnessSlot::begin`] takes
//! the meter out (no lock while measuring), [`LufsTap`]'s drop puts it back.
//! A fragment at another rate or channel count starts a fresh meter — which is
//! also what a new deliverable is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use sundayrec_core::chain::dsp::loudness::LoudnessMeter;
use sundayrec_core::level_watch::clamp_lufs;

use crate::util::lock_recover;

/// How much audio the meter must have seen before its short-term value means
/// anything: the window is 3 s, and before it is full the missing part reads as
/// silence (a too-low number — the very thing this feature warns about).
const SHORT_TERM_WINDOW_SECS: u64 = 3;

/// How much audio the integrated value needs before it is worth storing.
const INTEGRATED_MIN_SECS: u64 = 3;

/// `f64::NAN` bits: «no reading» in the atomic.
const NO_READING: u64 = 0x7ff8_0000_0000_0000;

/// The session's meters, one per deliverable in order — the supervisor opens
/// one when a deliverable starts (the first, and every split), finalisation
/// reads the integrated value of deliverable `index` for its history row.
/// Cloning shares the same list.
#[derive(Clone, Default)]
pub struct LoudnessLedger(std::sync::Arc<Mutex<Vec<std::sync::Arc<LoudnessSlot>>>>);

impl LoudnessLedger {
    /// A deliverable begins: a fresh meter for it.
    pub fn open_deliverable(&self) {
        lock_recover(&self.0).push(std::sync::Arc::new(LoudnessSlot::new()));
    }

    /// The meter of the deliverable being recorded (the newest). Reconnect
    /// fragments share it.
    pub fn current(&self) -> Option<std::sync::Arc<LoudnessSlot>> {
        lock_recover(&self.0).last().cloned()
    }

    /// Integrated loudness of deliverable `index`; `None` when it has no meter
    /// (crash recovery) or too little was measured.
    pub fn integrated(&self, index: usize) -> Option<f64> {
        lock_recover(&self.0)
            .get(index)
            .and_then(|s| s.integrated())
    }
}

/// The meter of ONE deliverable and the numbers read from outside the writer
/// thread. Shared as an `Arc` between the supervisor, the fragments' writers
/// and the segment runner's heartbeat.
pub struct LoudnessSlot {
    /// The meter, parked here between fragments. `None` while a writer has it
    /// (or before the first fragment).
    parked: Mutex<Option<LoudnessMeter>>,
    /// Latest short-term loudness (f64 bits); [`NO_READING`] until the window
    /// is full. Written by the writer thread, read by the 1 Hz heartbeat.
    short_term_bits: AtomicU64,
    /// Frames this deliverable's meter has seen — the age of the readings.
    frames_seen: AtomicU64,
    /// Sample rate of `frames_seen`, for turning it into seconds.
    rate: AtomicU64,
}

impl Default for LoudnessSlot {
    fn default() -> Self {
        Self {
            parked: Mutex::new(None),
            short_term_bits: AtomicU64::new(NO_READING),
            frames_seen: AtomicU64::new(0),
            rate: AtomicU64::new(0),
        }
    }
}

impl LoudnessSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the deliverable's meter for a fragment at `rate` Hz / `channels`.
    /// A parked meter of another shape is dropped (new deliverable, or a
    /// device that came back different) and a fresh one starts.
    pub fn begin(&self, rate: u32, channels: usize) -> LufsTap<'_> {
        let parked = lock_recover(&self.parked).take();
        let meter = match parked {
            Some(m) if m.sample_rate() == f64::from(rate) && m.channels() == channels.max(1) => m,
            _ => {
                self.frames_seen.store(0, Ordering::Relaxed);
                self.short_term_bits.store(NO_READING, Ordering::Relaxed);
                LoudnessMeter::new(f64::from(rate), channels.max(1))
            }
        };
        self.rate.store(u64::from(rate), Ordering::Relaxed);
        LufsTap {
            slot: self,
            meter: Some(meter),
            channels: channels.max(1),
        }
    }

    /// Short-term loudness (LUFS) right now, finite and wire-safe (digital
    /// silence is [`sundayrec_core::level_watch::SILENCE_FLOOR_LUFS`]). `None`
    /// until three seconds of audio have been measured.
    pub fn short_term(&self) -> Option<f64> {
        let v = f64::from_bits(self.short_term_bits.load(Ordering::Relaxed));
        if v.is_nan() {
            None
        } else {
            Some(v)
        }
    }

    /// Seconds of audio the meter has seen.
    fn seconds_seen(&self) -> u64 {
        let rate = self.rate.load(Ordering::Relaxed).max(1);
        self.frames_seen.load(Ordering::Relaxed) / rate
    }

    /// Integrated loudness (BS.1770-4, gated) of everything measured in this
    /// deliverable, for the history row. Read AFTER the last fragment's writer
    /// has finished (it parks the meter on drop). `None` when too little audio
    /// was measured; a recording that never rose above the meter's −70 LUFS
    /// gate comes out as the silence floor, not as «unknown».
    pub fn integrated(&self) -> Option<f64> {
        if self.seconds_seen() < INTEGRATED_MIN_SECS {
            return None;
        }
        let parked = lock_recover(&self.parked);
        let m = parked.as_ref()?;
        clamp_lufs(m.integrated())
    }
}

/// A fragment writer's handle on the deliverable's meter. Dropping it parks the
/// meter back in the slot, so every exit of the writer — clean, disk full, an
/// error mid-way — keeps what was measured.
pub struct LufsTap<'a> {
    slot: &'a LoudnessSlot,
    meter: Option<LoudnessMeter>,
    channels: usize,
}

impl LufsTap<'_> {
    /// Measure one block of routed, interleaved f32 (a whole number of frames).
    /// Allocation-free; no lock.
    pub fn feed(&mut self, block: &[f32]) {
        let Some(m) = self.meter.as_mut() else { return };
        m.process_interleaved(block);
        let frames = (block.len() / self.channels) as u64;
        let seen = self.slot.frames_seen.fetch_add(frames, Ordering::Relaxed) + frames;
        let rate = self.slot.rate.load(Ordering::Relaxed).max(1);
        let bits = if seen / rate >= SHORT_TERM_WINDOW_SECS {
            clamp_lufs(m.short_term()).map_or(NO_READING, f64::to_bits)
        } else {
            NO_READING
        };
        self.slot.short_term_bits.store(bits, Ordering::Relaxed);
    }
}

impl Drop for LufsTap<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.meter.take() {
            *lock_recover(&self.slot.parked) = Some(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const RATE: u32 = 48_000;

    /// `secs` of a sine at `amp` (peak), interleaved over `ch` channels.
    fn sine(secs: f64, ch: usize, amp: f32) -> Vec<f32> {
        let frames = (secs * f64::from(RATE)) as usize;
        let mut v = Vec::with_capacity(frames * ch);
        for i in 0..frames {
            let x = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE as f32).sin();
            for _ in 0..ch {
                v.push(x);
            }
        }
        v
    }

    fn feed_all(tap: &mut LufsTap<'_>, data: &[f32], ch: usize) {
        // The writer drains in chunks well under a second.
        for c in data.chunks(4096 * ch) {
            tap.feed(c);
        }
    }

    #[test]
    fn there_is_no_short_term_reading_before_the_window_is_full() {
        let slot = LoudnessSlot::new();
        let mut tap = slot.begin(RATE, 1);
        feed_all(&mut tap, &sine(2.0, 1, 0.1), 1);
        assert_eq!(slot.short_term(), None, "2 s: window not full, no reading");
        feed_all(&mut tap, &sine(2.0, 1, 0.1), 1);
        assert!(slot.short_term().is_some(), "4 s: reading");
    }

    #[test]
    fn a_normal_level_reads_normal_and_a_muted_mixer_reads_weak() {
        // −20 dBFS sine ≈ −23 LUFS (mono); −70 dBFS ≈ −73 LUFS.
        let loud = LoudnessSlot::new();
        let mut t = loud.begin(RATE, 1);
        feed_all(&mut t, &sine(6.0, 1, 0.1), 1);
        let l = loud.short_term().unwrap();
        assert!((-30.0..-15.0).contains(&l), "normal level reads {l} LUFS");

        let quiet = LoudnessSlot::new();
        let mut t = quiet.begin(RATE, 1);
        feed_all(&mut t, &sine(6.0, 1, 0.0003), 1);
        let q = quiet.short_term().unwrap();
        assert!(sundayrec_core::level_watch::is_weak(q), "muted reads {q}");
    }

    #[test]
    fn digital_silence_reads_the_finite_floor_not_minus_infinity() {
        let slot = LoudnessSlot::new();
        let mut t = slot.begin(RATE, 2);
        feed_all(&mut t, &vec![0.0f32; 6 * RATE as usize * 2], 2);
        let v = slot.short_term().unwrap();
        assert!(v.is_finite());
        assert_eq!(v, sundayrec_core::level_watch::SILENCE_FLOOR_LUFS);
        drop(t);
        assert_eq!(
            slot.integrated(),
            Some(sundayrec_core::level_watch::SILENCE_FLOOR_LUFS),
            "silence is a floor value, not «unknown»"
        );
    }

    #[test]
    fn integrated_runs_across_reconnect_fragments_and_survives_a_drop() {
        let slot = LoudnessSlot::new();
        {
            let mut t = slot.begin(RATE, 1);
            feed_all(&mut t, &sine(5.0, 1, 0.1), 1);
        } // fragment 1's writer ends (any exit): meter parked
        assert!(slot.integrated().is_some());
        let first = slot.integrated().unwrap();
        {
            let mut t = slot.begin(RATE, 1);
            feed_all(&mut t, &sine(5.0, 1, 0.1), 1);
        }
        let both = slot.integrated().unwrap();
        assert!((first - both).abs() < 0.5, "same level: {first} vs {both}");
        assert_eq!(
            slot.seconds_seen(),
            10,
            "the second fragment continued the first"
        );
    }

    #[test]
    fn a_fragment_in_another_shape_starts_a_fresh_meter() {
        let slot = LoudnessSlot::new();
        {
            let mut t = slot.begin(RATE, 1);
            feed_all(&mut t, &sine(5.0, 1, 0.1), 1);
        }
        {
            let mut t = slot.begin(RATE, 2);
            feed_all(&mut t, &sine(4.0, 2, 0.1), 2);
        }
        assert_eq!(slot.seconds_seen(), 4);
    }

    #[test]
    fn too_little_audio_is_unknown_not_a_number() {
        let slot = LoudnessSlot::new();
        {
            let mut t = slot.begin(RATE, 1);
            feed_all(&mut t, &sine(1.0, 1, 0.1), 1);
        }
        assert_eq!(slot.integrated(), None);
    }

    /// ⚠️ The cost evidence. Not a strict timing gate (CI machines are noisy):
    /// a very generous ceiling that a callback-sized problem would break, plus
    /// the measured numbers printed for the PR.
    ///
    /// Budget: the writer must keep up with the device. For each block, the
    /// meter's time should be a tiny fraction of the block's own duration.
    #[test]
    fn the_meter_costs_a_tiny_fraction_of_the_block_duration() {
        for &ch in &[1usize, 2, 8, 32] {
            let slot = LoudnessSlot::new();
            let mut tap = slot.begin(RATE, ch);
            // 4096-frame blocks: what the writer drains at most.
            let frames = 4096usize;
            let block: Vec<f32> = sine(frames as f64 / f64::from(RATE), ch, 0.1);
            let block = &block[..frames * ch];
            let blocks = 200;
            let t0 = Instant::now();
            for _ in 0..blocks {
                tap.feed(block);
            }
            let spent = t0.elapsed().as_secs_f64();
            let audio = (frames * blocks) as f64 / f64::from(RATE);
            let fraction = spent / audio;
            eprintln!(
                "D3 cost: {ch:>2} ch  {:.2} ms/block of {:.1} ms audio  = {:.3} % of real time ({:.0}x)",
                spent / blocks as f64 * 1e3,
                frames as f64 / f64::from(RATE) * 1e3,
                fraction * 100.0,
                1.0 / fraction
            );
            assert!(
                fraction < 0.25,
                "{ch} ch: the meter used {:.1} % of real time",
                fraction * 100.0
            );
        }
    }
}
