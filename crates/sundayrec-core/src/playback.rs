//! The playback engine's pure half (B5a, `docs/LYDMOTOR.md` fase 5): the cut plan, its
//! constant-power joins, and the reader that turns a recording plus a list of kept segments into
//! one continuous programme.
//!
//! The shell (`src-tauri/src/playback/`) owns the ffmpeg process, the threads, the ring and the
//! sound card. Everything here is a function of its inputs: no process, no clock, no file. The
//! source is reached through [`PcmOpener`]/[`PcmStream`], so the tests run it on synthetic
//! signals held in memory, and the shell runs it on an ffmpeg pipe.
//!
//! ## Two timelines
//!
//! * **source** — the recording as it lies on disk, in frames at its own sample rate;
//! * **programme** — what is heard: the [`KeepSegment`]s laid end to end. Its length is the sum
//!   of the keeps; the crossfades do not shorten it.
//!
//! [`CutPlan::program_to_source`] maps the one onto the other. It switches from one keep to the
//! next at the MIDDLE of the crossfade, which is where the join is in programme time too: the
//! playhead never jumps backwards, and it lands on the next keep's start exactly at the join.
//!
//! ## The joins: constant power
//!
//! At each join the outgoing keep runs on past its end and the incoming keep starts before its
//! start, by [`CROSSFADE_SEC`]/2 each, under gains `cos(π/2·x)` and `sin(π/2·x)` (`x` runs 0 → 1
//! over the fade). `cos² + sin² = 1`, so two UNCORRELATED signals of equal power (two different
//! moments of a service) keep their power through the join: no dip, which is what a linear
//! crossfade gives (−3 dB in the middle) and what a fade-out/fade-in gives (silence). Two
//! correlated signals rise by up to +3 dB for the 20 ms; that is the textbook trade, and the
//! right one for cuts between different moments.
//!
//! The material the fade borrows is the cut itself (the outgoing keep runs on into the cut, the
//! incoming one starts inside it), so it always exists. The half-width is shrunk per join to half
//! of the shorter keep, so a fade never reaches past the other end of a short keep
//! ([`CutPlan::new`]); two keeps that touch (no cut between them) get no fade at all, because the
//! material is continuous already.
//!
//! **The export does not do this yet.** `editor::join_fades` gives each interior edge a 15 ms
//! fade OUT and fade IN (a dip to silence). Making the export read this plan is what makes
//! «what you hear is what you get» true for cuts too; it is a later step, not B5a.
//!
//! ## The end
//!
//! The last [`END_FADE_SEC`] of the programme is faded out, so stopping at the end does not
//! click. The START needs nothing here: the shell's audio callback fades in every time it starts
//! or lands after a seek.

use std::io;

use crate::editor::KeepSegment;

/// Length of a crossfade at a join, seconds (half before the join, half after).
pub const CROSSFADE_SEC: f64 = 0.020;
/// Fade-out at the end of the programme, seconds.
pub const END_FADE_SEC: f64 = 0.010;

/// The gains of a constant-power crossfade at position `x` in `[0, 1]`: `(outgoing, incoming)`.
/// `out² + in² = 1` for every `x`.
pub fn constant_power(x: f64) -> (f32, f32) {
    let a = std::f64::consts::FRAC_PI_2 * x.clamp(0.0, 1.0);
    (a.cos() as f32, a.sin() as f32)
}

/// One kept span, in source frames: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Keep {
    start: u64,
    end: u64,
}

impl Keep {
    fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// The kept segments of a recording, as a programme (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct CutPlan {
    rate: u32,
    source_frames: u64,
    keeps: Vec<Keep>,
    /// Programme frame where each keep starts.
    starts: Vec<u64>,
    /// Half-width of the crossfade at the join AFTER keep `k` (`keeps.len() - 1` entries).
    halves: Vec<u64>,
    total: u64,
    end_fade: u64,
}

/// What the programme is made of at one position, until `len` frames later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// One keep, read straight from `src`.
    Plain { src: u64, len: u64 },
    /// A join: `a_src` (the outgoing keep, fading out) and `b_src` (the incoming one, fading in)
    /// mixed; `t` frames into a fade `width` frames wide.
    Fade {
        a_src: u64,
        b_src: u64,
        t: u64,
        width: u64,
        len: u64,
    },
}

impl CutPlan {
    /// The plan for `keeps` (seconds, as [`crate::editor::build_keeps`] gives them) of a source
    /// `source_duration` seconds long at `rate` Hz. Keeps are clamped to the source, sorted, and
    /// empty ones dropped. An empty list plays nothing.
    pub fn new(keeps: &[KeepSegment], source_duration: f64, rate: u32) -> CutPlan {
        let rate = rate.max(1);
        let to_frames = |s: f64| -> u64 {
            if s.is_finite() && s > 0.0 {
                (s * f64::from(rate)).round() as u64
            } else {
                0
            }
        };
        let source_frames = to_frames(source_duration);
        let mut ks: Vec<Keep> = keeps
            .iter()
            .map(|k| Keep {
                start: to_frames(k.start).min(source_frames),
                end: to_frames(k.end).min(source_frames),
            })
            .filter(|k| k.end > k.start)
            .collect();
        ks.sort_by_key(|k| k.start);
        // Overlapping keeps (not something build_keeps makes) are merged, so the programme never
        // plays the same source twice.
        let mut merged: Vec<Keep> = Vec::with_capacity(ks.len());
        for k in ks {
            match merged.last_mut() {
                Some(prev) if k.start < prev.end => prev.end = prev.end.max(k.end),
                _ => merged.push(k),
            }
        }
        let half_max = (CROSSFADE_SEC * f64::from(rate) / 2.0).round() as u64;
        let halves: Vec<u64> = merged
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                if a.end >= b.start {
                    return 0; // touching: the material is continuous, nothing to hide
                }
                half_max
                    .min(a.len() / 2)
                    .min(b.len() / 2)
                    .min(source_frames - a.end)
                    .min(b.start)
            })
            .collect();
        let mut starts = Vec::with_capacity(merged.len());
        let mut total = 0u64;
        for k in &merged {
            starts.push(total);
            total += k.len();
        }
        let end_fade = ((END_FADE_SEC * f64::from(rate)).round() as u64).min(total / 2);
        CutPlan {
            rate,
            source_frames,
            keeps: merged,
            starts,
            halves,
            total,
            end_fade,
        }
    }

    /// The whole recording, no cuts.
    pub fn whole(source_duration: f64, rate: u32) -> CutPlan {
        CutPlan::new(
            &[KeepSegment {
                start: 0.0,
                end: source_duration,
            }],
            source_duration,
            rate,
        )
    }

    /// Sample rate of both timelines.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Length of the programme in frames.
    pub fn program_frames(&self) -> u64 {
        self.total
    }

    /// Length of the programme in seconds.
    pub fn program_secs(&self) -> f64 {
        self.total as f64 / f64::from(self.rate)
    }

    /// Programme seconds of every join between two keeps (where a cut is heard).
    pub fn join_secs(&self) -> Vec<f64> {
        self.starts
            .iter()
            .skip(1)
            .map(|s| *s as f64 / f64::from(self.rate))
            .collect()
    }

    /// Number of joins that are crossfaded.
    pub fn crossfaded_joins(&self) -> usize {
        self.halves.iter().filter(|h| **h > 0).count()
    }

    /// The keep that holds programme frame `p` (`p < total`), and `p`'s offset into it.
    fn keep_at(&self, p: u64) -> (usize, u64) {
        let k = self.starts.partition_point(|s| *s <= p).saturating_sub(1);
        (k, p - self.starts[k])
    }

    /// The source frame heard at programme frame `p`. At and past the end: the last keep's end.
    pub fn program_to_source(&self, p: u64) -> u64 {
        if self.keeps.is_empty() {
            return 0;
        }
        if p >= self.total {
            return self.keeps[self.keeps.len() - 1].end;
        }
        let (k, local) = self.keep_at(p);
        self.keeps[k].start + local
    }

    /// [`Self::program_to_source`] in seconds.
    pub fn program_to_source_secs(&self, program_secs: f64) -> f64 {
        let p = (program_secs.max(0.0) * f64::from(self.rate)).round() as u64;
        self.program_to_source(p) as f64 / f64::from(self.rate)
    }

    /// The programme frame for source frame `s`: inside a keep, where it is heard; inside a cut
    /// (or before the first keep), the next keep's start; past the last keep, the end.
    pub fn source_to_program(&self, s: u64) -> u64 {
        for (k, keep) in self.keeps.iter().enumerate() {
            if s < keep.end {
                return self.starts[k] + s.saturating_sub(keep.start);
            }
        }
        self.total
    }

    /// [`Self::source_to_program`] in seconds.
    pub fn source_to_program_secs(&self, source_secs: f64) -> f64 {
        let s = (source_secs.max(0.0) * f64::from(self.rate)).round() as u64;
        self.source_to_program(s) as f64 / f64::from(self.rate)
    }

    /// What plays at programme frame `p` (`p < total`).
    pub fn region_at(&self, p: u64) -> Region {
        let (k, local) = self.keep_at(p);
        let keep = self.keeps[k];
        let h_in = if k > 0 { self.halves[k - 1] } else { 0 };
        let h_out = self.halves.get(k).copied().unwrap_or(0);
        if local < h_in {
            // Second half of the fade into this keep.
            let prev = self.keeps[k - 1];
            let t = h_in + local;
            return Region::Fade {
                a_src: prev.end - h_in + t,
                b_src: keep.start - h_in + t,
                t,
                width: 2 * h_in,
                len: h_in - local,
            };
        }
        let fade_out_from = keep.len() - h_out;
        if local >= fade_out_from {
            let next = self.keeps[k + 1];
            let t = local - fade_out_from;
            return Region::Fade {
                a_src: keep.end - h_out + t,
                b_src: next.start - h_out + t,
                t,
                width: 2 * h_out,
                len: 2 * h_out - t,
            };
        }
        Region::Plain {
            src: keep.start + local,
            len: fade_out_from - local,
        }
    }

    /// The end-fade gain at programme frame `p`.
    fn end_gain(&self, p: u64) -> f32 {
        let from = self.total - self.end_fade;
        if p < from || self.end_fade == 0 {
            return 1.0;
        }
        let x = ((p - from) as f64 + 0.5) / self.end_fade as f64;
        constant_power(x).0
    }

    #[cfg(test)]
    fn source_frames(&self) -> u64 {
        self.source_frames
    }
}

/// A sequential reader of planar PCM from some source position on.
pub trait PcmStream {
    /// Read up to `frames` frames into `dst[c][at..at + frames]` (each channel is at least that
    /// long). Returns the frames read; `0` means the source has ended. A short read is allowed
    /// (a pipe hands over what it has); the caller asks again.
    fn read_planar(&mut self, dst: &mut [Vec<f32>], at: usize, frames: usize) -> io::Result<usize>;
}

impl<S: PcmStream + ?Sized> PcmStream for Box<S> {
    fn read_planar(&mut self, dst: &mut [Vec<f32>], at: usize, frames: usize) -> io::Result<usize> {
        (**self).read_planar(dst, at, frames)
    }
}

/// Opens a [`PcmStream`] at a source frame (the shell: an ffmpeg process with `-ss`).
pub trait PcmOpener {
    type Stream: PcmStream;
    fn open(&mut self, source_frame: u64) -> io::Result<Self::Stream>;
}

impl<O: PcmOpener + ?Sized> PcmOpener for Box<O> {
    type Stream = O::Stream;
    fn open(&mut self, source_frame: u64) -> io::Result<Self::Stream> {
        (**self).open(source_frame)
    }
}

struct Slot<S> {
    /// The source frame this stream reads next.
    next: u64,
    stream: S,
    ended: bool,
    used: bool,
}

/// Reads the programme of a [`CutPlan`] from a source, block by block, from any programme
/// position. At most two source streams are open at once (both sides of a join); a stream is
/// reused as long as the programme reads on where it stopped, so a keep is ONE stream from start
/// to end, and a seek opens new ones.
///
/// The output is the same however it is cut into blocks and wherever it starts: frame `p` is a
/// function of `p` and the source only (tested).
pub struct ProgramReader<O: PcmOpener> {
    plan: CutPlan,
    opener: O,
    channels: usize,
    pos: u64,
    slots: Vec<Slot<O::Stream>>,
    a: Vec<Vec<f32>>,
    b: Vec<Vec<f32>>,
}

/// Longest block [`ProgramReader::read_block`] hands back.
pub const MAX_BLOCK: usize = 4096;

impl<O: PcmOpener> ProgramReader<O> {
    /// Read `channels` channels of `plan` through `opener`, from programme frame `start`.
    pub fn new(plan: CutPlan, opener: O, channels: usize, start: u64) -> Self {
        let channels = channels.max(1);
        let pos = start.min(plan.total);
        Self {
            plan,
            opener,
            channels,
            pos,
            slots: Vec::with_capacity(2),
            a: vec![vec![0.0; MAX_BLOCK]; channels],
            b: vec![vec![0.0; MAX_BLOCK]; channels],
        }
    }

    /// The next programme frame to be read.
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn plan(&self) -> &CutPlan {
        &self.plan
    }

    /// Read source frames `[src, src + n)` into `which` (`false` = `a`, `true` = `b`).
    fn pull(&mut self, src: u64, n: usize, into_b: bool) -> io::Result<()> {
        let idx = match self.slots.iter().position(|s| s.next == src) {
            Some(i) => i,
            None => {
                let stream = self.opener.open(src)?;
                self.slots.push(Slot {
                    next: src,
                    stream,
                    ended: false,
                    used: false,
                });
                self.slots.len() - 1
            }
        };
        let dst = if into_b { &mut self.b } else { &mut self.a };
        let slot = &mut self.slots[idx];
        slot.used = true;
        let mut got = 0;
        while got < n && !slot.ended {
            let r = slot.stream.read_planar(dst, got, n - got)?;
            if r == 0 {
                slot.ended = true;
            }
            got += r;
        }
        for d in dst.iter_mut() {
            d[got..n].fill(0.0);
        }
        slot.next = src + n as u64;
        Ok(())
    }

    /// Read the next block (at most `max` frames, and never across a region boundary) into
    /// `out`, which is resized to `channels` channels of exactly the returned length. `0` at the
    /// end of the programme.
    pub fn read_block(&mut self, out: &mut Vec<Vec<f32>>, max: usize) -> io::Result<usize> {
        out.resize_with(self.channels, Vec::new);
        if self.pos >= self.plan.total {
            for c in out.iter_mut() {
                c.clear();
            }
            return Ok(0);
        }
        for s in &mut self.slots {
            s.used = false;
        }
        let region = self.plan.region_at(self.pos);
        let max = max.clamp(1, MAX_BLOCK) as u64;
        let n = match region {
            Region::Plain { src, len } => {
                let n = len.min(max) as usize;
                self.pull(src, n, false)?;
                for (o, a) in out.iter_mut().zip(&self.a) {
                    o.clear();
                    o.extend_from_slice(&a[..n]);
                }
                n
            }
            Region::Fade {
                a_src,
                b_src,
                t,
                width,
                len,
            } => {
                let n = len.min(max) as usize;
                self.pull(a_src, n, false)?;
                self.pull(b_src, n, true)?;
                for (o, (a, b)) in out.iter_mut().zip(self.a.iter().zip(&self.b)) {
                    o.clear();
                    o.extend((0..n).map(|i| {
                        let x = ((t + i as u64) as f64 + 0.5) / width as f64;
                        let (ga, gb) = constant_power(x);
                        a[i] * ga + b[i] * gb
                    }));
                }
                n
            }
        };
        // End fade.
        if self.pos + n as u64 > self.plan.total - self.plan.end_fade {
            for (i, p) in (self.pos..self.pos + n as u64).enumerate() {
                let g = self.plan.end_gain(p);
                for o in out.iter_mut() {
                    o[i] *= g;
                }
            }
        }
        // Streams this block did not read from are past their keep: close them.
        self.slots.retain(|s| s.used);
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A source held in memory: one function of the source frame per channel.
    #[derive(Clone)]
    struct Synth {
        f: fn(u64, usize) -> f32,
        len: u64,
        opens: std::rc::Rc<std::cell::RefCell<Vec<u64>>>,
    }

    struct SynthStream {
        f: fn(u64, usize) -> f32,
        len: u64,
        at: u64,
    }

    impl PcmStream for SynthStream {
        fn read_planar(
            &mut self,
            dst: &mut [Vec<f32>],
            at: usize,
            frames: usize,
        ) -> io::Result<usize> {
            // Hand over at most 1000 frames per call, like a pipe would.
            let n = (frames as u64)
                .min(1000)
                .min(self.len.saturating_sub(self.at)) as usize;
            for (c, d) in dst.iter_mut().enumerate() {
                for (i, x) in d[at..at + n].iter_mut().enumerate() {
                    *x = (self.f)(self.at + i as u64, c);
                }
            }
            self.at += n as u64;
            Ok(n)
        }
    }

    impl PcmOpener for Synth {
        type Stream = SynthStream;
        fn open(&mut self, source_frame: u64) -> io::Result<SynthStream> {
            self.opens.borrow_mut().push(source_frame);
            Ok(SynthStream {
                f: self.f,
                len: self.len,
                at: source_frame,
            })
        }
    }

    fn synth(f: fn(u64, usize) -> f32, secs: f64) -> Synth {
        Synth {
            f,
            len: (secs * f64::from(SR)) as u64,
            opens: Default::default(),
        }
    }

    fn keeps(spans: &[(f64, f64)]) -> Vec<KeepSegment> {
        spans
            .iter()
            .map(|&(start, end)| KeepSegment { start, end })
            .collect()
    }

    fn read_all<O: PcmOpener>(r: &mut ProgramReader<O>, block: usize) -> Vec<Vec<f32>> {
        let mut out: Vec<Vec<f32>> = Vec::new();
        let mut buf = Vec::new();
        loop {
            let n = r.read_block(&mut buf, block).unwrap();
            if n == 0 {
                return out;
            }
            out.resize_with(buf.len(), Vec::new);
            for (o, b) in out.iter_mut().zip(&buf) {
                o.extend_from_slice(&b[..n]);
            }
        }
    }

    /// Deterministic white noise, independent per (frame, channel): an xorshift hash of the frame.
    fn noise(i: u64, c: usize) -> f32 {
        let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (c as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 31;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 29;
        ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.5
    }

    fn sine220(i: u64, _c: usize) -> f32 {
        (0.5 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / f64::from(SR)).sin()) as f32
    }

    #[test]
    fn constant_power_keeps_the_sum_of_squares_at_one() {
        for i in 0..=100 {
            let (a, b) = constant_power(f64::from(i) / 100.0);
            assert!((a * a + b * b - 1.0).abs() < 1e-6);
        }
        assert_eq!(constant_power(0.0), (1.0, 0.0));
    }

    #[test]
    fn the_programme_is_the_keeps_laid_end_to_end() {
        let plan = CutPlan::new(&keeps(&[(1.0, 3.0), (5.0, 6.5)]), 10.0, SR);
        assert_eq!(plan.program_frames(), (3.5 * f64::from(SR)) as u64);
        assert_eq!(plan.crossfaded_joins(), 1);
        assert_eq!(plan.join_secs(), vec![2.0]);
        // Before the join: the first keep. At the join: the second keep's start.
        assert_eq!(plan.program_to_source(0), SR as u64);
        assert_eq!(plan.program_to_source(2 * SR as u64 - 1), 3 * SR as u64 - 1);
        assert_eq!(plan.program_to_source(2 * SR as u64), 5 * SR as u64);
        assert_eq!(
            plan.program_to_source(u64::MAX),
            (6.5 * f64::from(SR)) as u64
        );
        // And back: a cut maps to the next keep's start.
        assert_eq!(plan.source_to_program(4 * SR as u64), 2 * SR as u64);
        assert_eq!(plan.source_to_program(0), 0);
        assert!((plan.source_to_program_secs(5.5) - 2.5).abs() < 1e-9);
        assert!((plan.program_to_source_secs(2.5) - 5.5).abs() < 1e-9);
        assert_eq!(plan.source_frames(), 10 * SR as u64);
    }

    #[test]
    fn the_fade_borrows_only_material_that_exists() {
        // The first keep starts at 0 (nothing before it, but no join there), the join between
        // touching keeps gets no fade, and a 4 ms keep shrinks the fade to 2 ms each side.
        let plan = CutPlan::new(
            &keeps(&[(0.0, 1.0), (1.0, 2.0), (3.0, 3.004), (5.0, 10.0)]),
            10.0,
            SR,
        );
        assert_eq!(plan.halves, vec![0, 96, 96]);
        // A keep that ends at the file's end needs nothing after it: the full 10 ms each side.
        let plan = CutPlan::new(&keeps(&[(0.0, 1.0), (2.0, 10.0)]), 10.0, SR);
        assert_eq!(plan.halves, vec![480]);
    }

    #[test]
    fn a_join_between_uncorrelated_signals_keeps_its_power() {
        // Oracle: short-window RMS through the join stays within 1 dB of the signal's own RMS,
        // where a fade-out/fade-in would drop to silence and a linear fade by 3 dB.
        let mut src = synth(noise, 10.0);
        let plan = CutPlan::new(&keeps(&[(0.0, 2.0137), (6.211, 9.0)]), 10.0, SR);
        let mut r = ProgramReader::new(plan.clone(), src.clone(), 2, 0);
        let out = read_all(&mut r, 1000);
        let rms = |s: &[f32]| {
            (s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
        };
        let whole = rms(&out[0]);
        let join = plan.starts[1] as usize;
        let w = 240; // 5 ms windows, sliding over the 20 ms fade and around it
        for start in (join - 960..join + 960 - w).step_by(60) {
            let db = 20.0 * (rms(&out[0][start..start + w]) / whole).log10();
            assert!(db.abs() < 1.0, "window at {start}: {db:.2} dB");
        }
        // The two sides are independent noise: the crossfade really mixed two streams.
        assert_eq!(
            *src.opens.borrow(),
            vec![0, (6.211 * f64::from(SR)).round() as u64 - 480]
        );
        let _ = src.open(0);
    }

    #[test]
    fn a_join_has_no_jump_where_a_hard_splice_has_one() {
        // A 220 Hz sine cut mid-cycle: the hard splice jumps by up to its amplitude; the
        // crossfade stays under twice the sine's own steepest step.
        let plan = CutPlan::new(&keeps(&[(0.0, 1.0013), (2.37, 4.0)]), 4.0, SR);
        let mut r = ProgramReader::new(plan.clone(), synth(sine220, 4.0), 1, 0);
        let out = read_all(&mut r, 512);
        let natural = 0.5 * 2.0 * std::f64::consts::PI * 220.0 / f64::from(SR);
        let worst = out[0]
            .windows(2)
            .map(|w| f64::from((w[1] - w[0]).abs()))
            .fold(0.0, f64::max);
        assert!(
            worst < 2.0 * natural,
            "worst step {worst:.4} vs natural {natural:.4}"
        );
        // The baseline the test would catch: splice the same two keeps with no fade.
        let j = plan.starts[1];
        let hard_a = sine220(plan.keeps[0].end - 1, 0);
        let hard_b = sine220(plan.keeps[1].start, 0);
        assert!(
            f64::from((hard_b - hard_a).abs()) > 4.0 * natural,
            "the test's cut must be mid-cycle"
        );
        assert!(j > 0);
    }

    #[test]
    fn the_output_does_not_depend_on_block_size_or_where_reading_starts() {
        let plan = CutPlan::new(&keeps(&[(0.5, 1.2), (1.9, 2.6), (3.0, 4.0)]), 4.0, SR);
        let whole = read_all(
            &mut ProgramReader::new(plan.clone(), synth(noise, 4.0), 2, 0),
            4096,
        );
        let odd = read_all(
            &mut ProgramReader::new(plan.clone(), synth(noise, 4.0), 2, 0),
            333,
        );
        assert_eq!(whole, odd);
        // Start inside the first fade, and inside a plain region.
        for start in [plan.starts[1] - 100, plan.starts[1] + 7, 12_345] {
            let tail = read_all(
                &mut ProgramReader::new(plan.clone(), synth(noise, 4.0), 2, start),
                777,
            );
            assert_eq!(tail[1][..], whole[1][start as usize..], "start {start}");
        }
    }

    #[test]
    fn a_keep_is_one_stream_and_a_short_source_reads_as_silence() {
        let src = synth(sine220, 1.0); // the plan says 2 s; the file has 1 s
        let plan = CutPlan::whole(2.0, SR);
        let mut r = ProgramReader::new(plan, src.clone(), 1, 0);
        let out = read_all(&mut r, 1000);
        assert_eq!(out[0].len(), 2 * SR as usize);
        assert!(out[0][SR as usize..].iter().all(|x| *x == 0.0));
        assert_eq!(*src.opens.borrow(), vec![0], "one keep, one stream");
    }

    #[test]
    fn the_end_fades_out() {
        let plan = CutPlan::whole(1.0, SR);
        let out = read_all(
            &mut ProgramReader::new(plan, synth(|_, _| 0.5, 1.0), 1, 0),
            4096,
        );
        let last = *out[0].last().unwrap();
        assert!(last.abs() < 0.01, "last sample {last}");
        assert_eq!(out[0][SR as usize - 481], 0.5);
    }

    #[test]
    fn an_empty_plan_plays_nothing() {
        let plan = CutPlan::new(&[], 10.0, SR);
        assert_eq!(plan.program_frames(), 0);
        assert_eq!(plan.program_to_source(5), 0);
        let out = read_all(&mut ProgramReader::new(plan, synth(noise, 1.0), 2, 0), 100);
        assert!(out.is_empty());
    }
}
