//! One read when a recording is opened (B4a, `docs/LYDMOTOR.md` fase 4).
//!
//! Until B4a a recording was decoded four times between «open» and «ready»: the waveform
//! (8 kHz mono), the sermon search (16 kHz mono), the channel diagnosis (`astats`, a whole
//! round to a null sink) and «Tale»'s analysis (16 kHz, one or two channels). The shell now
//! asks ffmpeg for ONE stream in the recording's own sample rate and its own channels (raw f32,
//! no filter, no resampling there) and hands every block to [`OpenFold`], which feeds all of
//! them:
//!
//! | Consumer | Fed from | Was |
//! | -------- | -------- | --- |
//! | waveform, 100 peaks a second | the mono mix at 8 kHz ([`Resampler`] from the 16 kHz stream) | `-ac 1 -ar 8000` |
//! | sermon search ([`crate::detect`]) | the mono mix at 16 kHz, kept only when asked for | `-ac 1 -ar 16000` |
//! | «Tale» ([`crate::tale`]) | the channels at 16 kHz | `-ar 16000 -ac 2` |
//! | channel diagnosis and noise floor | the channels as they are, at the file's rate ([`ChannelMeter`]) | `-af astats` |
//!
//! **The resampling is ours.** [`Resampler`] is a streaming windowed-sinc (Kaiser) converter
//! with an exact rational ratio: output sample *n* sits at input position *n·from/to*, so the
//! result is the same however the stream is cut into blocks, and there is no delay to
//! compensate. It is what the 16 kHz analyses read, so a decision they take must hold on its
//! output; the tests here and in the shell compare against ffmpeg's own resampling.
//!
//! **What `astats` measured, and what replaces it.** [`ChannelMeter`] reproduces the three
//! numbers the app read from `astats=metadata=0`, defined from `libavfilter/af_astats.c`:
//!
//! * `Peak level dB`: the largest absolute sample;
//! * `RMS level dB`: the root of the mean square over the whole file;
//! * `Noise floor dB`: the smallest value, over the file, of the **largest absolute sample in
//!   a sliding window of 50 ms** (`length` 0.05 s, counted once the window is full). The
//!   «Overall» figure is the largest of the channels' floors. Not a percentile and not an RMS:
//!   a held tone gives its own peak back, which is why a pure sine's floor equals its peak.
//!
//! Two channels at most: a source with more is down-mixed by ffmpeg to stereo before it gets
//! here (the diagnosis only ever read channels 1 and 2).
//!
//! No process, no clock, no file here: the shell owns the pipe.

use std::collections::VecDeque;

use crate::editor::{PeakAccumulator, PEAKS_BUCKET_SAMPLES, PEAKS_SAMPLE_RATE};
use crate::levels::{ChannelLevels, SILENCE_FLOOR_DB};
use crate::tale::{SpeechAnalysis, SpeechAnalyzer, ANALYSIS_RATE};

// ───────────────────────────── the resampler ─────────────────────────────

/// Zero crossings of the sinc on each side of the centre, counted at the OUTPUT rate. More is
/// a steeper transition band and a dearer filter: 10 puts the stop band 80+ dB down at the
/// output's Nyquist for the price of 20 output-rate taps.
const ZERO_CROSSINGS: f64 = 10.0;
/// Where the pass band ends, as a share of the output's Nyquist frequency.
const CUTOFF: f64 = 0.96;
/// Kaiser β: about -90 dB side lobes.
const KAISER_BETA: f64 = 9.0;
/// Taps are kept in whole multiples of this so the dot product runs in fixed-width lanes.
const LANES: usize = 8;

/// A streaming sample-rate converter for one channel (module docs).
///
/// Feed any number of blocks to [`process`](Self::process), then call
/// [`finish`](Self::finish) once to flush the tail.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Output samples per period of the reduced ratio.
    phases: u64,
    /// Input samples per period of the reduced ratio.
    step: u64,
    /// Taps per phase, padded to a multiple of [`LANES`].
    taps: usize,
    /// Taps before the centre (the window starts this far back from the input position).
    lead: usize,
    /// `phases` rows of `taps` coefficients, each row summing to 1.
    table: Vec<f32>,
    /// Input still needed by the coming outputs. `buf[0]` is input sample `base`.
    buf: Vec<f32>,
    base: i64,
    /// The next output sample to make.
    next: u64,
    /// Input samples seen.
    seen: u64,
    passthrough: bool,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

impl Resampler {
    /// Convert from `from` Hz to `to` Hz. Rates of zero are treated as 1 Hz.
    pub fn new(from: u32, to: u32) -> Self {
        let (from, to) = (u64::from(from.max(1)), u64::from(to.max(1)));
        let g = gcd(from, to);
        let (step, phases) = (from / g, to / g);
        if from == to {
            return Self {
                phases: 1,
                step: 1,
                taps: 0,
                lead: 0,
                table: Vec::new(),
                buf: Vec::new(),
                base: 0,
                next: 0,
                seen: 0,
                passthrough: true,
            };
        }
        // Everything below is in input samples. Going down, the filter's cut-off is the
        // OUTPUT's Nyquist (scaled by `CUTOFF`); going up, the input's.
        let ratio = (to as f64 / from as f64).min(1.0);
        let scale = ratio * CUTOFF;
        let half_width = ZERO_CROSSINGS / scale;
        let reach = half_width.ceil() as usize;
        let used = 2 * reach;
        let taps = used.next_multiple_of(LANES);
        let i0 = bessel_i0(KAISER_BETA);
        let mut table = vec![0.0f32; phases as usize * taps];
        for p in 0..phases as usize {
            let frac = p as f64 / phases as f64;
            let row = &mut table[p * taps..(p + 1) * taps];
            let mut sum = 0.0;
            let mut coeffs = vec![0.0f64; used];
            for (j, c) in coeffs.iter_mut().enumerate() {
                // input sample (centre - reach + 1 + j), centre at `frac` past its floor
                let x = j as f64 - reach as f64 + 1.0 - frac;
                let u = x / half_width;
                if u.abs() < 1.0 {
                    let w = bessel_i0(KAISER_BETA * (1.0 - u * u).sqrt()) / i0;
                    *c = scale * sinc(scale * x) * w;
                }
                sum += *c;
            }
            for (r, c) in row.iter_mut().zip(&coeffs) {
                *r = (*c / sum) as f32;
            }
        }
        let lead = reach - 1;
        Self {
            phases,
            step,
            taps,
            lead,
            table,
            buf: vec![0.0; lead],
            base: -(lead as i64),
            next: 0,
            seen: 0,
            passthrough: false,
        }
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        let mut acc = [0.0f32; LANES];
        for (ca, cb) in a.chunks_exact(LANES).zip(b.chunks_exact(LANES)) {
            for ((a, x), y) in acc.iter_mut().zip(ca).zip(cb) {
                *a += x * y;
            }
        }
        acc.iter().sum()
    }

    /// Make every output the input so far allows. `flush` pretends the input goes on as silence.
    fn drain(&mut self, out: &mut Vec<f32>, total: Option<u64>) {
        loop {
            if let Some(t) = total {
                if self.next >= t {
                    break;
                }
            }
            let pos = self.next * self.step;
            let centre = (pos / self.phases) as i64;
            let phase = (pos % self.phases) as usize;
            let start = centre - self.lead as i64 - self.base;
            let start = start.max(0) as usize;
            if start + self.taps > self.buf.len() {
                break;
            }
            let row = &self.table[phase * self.taps..(phase + 1) * self.taps];
            out.push(Self::dot(&self.buf[start..start + self.taps], row));
            self.next += 1;
        }
        // Forget what the next output no longer reads.
        let pos = self.next * self.step;
        let centre = (pos / self.phases) as i64;
        let keep_from = (centre - self.lead as i64 - self.base).max(0) as usize;
        let drop = keep_from.min(self.buf.len());
        if drop > 0 {
            self.buf.drain(..drop);
            self.base += drop as i64;
        }
    }

    /// Convert one block, appending the output samples it allows to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.seen += input.len() as u64;
        self.buf.extend_from_slice(input);
        self.drain(out, None);
    }

    /// Flush: the last outputs, as if the input went on as silence. Call once, after the last
    /// block. The total is exactly `floor(input samples · to / from)`.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.passthrough {
            return;
        }
        let total = self.seen * self.phases / self.step;
        self.buf.resize(self.buf.len() + self.taps, 0.0);
        self.drain(out, Some(total));
    }
}

// ───────────────────────────── the channel meter ─────────────────────────────

/// What `astats` called `length`: the window of the noise floor, seconds.
const FLOOR_WINDOW_S: f64 = 0.05;

/// One channel's peak, mean square and noise floor (module docs).
#[derive(Debug, Clone)]
pub struct ChannelMeter {
    window: u64,
    count: u64,
    peak: f32,
    sumsq: f64,
    /// Indices and values of the window's running maxima, largest first.
    queue: VecDeque<(u64, f32)>,
    floor: f32,
}

impl ChannelMeter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            // astats: `FFMAX(time_constant * rate + .5, 1)`
            window: ((FLOOR_WINDOW_S * f64::from(sample_rate) + 0.5) as u64).max(1),
            count: 0,
            peak: 0.0,
            sumsq: 0.0,
            queue: VecDeque::new(),
            floor: f32::INFINITY,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &x in samples {
            // A NaN or infinite sample has no level; call it silence rather than poison the sums.
            let a = if x.is_finite() { x.abs() } else { 0.0 };
            if a > self.peak {
                self.peak = a;
            }
            self.sumsq += f64::from(a) * f64::from(a);
            let i = self.count;
            while self.queue.back().is_some_and(|&(_, v)| v <= a) {
                self.queue.pop_back();
            }
            self.queue.push_back((i, a));
            while self
                .queue
                .front()
                .is_some_and(|&(j, _)| j + self.window <= i)
            {
                self.queue.pop_front();
            }
            self.count += 1;
            if self.count >= self.window {
                if let Some(&(_, top)) = self.queue.front() {
                    if top < self.floor {
                        self.floor = top;
                    }
                }
            }
        }
    }

    /// The largest absolute sample, linear.
    pub fn peak(&self) -> f64 {
        f64::from(self.peak)
    }

    /// Root mean square over everything pushed, linear.
    pub fn rms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            (self.sumsq / self.count as f64).sqrt()
        }
    }

    /// The noise floor, linear; `None` when the recording was shorter than one window.
    pub fn noise_floor(&self) -> Option<f64> {
        self.floor.is_finite().then(|| f64::from(self.floor))
    }
}

fn to_db(linear: f64) -> f64 {
    if linear > 0.0 && linear.is_finite() {
        20.0 * linear.log10()
    } else {
        SILENCE_FLOOR_DB
    }
}

/// What the channel meter found, as the diagnosis and the one-click vocal chain read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceLevels {
    /// Peak and RMS of channel 1 and 2. `None` for a stream with no samples at all.
    pub levels: Option<ChannelLevels>,
    /// The noise floor, dBFS: the loudest channel's, as `astats` rolled it up. `None` when it is
    /// silent (-inf) or the recording is shorter than 50 ms.
    pub noise_floor_db: Option<f64>,
    /// Frames seen.
    pub frames: u64,
}

/// [`ChannelMeter`] for one or two channels.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    channels: Vec<ChannelMeter>,
}

impl LevelMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self {
            channels: (0..channels.clamp(1, 2))
                .map(|_| ChannelMeter::new(sample_rate))
                .collect(),
        }
    }

    /// Feed a planar block (one slice per channel).
    pub fn push(&mut self, block: &[Vec<f32>]) {
        for (m, ch) in self.channels.iter_mut().zip(block) {
            m.push(ch);
        }
    }

    pub fn finish(&self) -> SourceLevels {
        let frames = self.channels.first().map_or(0, |m| m.count);
        if frames == 0 {
            return SourceLevels {
                levels: None,
                noise_floor_db: None,
                frames: 0,
            };
        }
        let left = &self.channels[0];
        let right = self.channels.get(1);
        let levels = ChannelLevels {
            peak_db_left: to_db(left.peak()),
            peak_db_right: right.map(|m| to_db(m.peak())),
            rms_db_left: Some(to_db(left.rms())),
            rms_db_right: right.map(|m| to_db(m.rms())),
        };
        // astats rolls the floors up with `FFMAX(0, …)`: the loudest channel's floor. A
        // channel too short for a window has none, and does not count.
        let floor = self
            .channels
            .iter()
            .filter_map(ChannelMeter::noise_floor)
            .fold(0.0, f64::max);
        let noise_floor_db = (floor > 0.0).then(|| to_db(floor));
        SourceLevels {
            levels: Some(levels),
            noise_floor_db,
            frames,
        }
    }
}

// ───────────────────────────── the 16 kHz half ─────────────────────────────

/// Everything the 16 kHz half found.
#[derive(Debug, Clone)]
pub struct Read16 {
    pub analysis: SpeechAnalysis,
    /// Max-abs per 10 ms of the 8 kHz mono mix: [`crate::editor::PEAKS_PER_SEC`] a second.
    pub peaks: Vec<f32>,
    /// The mono mix at 16 kHz, when it was asked for.
    pub mono16: Option<Vec<f32>>,
    /// Frames out, at 16 kHz.
    pub frames16: u64,
}

/// Everything that reads the recording at 16 kHz or below: «Tale»'s analysis, the waveform
/// peaks and (optionally) the PCM the sermon search works on.
pub struct Pass16 {
    resamplers: Vec<Resampler>,
    analyzer: SpeechAnalyzer,
    peak_rs: Resampler,
    peaks: PeakAccumulator,
    mono16: Option<Vec<f32>>,
    frames16: u64,
    planar: Vec<Vec<f32>>,
    mono: Vec<f32>,
    eight: Vec<f32>,
}

impl Pass16 {
    /// `keep_mono16`: collect the 16 kHz mono mix (the sermon search needs it, and it is the
    /// one thing here that grows with the recording: 345 MB for 90 minutes).
    pub fn new(sample_rate: u32, channels: usize, keep_mono16: bool) -> Self {
        let channels = channels.clamp(1, 2);
        Self {
            resamplers: (0..channels)
                .map(|_| Resampler::new(sample_rate, ANALYSIS_RATE))
                .collect(),
            analyzer: SpeechAnalyzer::new(channels),
            peak_rs: Resampler::new(ANALYSIS_RATE, PEAKS_SAMPLE_RATE),
            peaks: PeakAccumulator::new(PEAKS_BUCKET_SAMPLES),
            mono16: keep_mono16.then(Vec::new),
            frames16: 0,
            planar: vec![Vec::new(); channels],
            mono: Vec::new(),
            eight: Vec::new(),
        }
    }

    /// The 16 kHz channels in `self.planar` go to every consumer.
    fn feed(&mut self) {
        let n = self.planar.first().map_or(0, Vec::len);
        if n == 0 {
            return;
        }
        self.frames16 += n as u64;
        self.analyzer.push(&self.planar);
        self.mono.clear();
        if let [l, r] = self.planar.as_slice() {
            self.mono
                .extend(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
        } else {
            self.mono.extend_from_slice(&self.planar[0]);
        }
        if let Some(keep) = self.mono16.as_mut() {
            keep.extend_from_slice(&self.mono);
        }
        self.eight.clear();
        self.peak_rs.process(&self.mono, &mut self.eight);
        for &x in &self.eight {
            self.peaks.push_f32(x);
        }
    }

    /// Feed a planar block at the recording's own rate.
    pub fn push(&mut self, block: &[Vec<f32>]) {
        for ((r, ch), out) in self.resamplers.iter_mut().zip(block).zip(&mut self.planar) {
            out.clear();
            r.process(ch, out);
        }
        self.feed();
    }

    pub fn finish(mut self) -> Read16 {
        for (r, out) in self.resamplers.iter_mut().zip(&mut self.planar) {
            out.clear();
            r.finish(out);
        }
        self.feed();
        self.eight.clear();
        self.peak_rs.finish(&mut self.eight);
        for &x in &self.eight {
            self.peaks.push_f32(x);
        }
        Read16 {
            analysis: self.analyzer.finish(),
            peaks: self.peaks.finish(),
            mono16: self.mono16,
            frames16: self.frames16,
        }
    }
}

// ───────────────────────────── all of it ─────────────────────────────

/// What one read of a recording found.
#[derive(Debug, Clone)]
pub struct OpenRead {
    pub levels: SourceLevels,
    pub read16: Read16,
}

/// The whole fan-out in one value, for a caller that runs it on one thread.
pub struct OpenFold {
    meter: LevelMeter,
    pass16: Pass16,
}

impl OpenFold {
    pub fn new(sample_rate: u32, channels: usize, keep_mono16: bool) -> Self {
        Self {
            meter: LevelMeter::new(sample_rate, channels),
            pass16: Pass16::new(sample_rate, channels, keep_mono16),
        }
    }

    /// Feed a planar block at the recording's own rate (one slice per channel).
    pub fn push(&mut self, block: &[Vec<f32>]) {
        self.meter.push(block);
        self.pass16.push(block);
    }

    pub fn finish(self) -> OpenRead {
        OpenRead {
            levels: self.meter.finish(),
            read16: self.pass16.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, rate: u32, secs: f64, amp: f64) -> Vec<f32> {
        (0..(secs * f64::from(rate)) as usize)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin()) as f32
            })
            .collect()
    }

    fn convert(from: u32, to: u32, x: &[f32], chunk: usize) -> Vec<f32> {
        let mut r = Resampler::new(from, to);
        let mut out = Vec::new();
        for c in x.chunks(chunk) {
            r.process(c, &mut out);
        }
        r.finish(&mut out);
        out
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn the_output_length_is_exact_for_the_rates_a_recording_has() {
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (96_000, 16_000),
            (32_000, 16_000),
            (22_050, 16_000),
            (8_000, 16_000),
            (16_000, 8_000),
            (44_100, 8_000),
        ] {
            let x = vec![0.1f32; from as usize * 3 + 7];
            let y = convert(from, to, &x, 4_800);
            assert_eq!(
                y.len() as u64,
                x.len() as u64 * u64::from(to) / u64::from(from),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn the_result_does_not_depend_on_how_the_stream_is_cut_up() {
        let x = sine(1_000.0, 44_100, 2.0, 0.5);
        let whole = convert(44_100, 16_000, &x, x.len());
        for chunk in [1usize, 7, 441, 4_410, 48_000] {
            assert_eq!(convert(44_100, 16_000, &x, chunk), whole, "chunk {chunk}");
        }
    }

    #[test]
    fn a_tone_in_the_band_keeps_its_level_and_has_no_delay() {
        for from in [48_000u32, 44_100, 96_000] {
            let x = sine(1_000.0, from, 2.0, 0.5);
            let y = convert(from, 16_000, &x, 9_600);
            let mid = &y[4_000..y.len() - 4_000];
            let want = 0.5 / 2f64.sqrt();
            assert!((rms(mid) - want).abs() < 0.002, "{from}: rms {}", rms(mid));
            // Phase: the output at n is the sine at the same instant (zero-phase filter).
            let err = mid
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let t = (i + 4_000) as f64 / 16_000.0;
                    (f64::from(*v) - 0.5 * (2.0 * std::f64::consts::PI * 1_000.0 * t).sin()).abs()
                })
                .fold(0.0, f64::max);
            assert!(err < 0.005, "{from}: worst sample error {err}");
        }
    }

    #[test]
    fn what_would_fold_back_is_not_let_through() {
        // 12 kHz at 48 kHz would alias to 4 kHz at 16 kHz. It has to be 60+ dB down.
        for from in [48_000u32, 44_100] {
            let x = sine(12_000.0, from, 2.0, 0.5);
            let y = convert(from, 16_000, &x, 9_600);
            let mid = &y[4_000..y.len() - 4_000];
            let db = 20.0 * (rms(mid) / (0.5 / 2f64.sqrt())).log10();
            assert!(db < -60.0, "{from}: 12 kHz came through at {db:.1} dB");
        }
    }

    #[test]
    fn going_up_keeps_the_tone_and_the_length() {
        let x = sine(500.0, 8_000, 2.0, 0.5);
        let y = convert(8_000, 16_000, &x, 800);
        assert_eq!(y.len(), x.len() * 2);
        let mid = &y[2_000..y.len() - 2_000];
        assert!((rms(mid) - 0.5 / 2f64.sqrt()).abs() < 0.002);
    }

    #[test]
    fn the_same_rate_passes_straight_through() {
        let x = sine(300.0, 16_000, 1.0, 0.3);
        assert_eq!(convert(16_000, 16_000, &x, 333), x);
    }

    #[test]
    fn a_constant_stays_a_constant() {
        // Each row sums to 1: DC gain is exactly unity, whatever the phase.
        let x = vec![0.25f32; 44_100];
        let y = convert(44_100, 16_000, &x, 1_000);
        for v in &y[40..y.len() - 40] {
            assert!((v - 0.25).abs() < 1e-5, "{v}");
        }
    }

    // ── the meter ──

    #[test]
    fn peak_and_rms_are_the_plain_numbers() {
        let mut m = ChannelMeter::new(1_000);
        m.push(&[0.5, -1.0, 0.5, -0.5]);
        assert_eq!(m.peak(), 1.0);
        assert!((m.rms() - (0.4375f64).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn the_floor_is_the_smallest_window_maximum() {
        // 100 Hz → 5-sample window. A burst, 20 quiet samples at 0.1, a burst again.
        let mut m = ChannelMeter::new(100);
        let mut x = vec![0.9f32; 10];
        x.extend(std::iter::repeat_n(0.1, 20));
        x.extend(vec![0.8f32; 10]);
        m.push(&x);
        assert_eq!(m.noise_floor(), Some(f64::from(0.1f32)));
        // A short bump in the quiet part lifts only the windows that contain it.
        let mut m = ChannelMeter::new(100);
        let mut y = x.clone();
        y[20] = 0.5;
        m.push(&y);
        assert_eq!(m.noise_floor(), Some(f64::from(0.1f32)));
    }

    #[test]
    fn a_held_tone_has_its_own_peak_as_floor() {
        // astats: the floor of a pure sine equals its peak (the doc example in `levels`).
        let mut m = ChannelMeter::new(48_000);
        m.push(&sine(440.0, 48_000, 1.0, 0.25));
        let floor = m.noise_floor().unwrap();
        assert!((floor - m.peak()).abs() < 0.003, "{floor} vs {}", m.peak());
    }

    #[test]
    fn shorter_than_one_window_has_no_floor() {
        let mut m = ChannelMeter::new(48_000);
        m.push(&vec![0.5; 2_000]);
        assert_eq!(m.noise_floor(), None);
    }

    #[test]
    fn window_maximum_matches_a_brute_force() {
        // A deterministic pseudo-random signal; the deque must equal the O(n·w) definition.
        let mut s = 12345u32;
        let x: Vec<f32> = (0..3_000)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.4
            })
            .collect();
        let w = 37usize;
        let mut m = ChannelMeter::new(740); // 0.05 s * 740 + .5 = 37
        assert_eq!(m.window as usize, w);
        m.push(&x);
        let brute = (w - 1..x.len())
            .map(|t| x[t + 1 - w..=t].iter().fold(0.0f32, |a, v| a.max(v.abs())))
            .fold(f32::INFINITY, f32::min);
        assert_eq!(m.noise_floor(), Some(f64::from(brute)));
    }

    #[test]
    fn levels_map_to_channels_one_and_two_and_a_dead_one_is_the_floor_value() {
        let mut lm = LevelMeter::new(48_000, 2);
        let l = sine(440.0, 48_000, 1.0, 0.25);
        let r = vec![0.0f32; l.len()];
        lm.push(&[l, r]);
        let s = lm.finish();
        let lv = s.levels.unwrap();
        assert!((lv.peak_db_left - 20.0 * 0.25f64.log10()).abs() < 0.01);
        assert_eq!(lv.peak_db_right, Some(SILENCE_FLOOR_DB));
        assert_eq!(lv.rms_db_right, Some(SILENCE_FLOOR_DB));
        assert!(
            s.noise_floor_db.is_some(),
            "the live channel's floor counts"
        );
        assert_eq!(s.frames, 48_000);
    }

    #[test]
    fn mono_has_no_right_channel_and_nothing_pushed_has_no_levels() {
        let mut lm = LevelMeter::new(48_000, 1);
        lm.push(&[vec![0.5; 4_800]]);
        let lv = lm.finish().levels.unwrap();
        assert_eq!(lv.peak_db_right, None);
        assert_eq!(lv.rms_db_right, None);
        assert_eq!(LevelMeter::new(48_000, 2).finish().levels, None);
    }

    // ── the fold ──

    #[test]
    fn the_peaks_come_at_a_hundred_a_second_with_the_tail_bucket() {
        let mut fold = OpenFold::new(48_000, 1, false);
        let x = sine(200.0, 48_000, 2.505, 0.5);
        for c in x.chunks(48_000) {
            fold.push(&[c.to_vec()]);
        }
        let out = fold.finish();
        // 2.505 s = 250 buckets of 10 ms and half of a 251st
        assert_eq!(out.read16.peaks.len(), 251);
        let body = &out.read16.peaks[5..245];
        assert!(body.iter().all(|p| (p - 0.5).abs() < 0.01), "{body:?}");
    }

    #[test]
    fn mono16_is_kept_only_when_asked_for_and_is_the_mix_of_the_channels() {
        let l = sine(300.0, 48_000, 1.0, 0.4);
        let r = vec![0.0f32; l.len()];
        let run = |keep: bool| {
            let mut fold = OpenFold::new(48_000, 2, keep);
            fold.push(&[l.clone(), r.clone()]);
            fold.finish()
        };
        assert!(run(false).read16.mono16.is_none());
        let kept = run(true).read16;
        let mono = kept.mono16.unwrap();
        assert_eq!(mono.len() as u64, kept.frames16);
        assert_eq!(mono.len(), 16_000);
        // L alone at 0.4, R silent: the mix is half of L
        assert!((rms(&mono[2_000..14_000]) - 0.2 / 2f64.sqrt()).abs() < 0.002);
    }
}
