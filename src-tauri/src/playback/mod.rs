//! The playback engine (B5a, `docs/LYDMOTOR.md` fase 5): real-time listening to a recording, with
//! its cuts and its audio chain, on the sound card.
//!
//! ```text
//!  control (UI)  ── play / pause / seek ──▶  atomics + command channel
//!
//!  producer thread                                 audio thread (cpal callback)
//!  ───────────────                                 ───────────────────────────
//!  ffmpeg -ss … f32le ─▶ ProgramReader               RtRenderer::render
//!     (cut plan, constant-power joins)                 pop finished samples
//!  ─▶ BuiltChain (B2a)                                 fade gain (start/stop/seek)
//!  ─▶ Resampler → device rate          ══ SPSC ring ══▶  count frames = THE CLOCK
//!  ─▶ device channels, interleaved     ══ markers  ══▶  publish position (atomics)
//!
//!  reporter thread: reads the atomics ~30×/s and hands PlaybackEvents to the sink (the UI)
//! ```
//!
//! **The sound card is the clock.** The position the UI shows is the programme frame the device
//! was last handed, computed in the callback from frames it actually consumed ([`rt`]). The
//! reporter only reads it. If the device stops asking, the playhead stops; nothing advances it
//! by wall time. (The position is what was handed to the device, not what has left the speaker:
//! the device's own output latency, typically 5–20 ms, is not subtracted.)
//!
//! **The audio thread neither allocates nor waits** ([`rt`]: lock-free SPSC rings and atomics;
//! proven by a counting allocator and a source ratchet in its tests). Everything that allocates
//! — ffmpeg, the cut plan, the chain, the resampler — runs in the producer thread before the
//! ring ([`producer`] says why the chain is there and not in the callback).
//!
//! **Threads.** On a real device: one thread owns the cpal stream from creation to drop
//! (`cpal::Stream` is not `Send` on every host, and this keeps WASAPI's COM apartment on one
//! thread), one producer, one reporter. [`Player`] itself is `Send` and holds only channels,
//! atomics and join handles.
//!
//! UI and Tauri commands are B5b/B5c. The runner `examples/playback.rs` drives this module on
//! the Mac's speakers (RIG-DAY «Avspillingsmotoren»).

pub mod producer;
pub mod rt;
pub mod source;

#[cfg(test)]
pub(crate) mod alloc_probe;

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ringbuf::traits::Split;
use ringbuf::{HeapCons, HeapProd, HeapRb};
use sundayrec_core::chain::Chain;
use sundayrec_core::playback::{CutPlan, PcmOpener};

use producer::{Command, Producer, RING_SEC};
use rt::{Marker, RtRenderer, Shared, FADE_SEC};

/// What the engine tells the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackEvent {
    /// The device has been handed audio up to here.
    Position {
        gen: u64,
        program_secs: f64,
        source_secs: f64,
    },
    /// Generation `gen` played to the end of the programme.
    Ended { gen: u64 },
    /// The ring ran dry while playing (`total` so far). Audible as a gap.
    Underrun { total: u64 },
    /// The device reported a glitch (`total` so far).
    Xrun { total: u64 },
    /// The device failed (unplugged, taken) or the file could not be read.
    Error { message: String },
}

/// Counters for diagnosis and the runner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackStats {
    pub device_frames: u64,
    pub underruns: u64,
    pub xruns: u64,
    /// Programme seconds made per second of producer work (decode + chain + resampling).
    pub realtime_factor: Option<f64>,
}

/// How often the reporter looks at the atomics.
const REPORT_EVERY: Duration = Duration::from_millis(33);

pub type Sink = Box<dyn FnMut(PlaybackEvent) + Send + 'static>;

fn rings(
    device_rate: u32,
    device_channels: usize,
) -> (
    HeapProd<f32>,
    HeapCons<f32>,
    HeapProd<Marker>,
    HeapCons<Marker>,
) {
    let frames = (RING_SEC * f64::from(device_rate)).ceil() as usize;
    // Two layers per frame: `[dry | wet]` (Før and Etter).
    let (dp, dc) = HeapRb::<f32>::new(frames.max(1024) * device_channels.max(1) * 2).split();
    let (mp, mc) = HeapRb::<Marker>::new(1024).split();
    (dp, dc, mp, mc)
}

/// A running engine: one recording, one cut plan, one chain.
pub struct Player {
    shared: Arc<Shared>,
    plan: CutPlan,
    cmd: Sender<Command>,
    producer: Option<JoinHandle<()>>,
    reporter: Option<(Sender<()>, JoinHandle<()>)>,
    device: Option<(Sender<()>, JoinHandle<()>)>,
    device_rate: u32,
    device_channels: usize,
}

/// What the engine plays: the plan (cuts), the chain, and how to read the source.
pub struct Programme<O> {
    pub plan: CutPlan,
    pub chain: Chain,
    pub opener: O,
    /// Channels the opener delivers (1 or 2).
    pub source_channels: usize,
}

impl Player {
    /// Open the default output device and start the engine on it, paused at the start.
    pub fn open<O>(programme: Programme<O>, sink: Sink) -> Result<Player, String>
    where
        O: PcmOpener + Clone + Send + 'static,
        O::Stream: Send,
    {
        let shared = Arc::new(Shared::default());
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let source_rate = programme.plan.rate();
        let dev_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("playback-device".into())
            .spawn(move || device::run(dev_shared, source_rate, ready_tx, stop_rx))
            .map_err(|e| e.to_string())?;
        let (rate, channels, data, marks) = ready_rx.recv().map_err(|_| {
            "the playback device thread died before the device was open".to_string()
        })??;
        let mut p = Self::start(programme, shared, rate, channels, data, marks, sink)?;
        p.device = Some((stop_tx, handle));
        Ok(p)
    }

    /// The engine with no device: the caller drives the returned callback itself (tests, and
    /// anything that wants to render faster than real time).
    pub fn headless<O>(
        programme: Programme<O>,
        device_rate: u32,
        device_channels: usize,
        sink: Sink,
    ) -> Result<(Player, RtRenderer), String>
    where
        O: PcmOpener + Clone + Send + 'static,
        O::Stream: Send,
    {
        let shared = Arc::new(Shared::default());
        let (dp, dc, mp, mc) = rings(device_rate, device_channels);
        let r = RtRenderer::with_layers(
            dc,
            mc,
            shared.clone(),
            device_channels,
            device_rate,
            programme.plan.rate(),
            2,
        );
        let p = Self::start(
            programme,
            shared,
            device_rate,
            device_channels,
            dp,
            mp,
            sink,
        )?;
        Ok((p, r))
    }

    fn start<O>(
        programme: Programme<O>,
        shared: Arc<Shared>,
        device_rate: u32,
        device_channels: usize,
        data: HeapProd<f32>,
        marks: HeapProd<Marker>,
        sink: Sink,
    ) -> Result<Player, String>
    where
        O: PcmOpener + Clone + Send + 'static,
        O::Stream: Send,
    {
        let error = Arc::new(Mutex::new(None));
        shared.paused.store(true, Ordering::Release);
        // «Etter» until told otherwise (with no chain it is the same as «Før»).
        shared.wet_target.store(1.0f32.to_bits(), Ordering::Release);
        let (cmd, cmd_rx) = mpsc::channel();
        let plan = programme.plan.clone();
        let producer = Producer {
            plan: programme.plan,
            opener: programme.opener,
            chain: programme.chain,
            source_channels: programme.source_channels,
            device_rate,
            device_channels,
            data,
            marks,
            shared: shared.clone(),
            error: error.clone(),
        };
        let producer = std::thread::Builder::new()
            .name("playback-producer".into())
            .spawn(move || producer.run(cmd_rx))
            .map_err(|e| e.to_string())?;
        let (rep_tx, rep_rx) = mpsc::channel();
        let rep_shared = shared.clone();
        let rep_plan = plan.clone();
        let reporter = std::thread::Builder::new()
            .name("playback-reporter".into())
            .spawn(move || report(rep_shared, rep_plan, error, sink, rep_rx))
            .map_err(|e| e.to_string())?;
        let p = Player {
            shared,
            plan,
            cmd,
            producer: Some(producer),
            reporter: Some((rep_tx, reporter)),
            device: None,
            device_rate,
            device_channels,
        };
        let _ = p.seek_program(0.0);
        Ok(p)
    }

    pub fn plan(&self) -> &CutPlan {
        &self.plan
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    pub fn device_channels(&self) -> usize {
        self.device_channels
    }

    pub fn play(&self) {
        self.shared.paused.store(false, Ordering::Release);
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Release);
    }

    /// Choose what the ear gets: `true` = «Etter» (through the chain), `false` = «Før» (the
    /// programme as it is, cuts and all). Heard within [`rt::MIX_FADE_SEC`]: both are already in
    /// the ring, and the callback crossfades between them. Nothing is rendered, nothing waits.
    pub fn set_wet(&self, wet: bool) {
        let v: f32 = if wet { 1.0 } else { 0.0 };
        self.shared.wet_target.store(v.to_bits(), Ordering::Release);
    }

    /// Whether «Etter» is chosen.
    pub fn wet(&self) -> bool {
        f32::from_bits(self.shared.wet_target.load(Ordering::Acquire)) > 0.5
    }

    /// Change the chain while it plays: a new profile, a mixer setting. The producer builds and
    /// warms it up beside the one that runs and crossfades to it ([`producer`] says how), so
    /// the ear gets no click and no gap; it is heard after what is already in the ring (at
    /// most [`producer::LEAD_SEC`]). A newer chain replaces one still being warmed up.
    pub fn set_chain(&self, chain: Chain) {
        let _ = self.cmd.send(Command::SetChain(chain));
    }

    /// Jump to `secs` of programme time. The callback fades out, drops the old audio and fades
    /// the new in; a pause stays a pause.
    /// Returns the new generation: position events of older generations are stale.
    pub fn seek_program(&self, secs: f64) -> u64 {
        let frame = (secs.max(0.0) * f64::from(self.plan.rate())).round() as u64;
        let gen = self.shared.requested_gen.load(Ordering::Acquire) + 1;
        // The command first: the callback, seeing the new generation, discards everything up to
        // its start marker, which the producer pushes when it takes the command.
        let _ = self.cmd.send(Command::Seek {
            gen,
            program_frame: frame,
        });
        self.shared.requested_gen.store(gen, Ordering::Release);
        gen
    }

    /// Jump to `secs` of SOURCE time (a click on the waveform). Inside a cut: the next keep.
    pub fn seek_source(&self, secs: f64) -> u64 {
        self.seek_program(self.plan.source_to_program_secs(secs))
    }

    /// The programme position the device was last handed, seconds.
    pub fn position_secs(&self) -> f64 {
        self.shared.position_secs()
    }

    pub fn stats(&self) -> PlaybackStats {
        let ns = self.shared.work_ns.load(Ordering::Relaxed);
        let frames = self.shared.work_frames.load(Ordering::Relaxed);
        PlaybackStats {
            device_frames: self.shared.device_frames.load(Ordering::Relaxed),
            underruns: self.shared.underruns.load(Ordering::Relaxed),
            xruns: self.shared.xruns.load(Ordering::Relaxed),
            realtime_factor: (ns > 0 && frames > 0)
                .then(|| (frames as f64 / f64::from(self.plan.rate())) / (ns as f64 / 1e9)),
        }
    }

    /// Fade out, then stop every thread and close the device.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.pause();
        if self.device.is_some() {
            // Let the callback finish its fade (a few blocks) before the stream goes.
            let deadline = Instant::now() + Duration::from_secs_f64(FADE_SEC) * 20;
            while !self.shared.silent.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let _ = self.cmd.send(Command::Shutdown);
        if let Some(h) = self.producer.take() {
            let _ = h.join();
        }
        if let Some((tx, h)) = self.reporter.take() {
            let _ = tx.send(());
            let _ = h.join();
        }
        if let Some((tx, h)) = self.device.take() {
            let _ = tx.send(());
            let _ = h.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The reporter thread: turn the callback's atomics into events, ~30 times a second. It reads
/// only; a position event is sent when the callback has moved the position.
fn report(
    shared: Arc<Shared>,
    plan: CutPlan,
    error: Arc<Mutex<Option<String>>>,
    mut sink: Sink,
    stop: Receiver<()>,
) {
    let mut last_pos = None;
    let mut last_ended = 0;
    let mut last_under = 0;
    let mut last_xrun = 0;
    let mut told_device = false;
    loop {
        let bits = shared.position_bits.load(Ordering::Acquire);
        let gen = shared.playing_gen.load(Ordering::Acquire);
        if gen != 0 && last_pos != Some((gen, bits)) {
            last_pos = Some((gen, bits));
            let program_secs = f64::from_bits(bits);
            sink(PlaybackEvent::Position {
                gen,
                program_secs,
                source_secs: plan.program_to_source_secs(program_secs),
            });
        }
        let ended = shared.ended_gen.load(Ordering::Acquire);
        if ended != last_ended && ended != 0 {
            last_ended = ended;
            sink(PlaybackEvent::Ended { gen: ended });
        }
        let under = shared.underruns.load(Ordering::Relaxed);
        if under != last_under {
            last_under = under;
            tracing::debug!(underruns = under, "playback underrun");
            sink(PlaybackEvent::Underrun { total: under });
        }
        let xr = shared.xruns.load(Ordering::Relaxed);
        if xr != last_xrun {
            last_xrun = xr;
            sink(PlaybackEvent::Xrun { total: xr });
        }
        if !told_device && shared.device_failed.load(Ordering::Acquire) {
            told_device = true;
            sink(PlaybackEvent::Error {
                message: "the output device failed".into(),
            });
        }
        if let Some(message) = error.lock().ok().and_then(|mut e| e.take()) {
            sink(PlaybackEvent::Error { message });
        }
        match stop.recv_timeout(REPORT_EVERY) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
}

/// The device thread: open the default output, run the stream, drop it on stop.
mod device {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SizedSample};
    use sundayrec_core::stream_fault::StreamFault;

    use crate::recorder::native_capture::stream::{open_host, stream_fault, CpalHostKind};

    /// Frames the callback renders per pass when the device's format is not `f32`.
    const SCRATCH_FRAMES: usize = 4096;

    type Ready = Result<(u32, usize, HeapProd<f32>, HeapProd<Marker>), String>;

    pub(super) fn run(
        shared: Arc<Shared>,
        source_rate: u32,
        ready: Sender<Ready>,
        stop: Receiver<()>,
    ) {
        let stream = match open(shared, source_rate) {
            Ok((stream, rate, ch, dp, mp)) => {
                let _ = ready.send(Ok((rate, ch, dp, mp)));
                stream
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let _ = stop.recv();
        drop(stream);
    }

    #[allow(clippy::type_complexity)]
    fn open(
        shared: Arc<Shared>,
        source_rate: u32,
    ) -> Result<(cpal::Stream, u32, usize, HeapProd<f32>, HeapProd<Marker>), String> {
        let host = open_host(CpalHostKind::Default)?;
        let device = host
            .default_output_device()
            .ok_or_else(|| "no output device".to_string())?;
        let cfg = device
            .default_output_config()
            .map_err(|e| format!("querying the output device failed: {e}"))?;
        let rate = cfg.sample_rate();
        let ch = usize::from(cfg.channels()).max(1);
        let (dp, dc, mp, mc) = rings(rate, ch);
        let r = RtRenderer::with_layers(dc, mc, shared.clone(), ch, rate, source_rate, 2);
        let config = cpal::StreamConfig {
            channels: cfg.channels(),
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        use cpal::SampleFormat as SF;
        let stream = match cfg.sample_format() {
            SF::F32 => build::<f32>(&device, config, r, shared),
            SF::I16 => build::<i16>(&device, config, r, shared),
            SF::I32 => build::<i32>(&device, config, r, shared),
            SF::U16 => build::<u16>(&device, config, r, shared),
            SF::F64 => build::<f64>(&device, config, r, shared),
            SF::I24 => build::<cpal::I24>(&device, config, r, shared),
            other => return Err(format!("unsupported output sample format: {other:?}")),
        }
        .map_err(|e| format!("could not open the output device: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("could not start the output stream: {e}"))?;
        Ok((stream, rate, ch, dp, mp))
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut r: RtRenderer,
        shared: Arc<Shared>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let ch = r.channels();
        // Allocated here, once; the callback only borrows it.
        let mut scratch = vec![0.0f32; SCRATCH_FRAMES * ch];
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                for out in data.chunks_mut(scratch.len()) {
                    let s = &mut scratch[..out.len()];
                    r.render(s);
                    for (d, v) in out.iter_mut().zip(s.iter()) {
                        *d = T::from_sample(*v);
                    }
                }
            },
            move |e: cpal::Error| match stream_fault(e.kind()) {
                StreamFault::Xrun => {
                    shared.xruns.fetch_add(1, Ordering::Relaxed);
                }
                StreamFault::DeviceChanged | StreamFault::RealtimeDenied | StreamFault::Other => {}
                _ => shared.device_failed.store(true, Ordering::Release),
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests;
