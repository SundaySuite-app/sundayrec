//! The producer thread: everything between the file and the ring (B5a, B5b).
//!
//! ```text
//! ProgramReader (cut plan, crossfades; ffmpeg streams behind it)
//!   ├─▶ dry  (the programme as it is: «Før»)
//!   └─▶ Lane: BuiltChain (B2a's chain; its latency taken out by index; pre-roll after a seek)
//!         └─ wet («Etter»)     ── a profile change crossfades one Lane into the next ──
//!   both → Resampler per channel (open_read's Kaiser-sinc, programme rate → device rate)
//!        → device channel layout, interleaved as `[dry | wet]` per frame
//!        → ring  (+ Start/End markers)
//! ```
//!
//! **Why the chain and the resampler run here and not in the callback.** `BuiltChain::process_block`
//! takes a `Vec<Vec<f32>>` and builds slice lists per stage, the de-noiser's gate keeps queues,
//! and the resampler appends to `Vec`s: all of it allocates. The filmcraft effects themselves do
//! not, but the chain around them does, and rewriting it allocation-free would change the code the
//! export runs. Running the whole chain here, ahead of the ring, keeps ONE implementation for
//! preview and export («det du hører er det du får»), and leaves the callback with a copy and a
//! mix.
//!
//! ## Før and Etter (B5b)
//!
//! The producer makes BOTH the programme as it is and the programme through the chain, for the
//! same frames, and puts both in the ring. Which one the ear gets is the callback's mix
//! ([`super::rt`]), so «Før»/«Etter» is heard within 20 ms whatever the ring holds. The two are
//! time-aligned by INDEX, not by delay lines: every frame in a [`Stereo`] queue knows its
//! programme frame, the chain's latency is just where the queue starts, and an emission takes
//! the same programme frames out of every queue. (A chain that trails its input by 480 samples
//! and a dry path that does not would otherwise sound as a comb filter in the join; the test
//! `dry_and_wet_are_the_same_frames` puts a click through a chain with look-ahead and finds both
//! on the same frame.)
//!
//! ## A profile change while it plays
//!
//! A new chain is a different set of stages (a gate, a de-noiser, another compressor), not a
//! parameter on the one that runs, so it is a SECOND lane, not a `set_param`. The lane is built
//! and warmed up in a helper thread (it decodes the second before the point the reader stands at
//! and runs it through the new chain, as a seek's pre-roll does), while this thread goes on
//! filling the ring; the input blocks the reader reads meanwhile are kept and fed to the new
//! lane when it is ready. From then on both lanes see the same input, and the wet output is a
//! [`XFADE_SEC`] crossfade from the old lane's frames to the new one's, at the same programme
//! frames (smoothstep, gains summing to one: both are the same speech, so equal gain, not equal
//! power). Then the old lane is dropped.
//!
//! The change is heard after what is in the ring: this thread keeps the ring [`LEAD_SEC`] ahead,
//! not full, so that is at most half a second. A parameter change inside the running chain
//! would not be faster (the chain is before the ring, whatever it does), and the effects'
//! parameter glide (30 ms) is no help across stages that are not the same.
//!
//! The producer waits for room in the ring by sleeping on the command channel
//! (`recv_timeout`), so a seek wakes it at once; it never spins.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ringbuf::traits::{Observer, Producer as _};
use ringbuf::HeapProd;
use sundayrec_core::chain::{BuiltChain, Chain};
use sundayrec_core::open_read::Resampler;
use sundayrec_core::playback::{CutPlan, PcmOpener, ProgramReader, MAX_BLOCK};

use super::rt::{MarkKind, Marker, Shared};

/// Seconds of finished audio the ring holds.
pub const RING_SEC: f64 = 0.75;
/// How far ahead of the device the producer keeps the ring, seconds. A new chain is heard after
/// this much; the rest of the ring is margin the producer does not use up.
pub const LEAD_SEC: f64 = 0.5;
/// Programme run through the chain (and thrown away) before the position a seek lands on, so the
/// compressor, gate and de-noiser have settled when the ear gets there. Seconds.
pub const PREROLL_SEC: f64 = 1.0;
/// The crossfade from one chain to the next, seconds.
pub const XFADE_SEC: f64 = 0.040;
/// How long the producer sleeps on the command channel when the ring is full.
const FULL_WAIT: Duration = Duration::from_millis(3);
/// A warm-up that has not come back after this much programme has been read is given up (the
/// input kept for it is memory).
const WARM_GIVE_UP_SEC: f64 = 30.0;

pub enum Command {
    /// Play from programme frame `program_frame` as generation `gen`.
    Seek {
        gen: u64,
        program_frame: u64,
    },
    /// From now on the wet layer is this chain, crossfaded in at the producer's head.
    SetChain(Chain),
    Shutdown,
}

/// Two channels of programme-rate audio, each frame knowing its programme frame: the first in
/// the queue is `front` (negative before the programme starts: a chain's latency).
#[derive(Debug, Default)]
struct Stereo {
    front: i64,
    q: [VecDeque<f32>; 2],
}

impl Stereo {
    fn new(front: i64) -> Self {
        Self {
            front,
            q: Default::default(),
        }
    }

    /// One planar block's frames `a..b`, as two channels (mono is both).
    fn push(&mut self, src: &[Vec<f32>], a: usize, b: usize) {
        let right = if src.len() > 1 { 1 } else { 0 };
        self.q[0].extend(src[0][a..b].iter().copied());
        self.q[1].extend(src[right][a..b].iter().copied());
    }

    /// The programme frame after the last one held.
    fn end(&self) -> i64 {
        self.front + self.q[0].len() as i64
    }

    /// Drop the frames before programme frame `e`. Frames that have not arrived yet are dropped
    /// by the next call.
    fn discard_before(&mut self, e: i64) {
        let n = ((e - self.front).max(0) as usize).min(self.q[0].len());
        for c in &mut self.q {
            c.drain(..n);
        }
        self.front += n as i64;
    }

    /// Take `k` frames from the front (the caller has made `front` the frame it wants).
    fn take(&mut self, k: usize) -> [Vec<f32>; 2] {
        let k = k.min(self.q[0].len());
        self.front += k as i64;
        [
            self.q[0].drain(..k).collect(),
            self.q[1].drain(..k).collect(),
        ]
    }
}

/// A chain and what it has made: the wet frames, at their programme frames.
struct Lane {
    chain: BuiltChain,
    wet: Stereo,
}

impl Lane {
    /// `input_pos`: the first input frame the chain will be fed. Its first output is the
    /// frame `latency` earlier, which is where the queue starts.
    fn new(chain: BuiltChain, input_pos: u64) -> Self {
        let front = input_pos as i64 - chain.latency() as i64;
        Self {
            chain,
            wet: Stereo::new(front),
        }
    }

    fn feed(&mut self, block: &[Vec<f32>]) {
        if block.first().is_none_or(Vec::is_empty) {
            return;
        }
        let mut b: Vec<Vec<f32>> = block.to_vec();
        self.chain.process_block(&mut b);
        let n = b[0].len();
        self.wet.push(&b, 0, n);
    }

    /// The input is over: push the chain's latency through as silence.
    fn flush(&mut self) {
        let lat = self.chain.latency();
        if lat > 0 {
            let silence = vec![vec![0.0f32; lat]; self.chain.in_channels()];
            self.feed(&silence);
        }
    }
}

/// A chain being warmed up in a helper thread, and the input read meanwhile.
struct Warm {
    rx: Receiver<Result<BuiltChain, String>>,
    /// The input frame the reader stood at when the warm-up began.
    from: u64,
    history: Vec<Vec<Vec<f32>>>,
    frames: u64,
}

/// A lane that is being crossfaded in: from programme frame `x`, for `len` frames.
struct Incoming {
    lane: Lane,
    x: u64,
    len: u64,
}

/// Programme-rate audio ready for the resampler: the same frames, dry and wet.
struct Block {
    dry: [Vec<f32>; 2],
    wet: [Vec<f32>; 2],
}

impl Block {
    fn empty() -> Self {
        Self {
            dry: Default::default(),
            wet: Default::default(),
        }
    }

    fn len(&self) -> usize {
        self.dry[0].len()
    }
}

/// One start or seek, from its programme position to the end.
struct Job<O: PcmOpener> {
    gen: u64,
    reader: ProgramReader<O>,
    in_ch: usize,
    rate: u32,
    plan: CutPlan,
    /// Opens the second reader a warm-up uses.
    opener: O,
    lane: Lane,
    incoming: Option<Incoming>,
    warm: Option<Warm>,
    dry: Stereo,
    /// The next programme frame to emit.
    next_emit: u64,
    /// Programme frames still to emit.
    emit_left: u64,
    /// The reader has run dry and the lanes are flushed.
    exhausted: bool,
    input: Vec<Vec<f32>>,
}

/// Build a chain, and run the programme before input frame `p0` through it.
fn warmed_chain<O: PcmOpener>(
    chain: &Chain,
    plan: &CutPlan,
    opener: O,
    in_ch: usize,
    p0: u64,
) -> Result<BuiltChain, String> {
    let mut built = chain
        .build(plan.rate() as f32, in_ch)
        .map_err(|e| format!("the audio chain could not be built: {e}"))?;
    let preroll = (PREROLL_SEC * f64::from(plan.rate())) as u64;
    let mut pos = p0.saturating_sub(preroll);
    let mut reader = ProgramReader::new(plan.clone(), opener, in_ch, pos);
    let mut buf = Vec::new();
    while pos < p0 {
        let want = ((p0 - pos) as usize).min(MAX_BLOCK);
        let n = reader
            .read_block(&mut buf, want)
            .map_err(|e| format!("reading the recording failed: {e}"))?;
        if n == 0 {
            break;
        }
        built.process_block(&mut buf);
        pos += n as u64;
    }
    Ok(built)
}

impl<O: PcmOpener + Clone + Send + 'static> Job<O>
where
    O::Stream: Send,
{
    fn new(
        gen: u64,
        plan: &CutPlan,
        opener: O,
        chain: &Chain,
        channels: usize,
        start: u64,
    ) -> Result<Self, String> {
        let built = chain
            .build(plan.rate() as f32, channels)
            .map_err(|e| format!("the audio chain could not be built: {e}"))?;
        let start = start.min(plan.program_frames());
        let preroll = (PREROLL_SEC * f64::from(plan.rate())) as u64;
        let from = start.saturating_sub(preroll);
        Ok(Self {
            gen,
            reader: ProgramReader::new(plan.clone(), opener.clone(), channels, from),
            in_ch: channels,
            rate: plan.rate(),
            plan: plan.clone(),
            opener,
            lane: Lane::new(built, from),
            incoming: None,
            warm: None,
            dry: Stereo::new(from as i64),
            next_emit: start,
            emit_left: plan.program_frames() - start,
            exhausted: false,
            input: Vec::new(),
        })
    }

    /// Begin a crossfade to `chain`: warm it up in a helper thread, from where the reader is.
    fn start_warm(&mut self, chain: &Chain) {
        let from = self.reader.position();
        let (tx, rx) = mpsc::channel();
        let chain = chain.clone();
        let plan = self.plan.clone();
        let opener = self.opener.clone();
        let in_ch = self.in_ch;
        let spawned = std::thread::Builder::new()
            .name("playback-warm".into())
            .spawn(move || {
                let _ = tx.send(warmed_chain(&chain, &plan, opener, in_ch, from));
            });
        if spawned.is_ok() {
            self.warm = Some(Warm {
                rx,
                from,
                history: Vec::new(),
                frames: 0,
            });
        }
    }

    /// Take the warmed chain in when the helper has finished. Returns an error to tell, if any.
    fn poll_warm(&mut self) -> Option<String> {
        let w = self.warm.as_mut()?;
        match w.rx.try_recv() {
            Ok(Ok(chain)) => {
                let w = self.warm.take()?;
                self.join(chain, w);
                None
            }
            Ok(Err(e)) => {
                self.warm = None;
                Some(e)
            }
            Err(TryRecvError::Empty) => {
                if w.frames as f64 > WARM_GIVE_UP_SEC * f64::from(self.rate) {
                    tracing::warn!("playback: the new chain never came up; keeping the old one");
                    self.warm = None;
                }
                None
            }
            Err(TryRecvError::Disconnected) => {
                self.warm = None;
                None
            }
        }
    }

    /// The warmed chain stands where the reader stood when the warm-up began (`w.from`); bring
    /// it up to the reader with the input read since, and start the crossfade where both lanes
    /// have frames.
    fn join(&mut self, chain: BuiltChain, w: Warm) {
        let mut lane = Lane::new(chain, w.from);
        // The frames the new lane has from the start; the crossfade cannot begin before that.
        let first = lane.wet.front.max(self.next_emit as i64) as u64;
        for block in &w.history {
            lane.feed(block);
        }
        self.incoming = Some(Incoming {
            lane,
            x: first,
            len: ((XFADE_SEC * f64::from(self.rate)).round() as u64).max(1),
        });
    }

    /// The next frames both layers can give, or `None` until more input has been fed.
    fn emit(&mut self) -> Option<Block> {
        let e = self.next_emit as i64;
        self.dry.discard_before(e);
        self.lane.wet.discard_before(e);
        let mut end = (e + self.emit_left as i64)
            .min(self.dry.end())
            .min(self.lane.wet.end())
            .min(e + MAX_BLOCK as i64);
        let mut fading = None;
        if let Some(inc) = self.incoming.as_mut() {
            let (x, len) = (inc.x as i64, inc.len as i64);
            if e < x {
                end = end.min(x);
            } else {
                inc.lane.wet.discard_before(e);
                debug_assert!(inc.lane.wet.q[0].is_empty() || inc.lane.wet.front == e);
                end = end.min(inc.lane.wet.end()).min(x + len);
                fading = Some((x, len));
            }
        }
        if end <= e {
            return None;
        }
        let k = (end - e) as usize;
        let dry = self.dry.take(k);
        let mut wet = self.lane.wet.take(k);
        if let (Some((x, len)), Some(inc)) = (fading, self.incoming.as_mut()) {
            let new = inc.lane.wet.take(k);
            for i in 0..k {
                let t = (((e + i as i64) - x) as f32 / len as f32).clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t);
                for c in 0..2 {
                    wet[c][i] = wet[c][i] * (1.0 - s) + new[c][i] * s;
                }
            }
        }
        self.next_emit += k as u64;
        self.emit_left -= k as u64;
        if self
            .incoming
            .as_ref()
            .is_some_and(|inc| self.next_emit >= inc.x + inc.len)
        {
            if let Some(inc) = self.incoming.take() {
                self.lane = inc.lane;
            }
        }
        Some(Block { dry, wet })
    }

    /// Read one input block and feed it to the dry path and every lane.
    fn read_and_feed(&mut self) -> io::Result<()> {
        let n = self.reader.read_block(&mut self.input, MAX_BLOCK)?;
        if n == 0 {
            self.exhausted = true;
            self.lane.flush();
            if let Some(inc) = self.incoming.as_mut() {
                inc.lane.flush();
            }
            self.warm = None;
            return Ok(());
        }
        self.dry.push(&self.input, 0, n);
        self.lane.feed(&self.input);
        if let Some(inc) = self.incoming.as_mut() {
            inc.lane.feed(&self.input);
        }
        if let Some(w) = self.warm.as_mut() {
            w.history.push(self.input.clone());
            w.frames += n as u64;
        }
        Ok(())
    }

    /// The next programme-rate block, and whether it is the last of the programme.
    fn next_block(&mut self) -> io::Result<(Block, bool)> {
        loop {
            if self.emit_left == 0 {
                return Ok((Block::empty(), true));
            }
            if let Some(b) = self.emit() {
                let last = self.emit_left == 0;
                return Ok((b, last));
            }
            if self.exhausted {
                // The file ended before the plan said it would: end where the data ends.
                return Ok((Block::empty(), true));
            }
            self.read_and_feed()?;
        }
    }
}

/// Programme-rate blocks to device-rate, device-layout `[dry | wet]` frames.
struct Out {
    dry: [Resampler; 2],
    wet: [Resampler; 2],
    tmp: [[Vec<f32>; 2]; 2],
}

impl Out {
    fn new(programme_rate: u32, device_rate: u32) -> Self {
        let rs = || Resampler::new(programme_rate, device_rate);
        Self {
            dry: [rs(), rs()],
            wet: [rs(), rs()],
            tmp: Default::default(),
        }
    }

    /// Resample `b` (and flush the resamplers when `last`) and append it, interleaved, to `out`.
    fn run(&mut self, b: &Block, last: bool, dev_ch: usize, out: &mut Vec<f32>) {
        for (layer, (rs, src)) in [(&mut self.dry, &b.dry), (&mut self.wet, &b.wet)]
            .into_iter()
            .enumerate()
        {
            for c in 0..2 {
                let dst = &mut self.tmp[layer][c];
                dst.clear();
                rs[c].process(&src[c], dst);
                if last {
                    rs[c].finish(dst);
                }
            }
        }
        let frames = self.tmp.iter().flatten().map(Vec::len).min().unwrap_or(0);
        out.reserve(frames * dev_ch * 2);
        for i in 0..frames {
            for layer in 0..2 {
                for d in 0..dev_ch {
                    out.push(device_sample(&self.tmp[layer], i, d, dev_ch));
                }
            }
        }
    }
}

/// Device channel `d` of `dev_ch` for frame `i` of a stereo pair: left and right to the first
/// two, a mono device gets the mean, extra device channels stay silent.
fn device_sample(src: &[Vec<f32>; 2], i: usize, d: usize, dev_ch: usize) -> f32 {
    match d {
        0 if dev_ch == 1 => (src[0][i] + src[1][i]) * 0.5,
        0 | 1 => src[d][i],
        _ => 0.0,
    }
}

/// The producer's fixed inputs.
pub struct Producer<O: PcmOpener + Clone> {
    pub plan: CutPlan,
    pub opener: O,
    pub chain: Chain,
    pub source_channels: usize,
    pub device_rate: u32,
    pub device_channels: usize,
    pub data: HeapProd<f32>,
    pub marks: HeapProd<Marker>,
    pub shared: Arc<Shared>,
    /// The last error, for the reporter to tell.
    pub error: Arc<Mutex<Option<String>>>,
}

impl<O: PcmOpener + Clone + Send + 'static> Producer<O>
where
    O::Stream: Send,
{
    /// The thread body: run until `Shutdown` or the control side is gone.
    pub fn run(mut self, rx: Receiver<Command>) {
        let ch = self.device_channels.max(1);
        // The ring carries `[dry | wet]` per frame.
        let width = ch * 2;
        let lead = ((LEAD_SEC * f64::from(self.device_rate)) as usize).max(1) * width;
        let mut job: Option<Job<O>> = None;
        let mut out = Out::new(self.plan.rate(), self.device_rate);
        let mut chunk: Vec<f32> = Vec::new();
        let mut chunk_pos = 0usize;
        let mut chunk_last = false;
        let mut pushed_samples: u64 = 0;
        let mut pending: VecDeque<Marker> = VecDeque::new();
        let mut want_chain: Option<Chain> = None;
        let mut stalled = false;
        loop {
            let cmd = if job.is_none() && pending.is_empty() {
                rx.recv().ok().or(Some(Command::Shutdown))
            } else if stalled {
                match rx.recv_timeout(FULL_WAIT) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => Some(Command::Shutdown),
                }
            } else {
                match rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Command::Shutdown),
                }
            };
            // Only the newest of a burst of seeks matters: the callback waits for the newest.
            let mut cmd = cmd;
            while matches!(cmd, Some(Command::Seek { .. })) {
                match rx.try_recv() {
                    Ok(next) => cmd = Some(next),
                    Err(_) => break,
                }
            }
            match cmd {
                Some(Command::Shutdown) => return,
                Some(Command::SetChain(c)) => {
                    // Later jobs (a seek) start with it; the running one crossfades to it.
                    self.chain = c.clone();
                    want_chain = Some(c);
                }
                Some(Command::Seek { gen, program_frame }) => {
                    chunk.clear();
                    chunk_pos = 0;
                    chunk_last = false;
                    want_chain = None;
                    out = Out::new(self.plan.rate(), self.device_rate);
                    pending.push_back(Marker {
                        gen,
                        at: pushed_samples / width as u64,
                        kind: MarkKind::Start {
                            program_frame: program_frame.min(self.plan.program_frames()),
                        },
                    });
                    match Job::new(
                        gen,
                        &self.plan,
                        self.opener.clone(),
                        &self.chain,
                        self.source_channels,
                        program_frame,
                    ) {
                        Ok(j) => job = Some(j),
                        Err(e) => {
                            self.fail(e, gen, pushed_samples / width as u64, &mut pending);
                            job = None;
                        }
                    }
                }
                None => {}
            }
            // A new chain starts warming up as soon as the last one is in.
            if let Some(j) = job.as_mut() {
                if let Some(e) = j.poll_warm() {
                    tracing::warn!("playback: {e}");
                    if let Ok(mut slot) = self.error.lock() {
                        *slot = Some(e);
                    }
                }
                if j.warm.is_none() && j.incoming.is_none() && !j.exhausted {
                    if let Some(c) = want_chain.take() {
                        j.start_warm(&c);
                    }
                }
            }
            // Markers go out before any data behind them.
            while let Some(m) = pending.front() {
                if self.marks.try_push(*m).is_err() {
                    break;
                }
                pending.pop_front();
            }
            if !pending.is_empty() {
                stalled = true;
                continue;
            }
            let Some(j) = job.as_mut() else {
                stalled = false;
                continue;
            };
            if chunk_pos == chunk.len() {
                if chunk_last {
                    self.shared.produced_gen.store(j.gen, Ordering::Release);
                    job = None;
                    stalled = false;
                    continue;
                }
                // Keep the ring `lead` ahead and no more: a chain change is heard after that.
                if self.data.occupied_len() >= lead {
                    stalled = true;
                    continue;
                }
                let t = Instant::now();
                chunk.clear();
                match j.next_block() {
                    Ok((block, last)) => {
                        out.run(&block, last, ch, &mut chunk);
                        self.shared
                            .work_ns
                            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        self.shared
                            .work_frames
                            .fetch_add(block.len() as u64, Ordering::Relaxed);
                        chunk_pos = 0;
                        chunk_last = last;
                        if last {
                            let at = (pushed_samples + chunk.len() as u64) / width as u64;
                            pending.push_back(Marker {
                                gen: j.gen,
                                at,
                                kind: MarkKind::End,
                            });
                            continue; // the End marker goes out before the last data
                        }
                    }
                    Err(e) => {
                        let gen = j.gen;
                        chunk.clear();
                        chunk_pos = 0;
                        self.fail(
                            format!("reading the recording failed: {e}"),
                            gen,
                            pushed_samples / width as u64,
                            &mut pending,
                        );
                        job = None;
                        continue;
                    }
                }
            }
            let room = lead.saturating_sub(self.data.occupied_len());
            let n = self
                .data
                .push_slice(&chunk[chunk_pos..(chunk_pos + room).min(chunk.len())]);
            chunk_pos += n;
            pushed_samples += n as u64;
            stalled = chunk_pos < chunk.len() && (n == 0 || self.data.is_full() || room == n);
        }
    }

    /// Record `message` and end generation `gen` where the data stands, so the callback stops
    /// instead of waiting for audio that will not come.
    fn fail(&self, message: String, gen: u64, at: u64, pending: &mut VecDeque<Marker>) {
        tracing::warn!(gen, "playback: {message}");
        if let Ok(mut e) = self.error.lock() {
            *e = Some(message);
        }
        pending.push_back(Marker {
            gen,
            at,
            kind: MarkKind::End,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stereo_pair_takes_the_frames_it_was_asked_for_by_programme_index() {
        let mut s = Stereo::new(-3);
        s.push(&[vec![1.0, 2.0, 3.0, 4.0, 5.0]], 0, 5);
        assert_eq!(s.end(), 2);
        s.discard_before(0);
        assert_eq!((s.front, s.q[0].len()), (0, 2));
        // Frames that have not arrived yet are dropped when they do.
        s.discard_before(5);
        assert_eq!((s.front, s.q[0].len()), (2, 0));
        s.push(&[vec![7.0; 4], vec![8.0; 4]], 0, 4);
        s.discard_before(5);
        assert_eq!(s.front, 5);
        let [l, r] = s.take(10);
        assert_eq!((l.len(), r.len()), (1, 1));
        assert_eq!((l[0], r[0]), (7.0, 8.0));
    }

    #[test]
    fn mono_goes_to_both_sides_and_a_mono_device_gets_the_mean() {
        let st = [vec![1.0, 2.0], vec![3.0, 4.0]];
        assert_eq!(device_sample(&st, 0, 0, 2), 1.0);
        assert_eq!(device_sample(&st, 1, 1, 2), 4.0);
        assert_eq!(device_sample(&st, 0, 0, 1), 2.0);
        assert_eq!(device_sample(&st, 0, 3, 4), 0.0);
    }
}
