//! Loudness in Rust (B1a, docs/LYDMOTOR.md fase 1): measurement and gain plan.
//!
//! This replaces the two-pass ffmpeg `loudnorm` with something we steer ourselves:
//!
//! 1. **Measure** the processed signal with the BS.1770-4 / EBU R128 meter vendored from
//!    filmcraft ([`Meter`], [`measure_planar`]).
//! 2. **Plan** ([`plan_gain`]): one gain, in dB, that moves the integrated loudness to the
//!    preset's target. Always. Nothing here rides the level up and down, and nothing
//!    compresses the programme.
//! 3. **Apply** ([`GainStage`], [`apply_planar`]): the gain is a plain linear multiply. Only when
//!    the multiplied signal would pass the preset's true-peak ceiling is a look-ahead true-peak
//!    limiter placed after it, so the limiter takes the rare peaks and nothing else. When the
//!    peaks fit, the stage is bit-exact linear gain with zero latency.
//!
//! Pure: no I/O, no clock, no allocation beyond the limiter's own delay line. Callers feed PCM
//! and read numbers.
//!
//! ## The one honest limit
//!
//! A limiter cannot make a loud peak vanish for free. The plan reports how far over the ceiling
//! the peaks would land ([`GainPlan::peak_overshoot_db`]); if that is large the limiter works
//! hard and the loudness it lands on drifts a little under target. Callers that want a receipt
//! ("we limited 4 dB of peaks") have the number; the target is still the target.

use crate::mastering::MasterPreset;
use filmcraft_audio_dsp::effects::Limiter;
use filmcraft_audio_dsp::loudness::LoudnessMeter;
use filmcraft_audio_dsp::AudioEffect;

/// Below this integrated loudness a signal is room tone / a muted channel, not a programme.
/// Raising it 40 dB would only amplify noise, so the plan leaves it alone.
pub const SILENCE_FLOOR_LUFS: f64 = -50.0;

/// How far under the preset's ceiling the limiter is told to aim. The limiter's true-peak
/// detector is a 4x oversampling estimate, and a reference meter reads inter-sample peaks with
/// a different interpolator; this margin keeps the *measured* peak at or under the ceiling.
pub const LIMITER_MARGIN_DB: f64 = 0.3;

/// What a signal measures. `NEG_INFINITY` fields mean "nothing to measure".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Gated integrated loudness, LUFS.
    pub integrated_lufs: f64,
    /// Loudness range, LU.
    pub range_lu: f64,
    /// True peak over all channels, dBTP.
    pub true_peak_dbtp: f64,
    /// Sample peak over all channels, dBFS.
    pub sample_peak_dbfs: f64,
}

impl Loudness {
    /// True when the integrated loudness is a real number above [`SILENCE_FLOOR_LUFS`].
    pub fn is_programme(&self) -> bool {
        self.integrated_lufs.is_finite()
            && self.true_peak_dbtp.is_finite()
            && self.integrated_lufs > SILENCE_FLOOR_LUFS
    }
}

/// Streaming meter: feed PCM in blocks of any size, read [`Loudness`] at the end.
pub struct Meter(LoudnessMeter);

impl Meter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self(LoudnessMeter::new(f64::from(sample_rate), channels.max(1)))
    }

    /// Feed one block of planar audio (one slice per channel, equal lengths).
    pub fn process_planar(&mut self, channels: &[&[f32]]) {
        self.0.process(channels);
    }

    /// Feed one block of interleaved audio.
    pub fn process_interleaved(&mut self, data: &[f32]) {
        self.0.process_interleaved(data);
    }

    /// The measurement so far.
    pub fn loudness(&self) -> Loudness {
        let s = self.0.summary();
        Loudness {
            integrated_lufs: s.integrated_lufs,
            range_lu: s.loudness_range_lu,
            true_peak_dbtp: s.true_peak_dbtp,
            sample_peak_dbfs: s.sample_peak_dbfs,
        }
    }
}

/// Measure a whole planar programme in one call.
pub fn measure_planar(channels: &[&[f32]], sample_rate: u32) -> Loudness {
    let mut m = Meter::new(sample_rate, channels.len());
    m.process_planar(channels);
    m.loudness()
}

/// What to do to reach a loudness target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainPlan {
    /// Linear gain, dB. Applied first, to every sample alike.
    pub gain_db: f64,
    /// A true-peak limiter after the gain, or `None` when the peaks already fit.
    pub limiter: Option<LimiterPlan>,
    /// Integrated loudness the plan aims for (the preset's target), LUFS.
    pub target_lufs: f64,
    /// By how many dB the gained peaks would pass the ceiling without a limiter (0 when they
    /// fit). This is the most the limiter ever has to take.
    pub peak_overshoot_db: f64,
    /// False when the input is silence / room tone: the plan is then "leave it alone".
    pub measurable: bool,
}

/// Limiter settings: just the ceiling the limiter is aimed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterPlan {
    /// Ceiling, dBTP. This is the preset's ceiling minus [`LIMITER_MARGIN_DB`].
    pub ceiling_dbtp: f64,
}

/// Plan the gain that lands `measured` on the preset's loudness target.
///
/// The gain is `target − measured`, always. The ceiling only decides whether a limiter follows.
pub fn plan_gain(measured: &Loudness, preset: &MasterPreset) -> GainPlan {
    plan_gain_to(measured, preset.target_lufs, preset.true_peak_db)
}

/// [`plan_gain`] with the target and ceiling given directly.
pub fn plan_gain_to(measured: &Loudness, target_lufs: f64, ceiling_dbtp: f64) -> GainPlan {
    if !measured.is_programme() {
        return GainPlan {
            gain_db: 0.0,
            limiter: None,
            target_lufs,
            peak_overshoot_db: 0.0,
            measurable: false,
        };
    }
    let gain_db = target_lufs - measured.integrated_lufs;
    let overshoot = (measured.true_peak_dbtp + gain_db - ceiling_dbtp).max(0.0);
    GainPlan {
        gain_db,
        limiter: (overshoot > 0.0).then_some(LimiterPlan {
            ceiling_dbtp: ceiling_dbtp - LIMITER_MARGIN_DB,
        }),
        target_lufs,
        peak_overshoot_db: overshoot,
        measurable: true,
    }
}

/// The plan, applied: a linear gain and, when planned, the limiter after it.
///
/// Streaming. [`GainStage::process`] takes chunks in and gives the same number of samples back,
/// delayed by [`GainStage::latency`] (zero without a limiter); [`GainStage::flush`] returns the
/// tail. [`apply_planar`] does the delay bookkeeping for a whole buffer.
pub struct GainStage {
    gain: f32,
    limiter: Option<Limiter>,
    latency: usize,
    /// Leading output samples still to drop (the limiter's look-ahead), per [`Self::process`].
    skip: usize,
    channels: usize,
}

impl GainStage {
    pub fn new(plan: &GainPlan, sample_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let limiter = plan.limiter.map(|l| {
            let mut lim = Limiter::new(sample_rate as f32, channels);
            lim.set_param("input_gain", 0.0);
            lim.set_param("ceiling", l.ceiling_dbtp as f32);
            lim.set_param("true_peak", 1.0);
            // Params ease in from the next sample; settle the 20 ms input-gain smoother (unity,
            // so already there) and the ceiling before audio arrives.
            lim.reset();
            lim
        });
        let latency = limiter.as_ref().map_or(0, |l| l.latency());
        Self {
            gain: 10f64.powf(plan.gain_db / 20.0) as f32,
            limiter,
            latency,
            skip: latency,
            channels,
        }
    }

    /// Samples by which the raw limiter output trails its input (0 without a limiter).
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Process one chunk of planar audio in place, time-aligned: the limiter's look-ahead delay
    /// is removed here, so the first call returns `latency` samples fewer (the chunk is shrunk)
    /// and [`Self::flush`] returns them at the end. Total output length equals total input.
    pub fn process(&mut self, chunk: &mut [Vec<f32>]) {
        for ch in chunk.iter_mut() {
            for v in ch.iter_mut() {
                *v *= self.gain;
            }
        }
        let Some(lim) = self.limiter.as_mut() else { return };
        let mut views: Vec<&mut [f32]> = chunk.iter_mut().map(|c| c.as_mut_slice()).collect();
        lim.process(&mut views);
        if self.skip > 0 {
            let drop = self.skip.min(chunk.first().map_or(0, Vec::len));
            for ch in chunk.iter_mut() {
                ch.drain(..drop);
            }
            self.skip -= drop;
        }
    }

    /// The last `latency` samples, once the input has ended. Empty without a limiter.
    pub fn flush(&mut self) -> Vec<Vec<f32>> {
        let Some(lim) = self.limiter.as_mut() else {
            return vec![Vec::new(); self.channels];
        };
        // Samples still owed = look-ahead minus whatever `process` has not yet dropped.
        let owed = self.latency - self.skip;
        let mut tail = vec![vec![0.0f32; owed]; self.channels];
        let mut views: Vec<&mut [f32]> = tail.iter_mut().map(|c| c.as_mut_slice()).collect();
        lim.process(&mut views);
        tail
    }
}

/// Apply `plan` to a whole planar buffer in place. Length and time alignment are preserved.
pub fn apply_planar(plan: &GainPlan, sample_rate: u32, channels: &mut [Vec<f32>]) {
    let mut stage = GainStage::new(plan, sample_rate, channels.len());
    stage.process(channels);
    for (ch, tail) in channels.iter_mut().zip(stage.flush()) {
        ch.extend(tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastering::get_preset_by_id;

    fn loud(i: f64, tp: f64) -> Loudness {
        Loudness {
            integrated_lufs: i,
            range_lu: 5.0,
            true_peak_dbtp: tp,
            sample_peak_dbfs: tp,
        }
    }

    #[test]
    fn gain_is_target_minus_measured() {
        let p = get_preset_by_id("speech-clear").unwrap(); // −16 LUFS, −1 dBTP
        let plan = plan_gain(&loud(-24.0, -10.0), &p);
        assert!((plan.gain_db - 8.0).abs() < 1e-12);
        assert_eq!(plan.limiter, None, "−10 + 8 = −2 dBTP fits under −1");
        assert_eq!(plan.peak_overshoot_db, 0.0);
        assert!(plan.measurable);
    }

    #[test]
    fn limiter_only_when_peaks_would_pass_the_ceiling() {
        let p = get_preset_by_id("speech-clear").unwrap();
        // −24 → −16 is +8 dB; a −5 dBTP peak lands at +3: 4 dB over the −1 ceiling.
        let plan = plan_gain(&loud(-24.0, -5.0), &p);
        assert!((plan.gain_db - 8.0).abs() < 1e-12, "the target is never traded for the peak");
        let l = plan.limiter.expect("peaks need the limiter");
        assert!((l.ceiling_dbtp - (-1.0 - LIMITER_MARGIN_DB)).abs() < 1e-12);
        assert!((plan.peak_overshoot_db - 4.0).abs() < 1e-12);
    }

    #[test]
    fn every_preset_uses_its_own_target_and_ceiling() {
        for p in crate::mastering::master_presets() {
            let plan = plan_gain(&loud(-30.0, -3.0), &p);
            assert!((plan.gain_db - (p.target_lufs + 30.0)).abs() < 1e-12, "{}", p.id);
            assert_eq!(plan.target_lufs, p.target_lufs);
            let over = -3.0 + plan.gain_db - p.true_peak_db;
            assert_eq!(plan.limiter.is_some(), over > 0.0, "{}", p.id);
        }
    }

    #[test]
    fn silence_and_room_tone_are_left_alone() {
        let p = get_preset_by_id("speech-clear").unwrap();
        for m in [loud(f64::NEG_INFINITY, f64::NEG_INFINITY), loud(-57.2, -38.2), loud(f64::NAN, 0.0)] {
            let plan = plan_gain(&m, &p);
            assert_eq!(plan.gain_db, 0.0);
            assert_eq!(plan.limiter, None);
            assert!(!plan.measurable);
        }
    }

    #[test]
    fn already_on_target_is_unity() {
        let p = get_preset_by_id("music-speech").unwrap();
        let plan = plan_gain(&loud(p.target_lufs, -6.0), &p);
        assert!(plan.gain_db.abs() < 1e-12);
        assert_eq!(plan.limiter, None);
    }

    fn tone(amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.0613).sin()).collect()
    }

    #[test]
    fn without_a_limiter_the_stage_is_exact_linear_gain_and_has_no_latency() {
        let plan = plan_gain_to(&loud(-26.0, -12.0), -20.0, -1.0); // +6 dB, −6 dBTP: fits
        let mut audio = vec![tone(0.1, 5000), tone(0.05, 5000)];
        let orig = audio.clone();
        apply_planar(&plan, 48_000, &mut audio);
        let g = 10f32.powf(6.0 / 20.0);
        assert_eq!(GainStage::new(&plan, 48_000, 2).latency(), 0);
        for (a, o) in audio.iter().zip(&orig) {
            assert_eq!(a.len(), o.len());
            assert!(a.iter().zip(o).all(|(x, y)| (x - y * g).abs() < 1e-7));
        }
    }

    #[test]
    fn limited_output_keeps_length_alignment_and_stays_under_the_ceiling() {
        let sr = 48_000;
        let mut l = tone(0.02, 48_000);
        l[24_000] = 0.2; // a 10x peak among quiet material
        let m = measure_planar(&[&l], sr);
        let plan = plan_gain_to(&m, m.integrated_lufs + 20.0, -1.0);
        assert!(plan.limiter.is_some());
        let mut audio = vec![l.clone()];
        apply_planar(&plan, sr, &mut audio);
        assert_eq!(audio[0].len(), l.len());
        let ceiling = 10f32.powf(-1.0 / 20.0);
        assert!(audio[0].iter().all(|v| v.abs() <= ceiling + 1e-6));
        // The peak is still where it was (look-ahead delay removed): within a few ms.
        let at = audio[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        assert!(at.abs_diff(24_000) < 48, "peak moved to {at}");
    }

    #[test]
    fn chunked_streaming_equals_whole_buffer() {
        let sr = 48_000;
        let mut l = tone(0.03, 30_000);
        l[10_000] = 0.3;
        let m = measure_planar(&[&l], sr);
        let plan = plan_gain_to(&m, m.integrated_lufs + 18.0, -1.0);
        let mut whole = vec![l.clone()];
        apply_planar(&plan, sr, &mut whole);

        let mut stage = GainStage::new(&plan, sr, 1);
        let mut out: Vec<f32> = Vec::new();
        for c in l.chunks(777) {
            let mut chunk = vec![c.to_vec()];
            stage.process(&mut chunk);
            out.extend(&chunk[0]);
        }
        out.extend(&stage.flush()[0]);
        assert_eq!(out.len(), whole[0].len());
        assert!(out.iter().zip(&whole[0]).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn linear_gain_lands_the_measured_target() {
        // Pink-ish noise stands in for programme; no limiter needed at −3 dB of peak headroom.
        let sr = 48_000;
        let mut s = 12345u64;
        let noise: Vec<f32> = (0..sr as usize * 12)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.05
            })
            .collect();
        let m = measure_planar(&[&noise, &noise], sr);
        let plan = plan_gain_to(&m, -16.0, -1.0);
        let mut audio = vec![noise.clone(), noise];
        apply_planar(&plan, sr, &mut audio);
        let after = measure_planar(&[&audio[0], &audio[1]], sr);
        assert!((after.integrated_lufs - -16.0).abs() < 0.05, "{}", after.integrated_lufs);
    }
}
