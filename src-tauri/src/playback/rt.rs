//! The audio callback: what runs on the sound card's real-time thread (B5a).
//!
//! [`RtRenderer::render`] is the WHOLE of what the callback does. It reads two lock-free SPSC
//! rings ([`ringbuf`]: atomics only, no lock, no allocation on push or pop) and a handful of
//! atomics in [`Shared`], and writes the device buffer. Everything that could allocate, lock or
//! take time (ffmpeg, the cut plan, the chain, the resampler) has already happened in the
//! producer thread, BEFORE the ring: the ring carries finished, device-rate, device-channel
//! samples.
//!
//! ## The two rings
//!
//! * `data` — interleaved `f32` samples in the device's layout;
//! * `marks` — [`Marker`]s: where in the `data` stream a generation (one start or seek) begins,
//!   and where it ends. A marker is pushed BEFORE the data it points into, so a callback that
//!   sees a sample of a new generation also sees its marker (both rings publish with release
//!   and observe with acquire). That is the rule [`RtRenderer::flush_step`] leans on: take the
//!   `data` fill level FIRST, then look at the markers, and never discard more than that level.
//!
//! ## The clock
//!
//! The sound card is the clock. Every frame the device takes is counted (`device_frames`);
//! every frame of programme it is handed moves the published position (`position_bits`). Nobody
//! else moves the position: the reporter thread only READS it, so a stalled device is a stalled
//! playhead, never one that runs on by wall time.
//!
//! ## Fades
//!
//! Start, resume, pause and seek are click-free by a [`FADE_SEC`] raised-sine ramp on the
//! callback's own gain: a seek fades the old audio out (consuming it), discards what is left of
//! the old generation, waits until [`PRIME_SEC`] of the new one is buffered (so the first block
//! after a seek is not an underrun), and fades it in. A pause fades out and then holds: nothing
//! is consumed while paused. After an underrun the gain restarts from zero, so the audio that
//! comes back fades in instead of stepping in.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use ringbuf::traits::{Consumer, Observer};
use ringbuf::HeapCons;

/// Length of the start/stop/seek fade, seconds.
pub const FADE_SEC: f64 = 0.008;
/// Audio buffered before a (re)start is allowed to begin, seconds.
pub const PRIME_SEC: f64 = 0.100;

/// What a marker says about the `data` stream at frame `at` (counted from the first frame ever
/// pushed into the ring).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Generation `gen` starts here, at this programme frame (source sample rate).
    Start { program_frame: u64 },
    /// Generation `gen` ends here: the programme is over.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub gen: u64,
    pub at: u64,
    pub kind: MarkKind,
}

/// The atomics the control side, the producer, the reporter and the callback share.
#[derive(Debug, Default)]
pub struct Shared {
    /// Bumped by every start/seek; the callback follows it.
    pub requested_gen: AtomicU64,
    pub paused: AtomicBool,
    /// Frames the device has asked for (silence included): the clock.
    pub device_frames: AtomicU64,
    /// The programme position just handed to the device, seconds, as `f64::to_bits`.
    pub position_bits: AtomicU64,
    /// The generation the position belongs to (0 = none yet).
    pub playing_gen: AtomicU64,
    /// The generation that has reached its end (0 = none).
    pub ended_gen: AtomicU64,
    /// Callbacks that wanted programme and found the ring empty.
    pub underruns: AtomicU64,
    /// Device-reported glitches (cpal `Xrun` through the error callback).
    pub xruns: AtomicU64,
    /// The device hit a fault other than an xrun (unplugged, …).
    pub device_failed: AtomicBool,
    /// The callback's gain is zero and it is not consuming (safe to tear down).
    pub silent: AtomicBool,
    /// Producer work, for the real-time margin: wall nanoseconds spent, programme frames made.
    pub work_ns: AtomicU64,
    pub work_frames: AtomicU64,
    /// The generation whose last sample the producer has pushed (0 = none).
    pub produced_gen: AtomicU64,
}

impl Shared {
    pub fn position_secs(&self) -> f64 {
        f64::from_bits(self.position_bits.load(Ordering::Acquire))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Waiting for the requested generation's start (and its prime).
    Flushing,
    Playing,
    /// The generation reached its end marker.
    Ended,
}

/// The callback's state. Built (and its fade table allocated) off the audio thread; after that,
/// [`render`](Self::render) allocates nothing and waits for nothing.
pub struct RtRenderer {
    data: HeapCons<f32>,
    marks: HeapCons<Marker>,
    shared: Arc<Shared>,
    channels: usize,
    device_rate: f64,
    source_rate: f64,
    /// Device frames popped from `data` (played or discarded), since the ring was made.
    consumed: u64,
    state: State,
    gen: u64,
    start: Marker,
    /// The start marker of the generation being flushed to, once seen.
    pending: Option<Marker>,
    end_at: Option<u64>,
    /// Position on the fade, `0..=fade_len`; the gain is `fade[ramp]`.
    ramp: usize,
    fade: Vec<f32>,
    prime: usize,
}

impl RtRenderer {
    /// `channels` and `device_rate` describe the device; `source_rate` is the programme's rate
    /// (the [`MarkKind::Start`] frames are counted in it).
    pub fn new(
        data: HeapCons<f32>,
        marks: HeapCons<Marker>,
        shared: Arc<Shared>,
        channels: usize,
        device_rate: u32,
        source_rate: u32,
    ) -> Self {
        let fade_len = ((FADE_SEC * f64::from(device_rate)).round() as usize).max(1);
        let fade = (0..=fade_len)
            .map(|i| {
                let s = (std::f64::consts::FRAC_PI_2 * i as f64 / fade_len as f64).sin();
                (s * s) as f32
            })
            .collect();
        shared.silent.store(true, Ordering::Release);
        Self {
            data,
            marks,
            shared,
            channels: channels.max(1),
            device_rate: f64::from(device_rate.max(1)),
            source_rate: f64::from(source_rate.max(1)),
            consumed: 0,
            state: State::Flushing,
            gen: 0,
            start: Marker {
                gen: 0,
                at: 0,
                kind: MarkKind::Start { program_frame: 0 },
            },
            pending: None,
            end_at: None,
            ramp: 0,
            fade,
            prime: ((PRIME_SEC * f64::from(device_rate)).round() as usize).max(1),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames waiting in the ring (for tests and diagnosis; not used by the callback).
    pub fn buffered_frames(&self) -> usize {
        self.data.occupied_len() / self.channels
    }

    // RT-BEGIN — everything from here to RT-END runs on the audio thread. The ratchet test
    // `rt_section_has_no_allocation_or_lock` holds it to: no allocation, no lock, no I/O.

    /// Fill `out` (interleaved, [`Self::channels`] wide) with the next audio. The callback.
    pub fn render(&mut self, out: &mut [f32]) {
        let ch = self.channels;
        let frames = out.len() / ch;
        self.shared
            .device_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        let want = self.shared.requested_gen.load(Ordering::Acquire);
        let paused = self.shared.paused.load(Ordering::Acquire);
        let mut i = 0;
        let mut underran = false;
        while i < frames {
            match self.state {
                State::Flushing => {
                    if want == 0 || !self.flush_step(want) {
                        out[i * ch..frames * ch].fill(0.0);
                        break;
                    }
                }
                State::Ended => {
                    if want != self.gen {
                        self.state = State::Flushing;
                        continue;
                    }
                    out[i * ch..frames * ch].fill(0.0);
                    break;
                }
                State::Playing => {
                    let up = want == self.gen && !paused;
                    if !up && self.ramp == 0 {
                        if want != self.gen {
                            self.state = State::Flushing;
                            continue;
                        }
                        // Paused: hold, consume nothing.
                        out[i * ch..frames * ch].fill(0.0);
                        break;
                    }
                    self.see_end();
                    let mut n = frames - i;
                    if let Some(end) = self.end_at {
                        let left = (end - self.consumed) as usize;
                        if left == 0 {
                            self.state = State::Ended;
                            self.ramp = 0;
                            self.shared.ended_gen.store(self.gen, Ordering::Release);
                            continue;
                        }
                        n = n.min(left);
                    }
                    if !up {
                        n = n.min(self.ramp);
                    }
                    let avail = self.data.occupied_len() / ch;
                    if avail == 0 {
                        underran = true;
                        self.ramp = 0;
                        out[i * ch..frames * ch].fill(0.0);
                        break;
                    }
                    n = n.min(avail);
                    let dst = &mut out[i * ch..(i + n) * ch];
                    let got = self.data.pop_slice(dst);
                    debug_assert_eq!(got, dst.len());
                    for frame in dst.chunks_exact_mut(ch) {
                        if up {
                            if self.ramp < self.fade.len() - 1 {
                                self.ramp += 1;
                            }
                        } else {
                            self.ramp -= 1;
                        }
                        let g = self.fade[self.ramp];
                        for s in frame {
                            *s *= g;
                        }
                    }
                    self.consumed += n as u64;
                    i += n;
                    self.publish_position();
                }
            }
        }
        if underran {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        let silent = self.state != State::Playing || self.ramp == 0;
        self.shared.silent.store(silent, Ordering::Release);
    }

    /// Discard what belongs to older generations and look for `want`'s start. `true` once the
    /// start is reached and enough of the new generation is buffered (or it ends before that).
    fn flush_step(&mut self, want: u64) -> bool {
        let ch = self.channels;
        // FIRST the fill level: everything counted here was pushed before any marker we have not
        // seen yet, so it is safe to discard up to the matching start.
        let avail = self.data.occupied_len() / ch;
        if self.pending.is_some_and(|m| m.gen != want) {
            self.pending = None;
        }
        while self.pending.is_none() {
            match self.marks.try_peek().copied() {
                Some(m) if m.gen < want => {
                    self.marks.try_pop();
                }
                Some(m) if m.gen == want && matches!(m.kind, MarkKind::Start { .. }) => {
                    self.marks.try_pop();
                    self.pending = Some(m);
                }
                _ => break,
            }
        }
        let Some(start) = self.pending else {
            self.consumed += self.data.skip(avail * ch) as u64 / ch as u64;
            return false;
        };
        if self.consumed < start.at {
            let skip = ((start.at - self.consumed) as usize).min(avail);
            self.consumed += self.data.skip(skip * ch) as u64 / ch as u64;
            if self.consumed < start.at {
                return false;
            }
        }
        // At the start. Begin once primed, or once the generation's end is known (a short tail).
        self.end_at = None;
        self.see_end_of(want);
        let ready = self.data.occupied_len() / ch >= self.prime || self.end_at.is_some();
        if !ready {
            return false;
        }
        self.pending = None;
        self.state = State::Playing;
        self.gen = want;
        self.start = start;
        self.ramp = 0;
        self.shared.playing_gen.store(want, Ordering::Release);
        self.publish_position();
        true
    }

    fn see_end(&mut self) {
        if self.end_at.is_none() {
            self.see_end_of(self.gen);
        }
    }

    fn see_end_of(&mut self, gen: u64) {
        if let Some(m) = self.marks.try_peek().copied() {
            if m.gen == gen && m.kind == MarkKind::End {
                self.marks.try_pop();
                self.end_at = Some(m.at);
            }
        }
    }

    fn publish_position(&self) {
        let MarkKind::Start { program_frame } = self.start.kind else {
            return;
        };
        let secs = program_frame as f64 / self.source_rate
            + (self.consumed - self.start.at) as f64 / self.device_rate;
        self.shared
            .position_bits
            .store(secs.to_bits(), Ordering::Release);
    }

    // RT-END
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::{HeapProd, HeapRb};

    pub(crate) const RATE: u32 = 48_000;

    pub(crate) struct Rig {
        pub r: RtRenderer,
        pub data: HeapProd<f32>,
        pub marks: HeapProd<Marker>,
        pub shared: Arc<Shared>,
        pub pushed: u64,
    }

    impl Rig {
        pub fn new(channels: usize) -> Rig {
            let (data, dc) = HeapRb::<f32>::new(RATE as usize * channels).split();
            let (marks, mc) = HeapRb::<Marker>::new(64).split();
            let shared = Arc::new(Shared::default());
            let r = RtRenderer::new(dc, mc, shared.clone(), channels, RATE, RATE);
            Rig {
                r,
                data,
                marks,
                shared,
                pushed: 0,
            }
        }

        /// Start generation `gen` at programme frame `program_frame` (as a seek would).
        pub fn start(&mut self, gen: u64, program_frame: u64) {
            self.marks
                .try_push(Marker {
                    gen,
                    at: self.pushed,
                    kind: MarkKind::Start { program_frame },
                })
                .unwrap();
            self.shared.requested_gen.store(gen, Ordering::Release);
        }

        pub fn push(&mut self, f: impl Fn(u64) -> f32, frames: usize) {
            let ch = self.r.channels();
            for i in 0..frames as u64 {
                let v = f(self.pushed + i);
                for _ in 0..ch {
                    self.data.try_push(v).unwrap();
                }
            }
            self.pushed += frames as u64;
        }

        pub fn end(&mut self, gen: u64) {
            self.marks
                .try_push(Marker {
                    gen,
                    at: self.pushed,
                    kind: MarkKind::End,
                })
                .unwrap();
        }

        /// Run the callback over `frames` frames in blocks of 512; channel 0 back.
        pub fn run(&mut self, frames: usize) -> Vec<f32> {
            let ch = self.r.channels();
            let mut out = Vec::new();
            let mut buf = vec![0.0f32; 512 * ch];
            let mut left = frames;
            while left > 0 {
                let n = left.min(512);
                self.r.render(&mut buf[..n * ch]);
                out.extend(buf[..n * ch].iter().step_by(ch));
                left -= n;
            }
            out
        }
    }

    fn sine(phase: f64) -> impl Fn(u64) -> f32 {
        move |i| {
            (0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(RATE) + phase).sin())
                as f32
        }
    }

    fn worst_step(s: &[f32]) -> f32 {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    const NATURAL: f32 = 0.5 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;

    #[test]
    fn nothing_plays_before_a_start_and_the_start_fades_in() {
        let mut rig = Rig::new(2);
        assert!(rig.run(1000).iter().all(|x| *x == 0.0));
        rig.start(1, 0);
        rig.push(|_| 0.5, 2000); // under the prime: still waiting
        assert!(rig.run(512).iter().all(|x| *x == 0.0));
        rig.push(|_| 0.5, 6000);
        let out = rig.run(1000);
        assert!(
            out[0] < 0.01 && out[0] > 0.0,
            "fades in from silence: {}",
            out[0]
        );
        assert_eq!(out[999], 0.5);
        // The raised-sine fade's steepest step: 0.5 · (π/2) / fade length.
        assert!(worst_step(&out) < 0.5 * 1.6 / (FADE_SEC * f64::from(RATE)) as f32);
    }

    #[test]
    fn a_seek_fades_out_discards_the_old_generation_and_fades_in() {
        let mut rig = Rig::new(2);
        rig.start(1, 0);
        rig.push(sine(0.0), 20_000);
        let a = rig.run(4000);
        // Seek: the old audio still has 16 000 frames in the ring; the new one is a sine 2 rad
        // out of phase, starting at programme frame 96 000 (2 s).
        rig.start(2, 96_000);
        rig.push(sine(2.0), 20_000);
        let b = rig.run(12_000);
        let joined: Vec<f32> = a.iter().chain(&b).copied().collect();
        let worst = worst_step(&joined);
        assert!(
            worst < 1.5 * NATURAL,
            "worst step {worst} vs natural {NATURAL}"
        );
        // The old generation was cut short (fade = 384 frames), not played out.
        assert_eq!(rig.shared.playing_gen.load(Ordering::Acquire), 2);
        let played_new = 12_000 - 384;
        let pos = rig.shared.position_secs();
        assert!(
            (pos - (2.0 + played_new as f64 / f64::from(RATE))).abs() < 1e-9,
            "{pos}"
        );
    }

    #[test]
    fn pause_holds_without_consuming_and_resume_fades_in() {
        let mut rig = Rig::new(1);
        rig.start(1, 0);
        rig.push(sine(0.0), 40_000);
        let a = rig.run(4000);
        rig.shared.paused.store(true, Ordering::Release);
        let p = rig.run(4000);
        let held = rig.shared.position_secs();
        assert!(rig.shared.silent.load(Ordering::Acquire));
        let still = rig.run(4000);
        assert_eq!(
            rig.shared.position_secs(),
            held,
            "nothing consumed while paused"
        );
        assert!(still.iter().all(|x| *x == 0.0));
        rig.shared.paused.store(false, Ordering::Release);
        let b = rig.run(4000);
        for s in [&a, &p, &b] {
            assert!(worst_step(s) < 1.5 * NATURAL);
        }
        assert!(worst_step(&[*p.last().unwrap(), 0.0, b[0]]) < 1.5 * NATURAL);
    }

    #[test]
    fn the_end_marker_ends_and_an_empty_ring_counts_an_underrun() {
        let mut rig = Rig::new(2);
        rig.start(1, 0);
        rig.push(|_| 0.25, 6000);
        let _ = rig.run(6000 + 1000);
        let under = rig.shared.underruns.load(Ordering::Relaxed);
        assert!(under >= 1, "the ring ran dry while playing");
        rig.push(|_| 0.25, 100);
        rig.end(1);
        let out = rig.run(2000);
        assert_eq!(rig.shared.ended_gen.load(Ordering::Acquire), 1);
        assert_eq!(
            rig.shared.underruns.load(Ordering::Relaxed),
            under,
            "the end is not an underrun"
        );
        assert!(out[100..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn the_device_clock_alone_moves_the_position() {
        let mut rig = Rig::new(2);
        rig.start(1, 48_000); // programme 1.0 s
        rig.push(|_| 0.1, 30_000);
        assert_eq!(
            rig.shared.position_secs(),
            0.0,
            "no device frames, no position"
        );
        let _ = rig.run(4_800);
        assert!((rig.shared.position_secs() - 1.1).abs() < 1e-12);
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(
            (rig.shared.position_secs() - 1.1).abs() < 1e-12,
            "wall time does not move it"
        );
        let _ = rig.run(9_600);
        assert!((rig.shared.position_secs() - 1.3).abs() < 1e-12);
        assert_eq!(rig.shared.device_frames.load(Ordering::Relaxed), 14_400);
    }

    /// No allocation on the audio thread: the callback is driven through every state (silence,
    /// prime, play, seek fade + flush, pause, resume, underrun, end) with a counting global
    /// allocator armed for this thread. One allocation anywhere in `render` fails it.
    #[test]
    fn render_never_allocates() {
        let mut rig = Rig::new(2);
        let mut buf = vec![0.0f32; 1024];
        let mut drive = |rig: &mut Rig, blocks: usize| -> usize {
            crate::playback::alloc_probe::count(|| {
                for _ in 0..blocks {
                    rig.r.render(&mut buf);
                }
            })
        };
        let mut allocs = drive(&mut rig, 4); // nothing requested
        rig.start(1, 0);
        rig.push(sine(0.0), 20_000);
        allocs += drive(&mut rig, 20); // prime + play
        rig.start(2, 1000);
        rig.push(sine(1.0), 20_000);
        allocs += drive(&mut rig, 20); // fade out, flush 1, prime 2, fade in
        rig.shared.paused.store(true, Ordering::Release);
        allocs += drive(&mut rig, 4);
        rig.shared.paused.store(false, Ordering::Release);
        allocs += drive(&mut rig, 60); // play out, then underrun
        rig.push(sine(0.0), 1000);
        rig.end(2);
        allocs += drive(&mut rig, 6); // end
        assert!(rig.shared.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(rig.shared.ended_gen.load(Ordering::Acquire), 2);
        assert!(
            crate::playback::alloc_probe::armed_works(),
            "the probe must see allocations"
        );
        assert_eq!(allocs, 0, "the audio callback allocated");
    }

    /// The RT section of this file, read as source: no allocation, no lock, no I/O, no logging.
    /// A ratchet over what the counting allocator cannot see (a lock that happened not to be
    /// contended during the test still waits on the church PC).
    #[test]
    fn rt_section_has_no_allocation_or_lock() {
        let src = include_str!("rt.rs");
        let begin = src.find("// RT-BEGIN").unwrap();
        let end = src.find("// RT-END").unwrap();
        let rt = &src[begin..end];
        let code: String = rt
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "Mutex",
            "RwLock",
            ".lock(",
            "Condvar",
            "try_lock",
            "Vec::",
            "vec!",
            "Box::",
            "String",
            "format!",
            "to_string",
            "to_vec",
            "collect",
            "tracing::",
            "println!",
            "eprintln!",
            "Arc::new",
            "clone()",
            "sleep",
            "recv",
            "send(",
            "std::fs",
            "std::io",
        ] {
            assert!(!code.contains(banned), "`{banned}` in the audio callback");
        }
    }

    /// The callback never waits for the producer: with the ring empty and the producer gone
    /// quiet, a render returns at once with silence and counts the underrun.
    #[test]
    fn a_stalled_producer_is_silence_not_a_wait() {
        let mut rig = Rig::new(2);
        rig.start(1, 0);
        rig.push(sine(0.0), 5000);
        rig.end(1); // known end, so it starts below the prime
                    // Replace: a new generation whose data never comes.
        rig.start(2, 0);
        let t = std::time::Instant::now();
        let out = rig.run(48_000);
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        assert!(out.iter().skip(400).all(|x| *x == 0.0));
    }
}
