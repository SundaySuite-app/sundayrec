//! Level metering — the pure, lock-free bridge from an audio thread to the UI.
//!
//! This module is the *testable VU mat*: it computes per-block peak and RMS and
//! holds a peak-since-last-read in an atomic slot per channel. It has NO cpal,
//! NO Tauri dependency — only `std` (plus `serde`/`ts-rs` for the wire type the
//! event carries). The `src-tauri` cpal layer drives it from a real-time audio
//! callback; here it is exercised entirely under `cargo test`.
//!
//! Peaks are non-negative, so we keep a peak-hold-since-last-read using
//! `fetch_max` on the f32 bit pattern: for non-negative floats the bitwise
//! order matches the numeric order. (Ported from SundayStudio's
//! `audio/recorder/meters.rs`, with RMS added for SundayRec's VU.)

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Largest absolute sample in a block (linear, 0.0..=~1.0). Real-time safe:
/// no allocation, bounded work.
pub fn block_peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max)
}

/// Root-mean-square of a block (linear, 0.0..=~1.0) — the "energy" level a VU
/// shows, smoother than peak. Empty block reads as 0.0. Non-finite samples are
/// skipped defensively so a stray NaN never poisons the whole block. Real-time
/// safe: a single pass, no allocation.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum_sq = 0.0_f64;
    let mut n = 0u64;
    for &s in samples {
        if s.is_finite() {
            sum_sq += (s as f64) * (s as f64);
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    ((sum_sq / n as f64).sqrt()) as f32
}

/// Convert a linear level (0.0..1.0) to dBFS. Zero/at-floor reads as -inf.
pub fn peak_to_dbfs(level: f32) -> f32 {
    if level <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * level.log10()
    }
}

/// One atomic slot per channel holding the max linear level since the last
/// read. Lock-free and allocation-free on the observe path, so it is safe to
/// call from a real-time audio callback.
pub struct PeakMeters {
    slots: Vec<AtomicU32>,
}

impl PeakMeters {
    pub fn new(channels: usize) -> Self {
        Self {
            slots: (0..channels).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    pub fn channels(&self) -> usize {
        self.slots.len()
    }

    /// Record a level for a channel, keeping the max since the last `take`.
    /// Called from the audio thread once per block. Out-of-range channels and
    /// non-finite/negative values are ignored (defensive; never panics on the
    /// real-time path).
    pub fn observe(&self, channel: usize, level: f32) {
        if !level.is_finite() || level < 0.0 {
            return;
        }
        if let Some(slot) = self.slots.get(channel) {
            slot.fetch_max(level.to_bits(), Ordering::AcqRel);
        }
    }

    /// Read and reset a channel's held level (linear). The UI polls this.
    pub fn take(&self, channel: usize) -> f32 {
        match self.slots.get(channel) {
            Some(slot) => f32::from_bits(slot.swap(0, Ordering::AcqRel)),
            None => 0.0,
        }
    }

    /// Read and reset a channel's held level in dBFS (UI convenience).
    pub fn take_dbfs(&self, channel: usize) -> f32 {
        peak_to_dbfs(self.take(channel))
    }
}

/// A single VU snapshot, one entry per channel, in dBFS. This is the payload of
/// the `vu://levels` Tauri event. `f32::NEG_INFINITY` (silence) is serialised by
/// serde_json as `null`, which the renderer treats as "-∞ / floor".
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/VuLevels.ts")]
pub struct VuLevels {
    /// Per-channel peak level since the last sample, in dBFS (≤ 0).
    pub peak_dbfs: Vec<f32>,
    /// Per-channel RMS level since the last sample, in dBFS (≤ 0).
    pub rms_dbfs: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_peak_is_max_abs() {
        assert_eq!(block_peak(&[0.1, -0.7, 0.3]), 0.7);
        assert_eq!(block_peak(&[]), 0.0);
    }

    #[test]
    fn rms_of_constant_is_that_constant() {
        // RMS of a DC block equal to |v| is |v|.
        assert!((rms(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-6);
        assert!((rms(&[-0.5, -0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_of_full_scale_sine_is_about_0707() {
        // A full-scale sine has RMS ≈ 1/√2 ≈ 0.7071.
        let n = 4096;
        let buf: Vec<f32> = (0..n)
            .map(|i| (i as f32 / n as f32 * std::f32::consts::TAU).sin())
            .collect();
        assert!((rms(&buf) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-2);
    }

    #[test]
    fn rms_empty_and_nan_are_handled() {
        assert_eq!(rms(&[]), 0.0);
        // All-NaN → no finite samples → 0.0, not NaN.
        assert_eq!(rms(&[f32::NAN, f32::NAN]), 0.0);
        // A stray NaN does not poison the finite samples.
        assert!((rms(&[0.5, f32::NAN, 0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn dbfs_mapping() {
        assert!((peak_to_dbfs(1.0) - 0.0).abs() < 1e-4);
        assert!((peak_to_dbfs(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(peak_to_dbfs(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn meters_hold_max_then_reset_on_take() {
        let m = PeakMeters::new(2);
        m.observe(0, 0.3);
        m.observe(0, 0.8);
        m.observe(0, 0.5); // max so far is 0.8
        m.observe(1, 0.25);

        assert!((m.take(0) - 0.8).abs() < 1e-6);
        assert!((m.take(1) - 0.25).abs() < 1e-6);
        // After take, slots reset to 0.
        assert_eq!(m.take(0), 0.0);
    }

    #[test]
    fn meters_take_dbfs_reflects_held_level() {
        let m = PeakMeters::new(1);
        m.observe(0, 0.5);
        assert!((m.take_dbfs(0) + 6.0206).abs() < 1e-3);
        // Drained: now silence → -inf.
        assert_eq!(m.take_dbfs(0), f32::NEG_INFINITY);
    }

    #[test]
    fn meters_ignore_bad_input_and_oob_channels() {
        let m = PeakMeters::new(1);
        m.observe(0, f32::NAN);
        m.observe(0, -1.0);
        m.observe(5, 0.9); // out of range
        assert_eq!(m.take(0), 0.0);
        assert_eq!(m.take(5), 0.0);
        assert_eq!(m.channels(), 1);
    }

    #[test]
    fn vu_levels_serialises_infinity_as_null() {
        let v = VuLevels {
            peak_dbfs: vec![f32::NEG_INFINITY, -6.0],
            rms_dbfs: vec![f32::NEG_INFINITY, -9.0],
        };
        let json = serde_json::to_string(&v).expect("serialise");
        assert!(json.contains("null"), "−∞ should serialise as null: {json}");
        assert!(json.contains("-6"));
    }
}
