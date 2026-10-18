//! «Tale» renser av seg selv (B3a, `docs/LYDMOTOR.md` fase 3): analysen som
//! avgjør hva profilen «Tale» tar med, og hvordan støy- og romklangfjerningen
//! holdes unna musikk.
//!
//! No new buttons. The recording is read once (16 kHz, as the later analysis
//! passes will), [`SpeechAnalyzer`] measures it, [`SpeechAnalysis::decide`]
//! turns the measurements into a [`SpeechClean`], and
//! [`crate::chain::Chain::speech_clean`] turns that into stages that run in
//! front of the `speech-clear` preset's own chain.
//!
//! ## The four triggers
//!
//! | Stage | Switched on when | Measured by |
//! | ----- | ---------------- | ----------- |
//! | hum removal (`dehum`) | a line on 50/100/150 Hz stands at least [`HUM_RATIO_DB`] above the bands 4 Hz either side, in at least [`HUM_BLOCK_FRACTION`] of the 2 s blocks. 50 or 60 Hz is the family with the stronger line | Hann-windowed Goertzel at 18 frequencies on the 16 kHz stream |
//! | de-noise | **almost always**: unless the recording has no floor to speak of (floor under [`NO_FLOOR_DB`]) or the floor is already [`CLEAN_SNR_DB`] under the speech. Always mild ([`DENOISE_TALE_DB`]) and **only where it is safe**, see below | 50 ms RMS histogram: 5th percentile = floor, 95th = speech |
//! | de-reverb | the slowest-decaying tenth of the speech decays implies an RT60 of at least [`LONG_RT60_S`] | envelope decays: time from -5 to -20 dB below a peak, over at least [`MIN_DECAY_EVENTS`] events |
//! | click removal | the channel diagnosis sees crackle ([`crate::processing::channel_crackles`]: a channel that is all transients while the other carries programme material), or isolated single-sample spikes come at [`CLICKS_PER_MIN`] a minute or more | per-channel peak/RMS, and a spike detector on the second difference |
//!
//! ## De-noise: the choice, and how music is protected
//!
//! B0c measured, and B2b confirmed on the real chain: filmcraft's `denoise`
//! (minimum statistics) takes a stationary floor down by `reduction` with the
//! pulses untouched, but it cannot tell a *held tone* from a floor. Anything
//! that sustains for longer than the 1.5 s statistics window is learned as
//! floor and taken down as much (-8.6 dB at 10, -19.6 dB at 20); `dereverb`
//! does the same, scaled. An organ chord, a held pad or an accompaniment is
//! exactly such a signal, and a service has plenty of them.
//!
//! The three ways out, as measured (`tale` tests and `docs/LYDMOTOR.md`
//! «B3a»):
//!
//! * **(b) a targeted `afftdn`** (no `tn=1`, `nf` measured) took 13.5 dB of the
//!   floor in B0c without touching the pulses. It is not used, for two reasons:
//!   the whole point of B2b is that ffmpeg gets no audio filter any more (the
//!   chain runs in Rust, in the preview and the export alike), and `afftdn`
//!   with a fixed `nf` is wrong for the very reason B0c names: -50 gives only
//!   5.2 dB, -40 eats the tail, and `nf` would have to be measured per
//!   recording on top of everything else.
//! * **(c) very mild, only on a stationary floor** is what the analysis asks
//!   for anyway (the floor is measured; a clean recording is left alone), but
//!   mild alone is not enough: even 9 dB on a held chord is audible.
//! * **(a) gate it on speech.** Chosen. [`SpeechGate`] sits around the
//!   `denoise` and `dereverb` stages (the chain's `SpeechGated` stage): both
//!   effects run on the whole signal (their statistics must not be
//!   interrupted) but their output is mixed back in only while the signal is
//!   (1) speech-like or (2) down at the measured floor. A held chord is
//!   neither, so it passes at its dry level.
//!
//! "Speech-like" is the one thing a streaming gate can know without a model:
//! the *level moves*. Over the last 0.6 s the 10 ms levels span at least
//! [`GATE_DEPTH_DB`] between the tenth and ninetieth percentile, and the
//! upper end is clearly above the floor. Speech does this (syllables, phrase
//! pauses); an organ chord, a pad, a fan, a hum do not. "At the floor" is the
//! measured floor from the analysis plus [`GATE_NEAR_FLOOR_DB`]: it is a
//! *fixed* number, so the gate cannot be fooled into reading a long note as
//! floor the way a tracking estimate would be. Wet attacks in 30 ms and goes
//! dry in 200 ms after a 0.6 s hold. The hold is shorter than the 1.5 s the
//! de-noiser needs to learn a note as floor, so a chord that follows speech
//! is never reduced.
//!
//! **Known limit:** a signal whose level moves like speech (drums, a strummed
//! guitar, a choir with consonants) opens the gate, and a sustained pad under
//! it then meets the de-noiser at up to [`DENOISE_TALE_DB`]. That is the same
//! trade as an engineer ducking the backing under a voice, and the profile for
//! music («Tale og musikk») does not use this chain at all.

use std::collections::VecDeque;

use crate::pcm_stream::{for_each_block, PcmFormat};
use crate::processing::{channel_crackles, ChannelLevelsDb};

/// The rate the analysis reads the file at.
pub const ANALYSIS_RATE: u32 = 16_000;

/// Hum: a harmonic line must stand this far above the bands 4 Hz either side, dB.
pub const HUM_RATIO_DB: f64 = 15.0;
/// Hum: the line must stand out in at least this share of the (non-silent) blocks.
pub const HUM_BLOCK_FRACTION: f64 = 0.6;
/// Hum: a line under this level, dBFS (peak of a sine), is not worth a notch.
const HUM_MIN_LEVEL_DB: f64 = -75.0;
/// Hum block, samples (2 s: 0.5 Hz between bins).
const HUM_BLOCK: usize = 2 * ANALYSIS_RATE as usize;
/// Hum: distance to the reference bands, Hz.
const HUM_SIDE_HZ: f64 = 4.0;
/// Hum: the harmonics looked at.
const HUM_HARMONICS: usize = 3;
/// A block quieter than this (RMS dBFS) says nothing about hum.
const HUM_SILENT_DB: f64 = -75.0;

/// De-noise: under this floor (dBFS RMS) there is nothing to take away.
pub const NO_FLOOR_DB: f64 = -85.0;
/// De-noise: a floor this far (dB) under the speech is already inaudible.
pub const CLEAN_SNR_DB: f64 = 60.0;
/// The reduction «Tale» asks `denoise` for, dB: mild, and under the stage's own cap.
pub const DENOISE_TALE_DB: f64 = 9.0;

/// De-reverb: the estimated RT60 (s) from which the room counts as long.
pub const LONG_RT60_S: f64 = 1.0;
/// De-reverb: fewest decays the estimate is built from.
pub const MIN_DECAY_EVENTS: usize = 12;
/// Envelope hop for the decay measurement, s.
const ENV_HOP_S: f64 = 0.02;

/// Clicks: isolated spikes per minute from which the recording counts as crackling.
pub const CLICKS_PER_MIN: f64 = 20.0;
/// Clicks: fewest spikes, however short the recording.
const MIN_CLICKS: usize = 5;

/// Gate: the span of the 10 ms levels over the last 0.6 s that counts as speech, dB.
pub const GATE_DEPTH_DB: f32 = 9.0;
/// Gate: how far above the floor the upper end must be to be speech, dB.
const GATE_SPEECH_ABOVE_FLOOR_DB: f32 = 15.0;
/// Gate: «at the floor» is the measured floor plus this, dB.
pub const GATE_NEAR_FLOOR_DB: f32 = 6.0;
/// Gate: and nothing in the last 0.6 s may have been over the floor by more than this, dB.
const GATE_QUIET_SPAN_DB: f32 = 12.0;

/// Which mains the hum belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mains {
    Hz50,
    Hz60,
}

impl Mains {
    pub fn hz(self) -> f64 {
        match self {
            Mains::Hz50 => 50.0,
            Mains::Hz60 => 60.0,
        }
    }
}

/// What the analysis measured.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechAnalysis {
    pub duration_s: f64,
    /// 5th percentile of the 50 ms RMS levels, dBFS.
    pub noise_floor_db: f64,
    /// 95th percentile of the 50 ms RMS levels, dBFS.
    pub speech_db: f64,
    /// The mains family and its share of blocks with a line, when there is hum.
    pub hum: Option<(Mains, f64)>,
    /// RT60 estimate (s), when there were enough decays to estimate one.
    pub rt60_s: Option<f64>,
    /// Isolated spikes per minute, the worst channel.
    pub clicks_per_min: f64,
    /// The channel diagnosis sees a channel that is all crackle.
    pub channel_crackle: bool,
}

/// What «Tale» takes along for this recording. Plain data, so a UI can say it
/// in three words (B3b) and a test can read it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechClean {
    pub hum: Option<Mains>,
    pub denoise: bool,
    /// De-reverb amount, percent, when the room is long.
    pub dereverb: Option<f32>,
    pub declick: bool,
    /// The measured floor the gate reads «at the floor» from, dBFS.
    pub floor_db: f32,
}

impl SpeechClean {
    /// Nothing to do: the stages are not added and the file passes untouched.
    pub fn is_noop(&self) -> bool {
        self.hum.is_none() && !self.denoise && self.dereverb.is_none() && !self.declick
    }

    /// Stable codes for what was done, in the order of the chain, for the
    /// explanation in the sound step (B3b).
    pub fn said(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.hum.is_some() {
            v.push("hum");
        }
        if self.declick {
            v.push("clicks");
        }
        if self.denoise {
            v.push("noise");
        }
        if self.dereverb.is_some() {
            v.push("reverb");
        }
        v
    }
}

impl SpeechAnalysis {
    /// The triggers (module docs).
    pub fn decide(&self) -> SpeechClean {
        let denoise = self.noise_floor_db > NO_FLOOR_DB
            && self.speech_db - self.noise_floor_db < CLEAN_SNR_DB
            && self.duration_s >= 1.0;
        let dereverb = self.rt60_s.filter(|r| *r >= LONG_RT60_S).map(|r| {
            // Mild, and a little more the longer the room: 30 % at 1 s, 70 % at 3 s.
            (30.0 + 20.0 * (r - LONG_RT60_S)).clamp(30.0, 70.0) as f32
        });
        SpeechClean {
            hum: self.hum.map(|(m, _)| m),
            denoise,
            dereverb,
            declick: self.channel_crackle || self.clicks_per_min >= CLICKS_PER_MIN,
            floor_db: self.noise_floor_db.clamp(-90.0, -20.0) as f32,
        }
    }
}

// ───────────────────────────── the analyzer ─────────────────────────────

/// Streaming analysis of a 16 kHz recording, one or two channels.
pub struct SpeechAnalyzer {
    channels: usize,
    frames: u64,
    // hum
    hum_buf: Vec<f32>,
    hum_win: Vec<f32>,
    /// Per valid block, per mains family (50, 60), per harmonic: line stood out.
    hum_hits: [[u32; HUM_HARMONICS]; 2],
    hum_blocks: u32,
    // levels
    lvl_acc: f64,
    lvl_n: usize,
    lvl_hist: Vec<u32>,
    // envelope for the decays
    env_acc: f64,
    env_n: usize,
    env: Vec<f32>,
    // channel levels
    peak: [f64; 2],
    sumsq: [f64; 2],
    // clicks
    clicks: [ClickDetector; 2],
}

const HIST_LO: f64 = -120.0;
/// Frames under this level are digital silence, not a noise floor, dBFS.
const DIGITAL_SILENCE_DB: f64 = -95.0;
const HIST_STEP: f64 = 0.5;
const HIST_BINS: usize = 241;
const LEVEL_WIN: usize = (ANALYSIS_RATE as usize) / 20; // 50 ms

impl SpeechAnalyzer {
    pub fn new(channels: usize) -> Self {
        let n = HUM_BLOCK;
        let win = (0..n)
            .map(|i| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / n as f64).cos())
                    as f32
            })
            .collect();
        Self {
            channels: channels.clamp(1, 2),
            frames: 0,
            hum_buf: Vec::with_capacity(n),
            hum_win: win,
            hum_hits: [[0; HUM_HARMONICS]; 2],
            hum_blocks: 0,
            lvl_acc: 0.0,
            lvl_n: 0,
            lvl_hist: vec![0; HIST_BINS],
            env_acc: 0.0,
            env_n: 0,
            env: Vec::new(),
            peak: [0.0; 2],
            sumsq: [0.0; 2],
            clicks: [ClickDetector::new(), ClickDetector::new()],
        }
    }

    /// Channels the analyzer expects per block.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Feed a planar block (`channels` slices of equal length).
    pub fn push(&mut self, block: &[Vec<f32>]) {
        let n = block.iter().map(Vec::len).min().unwrap_or(0);
        let ch = self.channels.min(block.len());
        if n == 0 || ch == 0 {
            return;
        }
        let env_win = (ENV_HOP_S * f64::from(ANALYSIS_RATE)) as usize;
        for i in 0..n {
            let mut mono = 0.0f64;
            for (c, b) in block.iter().take(ch).enumerate() {
                let v = f64::from(b[i]);
                mono += v;
                self.peak[c] = self.peak[c].max(v.abs());
                self.sumsq[c] += v * v;
            }
            let mono = mono / ch as f64;
            let sq = mono * mono;
            self.lvl_acc += sq;
            self.lvl_n += 1;
            if self.lvl_n == LEVEL_WIN {
                let db = ms_db(self.lvl_acc / LEVEL_WIN as f64);
                let bin =
                    (((db - HIST_LO) / HIST_STEP).round().max(0.0) as usize).min(HIST_BINS - 1);
                self.lvl_hist[bin] += 1;
                self.lvl_acc = 0.0;
                self.lvl_n = 0;
            }
            self.env_acc += sq;
            self.env_n += 1;
            if self.env_n == env_win {
                self.env.push(ms_db(self.env_acc / env_win as f64) as f32);
                self.env_acc = 0.0;
                self.env_n = 0;
            }
            self.hum_buf.push(mono as f32);
            if self.hum_buf.len() == HUM_BLOCK {
                self.hum_block();
                self.hum_buf.clear();
            }
        }
        for (det, b) in self.clicks.iter_mut().zip(block).take(ch) {
            det.push(&b[..n]);
        }
        self.frames += n as u64;
    }

    fn hum_block(&mut self) {
        let rms = ms_db(
            self.hum_buf
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                / HUM_BLOCK as f64,
        );
        if rms < HUM_SILENT_DB {
            return;
        }
        // 18 frequencies: two mains families x three harmonics x (line, low, high).
        let mut freqs = [0.0f64; 18];
        let mut k = 0;
        for f0 in [50.0, 60.0] {
            for h in 1..=HUM_HARMONICS {
                let f = f0 * h as f64;
                freqs[k] = f;
                freqs[k + 1] = f - HUM_SIDE_HZ;
                freqs[k + 2] = f + HUM_SIDE_HZ;
                k += 3;
            }
        }
        let sr = f64::from(ANALYSIS_RATE);
        let coef: Vec<f64> = freqs
            .iter()
            .map(|f| 2.0 * (2.0 * std::f64::consts::PI * f / sr).cos())
            .collect();
        let mut s1 = [0.0f64; 18];
        let mut s2 = [0.0f64; 18];
        for (x, w) in self.hum_buf.iter().zip(&self.hum_win) {
            let v = f64::from(*x) * f64::from(*w);
            for j in 0..18 {
                let s0 = v + coef[j] * s1[j] - s2[j];
                s2[j] = s1[j];
                s1[j] = s0;
            }
        }
        // Power at each frequency; amplitude of a sine = 2*sqrt(P)/sum(w), sum(w) = N/2.
        let power: Vec<f64> = (0..18)
            .map(|j| (s1[j] * s1[j] + s2[j] * s2[j] - coef[j] * s1[j] * s2[j]).max(0.0))
            .collect();
        let amp_db = |p: f64| 20.0 * (4.0 * p.sqrt() / HUM_BLOCK as f64).max(1e-12).log10();
        self.hum_blocks += 1;
        for fam in 0..2 {
            for h in 0..HUM_HARMONICS {
                let j = (fam * HUM_HARMONICS + h) * 3;
                let side = 0.5 * (power[j + 1] + power[j + 2]);
                let ratio = 10.0 * (power[j].max(1e-30) / side.max(1e-30)).log10();
                if ratio >= HUM_RATIO_DB && amp_db(power[j]) >= HUM_MIN_LEVEL_DB {
                    self.hum_hits[fam][h] += 1;
                }
            }
        }
    }

    /// Percentile `p` of the 50 ms levels, over the frames that are not digital
    /// silence (under [`DIGITAL_SILENCE_DB`]): a recording cut from a longer
    /// one has gaps of exact zeros that say nothing about the room.
    fn hist_percentile(&self, p: f64) -> f64 {
        let from = (((DIGITAL_SILENCE_DB - HIST_LO) / HIST_STEP).ceil()) as usize;
        let total: u64 = self.lvl_hist[from..].iter().map(|v| u64::from(*v)).sum();
        if total == 0 {
            return HIST_LO;
        }
        let target = (p * total as f64).ceil().max(1.0) as u64;
        let mut acc = 0u64;
        for (i, c) in self.lvl_hist.iter().enumerate().skip(from) {
            acc += u64::from(*c);
            if acc >= target {
                return HIST_LO + i as f64 * HIST_STEP;
            }
        }
        0.0
    }

    /// Close the analysis.
    pub fn finish(self) -> SpeechAnalysis {
        let duration_s = self.frames as f64 / f64::from(ANALYSIS_RATE);
        let noise_floor_db = self.hist_percentile(0.05);
        let speech_db = self.hist_percentile(0.95);

        let hum = if self.hum_blocks >= 1 {
            let mut best: Option<(Mains, f64)> = None;
            for (fam, mains) in [(0, Mains::Hz50), (1, Mains::Hz60)] {
                let top = self.hum_hits[fam].iter().copied().max().unwrap_or(0);
                let frac = f64::from(top) / f64::from(self.hum_blocks);
                if frac >= HUM_BLOCK_FRACTION && best.is_none_or(|(_, b)| frac > b) {
                    best = Some((mains, frac));
                }
            }
            best
        } else {
            None
        };

        let rt60_s = estimate_rt60(&self.env);

        let secs = duration_s.max(1.0);
        let clicks_per_min = self
            .clicks
            .iter()
            .take(self.channels)
            .map(|c| c.count() as f64)
            .map(|n| {
                if n < MIN_CLICKS as f64 {
                    0.0
                } else {
                    n * 60.0 / secs
                }
            })
            .fold(0.0, f64::max);

        let channel_crackle = if self.channels == 2 && self.frames > 0 {
            let n = self.frames as f64;
            let db = |v: f64| 20.0 * v.max(1e-9).log10();
            channel_crackles(ChannelLevelsDb {
                peak_left_db: db(self.peak[0]),
                peak_right_db: db(self.peak[1]),
                rms_left_db: Some(db((self.sumsq[0] / n).sqrt())),
                rms_right_db: Some(db((self.sumsq[1] / n).sqrt())),
            })
        } else {
            false
        };

        SpeechAnalysis {
            duration_s,
            noise_floor_db,
            speech_db,
            hum,
            rt60_s,
            clicks_per_min,
            channel_crackle,
        }
    }
}

/// Read a raw f32le stream of `fmt` (16 kHz, 1 or 2 channels) through the analyzer.
pub fn analyze_stream<R: std::io::Read + ?Sized>(
    r: &mut R,
    fmt: PcmFormat,
    mut progress: impl FnMut(u64),
) -> std::io::Result<SpeechAnalysis> {
    let mut a = SpeechAnalyzer::new(fmt.channels);
    let mut done = 0u64;
    for_each_block(r, a.channels(), |block| {
        done += block.first().map_or(0, Vec::len) as u64;
        a.push(block);
        progress(done);
        Ok(())
    })?;
    Ok(a.finish())
}

/// Mean square to dB, floored so digital silence is a number.
fn ms_db(ms: f64) -> f64 {
    (10.0 * ms.max(1e-12).log10()).max(HIST_LO)
}

/// RT60 from the decays in a 20 ms level envelope (dB): the time speech takes
/// to fall from 5 to 20 dB under a peak, times four. A room cannot let a sound
/// die faster than its own decay, but the speaker can always stop slower, so
/// the *fast* tenth of the events is the room; the slow ones are phrasing.
fn estimate_rt60(env_raw: &[f32]) -> Option<f64> {
    if env_raw.len() < 100 {
        return None;
    }
    // 3-point mean for a steadier slope.
    let env: Vec<f32> = (0..env_raw.len())
        .map(|i| {
            let a = env_raw[i.saturating_sub(1)];
            let c = env_raw[(i + 1).min(env_raw.len() - 1)];
            (a + env_raw[i] + c) / 3.0
        })
        .collect();
    let mut sorted = env.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p5 = sorted[(sorted.len() as f64 * 0.05) as usize];
    let loud = sorted[(sorted.len() as f64 * 0.95) as usize];
    // In a long room the tails fill the pauses, and the 5th percentile climbs
    // with them (measured: -58 dB became -44 dB with an RT60 of 1.8 s). The
    // floor is therefore never taken higher than 45 dB under the loud speech.
    let floor = p5.min(loud - 45.0);
    const SPAN: usize = 3; // a peak is the maximum within +-60 ms
    const MAX_FRAMES: usize = 150; // and the fall must finish within 3 s
    let mut rts: Vec<f64> = Vec::new();
    let mut i = SPAN;
    while i + 1 < env.len() {
        let peak = env[i];
        let lo = i.saturating_sub(SPAN);
        let hi = (i + SPAN).min(env.len() - 1);
        let is_peak = env[lo..=hi].iter().all(|v| *v <= peak);
        if !is_peak || peak < (floor + 35.0).max(loud - 25.0) {
            i += 1;
            continue;
        }
        let (mut t5, mut tm, mut t20) = (None, None, None);
        let mut lowest = peak;
        let mut j = i + 1;
        while j < env.len() && j - i <= MAX_FRAMES {
            let v = env[j];
            if v > lowest + 3.0 {
                break; // the next syllable starts: not a free decay
            }
            lowest = lowest.min(v);
            if t5.is_none() && v <= peak - 5.0 {
                t5 = Some(j);
            }
            if tm.is_none() && v <= peak - 12.5 {
                tm = Some(j);
            }
            if v <= peak - 20.0 {
                t20 = Some(j);
                break;
            }
            j += 1;
        }
        if let (Some(a), Some(m), Some(b)) = (t5, tm, t20) {
            // The fall must be straight (a line in dB): the noise floor, or the
            // next sound, bends the end of it, and the bend reads as a slow room.
            let (t1, t2) = ((m - a) as f64, (b - m) as f64);
            if env[b] >= floor + 6.0 && b > a && t2 <= 2.0 * t1 + 2.0 {
                rts.push(4.0 * (b - a) as f64 * ENV_HOP_S);
            }
            i = b; // one event per fall
        }
        i += 1;
    }
    if rts.len() < MIN_DECAY_EVENTS {
        return None;
    }
    rts.sort_by(f64::total_cmp);
    Some(rts[rts.len() / 10])
}

/// Counts isolated spikes: a sample far over the signal around it (not just
/// over the local second difference, which voiced speech's glottal pulses and
/// sibilants also exceed), with quiet neighbourhoods on both sides. A speech
/// onset is loud on the right of its first sample; a click is not.
struct ClickDetector {
    /// The signal.
    x: Vec<f32>,
    /// |second difference| of `x`.
    d: Vec<f32>,
    x1: f32,
    x2: f32,
    /// Index (in `x`) of the next sample to judge.
    next: usize,
    last_hit: Option<u64>,
    base: u64,
    count: usize,
}

/// Samples either side that have to be quiet.
const CLICK_SIDE: usize = 32;
/// ...but for this many next to the spike (its own ringing).
const CLICK_NEAR: usize = 8;
/// Samples either side the signal's level is read over.
const CLICK_CONTEXT: usize = 256;
/// A spike's height must be this many times the local RMS.
const CLICK_REL: f32 = 6.0;
/// ...and at least this high, absolute (-26 dBFS).
const CLICK_MIN_ABS: f32 = 0.05;
const CLICK_REFRACTORY: u64 = 64;

impl ClickDetector {
    fn new() -> Self {
        Self {
            x: Vec::new(),
            d: Vec::new(),
            x1: 0.0,
            x2: 0.0,
            next: 0,
            last_hit: None,
            base: 0,
            count: 0,
        }
    }

    fn count(&self) -> usize {
        self.count
    }

    fn push(&mut self, x: &[f32]) {
        for v in x {
            self.d.push((v - 2.0 * self.x1 + self.x2).abs());
            self.x2 = self.x1;
            self.x1 = *v;
            self.x.push(*v);
        }
        let end = self.x.len().saturating_sub(CLICK_CONTEXT);
        while self.next < end {
            let i = self.next;
            self.next += 1;
            let a = self.d[i];
            // The spike sits one sample before the second difference peaks.
            if i < CLICK_CONTEXT + 1 || a < 2.0 * CLICK_MIN_ABS {
                continue;
            }
            let c = i - 1; // the spike's own sample
            let h = self.x[c].abs();
            if h < CLICK_MIN_ABS || self.d[i - 4..=i + 4].iter().any(|v| *v > a) {
                continue;
            }
            let mut e = 0.0f32;
            let mut n = 0usize;
            for (k, v) in self.x[c - CLICK_CONTEXT..=c + CLICK_CONTEXT]
                .iter()
                .enumerate()
            {
                if k.abs_diff(CLICK_CONTEXT) > 4 {
                    e += v * v;
                    n += 1;
                }
            }
            let rms = (e / n as f32).sqrt();
            if h < CLICK_REL * rms {
                continue;
            }
            let quiet = |range: std::ops::Range<usize>| {
                self.d[range.clone()].iter().sum::<f32>() / range.len() as f32 <= 0.2 * a
            };
            if !quiet(i - CLICK_SIDE..i - CLICK_NEAR) || !quiet(i + CLICK_NEAR..i + CLICK_SIDE) {
                continue;
            }
            let at = self.base + i as u64;
            if self.last_hit.is_none_or(|h| at - h >= CLICK_REFRACTORY) {
                self.count += 1;
            }
            self.last_hit = Some(at);
        }
        let keep_from = self.next.saturating_sub(CLICK_CONTEXT + 1);
        if keep_from > 65_536 {
            self.x.drain(..keep_from);
            self.d.drain(..keep_from);
            self.next -= keep_from;
            self.base += keep_from as u64;
        }
    }
}

// ───────────────────────────── the speech gate ─────────────────────────────

/// Decides, sample by sample, how much of a processed signal to let through:
/// 1 while the input is speech-like or sits at the measured floor, 0 otherwise
/// (module docs). Pure and deterministic; the chain's `SpeechGated` stage runs
/// one next to the effect it guards.
pub struct SpeechGate {
    hop: usize,
    acc: f64,
    n: usize,
    levels: VecDeque<f32>,
    floor_db: f32,
    hold_left: usize,
    hold_hops: usize,
    target: f32,
    g: f32,
    a_up: f32,
    a_down: f32,
}

impl SpeechGate {
    pub fn new(sample_rate: f32, floor_db: f32) -> Self {
        let hop = ((sample_rate * 0.010) as usize).max(1);
        let coef = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sample_rate)).exp();
        Self {
            hop,
            acc: 0.0,
            n: 0,
            levels: VecDeque::with_capacity(64),
            floor_db,
            hold_left: 0,
            hold_hops: 60,
            target: 1.0, // until it has heard anything, treat as speech: the start of a file
            g: 1.0,
            a_up: coef(30.0),
            a_down: coef(200.0),
        }
    }

    /// The share of the processed signal to let through right now, 0..=1.
    pub fn amount(&self) -> f32 {
        self.g
    }

    /// Advance by one block of the (mono) input and write the amount per sample.
    pub fn process(&mut self, mono: &[f32], out: &mut [f32]) {
        for (x, o) in mono.iter().zip(out.iter_mut()) {
            self.acc += f64::from(*x) * f64::from(*x);
            self.n += 1;
            if self.n == self.hop {
                let db = (10.0 * (self.acc / self.hop as f64).max(1e-12).log10()) as f32;
                self.acc = 0.0;
                self.n = 0;
                self.hop_done(db);
            }
            let a = if self.target > self.g {
                self.a_up
            } else {
                self.a_down
            };
            self.g += (self.target - self.g) * a;
            *o = self.g;
        }
    }

    fn hop_done(&mut self, db: f32) {
        if self.levels.len() == 60 {
            self.levels.pop_front();
        }
        self.levels.push_back(db);
        if self.levels.len() < 20 {
            return; // 0.2 s to form an opinion
        }
        let mut v: Vec<f32> = self.levels.iter().copied().collect();
        v.sort_by(f32::total_cmp);
        let p10 = v[v.len() / 10];
        let p90 = v[v.len() * 9 / 10];
        let speech_like =
            p90 - p10 >= GATE_DEPTH_DB && p90 >= self.floor_db + GATE_SPEECH_ABOVE_FLOOR_DB;
        let at_floor =
            db <= self.floor_db + GATE_NEAR_FLOOR_DB && p90 <= self.floor_db + GATE_QUIET_SPAN_DB;
        if speech_like {
            self.hold_left = self.hold_hops;
        } else {
            self.hold_left = self.hold_left.saturating_sub(1);
        }
        self.target = if speech_like || at_floor || self.hold_left > 0 {
            1.0
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests;
