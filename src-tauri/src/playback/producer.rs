//! The producer thread: everything between the file and the ring (B5a).
//!
//! ```text
//! ProgramReader (cut plan, crossfades; ffmpeg streams behind it)
//!   → BuiltChain (B2a's chain, latency removed, one second of pre-roll after a seek)
//!   → Resampler per channel (open_read's Kaiser-sinc, programme rate → device rate)
//!   → device channel layout, interleaved
//!   → ring  (+ Start/End markers)
//! ```
//!
//! **Why the chain and the resampler run here and not in the callback.** `BuiltChain::process_block`
//! takes a `Vec<Vec<f32>>` and builds slice lists per stage, the de-noiser's gate keeps queues,
//! and the resampler appends to `Vec`s: all of it allocates. The filmcraft effects themselves do
//! not, but the chain around them does, and rewriting it allocation-free would change the code the
//! export runs. Running the whole chain here, ahead of the ring, keeps ONE implementation for
//! preview and export («det du hører er det du får»), and leaves the callback with a copy and a
//! gain. The price is latency for changes made while playing: a new chain (B5b's profile switch)
//! reaches the ear after what is already in the ring ([`RING_SEC`]), unless it is applied as a
//! seek to the current position, which flushes the ring with a fade.
//!
//! The producer waits for room in the ring by sleeping on the command channel
//! (`recv_timeout`), so a seek wakes it at once; it never spins.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
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
/// Programme run through the chain (and thrown away) before the position a seek lands on, so the
/// compressor, gate and de-noiser have settled when the ear gets there. Seconds.
pub const PREROLL_SEC: f64 = 1.0;
/// How long the producer sleeps on the command channel when the ring is full.
const FULL_WAIT: Duration = Duration::from_millis(3);

pub enum Command {
    /// Play from programme frame `program_frame` as generation `gen`.
    Seek {
        gen: u64,
        program_frame: u64,
    },
    Shutdown,
}

/// One start or seek, from its programme position to the end.
struct Job<O: PcmOpener> {
    gen: u64,
    reader: ProgramReader<O>,
    chain: BuiltChain,
    /// Chain-output frames still to throw away (the pre-roll and the chain's latency).
    skip: u64,
    /// Programme frames still to emit.
    emit_left: u64,
    resamplers: Vec<Resampler>,
    planar: Vec<Vec<f32>>,
    resampled: Vec<Vec<f32>>,
}

impl<O: PcmOpener> Job<O> {
    fn new(
        gen: u64,
        plan: &CutPlan,
        opener: O,
        chain: &Chain,
        channels: usize,
        start: u64,
        device_rate: u32,
    ) -> Result<Self, String> {
        let built = chain
            .build(plan.rate() as f32, channels)
            .map_err(|e| format!("lydkjeden kunne ikke bygges: {e}"))?;
        let start = start.min(plan.program_frames());
        let preroll = (PREROLL_SEC * f64::from(plan.rate())) as u64;
        let from = start.saturating_sub(preroll);
        let resamplers = (0..built.out_channels())
            .map(|_| Resampler::new(plan.rate(), device_rate))
            .collect();
        Ok(Self {
            gen,
            skip: (start - from) + built.latency() as u64,
            emit_left: plan.program_frames() - start,
            reader: ProgramReader::new(plan.clone(), opener, channels, from),
            chain: built,
            resamplers,
            planar: Vec::new(),
            resampled: vec![Vec::new(); 2],
        })
    }

    /// The next chunk, interleaved in `dev_ch` channels at the device rate, into `out`.
    /// Returns `(programme frames made, last)`.
    fn next_chunk(&mut self, out: &mut Vec<f32>, dev_ch: usize) -> io::Result<(u64, bool)> {
        out.clear();
        let n = self.reader.read_block(&mut self.planar, MAX_BLOCK)?;
        let mut last = false;
        if n == 0 {
            // The programme is read: push the chain's latency through as silence.
            let lat = self.chain.latency();
            for c in &mut self.planar {
                c.clear();
                c.resize(lat, 0.0);
            }
            last = true;
        }
        if self.planar.first().is_some_and(|c| !c.is_empty()) {
            self.chain.process_block(&mut self.planar);
        }
        let frames = self.planar.first().map_or(0, Vec::len) as u64;
        let drop = self.skip.min(frames);
        self.skip -= drop;
        let keep = (frames - drop).min(self.emit_left);
        self.emit_left -= keep;
        if self.emit_left == 0 {
            last = true;
        }
        let (a, b) = (drop as usize, (drop + keep) as usize);
        self.resampled.resize_with(self.planar.len(), Vec::new);
        for ((rs, src), dst) in self
            .resamplers
            .iter_mut()
            .zip(&self.planar)
            .zip(&mut self.resampled)
        {
            dst.clear();
            rs.process(&src[a..b], dst);
            if last {
                rs.finish(dst);
            }
        }
        interleave(&self.resampled[..self.planar.len()], dev_ch, out);
        Ok((keep, last))
    }
}

/// Lay planar `src` out in the device's `dev_ch` channels: mono goes to the first two, stereo to
/// left and right, a mono device gets the mean, extra device channels stay silent.
pub(crate) fn interleave(src: &[Vec<f32>], dev_ch: usize, out: &mut Vec<f32>) {
    let frames = src.iter().map(Vec::len).min().unwrap_or(0);
    out.reserve(frames * dev_ch);
    let c = src.len();
    for i in 0..frames {
        for d in 0..dev_ch {
            let v = match (c, d) {
                (0, _) => 0.0,
                (1, 0 | 1) => src[0][i],
                (_, 0) if dev_ch == 1 => (src[0][i] + src[1][i]) * 0.5,
                (_, 0 | 1) => src[d][i],
                _ => 0.0,
            };
            out.push(v);
        }
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

impl<O: PcmOpener + Clone> Producer<O> {
    /// The thread body: run until `Shutdown` or the control side is gone.
    pub fn run(mut self, rx: Receiver<Command>) {
        let ch = self.device_channels.max(1);
        let mut job: Option<Job<O>> = None;
        let mut chunk: Vec<f32> = Vec::new();
        let mut chunk_pos = 0usize;
        let mut chunk_last = false;
        let mut pushed_samples: u64 = 0;
        let mut pending: VecDeque<Marker> = VecDeque::new();
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
                Some(Command::Seek { gen, program_frame }) => {
                    chunk.clear();
                    chunk_pos = 0;
                    chunk_last = false;
                    pending.push_back(Marker {
                        gen,
                        at: pushed_samples / ch as u64,
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
                        self.device_rate,
                    ) {
                        Ok(j) => job = Some(j),
                        Err(e) => {
                            self.fail(e, gen, pushed_samples / ch as u64, &mut pending);
                            job = None;
                        }
                    }
                }
                None => {}
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
                let t = Instant::now();
                match j.next_chunk(&mut chunk, ch) {
                    Ok((made, last)) => {
                        self.shared
                            .work_ns
                            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        self.shared.work_frames.fetch_add(made, Ordering::Relaxed);
                        chunk_pos = 0;
                        chunk_last = last;
                        if last {
                            let at = (pushed_samples + chunk.len() as u64) / ch as u64;
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
                            format!("avspillingen kunne ikke lese fila: {e}"),
                            gen,
                            pushed_samples / ch as u64,
                            &mut pending,
                        );
                        job = None;
                        continue;
                    }
                }
            }
            let n = self.data.push_slice(&chunk[chunk_pos..]);
            chunk_pos += n;
            pushed_samples += n as u64;
            stalled = chunk_pos < chunk.len() && self.data.is_full();
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
    fn mono_goes_to_both_sides_and_a_mono_device_gets_the_mean() {
        let mut out = Vec::new();
        interleave(&[vec![1.0, 2.0]], 2, &mut out);
        assert_eq!(out, vec![1.0, 1.0, 2.0, 2.0]);
        out.clear();
        interleave(&[vec![1.0], vec![3.0]], 1, &mut out);
        assert_eq!(out, vec![2.0]);
        out.clear();
        interleave(&[vec![1.0], vec![3.0]], 4, &mut out);
        assert_eq!(out, vec![1.0, 3.0, 0.0, 0.0]);
    }
}
