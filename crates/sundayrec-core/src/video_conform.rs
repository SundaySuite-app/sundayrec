//! The conformer (C2, docs/VIDEOMOTOR.md §C2): camera frames in, a fixed frame
//! rate locked to the audio clock out.
//!
//! **The audio clock is the boss.** The recording's timeline is the WAV, in
//! samples. Output frame `k` — a *slot* — sits on sample `k × rate / fps`
//! ([`slot_sample`]); a recording of `N` samples has exactly [`slot_count`]`(N)`
//! slots, so the video is as long as the audio by construction, whatever the
//! camera did. Each frame is placed on that timeline by the
//! [`AudioClock`](crate::av_clock::AudioClock) (host time → sample position,
//! drift and host-clock jumps handled there).
//!
//! # Which picture goes in a slot
//!
//! - **The newest frame placed at or before the slot.** More than one since the
//!   last slot (a 60 fps camera into 30 fps): the extras are dropped and
//!   counted ([`ConformStats::dropped_surplus`]).
//! - **None new:** the previous picture is repeated ([`Picture::Repeat`]) — a
//!   camera that falls to 15 fps in poor light gives every picture twice.
//! - **The camera is gone:** the last picture is repeated while it is at most
//!   [`HOLD_NS`] (2 s) older than the slot, then [`Picture::Black`].
//! - **Video starts after the audio:** the slots before the first frame are
//!   black.
//! - **The machine slept:** the host clock jumped; [`AudioClock`] restarts its
//!   fit and keeps the old line for frames captured before the jump. Here that
//!   only means a frame is not placed until the clock has accepted a pair at
//!   least as new as the frame ([`AudioClock::trusted_host`]): a frame stamped
//!   after a jump the clock has not confirmed yet waits instead of landing an
//!   hour in the future. The same holds at the start: no frame is placed until
//!   the clock has a line of several agreeing pairs, so one bad first
//!   timestamp cannot misplace a camera that started before the audio.
//!
//! # When a slot is decided
//!
//! A slot can only be decided once no frame for it can still arrive. Frames
//! reach us a camera pipeline later than they were captured, so slot `k` is
//! decided when the audio clock has passed it by [`DECIDE_AFTER_NS`]
//! ([`VideoConformer::advance`]). A frame that turns up later than that is still
//! shown at the next slot — one slot late, within the ±1-frame promise — unless
//! that would be more than one slot late or older than the picture already
//! shown; then it is dropped ([`ConformStats::dropped_late`]).
//! [`VideoConformer::finish`] decides the rest when the recording stops.
//!
//! # The promise, and where it is proven
//!
//! Random drift (±300 ppm and a wobble on top; real cards drift 20–50), jitter
//! on every timestamp, bad timestamps, camera holes, fps drops, video starting
//! after the audio and host-clock jumps from sleep never put a picture more than
//! **one slot** away from where it belongs, over **3 hours** of simulated
//! recording. The proptest at the bottom of this file is that proof; it measures
//! against an oracle that knows every frame's true audio time.
//!
//! Generic over the frame payload `T`: the conformer never looks inside a frame,
//! clones one, or touches I/O or a clock. Numbers in, numbers out.

use std::collections::VecDeque;

use crate::av_clock::AudioClock;

/// How long the last picture is repeated after the camera stops delivering,
/// before the slots go black.
pub const HOLD_NS: u64 = 2_000_000_000;

/// How far the audio must be past a slot before it is decided — the room a
/// frame has to arrive through the camera pipeline.
pub const DECIDE_AFTER_NS: u64 = 500_000_000;

/// Frames waiting for the audio clock to catch up (see the module header). More
/// than this means the audio is not coming; the oldest are dropped.
pub const MAX_UNPLACED: usize = 256;

/// What a slot shows.
#[derive(Debug, PartialEq, Eq)]
pub enum Picture<T> {
    /// A new frame from the camera.
    New(T),
    /// The same picture as the previous slot.
    Repeat,
    /// Black: no picture yet, or the camera has been gone longer than
    /// [`HOLD_NS`].
    Black,
}

/// One output frame.
#[derive(Debug, PartialEq, Eq)]
pub struct Slot<T> {
    /// Slot number `k`; it sits on [`slot_sample`]`(k)`.
    pub index: u64,
    pub picture: Picture<T>,
}

/// The output frame rate, as an exact fraction (`30000/1001` is not `29.97`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fps {
    pub num: u32,
    pub den: u32,
}

impl Fps {
    /// Panics on a zero numerator or denominator.
    pub fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "fps must be a positive fraction");
        Fps { num, den }
    }
}

/// The sample position slot `k` sits on: `k × rate × den / num`, fractional.
pub fn slot_sample(k: u64, rate: u32, fps: Fps) -> f64 {
    (k as u128 * rate as u128 * fps.den as u128) as f64 / fps.num as f64
}

/// How many slots a recording of `total_samples` has: every `k` with
/// `slot_sample(k) < total_samples`, i.e. `⌈total × num / (rate × den)⌉`.
pub fn slot_count(total_samples: u64, rate: u32, fps: Fps) -> u64 {
    let n = total_samples as u128 * fps.num as u128;
    let d = rate as u128 * fps.den as u128;
    n.div_ceil(d) as u64
}

/// Counters for diagnostics and telemetry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConformStats {
    pub slots: u64,
    pub new: u64,
    pub repeated: u64,
    pub black: u64,
    /// More than one frame for a slot; all but the newest dropped.
    pub dropped_surplus: u64,
    /// Arrived after its slot was decided and could not be shown within one slot.
    pub dropped_late: u64,
    /// Could not be placed on the audio timeline (no audio, or too many waiting).
    pub dropped_unplaced: u64,
}

/// Camera frames → fixed-rate slots on the audio timeline. See the module
/// header.
#[derive(Debug)]
pub struct VideoConformer<T> {
    rate: u32,
    fps: Fps,
    hold_samples: f64,
    decide_after_samples: u64,
    next_slot: u64,
    /// Pushed, not yet placed: `(host ns, frame)`, in arrival order.
    unplaced: VecDeque<(u64, T)>,
    /// Placed, not yet shown: `(sample position, frame)`, sorted by position.
    placed: VecDeque<(f64, T)>,
    last_shown: Option<f64>,
    stats: ConformStats,
}

impl<T> VideoConformer<T> {
    /// A conformer for audio at `rate` Hz and output at `fps`.
    pub fn new(rate: u32, fps: Fps) -> Self {
        assert!(rate > 0, "sample rate must be positive");
        VideoConformer {
            rate,
            fps,
            hold_samples: HOLD_NS as f64 * rate as f64 / 1e9,
            decide_after_samples: (DECIDE_AFTER_NS as u128 * rate as u128 / 1_000_000_000) as u64,
            next_slot: 0,
            unplaced: VecDeque::new(),
            placed: VecDeque::new(),
            last_shown: None,
            stats: ConformStats::default(),
        }
    }

    /// A frame from the camera, captured at `host_ns` on the host clock (the
    /// clock the audio pairs are stamped with).
    pub fn push_frame(&mut self, host_ns: u64, frame: T) {
        if self.unplaced.len() >= MAX_UNPLACED {
            self.unplaced.pop_front();
            self.stats.dropped_unplaced += 1;
        }
        self.unplaced.push_back((host_ns, frame));
    }

    /// Decide every slot the audio clock is far enough past, appending them to
    /// `out` in order.
    pub fn advance(&mut self, clock: &AudioClock, out: &mut Vec<Slot<T>>) {
        self.place(clock, false);
        let Some(end) = clock.newest_sample() else {
            return;
        };
        let Some(limit) = end.checked_sub(self.decide_after_samples) else {
            return;
        };
        // Slot k is decided once k·rate·den ≤ limit·num (exact integers).
        let per_slot = self.rate as u128 * self.fps.den as u128;
        let rhs = limit as u128 * self.fps.num as u128;
        while self.next_slot as u128 * per_slot <= rhs {
            self.emit(out);
        }
    }

    /// The recording stopped after `total_samples` of audio: place what is
    /// left and decide every remaining slot, so the output has exactly
    /// [`slot_count`]`(total_samples)` slots in all.
    pub fn finish(&mut self, clock: &AudioClock, total_samples: u64, out: &mut Vec<Slot<T>>) {
        self.place(clock, true);
        let n = slot_count(total_samples, self.rate, self.fps);
        while self.next_slot < n {
            self.emit(out);
        }
    }

    /// The next slot number to be decided.
    pub fn next_slot(&self) -> u64 {
        self.next_slot
    }

    pub fn stats(&self) -> ConformStats {
        self.stats
    }

    fn slot_pos(&self, k: u64) -> f64 {
        slot_sample(k, self.rate, self.fps)
    }

    /// Move frames from `unplaced` to `placed`. Normally only frames no newer
    /// than the clock's newest accepted pair on a trusted line; `all` (at finish) places the rest
    /// by extrapolation.
    fn place(&mut self, clock: &AudioClock, all: bool) {
        let newest_host = clock.trusted_host();
        while let Some(&(host, _)) = self.unplaced.front() {
            if !all && newest_host.is_none_or(|h| host > h) {
                break;
            }
            let (host, frame) = self.unplaced.pop_front().expect("front exists");
            let Some(pos) = clock.sample_at(host) else {
                self.stats.dropped_unplaced += 1;
                continue;
            };
            // Older than what is already on screen, or would be shown more
            // than one slot after it belongs: drop.
            let stale = self.last_shown.is_some_and(|l| pos <= l);
            let too_late = self.next_slot >= 2 && pos <= self.slot_pos(self.next_slot - 2);
            if stale || too_late {
                self.stats.dropped_late += 1;
                continue;
            }
            let at = self.placed.partition_point(|(p, _)| *p <= pos);
            self.placed.insert(at, (pos, frame));
        }
    }

    fn emit(&mut self, out: &mut Vec<Slot<T>>) {
        let k = self.next_slot;
        let slot = self.slot_pos(k);
        let mut newest: Option<(f64, T)> = None;
        while self.placed.front().is_some_and(|(p, _)| *p <= slot) {
            if newest.is_some() {
                self.stats.dropped_surplus += 1;
            }
            newest = self.placed.pop_front();
        }
        let picture = match newest {
            Some((pos, frame)) if slot - pos <= self.hold_samples => {
                self.last_shown = Some(pos);
                self.stats.new += 1;
                Picture::New(frame)
            }
            stale => {
                if stale.is_some() {
                    self.stats.dropped_late += 1;
                }
                match self.last_shown {
                    Some(l) if slot - l <= self.hold_samples => {
                        self.stats.repeated += 1;
                        Picture::Repeat
                    }
                    _ => {
                        self.stats.black += 1;
                        Picture::Black
                    }
                }
            }
        };
        self.stats.slots += 1;
        self.next_slot += 1;
        out.push(Slot { index: k, picture });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const FPS30: Fps = Fps { num: 30, den: 1 };
    const BUF: u64 = 1024;

    /// Host time of audio second `t` with a clock exactly on nominal.
    fn host(t: f64) -> u64 {
        (1e12 + t * 1e9).round() as u64
    }

    /// Frames are captured this long before their nominal time `n / cam_fps`, so
    /// a frame never sits exactly on a slot boundary (where rounding decides).
    const PHASE: f64 = 0.005;

    /// Drive `secs` of audio; `frame_at(t)` says whether the camera's frame at
    /// nominal second `t` exists (camera at `cam_fps`, starting at 0, captured
    /// [`PHASE`] early). Frames are delivered 30 ms after capture. Returns the
    /// slots, with `New(t_ms)` of the nominal time.
    fn run(
        secs: f64,
        cam_fps: f64,
        frame_at: impl Fn(f64) -> bool,
    ) -> (Vec<Slot<u64>>, ConformStats) {
        let mut clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        let mut out = vec![];
        let total = (secs * RATE as f64) as u64;
        let mut next_frame = 0u64;
        let mut s = 0u64;
        while s < total {
            let now = (s + BUF) as f64 / RATE as f64;
            loop {
                let t = next_frame as f64 / cam_fps;
                if t + 0.030 > now {
                    break;
                }
                if frame_at(t) {
                    conf.push_frame(host(t - PHASE), (t * 1000.0).round() as u64);
                }
                next_frame += 1;
            }
            clock.observe(s, host(s as f64 / RATE as f64));
            conf.advance(&clock, &mut out);
            s += BUF;
        }
        conf.finish(&clock, total, &mut out);
        (out, conf.stats())
    }

    fn kinds(slots: &[Slot<u64>]) -> String {
        slots
            .iter()
            .map(|s| match s.picture {
                Picture::New(_) => 'N',
                Picture::Repeat => 'R',
                Picture::Black => 'B',
            })
            .collect()
    }

    #[test]
    fn slot_math_is_exact_for_ntsc_rates() {
        let ntsc = Fps::new(30_000, 1001);
        assert_eq!(slot_sample(30_000, 48_000, ntsc), 48_048_000.0);
        assert_eq!(slot_count(48_048_000, 48_000, ntsc), 30_000);
        assert_eq!(slot_count(48_048_001, 48_000, ntsc), 30_001);
        assert_eq!(slot_count(0, 48_000, FPS30), 0);
        assert_eq!(slot_count(1, 48_000, FPS30), 1);
        // Three hours at 44.1 kHz, 25 fps: exactly 270 000 slots.
        assert_eq!(
            slot_count(3 * 3600 * 44_100, 44_100, Fps::new(25, 1)),
            270_000
        );
    }

    #[test]
    fn a_steady_30_fps_camera_gives_one_new_frame_per_slot_and_exact_length() {
        let (slots, st) = run(10.0, 30.0, |_| true);
        assert_eq!(slots.len(), 300);
        assert!(slots.iter().enumerate().all(|(i, s)| s.index == i as u64));
        assert_eq!(st.new, 300);
        assert_eq!(
            st.dropped_surplus + st.dropped_late + st.dropped_unplaced,
            0
        );
    }

    #[test]
    fn a_camera_at_15_fps_repeats_every_picture() {
        let (slots, st) = run(4.0, 15.0, |_| true);
        assert_eq!(&kinds(&slots)[..8], "NRNRNRNR");
        assert_eq!(st.new, 60);
        assert_eq!(st.repeated, 60);
    }

    #[test]
    fn a_camera_at_60_fps_drops_the_surplus() {
        let (slots, st) = run(4.0, 60.0, |_| true);
        assert_eq!(st.new, 120);
        // Every other frame, except at slot 0 (nothing before it).
        assert_eq!(st.dropped_surplus, 119);
        // Each slot shows the frame exactly on it (t = k/30), not the one before.
        assert_eq!(slots[45].picture, Picture::New(1500));
    }

    #[test]
    fn camera_gone_repeats_two_seconds_then_black_then_recovers() {
        // Camera gone from 3.0 s to 8.0 s.
        let (slots, _) = run(10.0, 30.0, |t| !(3.0..8.0).contains(&t));
        let k = kinds(&slots);

        // Last frame captured at 2.962 s, shown at slot 89 (2.967 s). Repeated
        // while at most 2 s old: slots 90..=148 (4.933 s); slot 149 (4.967 s)
        // is 2.005 s after the capture — black.
        assert_eq!(&k[89..90], "N");
        assert!(k[90..149].chars().all(|c| c == 'R'), "{}", &k[85..160]);
        assert!(k[149..240].chars().all(|c| c == 'B'), "{}", &k[145..245]);
        assert_eq!(&k[240..241], "N");
        assert_eq!(slots[240].picture, Picture::New(8000));
    }

    #[test]
    fn video_starting_after_the_audio_is_black_until_the_first_frame() {
        let (slots, st) = run(5.0, 30.0, |t| t >= 1.5);
        let k = kinds(&slots);
        assert!(k[..45].chars().all(|c| c == 'B'));
        assert_eq!(slots[45].picture, Picture::New(1500));
        assert_eq!(st.black, 45);
        assert_eq!(slots.len(), 150);
    }

    #[test]
    fn a_camera_started_before_the_audio_shows_the_frame_at_zero() {
        let mut clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        for i in 0..10 {
            let t = -0.3 + i as f64 / 30.0; // -0.3 .. 0.0
            conf.push_frame(host(t + 1.0), i);
        }
        clock.observe(0, host(1.0));
        let mut out = vec![];
        conf.finish(&clock, 1, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].picture, Picture::New(9));
        assert_eq!(conf.stats().dropped_surplus, 9);
    }

    #[test]
    fn a_bad_first_audio_timestamp_does_not_misplace_early_frames() {
        // The camera runs from 0.3 s before the audio; its frames are queued
        // when the first audio pair arrives — 319 ms late. Placed on that
        // one-pair line, the frame from 0.3 s would land on slot 0. They wait
        // for a line of agreeing pairs instead. Id: capture time + 1 s in ms.
        let mut clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        let mut out = vec![];
        let id = |t: f64| ((t + 1.0) * 1000.0).round() as u64;
        let mut f = -9i64; // frame f at f/30 s: −0.3 s …
        let mut push_upto = |conf: &mut VideoConformer<u64>, upto: f64| {
            while (f as f64 / 30.0) <= upto {
                let t = f as f64 / 30.0;
                conf.push_frame(host(t - PHASE), id(t));
                f += 1;
            }
        };
        push_upto(&mut conf, 0.4);
        clock.observe(0, host(0.0) + 319_000_000);
        conf.advance(&clock, &mut out);
        let total = 2 * RATE as u64;
        let mut s = BUF;
        while s < total {
            let t = s as f64 / RATE as f64;
            push_upto(&mut conf, t);
            clock.observe(s, host(t));
            conf.advance(&clock, &mut out);
            s += BUF;
        }
        conf.finish(&clock, total, &mut out);
        assert_eq!(clock.resets(), 0);
        assert_eq!(clock.outliers(), 1);
        assert_eq!(out[0].picture, Picture::New(id(0.0)));
        assert_eq!(out[9].picture, Picture::New(id(0.3)));
    }

    #[test]
    fn a_host_clock_jump_from_sleep_keeps_pictures_on_the_audio() {
        // 10 s of audio, then the machine sleeps for an hour (the host clock
        // jumps, the sample count does not), then 10 s more. The camera keeps
        // its frames on the same host clock and needs 0.5 s after waking.
        let mut clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        let mut out = vec![];
        let slept = 3_600_000_000_000u64;
        let sleep_at = 10.0;
        let h = |t: f64| host(t) + if t >= sleep_at { slept } else { 0 };
        let total = 20 * RATE as u64;
        let mut f = 0u64;
        let mut s = 0u64;
        while s < total {
            let now = (s + BUF) as f64 / RATE as f64;
            while (f as f64 / 30.0) + 0.030 <= now {
                let t = f as f64 / 30.0;
                if !(sleep_at..sleep_at + 0.5).contains(&t) {
                    conf.push_frame(h(t - PHASE), f);
                }
                f += 1;
            }
            clock.observe(s, h(s as f64 / RATE as f64));
            conf.advance(&clock, &mut out);
            s += BUF;
        }
        conf.finish(&clock, total, &mut out);
        assert_eq!(clock.resets(), 1);
        assert_eq!(out.len(), 600);
        // Every new picture is the frame captured at exactly its slot.
        for s in &out {
            if let Picture::New(f) = s.picture {
                assert_eq!(f, s.index, "slot {}", s.index);
            }
        }
        let k = kinds(&out);
        assert!(k[300..315].chars().all(|c| c == 'R'), "{}", &k[295..320]);
        assert_eq!(&k[315..316], "N");
        assert_eq!(conf.stats().dropped_late + conf.stats().dropped_unplaced, 0);
    }

    #[test]
    fn a_frame_more_than_one_slot_late_is_dropped_not_shown_late() {
        let mut clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        let mut out = vec![];
        // 2 s of audio, no frames: slots up to 1.5 s are decided (black).
        let mut s = 0;
        while s <= 2 * RATE as u64 {
            clock.observe(s, host(s as f64 / RATE as f64));
            s += BUF;
        }
        conf.advance(&clock, &mut out);
        let decided = conf.next_slot();
        assert!(decided >= 45);
        // A frame from 0.5 s turns up now: far too late.
        conf.push_frame(host(0.5), 1);
        // One from just before the next slot is shown at it.
        let just = (decided - 1) as f64 / 30.0 + 0.001;
        conf.push_frame(host(just), 2);
        conf.finish(&clock, 2 * RATE as u64 + BUF, &mut out);
        assert_eq!(conf.stats().dropped_late, 1);
        assert_eq!(out[decided as usize].picture, Picture::New(2));
    }

    #[test]
    fn no_audio_means_no_slots() {
        let clock = AudioClock::new(RATE);
        let mut conf = VideoConformer::new(RATE, FPS30);
        conf.push_frame(host(0.0), 0u8);
        let mut out = vec![];
        conf.advance(&clock, &mut out);
        assert!(out.is_empty());
        conf.finish(&clock, 0, &mut out);
        assert!(out.is_empty());
        assert_eq!(conf.stats().dropped_unplaced, 1);
    }

    // ── The proof: 3 hours of simulated recording against an oracle ─────────

    mod simulation {
        use super::super::*;
        use proptest::prelude::*;

        /// splitmix64 — the scenario's own randomness, seeded by proptest, so a
        /// case is a few dozen numbers rather than millions of draws.
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            }
            /// Uniform in [-1, 1).
            fn sym(&mut self) -> f64 {
                (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
            }
            fn unit(&mut self) -> f64 {
                (self.next() >> 11) as f64 / (1u64 << 53) as f64
            }
        }

        #[derive(Clone, Debug)]
        pub struct Scenario {
            pub seed: u64,
            pub secs: f64,
            pub rate: u32,
            pub fps: Fps,
            pub buf: u64,
            pub cam_fps: f64,
            /// Sound card vs host clock, ppm, plus a slow sinusoidal wobble.
            pub drift_ppm: f64,
            pub wobble_ppm: f64,
            pub wobble_period_s: f64,
            /// ± uniform jitter on every audio pair's host time.
            pub audio_jitter_ns: f64,
            /// ± uniform jitter on every frame's host time.
            pub frame_jitter_ns: f64,
            /// Camera pipeline delay: base + uniform extra, seconds.
            pub delivery_s: (f64, f64),
            /// Probability per audio pair of a wild timestamp (±50 ms – 2 s).
            pub outlier_p: f64,
            /// First frame's audio time (negative: camera started first).
            pub video_start_s: f64,
            /// Camera gone: (start, length) in audio seconds.
            pub holes: Vec<(f64, f64)>,
            /// Poor light: (start, length, divisor) — fps / divisor.
            pub fps_drops: Vec<(f64, f64, u32)>,
            /// Host-clock jumps: (audio second, jump ns, camera re-init s).
            pub sleeps: Vec<(f64, f64, f64)>,
            /// Steps too small to detect (lost buffers): (audio second, ns).
            pub small_steps: Vec<(f64, f64)>,
        }

        pub struct Outcome {
            pub slots: u64,
            pub max_place_err_ms: f64,
            pub max_slot_dev: i64,
            pub resets: u32,
            pub stats: ConformStats,
        }

        /// Run a scenario and check every slot against the oracle. Panics
        /// (with the scenario) on a broken promise.
        pub fn simulate(sc: &Scenario) -> Outcome {
            let rate = sc.rate as f64;
            let mut rng = Rng(sc.seed);
            let total = ((sc.secs * rate) as u64 / sc.buf) * sc.buf;

            // Jumps happen between buffers: snap them to buffer boundaries.
            let snap = |t: f64| ((t * rate / sc.buf as f64).round() * sc.buf as f64) / rate;
            let mut jumps: Vec<(f64, f64)> = sc
                .sleeps
                .iter()
                .map(|&(t, d, _)| (snap(t), d))
                .chain(sc.small_steps.iter().map(|&(t, d)| (snap(t), d)))
                .collect();
            jumps.sort_by(|a, b| a.0.total_cmp(&b.0));
            let omega = std::f64::consts::TAU / sc.wobble_period_s;
            let host0 = 2_000_000_000_000.0 + (rng.next() % 1_000_000_000_000) as f64;
            // True host time (ns) of audio second t.
            let host_of = |t: f64| -> f64 {
                let mut h = host0
                    + 1e9
                        * (t * (1.0 + sc.drift_ppm * 1e-6)
                            + sc.wobble_ppm * 1e-6 * (1.0 - (omega * t).cos()) / omega);
                for &(at, d) in &jumps {
                    if t >= at {
                        h += d;
                    }
                }
                h
            };
            let hole = |t: f64| -> Option<f64> {
                sc.holes
                    .iter()
                    .map(|&(a, l)| (a, a + l))
                    .chain(sc.sleeps.iter().map(|&(a, _, r)| (snap(a), snap(a) + r)))
                    .find(|&(a, b)| t >= a && t < b)
                    .map(|(_, b)| b)
            };
            let divisor = |t: f64| -> u32 {
                sc.fps_drops
                    .iter()
                    .find(|&&(a, l, _)| t >= a && t < a + l)
                    .map_or(1, |d| d.2)
            };

            // Frames: true audio second, host stamp, delivery time.
            let mut true_t: Vec<f64> = Vec::new();
            let mut stamp: Vec<u64> = Vec::new();
            let mut deliver: Vec<f64> = Vec::new();
            let mut t = sc.video_start_s;
            let end_s = total as f64 / rate;
            while t < end_s + 1.0 {
                if let Some(b) = hole(t) {
                    t = b;
                    continue;
                }
                true_t.push(t);
                stamp.push((host_of(t) + sc.frame_jitter_ns * rng.sym()).max(0.0) as u64);
                deliver.push(t + sc.delivery_s.0 + sc.delivery_s.1 * rng.unit());
                t += divisor(t) as f64 / sc.cam_fps;
            }

            let mut clock = AudioClock::new(sc.rate);
            let mut conf: VideoConformer<u32> = VideoConformer::new(sc.rate, sc.fps);
            let mut out: Vec<Slot<u32>> = Vec::new();
            // Per slot: -1 black, else the frame id on screen.
            let mut shown: Vec<i64> =
                Vec::with_capacity(slot_count(total, sc.rate, sc.fps) as usize);
            let mut on_screen: i64 = -1;
            let mut drain = |out: &mut Vec<Slot<u32>>, shown: &mut Vec<i64>| {
                for s in out.drain(..) {
                    assert_eq!(s.index as usize, shown.len(), "slots out of order");
                    match s.picture {
                        Picture::New(id) => on_screen = id as i64,
                        Picture::Repeat => assert!(on_screen >= 0, "repeat with nothing shown"),
                        Picture::Black => on_screen = -1,
                    }
                    shown.push(on_screen);
                }
            };
            let (mut pushed, mut measured) = (0usize, 0usize);
            let mut max_err = 0.0f64;
            let mut measure = |upto: usize, measured: &mut usize, clock: &AudioClock, all: bool| {
                while *measured < upto {
                    let i = *measured;
                    if !all && clock.trusted_host().is_none_or(|h| stamp[i] > h) {
                        break;
                    }
                    let pos = clock.sample_at(stamp[i]).expect("clock has pairs");
                    max_err = max_err.max((pos / rate - true_t[i]).abs());
                    *measured += 1;
                }
            };

            let mut s = 0u64;
            while s < total {
                let now = (s + sc.buf) as f64 / rate;
                while pushed < true_t.len() && deliver[pushed] <= now {
                    conf.push_frame(stamp[pushed], pushed as u32);
                    pushed += 1;
                }
                let t = s as f64 / rate;
                let mut h = host_of(t) + sc.audio_jitter_ns * rng.sym();
                if rng.unit() < sc.outlier_p {
                    let mag = 50e6 + rng.unit() * 1.95e9;
                    h += if rng.next() & 1 == 0 { mag } else { -mag };
                }
                clock.observe(s, h.max(0.0) as u64);
                measure(pushed, &mut measured, &clock, false);
                conf.advance(&clock, &mut out);
                drain(&mut out, &mut shown);
                s += sc.buf;
            }
            // Frames captured before the end that are still in the pipeline.
            while pushed < true_t.len() && true_t[pushed] < end_s {
                conf.push_frame(stamp[pushed], pushed as u32);
                pushed += 1;
            }
            measure(pushed, &mut measured, &clock, true);
            conf.finish(&clock, total, &mut out);
            drain(&mut out, &mut shown);

            // ── Checks ──
            let n = slot_count(total, sc.rate, sc.fps);
            assert_eq!(
                shown.len() as u64,
                n,
                "video must be exactly as long as the audio"
            );
            let period = sc.fps.den as f64 / sc.fps.num as f64;
            let hold = HOLD_NS as f64 / 1e9;
            // What a perfect conformer (true times) shows at audio second t.
            let oracle = |t: f64| -> i64 {
                let i = true_t.partition_point(|&f| f <= t);
                if i == 0 || t - true_t[i - 1] > hold {
                    -1
                } else {
                    (i - 1) as i64
                }
            };
            // Is the oracle black at some instant of [a, b]? Black after frame
            // i is (true_t[i] + hold, true_t[i + 1]); one that starts before
            // `a` shows at `a`, so only those starting in [a, b) need a look.
            let oracle_black_within = |a: f64, b: f64| -> bool {
                if oracle(a) < 0 {
                    return true;
                }
                let first = true_t.partition_point(|&f| f + hold < a);
                (first..true_t.len())
                    .take_while(|&i| true_t[i] + hold < b)
                    .any(|i| {
                        true_t
                            .get(i + 1)
                            .is_none_or(|&next| next > true_t[i] + hold)
                    })
            };
            // Does the oracle show a picture at some instant of [a, b]?
            let oracle_picture_within = |a: f64, b: f64| -> bool {
                let i = true_t.partition_point(|&f| f <= a);
                oracle(a) >= 0 || true_t.get(i).is_some_and(|&f| f <= b)
            };
            let mut conf_first: Vec<i64> = vec![-1; true_t.len()];
            let mut oracle_first: Vec<i64> = vec![-1; true_t.len()];
            let mut last_id = -1i64;
            for (k, &id) in shown.iter().enumerate() {
                let tk = k as f64 * period;
                let o = oracle(tk);
                if (id < 0) != (o < 0) {
                    // Black against a picture: only where the oracle itself
                    // shows the same somewhere within one slot either side (2 s
                    // hold edge, first frame). The whole interval, not just its
                    // ends: a camera back just after the hold ran out leaves a
                    // black stretch narrower than a slot, which a placement
                    // error far below one slot can land a slot in.
                    let (a, b) = (tk - period, tk + period);
                    let ok = if id < 0 {
                        oracle_black_within(a, b)
                    } else {
                        oracle_picture_within(a, b)
                    };
                    assert!(ok, "slot {k} ({tk:.3} s): showed {id}, oracle {o}");
                }
                if id >= 0 {
                    assert!(id >= last_id, "slot {k}: picture went back in time");
                    last_id = id;
                    if conf_first[id as usize] < 0 {
                        conf_first[id as usize] = k as i64;
                    }
                }
                if o >= 0 && oracle_first[o as usize] < 0 {
                    oracle_first[o as usize] = k as i64;
                }
            }
            let mut max_dev = 0i64;
            for (c, o) in conf_first.iter().zip(&oracle_first) {
                if *c >= 0 && *o >= 0 {
                    max_dev = max_dev.max((c - o).abs());
                }
            }
            let resets = clock.resets();
            // Every sleep restarts the fit exactly once, and nothing else does —
            // drift, jitter, bad timestamps and small steps must be absorbed by
            // the fit, not papered over by restarts. Never seen: a jump at or
            // before the first pair, or after the last. Maybe seen (counted in
            // the upper bound only): one in the last second (confirmed or not
            // before the end), two within a second (may be confirmed as one),
            // one a small step partly cancels.
            let mut at: Vec<f64> = sc
                .sleeps
                .iter()
                .map(|s| snap(s.0))
                .filter(|&t| t > 0.0 && t < end_s)
                .collect();
            at.sort_by(f64::total_cmp);
            let maybe = at.len() as u32;
            at.dedup_by(|b, a| *b - *a < 1.0);
            let near_step = |t: f64| sc.small_steps.iter().any(|s| (snap(s.0) - t).abs() < 1.0);
            let sure = at
                .iter()
                .filter(|&&t| t < end_s - 1.0 && !near_step(t))
                .count() as u32;
            assert!(
                (sure..=maybe).contains(&resets),
                "{resets} resets for {sure}..={maybe} sleeps"
            );
            Outcome {
                slots: n,
                max_place_err_ms: max_err * 1e3,
                max_slot_dev: max_dev,
                resets,
                stats: conf.stats(),
            }
        }

        fn scenario() -> impl Strategy<Value = Scenario> {
            let secs = 3.0 * 3600.0;
            let at = 0.0..secs;
            (
                (
                    any::<u64>(),
                    prop_oneof![Just(44_100u32), Just(48_000u32)],
                    prop_oneof![
                        Just(Fps::new(30, 1)),
                        Just(Fps::new(25, 1)),
                        Just(Fps::new(30_000, 1001))
                    ],
                    prop_oneof![Just(1024u64), Just(2048u64), Just(4096u64)],
                    prop_oneof![
                        Just(30.0),
                        Just(30_000.0 / 1001.0),
                        Just(25.0),
                        Just(60.0),
                        Just(24.0)
                    ],
                ),
                (
                    -300.0..300.0f64,
                    0.0..20.0f64,
                    300.0..3600.0f64,
                    0.0..3e6f64,
                    0.0..2e6f64,
                    (0.0..0.15f64, 0.0..0.1f64),
                    0.0..2e-4f64,
                    -3.0..15.0f64,
                ),
                (
                    prop::collection::vec((at.clone(), 0.05..120.0f64), 0..6),
                    prop::collection::vec(
                        (
                            at.clone(),
                            1.0..600.0f64,
                            prop_oneof![Just(2u32), Just(4u32)],
                        ),
                        0..6,
                    ),
                    prop::collection::vec(
                        (
                            at.clone(),
                            prop_oneof![40e6..2e9f64, 2e9..8.0 * 3600e9f64],
                            0.0..5.0f64,
                        ),
                        0..3,
                    ),
                    prop::collection::vec((at, -15e6..15e6f64), 0..5),
                ),
            )
                .prop_map(
                    move |(
                        (seed, rate, fps, buf, cam_fps),
                        (drift, wob, wobp, aj, fj, delivery, outp, vstart),
                        (holes, fps_drops, sleeps, small_steps),
                    )| Scenario {
                        seed,
                        secs,
                        rate,
                        fps,
                        buf,
                        cam_fps,
                        drift_ppm: drift,
                        wobble_ppm: wob,
                        wobble_period_s: wobp,
                        audio_jitter_ns: aj,
                        frame_jitter_ns: fj,
                        delivery_s: delivery,
                        outlier_p: outp,
                        video_start_s: vstart,
                        holes,
                        fps_drops,
                        sleeps,
                        small_steps,
                    },
                )
        }

        /// `PROPTEST_CASES` overrides; CI runs the default.
        fn cases() -> u32 {
            std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(12)
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

            /// The C2 gate: never more than one slot off the audio clock over
            /// three hours, whatever the clocks and the camera do.
            #[test]
            fn three_hours_never_drift_more_than_one_frame(sc in scenario()) {
                let o = simulate(&sc);
                let period_ms = 1e3 * sc.fps.den as f64 / sc.fps.num as f64;
                eprintln!(
                    "c2-proptest: {} slots, max place err {:.3} ms, max slot dev {}, resets {}, drift {:.0} ppm, {:?}",
                    o.slots, o.max_place_err_ms, o.max_slot_dev, o.resets, sc.drift_ppm, o.stats
                );
                prop_assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
                prop_assert!(o.max_place_err_ms < period_ms, "placement error {} ms", o.max_place_err_ms);
            }
        }

        /// The case proptest found (regression seed `cc c297fed8…`): the
        /// machine sleeps at 935.28 s (the host clock jumps ~6 h) and the camera
        /// is back 2.0046 s later, so the last picture before the gap is 2.021 s
        /// old when the next arrives — 21 ms past the 2 s hold. The oracle's
        /// black stretch is narrower than a 25 fps slot and holds none; the
        /// conformer, ~1 ms off on both frames, lands slot 23432 (937.280 s) in
        /// it and shows black there. That is within one slot, and the check
        /// must accept it.
        fn camera_back_just_past_the_hold_after_a_sleep() -> Scenario {
            Scenario {
                seed: 14_375_585_711_776_970,
                secs: 3.0 * 3600.0,
                rate: 48_000,
                fps: Fps::new(25, 1),
                buf: 1024,
                cam_fps: 60.0,
                drift_ppm: -207.395_670_272_507_9,
                wobble_ppm: 0.820_397_421_187_904_4,
                wobble_period_s: 3_245.260_873_421_877_5,
                audio_jitter_ns: 2_286_460.982_533_921,
                frame_jitter_ns: 1_492_068.136_998_755_2,
                delivery_s: (0.117_175_295_390_737_93, 0.089_846_865_176_144_14),
                outlier_p: 5.877_540_590_347_608e-5,
                video_start_s: 5.725_272_761_096_951,
                holes: vec![
                    (1_068.895_740_910_397_8, 81.621_355_168_369_04),
                    (5_487.389_188_809_23, 29.432_495_548_844_92),
                    (1_732.548_448_446_179_7, 98.329_731_450_644_15),
                    (3_072.518_500_605_394, 107.856_921_173_837_8),
                    (10_264.834_403_005_416, 95.047_000_132_117_52),
                ],
                fps_drops: vec![
                    (4_132.918_017_153_882, 392.769_161_357_057_25, 4),
                    (10_331.234_319_899_264, 295.922_444_827_360_4, 4),
                    (1_098.506_870_123_693_7, 190.536_458_384_343_94, 2),
                    (8_742.471_262_575_94, 242.985_596_926_562_83, 4),
                ],
                sleeps: vec![
                    (
                        8_816.200_255_175_794,
                        566_954_012.029_645_1,
                        3.245_744_434_443_013_3,
                    ),
                    (
                        935.283_973_679_955,
                        21_900_000_051_853.617,
                        2.004_562_458_535_596_6,
                    ),
                ],
                small_steps: vec![
                    (4_910.288_089_088_366, 4_742_265.449_413_051),
                    (6_352.829_625_104_408, -3_671_676.931_176_415_6),
                    (6_548.809_165_167_648, -10_471_659.696_444_346),
                    (5_082.568_966_084_262, 12_885_745.055_271_948),
                ],
            }
        }

        #[test]
        fn a_sleep_with_the_camera_back_just_past_the_hold_stays_within_one_slot() {
            let sc = camera_back_just_past_the_hold_after_a_sleep();
            let o = simulate(&sc);
            assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
            assert!(o.max_place_err_ms < 40.0, "{}", o.max_place_err_ms);
            assert_eq!(o.resets, 2);
        }

        /// The same gap without the sleep (a plain camera hole, no host-clock
        /// jump) gives the same black slot: the jump and the clock's segments
        /// play no part, it is the hold edge alone.
        #[test]
        fn the_same_gap_without_a_sleep_is_the_same_hold_edge() {
            let mut sc = camera_back_just_past_the_hold_after_a_sleep();
            let (at, _, reinit) = sc.sleeps.pop().expect("the sleep at 935 s");
            // The simulation snaps a sleep to a buffer boundary; the hole must
            // start on the same one to drop the same frames.
            let per_buf = sc.buf as f64 / sc.rate as f64;
            sc.holes.push(((at / per_buf).round() * per_buf, reinit));
            let o = simulate(&sc);
            assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
            assert_eq!(o.resets, 1);
        }

        /// Proptest regression `cc 5d24cd87…`: a 40 ms jump 0.7 s before the
        /// end is seen and confirmed — one reset, which the check must allow.
        #[test]
        fn a_jump_in_the_last_second_may_reset() {
            let sc = Scenario {
                seed: 0,
                secs: 3.0 * 3600.0,
                rate: 44_100,
                fps: Fps::new(30, 1),
                buf: 1024,
                cam_fps: 30.0,
                drift_ppm: 0.0,
                wobble_ppm: 0.0,
                wobble_period_s: 300.0,
                audio_jitter_ns: 0.0,
                frame_jitter_ns: 0.0,
                delivery_s: (0.0, 0.0),
                outlier_p: 0.0,
                video_start_s: 0.0,
                holes: vec![],
                fps_drops: vec![],
                sleeps: vec![(10_799.264_737_537_813, 40e6, 0.0)],
                small_steps: vec![],
            };
            let o = simulate(&sc);
            assert_eq!(o.resets, 1);
            assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
        }

        /// Proptest regression `cc` in the file for this case: the camera
        /// starts 2.56 s before the audio and the very first audio pair is
        /// 319 ms off. Frames placed on that one-pair line would stay 319 ms
        /// (≈ 10 slots) wrong; they must wait for a line of agreeing pairs.
        #[test]
        fn a_bad_first_timestamp_does_not_misplace_a_camera_that_started_first() {
            let sc = Scenario {
                seed: 1_525_568_686_406_554_132,
                secs: 3.0 * 3600.0,
                rate: 44_100,
                fps: Fps::new(30, 1),
                buf: 2048,
                cam_fps: 30_000.0 / 1001.0,
                drift_ppm: 0.0,
                wobble_ppm: 0.0,
                wobble_period_s: 300.0,
                audio_jitter_ns: 0.0,
                frame_jitter_ns: 0.0,
                delivery_s: (0.0, 0.0),
                outlier_p: 0.000_112_913_845_181_734_76,
                video_start_s: -2.561_706_775_641_192_6,
                holes: vec![
                    (4_783.974_056_871_886, 52.640_721_457_600_5),
                    (4_760.066_895_267_420_5, 61.856_349_677_345_55),
                ],
                fps_drops: vec![(0.0, 492.377_326_120_127_6, 4)],
                sleeps: vec![],
                small_steps: vec![],
            };
            let o = simulate(&sc);
            assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
            assert!(o.max_place_err_ms < 33.4, "{}", o.max_place_err_ms);
            assert_eq!(o.resets, 0);
        }

        /// The worst plausible case, pinned: 50 ppm the "wrong" way, jitter
        /// at the top of the range, 10 ms callbacks, every kind of hole.
        #[test]
        fn pinned_worst_case_90_minutes_at_10_ms_callbacks() {
            let sc = Scenario {
                seed: 7,
                secs: 90.0 * 60.0,
                rate: 48_000,
                fps: Fps::new(30, 1),
                buf: 480,
                cam_fps: 30.0,
                drift_ppm: -50.0,
                wobble_ppm: 10.0,
                wobble_period_s: 600.0,
                audio_jitter_ns: 3e6,
                frame_jitter_ns: 2e6,
                delivery_s: (0.1, 0.1),
                outlier_p: 1e-4,
                video_start_s: 4.0,
                holes: vec![(600.0, 30.0), (1800.0, 1.0)],
                fps_drops: vec![(1200.0, 300.0, 2)],
                sleeps: vec![(2400.0, 3600e9, 2.0), (3600.0, 0.5e9, 0.0)],
                small_steps: vec![(3000.0, 15e6), (4000.0, -15e6)],
            };
            let o = simulate(&sc);
            assert!(o.max_slot_dev <= 1, "slot deviation {}", o.max_slot_dev);
            assert!(o.max_place_err_ms < 33.3, "{}", o.max_place_err_ms);
            assert_eq!(o.resets, 2);
        }
    }
}
