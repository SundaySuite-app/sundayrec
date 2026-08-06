//! The NATIVE rolling pre-roll buffer: one cpal stream, one ring, one writer
//! that rotates WAV segment files — and, while it runs, the app's `vu://levels`
//! emitter.
//!
//! ## Why this replaces the ffmpeg buffer
//!
//! The classic buffer (`recorder::preroll`) ran an ffmpeg avfoundation/dshow
//! capture in a 90-second loop, re-opening the device on every segment. That
//! inherits every problem the 2026-08-01 rebuild removed from the recording
//! path — avfoundation drops samples below ffmpeg's observability — and adds one
//! of its own: it is a SECOND owner of the microphone, so the live meters and
//! the buffer could not both exist. Pre-roll therefore had to stay off while
//! anyone looked at a level bar.
//!
//! The native buffer is the same stack the recorder uses:
//!
//! ```text
//!   cpal input stream ──► RT callback ──► SPSC ring ──► rotating writer ──► N × .wav
//!   (device native fmt,   (meter ALL native chs,        (f32→s16-LE, 15 s
//!    ALL channels)         route the picked ones)        per file, keep 7)
//! ```
//!
//! and because it holds the device with a stream that already meters every
//! native channel, it can BE the VU: the stream thread samples the meters every
//! 33 ms and emits `vu://levels` itself. One owner, both jobs. See the invariant
//! documented on [`crate::audio::vu::emit_vu_levels`].
//!
//! ## Rotation, not restart
//!
//! Nothing re-opens the device: the buffer runs one continuous stream for its
//! whole life. Only the FILE rotates — every
//! [`PREROLL_NATIVE_SEGMENT_S`](sundayrec_core::preroll::PREROLL_NATIVE_SEGMENT_S)
//! seconds the writer closes the current WAV, pushes it onto the retained list
//! and deletes anything past
//! [`preroll_segments_to_retain`](sundayrec_core::preroll::preroll_segments_to_retain).
//! The retained files always cover more than the 90 s window the harvest can ask
//! for, and the harvest slices its clip straight out of their bytes (P5.2).
//!
//! A device that disappears is the only restart: the stream's error callback
//! reports it, the supervisor tears the stack down, waits the tested
//! [`preroll_restart_delay`](sundayrec_core::preroll::preroll_restart_delay)
//! back-off and re-opens — warning the user ONCE per failure streak, exactly
//! like the classic loop.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::StreamTrait;
use ringbuf::traits::{Consumer, Split};
use sundayrec_core::audio::MeterBanks;
use sundayrec_core::preroll::{
    harvest_trim_ms, preroll_captured_ms, preroll_keep_bytes, preroll_kept_ms,
    preroll_restart_delay, preroll_segments_to_drop, preroll_segments_to_retain,
    preroll_start_offset_ms, preroll_tail_slices, PREROLL_NATIVE_SEGMENT_S, PREROLL_SEGMENT_CAP_S,
    RESTART_GAP_MS,
};
use sundayrec_core::wav::{self, WavSpec};
use tauri::AppHandle;

use crate::audio::asio::build_route_plan;
use crate::audio::vu::{emit_vu_levels, VU_SAMPLE_INTERVAL};
use crate::recorder::native_capture::stream::{
    build_input_stream_any, find_device, negotiate, open_host, CpalHostKind, StreamSink,
};
use crate::recorder::native_capture::writer::{patch_sizes, FLUSH_EVERY};
use crate::recorder::preroll::{
    sleep_while_active, warn_preroll_dead, PrerollClip, PrerollSettings,
};
use crate::util::lock_recover;

/// Bound on joining the stream thread (it polls its stop flag every sampler
/// tick and then drops the stream — only a wedged CoreAudio teardown gets here).
const STREAM_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on joining the writer thread (it drains the ring, patches the last
/// header and returns).
const WRITER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Sleep when the ring is empty (mirrors the capture writer's cadence).
const IDLE_SLEEP: Duration = Duration::from_millis(2);
/// Drain scratch size in samples (f32), mirroring the capture writer.
const DRAIN_CHUNK: usize = 8192;

/// One finalized rolling segment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerollSegmentFile {
    pub path: PathBuf,
    /// PCM payload bytes (file length minus [`wav::HEADER_LEN`]).
    pub data_bytes: u64,
    /// Frames written — `data_bytes / spec.bytes_per_frame()`, tracked
    /// independently so the harvest never has to divide to learn the truth.
    pub frames: u64,
}

/// One live rolling buffer: the `!Send` stream on its own thread (doubling as
/// the VU sampler), the rotating writer thread, and the retained segment list.
struct PrerollRun {
    /// Generation, so a supervisor that lost the race can tell whether the run
    /// in the slot is still the one it published.
    gen: u64,
    stream_stop: Arc<AtomicBool>,
    writer_stop: Arc<AtomicBool>,
    stream_join: Option<JoinHandle<()>>,
    writer_join: Option<JoinHandle<()>>,
    /// Finalized segments, oldest first — published by the writer on every
    /// rotation so a bounded join that times out still leaves the truth behind.
    segments: Arc<Mutex<Vec<PrerollSegmentFile>>>,
    /// The routed capture format (what the harvest clip will carry).
    spec: WavSpec,
    /// Samples dropped by the RT callback on ring overrun.
    overrun: Arc<AtomicU64>,
}

/// The native rolling pre-roll engine. Held by the [`crate::recorder::preroll`]
/// facade, which decides between this and the classic ffmpeg engine.
#[derive(Default)]
pub struct NativePrerollEngine {
    /// `true` while the buffer should keep running/retrying.
    active: Arc<AtomicBool>,
    /// The live run, published by the supervisor, taken by harvest/stop.
    run: Arc<Mutex<Option<PrerollRun>>>,
    /// The supervisor task.
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// The live stream's NATIVE channel count (0 = no stream open). This is the
    /// width of the `vu://levels` payloads the buffer emits, so `start_vu` can
    /// answer an adopting caller without opening anything.
    channels: Arc<AtomicU16>,
    /// Run-generation counter.
    gen: Arc<AtomicU64>,
    /// Where segment files live (app-data `tmp`; tests pass a tempdir).
    tmp_dir: PathBuf,
}

impl NativePrerollEngine {
    pub fn new(tmp_dir: PathBuf) -> Self {
        Self {
            tmp_dir,
            ..Default::default()
        }
    }

    /// Whether the buffer is running (or retrying after a device loss).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// The live stream's native channel count, or `None` when no stream is
    /// currently open (starting, retrying, or stopped). `start_vu` adopts the
    /// buffer's metering with this.
    pub fn vu_channels(&self) -> Option<u16> {
        let n = self.channels.load(Ordering::SeqCst);
        (self.is_active() && n > 0).then_some(n)
    }

    /// Start the rolling buffer. Stops any previous one first. Returns
    /// immediately — the device open happens in the supervisor, which retries
    /// with the tested back-off and warns ONCE per failure streak.
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — opens a real input device.
    pub fn start(&self, app: AppHandle, settings: PrerollSettings) {
        self.stop();
        let _ = std::fs::create_dir_all(&self.tmp_dir);

        self.active.store(true, Ordering::SeqCst);
        let active = Arc::clone(&self.active);
        let run = Arc::clone(&self.run);
        let channels = Arc::clone(&self.channels);
        let gen = Arc::clone(&self.gen);
        let tmp_dir = self.tmp_dir.clone();

        let task = tauri::async_runtime::spawn(async move {
            preroll_loop(app, active, run, channels, gen, tmp_dir, settings).await;
        });
        *lock_recover(&self.task) = Some(task);
    }

    /// Stop the buffer WITHOUT harvesting, best-effort and non-blocking: the
    /// device teardown + segment cleanup finish on a detached task.
    ///
    /// Use [`Self::stop_and_release`] when the caller must know the device is
    /// free before it (or anyone else) opens it.
    pub fn stop(&self) {
        if let Some(run) = self.take_run() {
            tauri::async_runtime::spawn(async move {
                discard_run(run).await;
            });
        }
    }

    /// Stop the buffer and AWAIT the device release + segment cleanup. The
    /// stream thread is joined (bounded), so when this returns the input device
    /// really is free — the precondition for handing metering back to the VU
    /// engine.
    pub async fn stop_and_release(&self) {
        if let Some(run) = self.take_run() {
            discard_run(run).await;
        }
    }

    /// Stop the buffer and return the last `requested_seconds` of it as ONE
    /// WAV, ready to be `-c copy`-prepended to the recording. `None` when
    /// nothing usable was buffered.
    ///
    /// No ffmpeg, no decode, no re-encode: the retained segments already hold
    /// s16-LE PCM in the recording's own layout, so the clip is a byte copy of
    /// their tail behind a fresh header. The window is chosen by the same tested
    /// core mat the ffmpeg engine used — only `captured_ms` is better, coming
    /// from the writer's exact frame count instead of a wall clock.
    pub async fn harvest(&self, requested_seconds: u32) -> Option<PrerollClip> {
        let mut run = self.take_run()?;
        let spec = run.spec;
        // Stop FIRST: the device is released and the last segment finalized
        // before a single byte is read.
        stop_run_bounded(&mut run).await;
        let segments = lock_recover(&run.segments).clone();
        let clip = build_clip(&segments, spec, requested_seconds, &self.tmp_dir).await;
        // The rolling segments are consumed either way — the clip is the only
        // thing that outlives a harvest.
        delete_segments(&segments).await;
        clip
    }

    /// Wind the engine down and take the live run out of the slot, if any.
    /// Clearing `active` FIRST means the supervisor cannot publish a new run
    /// behind us.
    fn take_run(&self) -> Option<PrerollRun> {
        self.active.store(false, Ordering::SeqCst);
        self.channels.store(0, Ordering::SeqCst);
        if let Some(task) = lock_recover(&self.task).take() {
            // Safe to abort: the supervisor publishes a run into the slot
            // SYNCHRONOUSLY after spawning its threads (no await in between),
            // so an abort can never orphan a stream we don't own.
            task.abort();
        }
        lock_recover(&self.run).take()
    }
}

/// The supervisor: keep a run alive while `active`, restarting on device loss.
async fn preroll_loop(
    app: AppHandle,
    active: Arc<AtomicBool>,
    run_slot: Arc<Mutex<Option<PrerollRun>>>,
    channels: Arc<AtomicU16>,
    gen: Arc<AtomicU64>,
    tmp_dir: PathBuf,
    settings: PrerollSettings,
) {
    let mut attempt: u32 = 0;
    // Whether THIS failure streak has already told the user (P2 semantics: a
    // buffer retrying every few seconds for an hour warns once; a device that
    // comes back and dies again warns again).
    let mut warned_dead = false;

    while active.load(Ordering::SeqCst) {
        match spawn_preroll_run(&app, &tmp_dir, &settings, &run_slot, &gen, &channels).await {
            Ok(mut err_rx) => {
                attempt = 0;
                warned_dead = false;
                match wait_for_trouble(&active, &mut err_rx).await {
                    // stop/harvest took the run — it owns the teardown now.
                    Trouble::Stopped => break,
                    Trouble::Device(msg) => {
                        tracing::warn!("preroll(native): {msg} — restarting the buffer");
                        channels.store(0, Ordering::SeqCst);
                        // Take under the lock, drop the guard, THEN await — a
                        // `MutexGuard` held across an await is neither `Send`
                        // nor safe against the harvest taking the same slot.
                        let dead = lock_recover(&run_slot).take();
                        if let Some(run) = dead {
                            discard_run(run).await;
                        }
                        if !sleep_while_active(&active, RESTART_GAP_MS).await {
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let delay = preroll_restart_delay(attempt);
                tracing::warn!(
                    attempt,
                    delay,
                    "preroll(native): could not open device: {e}"
                );
                if sundayrec_core::notify::should_warn_preroll_dead(attempt, warned_dead) {
                    warned_dead = true;
                    warn_preroll_dead(
                        &app,
                        "device_open_failed",
                        "Forhåndsbufferen får ikke åpnet lydenheten — det som skjer før du trykker \
                         opptak blir ikke tatt vare på.",
                    );
                }
                attempt = attempt.saturating_add(1);
                if !sleep_while_active(&active, delay).await {
                    break;
                }
            }
        }
    }
}

/// Why the supervisor stopped waiting on a live run.
enum Trouble {
    /// `active` was cleared — stop/harvest owns the run.
    Stopped,
    /// The stream or the writer failed; the run must be rebuilt.
    Device(String),
}

/// Park until the run reports an error or the engine is stopped. Polls `active`
/// in 100 ms slices so a stop is prompt; `recv` is cancel-safe.
async fn wait_for_trouble(
    active: &Arc<AtomicBool>,
    err_rx: &mut tokio::sync::mpsc::Receiver<String>,
) -> Trouble {
    loop {
        if !active.load(Ordering::SeqCst) {
            return Trouble::Stopped;
        }
        match tokio::time::timeout(Duration::from_millis(100), err_rx.recv()).await {
            Ok(Some(msg)) => return Trouble::Device(msg),
            Ok(None) => return Trouble::Device("capture threads ended".into()),
            Err(_) => continue,
        }
    }
}

/// Open the device, start the stream + rotating writer, publish the run into
/// `slot`, and return the error channel both threads report on.
///
/// ⚠️ HARDWARE-UNVERIFIED — opens a real input device.
async fn spawn_preroll_run(
    app: &AppHandle,
    tmp_dir: &Path,
    settings: &PrerollSettings,
    slot: &Arc<Mutex<Option<PrerollRun>>>,
    gen: &Arc<AtomicU64>,
    channels: &Arc<AtomicU16>,
) -> Result<tokio::sync::mpsc::Receiver<String>, String> {
    let device_name = settings.audio_device_name.clone();
    let requested_rate = settings.sample_rate;

    // Probe on a blocking thread: the (!Send) device handle never escapes.
    let negotiated = {
        let name = device_name.clone();
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let h = open_host(CpalHostKind::Default)?;
            let device = find_device(&h, &name)?;
            negotiate(&device, requested_rate)
        })
        .await
        .map_err(|e| format!("device probe task failed: {e}"))??
    };

    // The buffer must write the SAME layout the recording will, or the harvest
    // clip is dropped by the concat's `wav_prepend_compatible` guard.
    let plan = build_route_plan(
        settings.channel_mode,
        settings.input_channel_l,
        settings.input_channel_r,
        negotiated.channels,
    );
    let out_ch = plan.len().max(1) as u16;
    let spec = WavSpec {
        channels: out_ch,
        sample_rate: negotiated.sample_rate,
    };
    let frames_per_segment = u64::from(spec.sample_rate) * u64::from(PREROLL_NATIVE_SEGMENT_S);
    let retain = preroll_segments_to_retain(PREROLL_SEGMENT_CAP_S, PREROLL_NATIVE_SEGMENT_S);
    tracing::info!(
        device = %device_name,
        rate = spec.sample_rate,
        native_channels = negotiated.channels,
        routed_channels = out_ch,
        format = ?negotiated.sample_format,
        retain,
        "preroll(native): rolling buffer starting"
    );

    // ≥1 s of routed audio, exactly like the capture engine's ring.
    let ring_capacity = (spec.sample_rate as usize * out_ch as usize).max(96_000);
    let (prod, cons) = ringbuf::HeapRb::<f32>::new(ring_capacity).split();

    let stream_stop = Arc::new(AtomicBool::new(false));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let overrun = Arc::new(AtomicU64::new(0));
    let segments: Arc<Mutex<Vec<PrerollSegmentFile>>> = Arc::new(Mutex::new(Vec::new()));
    // Meters are sized to the NATIVE width — the buffer is the VU emitter.
    let meters = Arc::new(MeterBanks::new(negotiated.channels.max(1) as usize));
    let (err_tx, err_rx) = tokio::sync::mpsc::channel::<String>(2);

    // Writer first, so the stream never produces into a ring nobody drains.
    let writer_cfg = RotatingWriterConfig {
        spec,
        dir: tmp_dir.to_path_buf(),
        id: segment_id(),
        frames_per_segment,
        retain,
    };
    let w_stop = Arc::clone(&writer_stop);
    let w_segments = Arc::clone(&segments);
    let w_err = err_tx.clone();
    let writer_join = std::thread::Builder::new()
        .name("preroll-writer".into())
        .spawn(move || run_rotating_writer(cons, writer_cfg, w_stop, w_segments, w_err))
        .map_err(|e| format!("could not spawn pre-roll writer thread: {e}"))?;

    // Stream thread: reopen host+device (the Stream is !Send), build, play,
    // report readiness, then SAMPLE THE METERS until stopped — this thread
    // would otherwise only park, and the buffer is the VU while it runs.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let st_stop = Arc::clone(&stream_stop);
    let st_meters = Arc::clone(&meters);
    let st_overrun = Arc::clone(&overrun);
    let st_name = device_name.clone();
    let st_app = app.clone();
    let st_err = err_tx;
    let stream_join = std::thread::Builder::new()
        .name("preroll-capture".into())
        .spawn(move || {
            let build = (|| -> Result<cpal::Stream, String> {
                let h = open_host(CpalHostKind::Default)?;
                let device = find_device(&h, &st_name)?;
                let config = cpal::StreamConfig {
                    channels: negotiated.channels,
                    sample_rate: negotiated.sample_rate,
                    buffer_size: cpal::BufferSize::Default,
                };
                let err_fn = move |e: cpal::StreamError| {
                    tracing::error!("preroll(native) stream error: {e}");
                    let _ = st_err.try_send(e.to_string());
                };
                let stream = build_input_stream_any(
                    &device,
                    &config,
                    negotiated.sample_format,
                    negotiated.channels as usize,
                    StreamSink::Preroll {
                        meters: Arc::clone(&st_meters),
                        plan,
                        prod,
                        overrun: st_overrun,
                    },
                    err_fn,
                )?;
                stream.play().map_err(|e| format!("starting stream: {e}"))?;
                Ok(stream)
            })();
            match build {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    while !st_stop.load(Ordering::Relaxed) {
                        // A failed emit (window closed) must NOT take the buffer
                        // down — recording the seconds before the press is the
                        // job; metering is the passenger.
                        let _ = emit_vu_levels(&st_app, &st_meters);
                        std::thread::sleep(VU_SAMPLE_INTERVAL);
                    }
                    drop(stream); // stops capture, releases the device
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| format!("could not spawn pre-roll capture thread: {e}"))?;

    // PUBLISH BEFORE AWAITING: from here on the threads are reachable from the
    // slot, so a `stop()` (which aborts this task) always finds and stops them.
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
    *lock_recover(slot) = Some(PrerollRun {
        gen: my_gen,
        stream_stop,
        writer_stop,
        stream_join: Some(stream_join),
        writer_join: Some(writer_join),
        segments,
        spec,
        overrun,
    });

    let ready = match ready_rx.await {
        Ok(r) => r,
        Err(_) => Err("pre-roll capture thread exited before signalling".to_string()),
    };
    match ready {
        Ok(()) => {
            channels.store(negotiated.channels, Ordering::SeqCst);
            Ok(err_rx)
        }
        Err(e) => {
            // Take OUR run back (a stop may already have taken it) and tear down.
            let mine = {
                let mut guard = lock_recover(slot);
                match guard.as_ref() {
                    Some(r) if r.gen == my_gen => guard.take(),
                    _ => None,
                }
            };
            if let Some(run) = mine {
                discard_run(run).await;
            }
            Err(e)
        }
    }
}

/// Stop a run's threads (stream first — producer gone — then the writer, which
/// drains the ring and patches its last header) with bounded joins, so a wedged
/// teardown can never freeze a record start.
async fn stop_run_bounded(run: &mut PrerollRun) {
    run.stream_stop.store(true, Ordering::Relaxed);
    if let Some(h) = run.stream_join.take() {
        let join = tokio::task::spawn_blocking(move || {
            let _ = h.join();
        });
        if tokio::time::timeout(STREAM_JOIN_TIMEOUT, join)
            .await
            .is_err()
        {
            tracing::warn!("preroll(native): stream thread did not stop in time — continuing");
        }
    }
    run.writer_stop.store(true, Ordering::Relaxed);
    if let Some(h) = run.writer_join.take() {
        let join = tokio::task::spawn_blocking(move || {
            let _ = h.join();
        });
        if tokio::time::timeout(WRITER_JOIN_TIMEOUT, join)
            .await
            .is_err()
        {
            tracing::warn!("preroll(native): writer thread did not finalize in time");
        }
    }
    // Health of the run that just ended: ring overruns are the ONLY way the
    // native buffer can lose audio, so they are never silent.
    let overrun = run.overrun.load(Ordering::Relaxed);
    if overrun > 0 {
        tracing::warn!(overrun, "preroll(native): ring overran — samples dropped");
    }
    tracing::info!(
        rate = run.spec.sample_rate,
        channels = run.spec.channels,
        segments = lock_recover(&run.segments).len(),
        "preroll(native): buffer stopped"
    );
}

/// Stop a run and delete every segment file it produced (the non-harvest path:
/// an un-harvested buffer is litter, not data).
async fn discard_run(mut run: PrerollRun) {
    stop_run_bounded(&mut run).await;
    let segments = lock_recover(&run.segments).clone();
    delete_segments(&segments).await;
}

/// Remove segment files, best-effort.
pub(crate) async fn delete_segments(segments: &[PrerollSegmentFile]) {
    for s in segments {
        let _ = tokio::fs::remove_file(&s.path).await;
    }
}

// ── The harvest ──────────────────────────────────────────────────────────────

/// Turn the retained segments into one clip holding the last
/// `requested_seconds` (or as much as exists). Free-standing so the whole
/// harvest decision + stitch is testable against segment files on disk without
/// a device.
async fn build_clip(
    segments: &[PrerollSegmentFile],
    spec: WavSpec,
    requested_seconds: u32,
    tmp_dir: &Path,
) -> Option<PrerollClip> {
    let bpf = spec.bytes_per_frame();
    let frames: u64 = segments.iter().map(|s| s.frames).sum();
    // The "did we capture anything real?" gate is the classic engine's, applied
    // to the same quantity it always meant: bytes on disk.
    let bytes_on_disk: u64 = segments
        .iter()
        .map(|s| s.data_bytes + wav::HEADER_LEN as u64)
        .sum();
    let captured_ms = preroll_captured_ms(frames, spec.sample_rate);
    let trim_ms = harvest_trim_ms(captured_ms, requested_seconds, bytes_on_disk)?;

    let lens: Vec<u64> = segments.iter().map(|s| s.data_bytes).collect();
    let slices = preroll_tail_slices(
        &lens,
        bpf,
        preroll_keep_bytes(trim_ms, spec.sample_rate, bpf),
    );
    if slices.is_empty() {
        return None;
    }
    // Report what the clip REALLY holds, not what was asked for: the slicing
    // rounds to whole frames, so these can differ by up to one frame.
    let kept_bytes: u64 = slices.iter().map(|s| s.len).sum();
    let trim_ms = preroll_kept_ms(kept_bytes, spec.sample_rate, bpf);
    let start_offset_ms = preroll_start_offset_ms(captured_ms, trim_ms);

    let out = tmp_dir.join(format!("sundayrec-preroll-clip-{}.wav", segment_id()));
    let paths: Vec<PathBuf> = segments.iter().map(|s| s.path.clone()).collect();
    let out_for_blocking = out.clone();
    let stitched =
        tokio::task::spawn_blocking(move || stitch_clip(&paths, &slices, spec, &out_for_blocking))
            .await;
    match stitched {
        Ok(Ok(())) => {
            tracing::info!(
                trim_ms,
                start_offset_ms,
                captured_ms,
                rate = spec.sample_rate,
                channels = spec.channels,
                clip = %out.display(),
                "preroll(native): harvested clip (byte copy, no ffmpeg)"
            );
            Some(PrerollClip {
                raw_path: out.to_string_lossy().into_owned(),
                trim_ms,
                start_offset_ms,
            })
        }
        Ok(Err(e)) => {
            tracing::warn!("preroll(native): could not stitch the clip: {e}");
            let _ = std::fs::remove_file(&out);
            None
        }
        Err(e) => {
            tracing::warn!("preroll(native): stitch task failed: {e}");
            let _ = std::fs::remove_file(&out);
            None
        }
    }
}

/// Write `slices` (in order) out of `paths` into one WAV at `out`.
///
/// Blocking, byte-for-byte: a fresh 44-byte header for the total payload, then
/// each range copied straight out of its segment's data. The result is exactly
/// the same PCM the recorder is about to write, which is what makes the concat
/// prepend a lossless `-c copy`.
fn stitch_clip(
    paths: &[PathBuf],
    slices: &[sundayrec_core::preroll::PrerollSlice],
    spec: WavSpec,
    out: &Path,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let total: u64 = slices.iter().map(|s| s.len).sum();
    let file = File::create(out)?;
    let mut w = BufWriter::with_capacity(256 * 1024, file);
    // The retained window can never approach the u32 RIFF ceiling (90 s at
    // 96 kHz stereo is ~35 MB), but saturate rather than wrap regardless.
    w.write_all(&wav::header(spec, total.min(u64::from(u32::MAX)) as u32))?;

    let mut buf = vec![0u8; 256 * 1024];
    for s in slices {
        let path = paths.get(s.segment).ok_or_else(|| {
            std::io::Error::other(format!("slice references missing segment {}", s.segment))
        })?;
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(wav::HEADER_LEN as u64 + s.start))?;
        let mut left = s.len;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..want])?;
            w.write_all(&buf[..want])?;
            left -= want as u64;
        }
    }
    w.flush()?;
    w.get_ref().sync_all()?;
    Ok(())
}

// ── The rotating writer ──────────────────────────────────────────────────────

/// Everything the rotating writer needs, fixed at spawn.
struct RotatingWriterConfig {
    spec: WavSpec,
    dir: PathBuf,
    /// Shared prefix for this buffer's files, so two engines never collide.
    id: String,
    frames_per_segment: u64,
    retain: usize,
}

/// One WAV file being written.
struct OpenSegment {
    path: PathBuf,
    out: BufWriter<File>,
    /// Positional handle for in-place RIFF size patching (see the capture
    /// writer: on Windows `seek_write` moves THIS handle's cursor).
    patch: File,
    data_bytes: u64,
    frames: u64,
}

/// Open segment `seq` and write its (zero-length) header.
fn open_segment(cfg: &RotatingWriterConfig, seq: u64) -> std::io::Result<OpenSegment> {
    let path = cfg
        .dir
        .join(format!("sundayrec-preroll-native-{}-{seq}.wav", cfg.id));
    let file = File::create(&path)?;
    let patch = OpenOptions::new().write(true).open(&path)?;
    let mut out = BufWriter::with_capacity(64 * 1024, file);
    out.write_all(&wav::header(cfg.spec, 0))?;
    out.flush()?;
    Ok(OpenSegment {
        path,
        out,
        patch,
        data_bytes: 0,
        frames: 0,
    })
}

/// Finalize the open segment (flush + final header patch) and return its record.
fn close_segment(seg: &mut OpenSegment) -> std::io::Result<PrerollSegmentFile> {
    seg.out.flush()?;
    patch_sizes(&seg.patch, seg.data_bytes)?;
    Ok(PrerollSegmentFile {
        path: seg.path.clone(),
        data_bytes: seg.data_bytes,
        frames: seg.frames,
    })
}

/// The writer body: drain the ring into rotating WAV files, retaining the last
/// `cfg.retain` of them.
///
/// Unlike the capture writer this never fsyncs and emits no events: the files
/// are ephemeral (a harvest reads them seconds later, a stop deletes them), and
/// the only thing anyone needs to hear about is a failure — which goes out on
/// `err_tx` so the supervisor rebuilds the whole stack.
fn run_rotating_writer(
    mut cons: ringbuf::HeapCons<f32>,
    cfg: RotatingWriterConfig,
    stop: Arc<AtomicBool>,
    segments: Arc<Mutex<Vec<PrerollSegmentFile>>>,
    err_tx: tokio::sync::mpsc::Sender<String>,
) {
    let channels = cfg.spec.channels.max(1) as usize;
    let mut seq: u64 = 0;
    let mut seg = match open_segment(&cfg, seq) {
        Ok(s) => s,
        Err(e) => {
            let _ = err_tx.try_send(format!("creating pre-roll segment: {e}"));
            return;
        }
    };

    let mut samples = vec![0.0f32; DRAIN_CHUNK];
    let mut bytes: Vec<u8> = Vec::with_capacity(DRAIN_CHUNK * 2);
    let mut last_flush = Instant::now();

    loop {
        let n = cons.pop_slice(&mut samples);
        if n > 0 {
            let block = &samples[..n];
            debug_assert_eq!(n % channels, 0, "ring drain must be frame-aligned");
            wav::encode_s16le(block, &mut bytes);
            if let Err(e) = seg.out.write_all(&bytes) {
                let _ = err_tx.try_send(format!("writing pre-roll segment: {e}"));
                return;
            }
            seg.data_bytes += bytes.len() as u64;
            seg.frames += (n / channels) as u64;
        } else if stop.load(Ordering::Relaxed) {
            break; // stop requested and the ring is drained
        } else {
            std::thread::sleep(IDLE_SLEEP);
        }

        // Rotate on the frame budget: close this file, retire the oldest.
        if seg.frames >= cfg.frames_per_segment {
            match close_segment(&mut seg) {
                Ok(done) => retire(&segments, done, cfg.retain),
                Err(e) => {
                    let _ = err_tx.try_send(format!("closing pre-roll segment: {e}"));
                    return;
                }
            }
            seq += 1;
            seg = match open_segment(&cfg, seq) {
                Ok(s) => s,
                Err(e) => {
                    let _ = err_tx.try_send(format!("rotating pre-roll segment: {e}"));
                    return;
                }
            };
            last_flush = Instant::now();
            continue;
        }

        // Periodic flush + header patch: a segment file on disk is always a
        // valid, playable WAV, so a crash-recovery scan or a debugging owner
        // never meets a header claiming zero bytes.
        if last_flush.elapsed() >= FLUSH_EVERY {
            if let Err(e) = seg
                .out
                .flush()
                .and_then(|()| patch_sizes(&seg.patch, seg.data_bytes))
            {
                let _ = err_tx.try_send(format!("flushing pre-roll segment: {e}"));
                return;
            }
            last_flush = Instant::now();
        }
    }

    // Stopped: finalize the partial segment so the harvest can slice it.
    match close_segment(&mut seg) {
        Ok(done) => retire(&segments, done, cfg.retain),
        Err(e) => {
            let _ = err_tx.try_send(format!("finalizing pre-roll segment: {e}"));
        }
    }
}

/// Push a finalized segment onto the retained list and delete anything past
/// `retain` (oldest first). Deletion is synchronous — this runs on the writer
/// thread, where a blocking unlink of a temp file is cheap and ordering matters
/// more than latency.
fn retire(segments: &Arc<Mutex<Vec<PrerollSegmentFile>>>, done: PrerollSegmentFile, retain: usize) {
    let mut guard = lock_recover(segments);
    guard.push(done);
    let drop_count = preroll_segments_to_drop(guard.len(), retain);
    for old in guard.drain(..drop_count) {
        let _ = std::fs::remove_file(&old.path);
    }
}

/// A short id for this buffer's segment filenames, derived from the wall clock.
fn segment_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Producer;
    use ringbuf::HeapRb;

    const SPEC: WavSpec = WavSpec {
        channels: 2,
        sample_rate: 48_000,
    };

    fn cfg(dir: &Path, frames_per_segment: u64, retain: usize) -> RotatingWriterConfig {
        RotatingWriterConfig {
            spec: SPEC,
            dir: dir.to_path_buf(),
            id: "test".into(),
            frames_per_segment,
            retain,
        }
    }

    /// Interleaved stereo block (L=+0.5, R=−0.5) of `frames` frames.
    fn block(frames: usize) -> Vec<f32> {
        (0..frames * 2)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    struct WriterRig {
        prod: ringbuf::HeapProd<f32>,
        stop: Arc<AtomicBool>,
        segments: Arc<Mutex<Vec<PrerollSegmentFile>>>,
        err: tokio::sync::mpsc::Receiver<String>,
        join: JoinHandle<()>,
        _dir: tempfile::TempDir,
        dir: PathBuf,
    }

    fn spawn_writer(frames_per_segment: u64, retain: usize) -> WriterRig {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        let (prod, cons) = HeapRb::<f32>::new(1 << 18).split();
        let stop = Arc::new(AtomicBool::new(false));
        let segments = Arc::new(Mutex::new(Vec::new()));
        let (tx, err) = tokio::sync::mpsc::channel::<String>(2);
        let c = cfg(&path, frames_per_segment, retain);
        let s = Arc::clone(&stop);
        let segs = Arc::clone(&segments);
        let join = std::thread::spawn(move || run_rotating_writer(cons, c, s, segs, tx));
        WriterRig {
            prod,
            stop,
            segments,
            err,
            join,
            _dir: dir,
            dir: path,
        }
    }

    /// The rotation sequence number encoded in a segment filename.
    fn seq_of(path: &Path) -> u64 {
        path.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('-').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unparseable segment name: {path:?}"))
    }

    fn wav_files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("read dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn writes_one_valid_segment_and_finalizes_on_stop() {
        let mut rig = spawn_writer(48_000, 7);
        let frames = 2_400usize;
        assert_eq!(rig.prod.push_slice(&block(frames)), frames * 2);
        std::thread::sleep(Duration::from_millis(60));
        rig.stop.store(true, Ordering::Relaxed);
        rig.join.join().expect("writer joins");

        let segs = lock_recover(&rig.segments).clone();
        assert_eq!(segs.len(), 1, "one finalized segment");
        assert_eq!(segs[0].frames, frames as u64);
        assert_eq!(segs[0].data_bytes, (frames * 2 * 2) as u64);

        let on_disk = std::fs::read(&segs[0].path).expect("segment readable");
        assert_eq!(
            on_disk.len() as u64,
            wav::HEADER_LEN as u64 + segs[0].data_bytes
        );
        let info = wav::parse_header(&on_disk).expect("valid header");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.format_tag, 1);
        assert_eq!(info.bits_per_sample, 16);
        // The header's data field is final, not the zero it opened with.
        let data_field = u32::from_le_bytes(on_disk[40..44].try_into().unwrap());
        assert_eq!(u64::from(data_field), segs[0].data_bytes);
        assert!(rig.err.try_recv().is_err(), "no writer errors");
    }

    #[test]
    fn rotates_and_retires_the_oldest_beyond_retain() {
        // 100-frame segments, retain 3: push 10 segments' worth and only the
        // last three may survive — on disk AND in the list.
        let mut rig = spawn_writer(100, 3);
        for _ in 0..10 {
            assert_eq!(rig.prod.push_slice(&block(100)), 200);
            std::thread::sleep(Duration::from_millis(8));
        }
        std::thread::sleep(Duration::from_millis(40));
        rig.stop.store(true, Ordering::Relaxed);
        rig.join.join().expect("writer joins");

        let segs = lock_recover(&rig.segments).clone();
        assert_eq!(
            segs.len(),
            3,
            "retained exactly `retain` segments: {segs:?}"
        );
        // Oldest first, contiguous sequence numbers — the list order IS the
        // chronological order the harvest slices along.
        let seqs: Vec<u64> = segs.iter().map(|s| seq_of(&s.path)).collect();
        assert!(
            seqs.windows(2).all(|w| w[1] == w[0] + 1),
            "oldest first, contiguous: {seqs:?}"
        );
        // The survivors are the NEWEST files: nothing before them is left on
        // disk, and the newest retained is the highest sequence ever opened.
        // (How MANY rotations happen depends on drain granularity — a single
        // drain can carry more than one segment's worth — so the sequence
        // numbers themselves are not asserted, only that they are the last.)
        let max_seq = *seqs.last().unwrap();
        assert!(max_seq >= 3, "the buffer really rotated: {seqs:?}");
        // The retired files are really gone; nothing else litters the dir.
        let on_disk = wav_files(&rig.dir);
        assert_eq!(
            on_disk.len(),
            3,
            "only the retained files remain: {on_disk:?}"
        );
        for s in &segs {
            assert!(s.path.exists(), "retained segment missing: {:?}", s.path);
        }
    }

    #[test]
    fn every_segment_is_frame_aligned_and_playable() {
        // Frame alignment across rotations is what makes the harvest's byte
        // slicing safe: no segment may end mid-frame.
        let mut rig = spawn_writer(64, 4);
        for _ in 0..12 {
            assert_eq!(rig.prod.push_slice(&block(48)), 96);
            std::thread::sleep(Duration::from_millis(6));
        }
        std::thread::sleep(Duration::from_millis(40));
        rig.stop.store(true, Ordering::Relaxed);
        rig.join.join().expect("writer joins");

        let segs = lock_recover(&rig.segments).clone();
        assert!(!segs.is_empty());
        let bpf = SPEC.bytes_per_frame();
        for s in &segs {
            assert_eq!(s.data_bytes % bpf, 0, "segment ends mid-frame: {s:?}");
            assert_eq!(s.data_bytes / bpf, s.frames);
            let on_disk = std::fs::read(&s.path).expect("readable");
            assert!(wav::parse_header(&on_disk).is_some());
            assert_eq!(on_disk.len() as u64, wav::HEADER_LEN as u64 + s.data_bytes);
        }
    }

    #[test]
    fn an_unwritable_directory_reports_instead_of_panicking() {
        let (_prod, cons) = HeapRb::<f32>::new(64).split();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(2);
        let c = cfg(Path::new("/nonexistent-dir-sundayrec-preroll"), 48_000, 7);
        run_rotating_writer(
            cons,
            c,
            Arc::new(AtomicBool::new(true)),
            Arc::new(Mutex::new(Vec::new())),
            tx,
        );
        let msg = rx.try_recv().expect("an error was reported");
        assert!(msg.contains("creating pre-roll segment"), "{msg}");
    }

    #[test]
    fn engine_starts_inactive_and_stop_is_safe_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let engine = NativePrerollEngine::new(dir.path().to_path_buf());
        assert!(!engine.is_active());
        assert_eq!(engine.vu_channels(), None);
        engine.stop();
        engine.stop();
        assert!(!engine.is_active());
    }

    // ── The harvest ──────────────────────────────────────────────────────────

    /// Write a segment file whose every frame CARRIES ITS GLOBAL INDEX, so a
    /// stitched clip can be checked frame-by-frame against the exact bytes it
    /// should hold — the "byte-exact" claim, verified rather than assumed.
    fn write_segment(
        dir: &Path,
        seq: u64,
        spec: WavSpec,
        first_frame: u64,
        frames: u64,
    ) -> PrerollSegmentFile {
        let mut data = Vec::with_capacity((frames * spec.bytes_per_frame()) as usize);
        for k in 0..frames {
            let idx = (first_frame + k) as i16;
            for ch in 0..spec.channels {
                // Channel 0 carries +index, channel 1 −index: a swapped or
                // misaligned frame is immediately visible.
                let v = if ch == 0 { idx } else { idx.wrapping_neg() };
                data.extend_from_slice(&v.to_le_bytes());
            }
        }
        let path = dir.join(format!("seg-{seq}.wav"));
        let mut bytes = wav::header(spec, data.len() as u32).to_vec();
        bytes.extend_from_slice(&data);
        std::fs::write(&path, &bytes).expect("write segment");
        PrerollSegmentFile {
            path,
            data_bytes: data.len() as u64,
            frames,
        }
    }

    /// Decode a stitched clip into its (channel-0) frame indices.
    fn clip_frames(path: &Path, spec: WavSpec) -> Vec<i16> {
        let bytes = std::fs::read(path).expect("clip readable");
        bytes[wav::HEADER_LEN..]
            .chunks_exact(spec.bytes_per_frame() as usize)
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect()
    }

    #[tokio::test]
    async fn harvest_stitches_the_exact_tail_frames() {
        // 1 kHz stereo so a "second" is 1000 frames: three segments of 1000,
        // harvest 2 s → the last 2000 frames, spanning two of the three.
        let spec = WavSpec {
            channels: 2,
            sample_rate: 1_000,
        };
        let dir = tempfile::tempdir().unwrap();
        let segments: Vec<PrerollSegmentFile> = (0..3)
            .map(|i| write_segment(dir.path(), i, spec, i * 1_000, 1_000))
            .collect();

        let clip = build_clip(&segments, spec, 2, dir.path())
            .await
            .expect("a clip");
        // 3 s captured − 300 ms margin > 2 s requested, so the request wins.
        assert_eq!(clip.trim_ms, 2_000);
        assert_eq!(clip.start_offset_ms, 1_000, "the last 2 s of a 3 s buffer");

        let path = PathBuf::from(&clip.raw_path);
        let info = wav::parse_header(&std::fs::read(&path).unwrap()).expect("valid wav");
        assert_eq!(info.sample_rate, 1_000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.format_tag, 1);
        assert_eq!(info.bits_per_sample, 16);

        let frames = clip_frames(&path, spec);
        assert_eq!(frames.len(), 2_000, "exactly the requested window");
        // BYTE-EXACT: the clip starts at global frame 1000 and runs contiguously
        // across the segment seam without a repeated or dropped frame.
        assert_eq!(frames[0], 1_000);
        assert_eq!(frames[999], 1_999);
        assert_eq!(frames[1_000], 2_000, "no discontinuity at the seam");
        assert_eq!(*frames.last().unwrap(), 2_999);
        assert!(
            frames.windows(2).all(|w| w[1] == w[0] + 1),
            "frames must be strictly contiguous"
        );
    }

    #[tokio::test]
    async fn harvest_takes_only_what_the_buffer_holds() {
        // The buffer only ran 2 s but 30 s were asked for: keep 2 s − the 300 ms
        // safety margin, and start the offset at the margin.
        let spec = WavSpec {
            channels: 2,
            sample_rate: 1_000,
        };
        let dir = tempfile::tempdir().unwrap();
        let segments = vec![write_segment(dir.path(), 0, spec, 0, 2_000)];
        let clip = build_clip(&segments, spec, 30, dir.path())
            .await
            .expect("a clip");
        assert_eq!(clip.trim_ms, 1_700);
        assert_eq!(clip.start_offset_ms, 300);
        let frames = clip_frames(Path::new(&clip.raw_path), spec);
        assert_eq!(frames.len(), 1_700);
        assert_eq!(frames[0], 300, "the oldest 300 ms are the ones dropped");
    }

    #[tokio::test]
    async fn harvest_spans_every_retained_segment_when_asked_for_everything() {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 1_000,
        };
        let dir = tempfile::tempdir().unwrap();
        let segments: Vec<PrerollSegmentFile> = (0..7)
            .map(|i| write_segment(dir.path(), i, spec, i * 500, 500))
            .collect();
        // 3.5 s buffered, 60 s requested → everything but the 300 ms margin.
        let clip = build_clip(&segments, spec, 60, dir.path())
            .await
            .expect("a clip");
        assert_eq!(clip.trim_ms, 3_200);
        let frames = clip_frames(Path::new(&clip.raw_path), spec);
        assert_eq!(frames.len(), 3_200);
        assert_eq!(frames[0], 300);
        assert!(frames.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[tokio::test]
    async fn nothing_captured_yields_no_clip() {
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
        };
        let dir = tempfile::tempdir().unwrap();
        // No segments at all.
        assert!(build_clip(&[], spec, 15, dir.path()).await.is_none());
        // A buffer that only just opened: below MIN_VALID_SEGMENT_BYTES.
        let tiny = vec![write_segment(dir.path(), 0, spec, 0, 100)]; // 400 B + header
        assert!(build_clip(&tiny, spec, 15, dir.path()).await.is_none());
        // Big enough on disk, but shorter than the 300 ms safety margin.
        let short = vec![write_segment(dir.path(), 1, spec, 0, 4_800)]; // 100 ms
        assert!(build_clip(&short, spec, 15, dir.path()).await.is_none());
        // No clip file was left behind by any of those.
        let clips: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("preroll-clip"))
            .collect();
        assert!(clips.is_empty(), "no half-made clips: {clips:?}");
    }

    #[tokio::test]
    async fn a_missing_segment_file_fails_the_stitch_cleanly() {
        // The list says a segment exists but the file is gone (a temp sweep, a
        // full disk): no clip, no panic, no stray output.
        let spec = WavSpec {
            channels: 2,
            sample_rate: 1_000,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut segments = vec![write_segment(dir.path(), 0, spec, 0, 4_000)];
        std::fs::remove_file(&segments[0].path).unwrap();
        segments[0].path = dir.path().join("gone.wav");
        assert!(build_clip(&segments, spec, 2, dir.path()).await.is_none());
    }

    /// The clip must survive `concat::wav_prepend_compatible`, which parses both
    /// headers and demands identical s16 PCM. This asserts against the same core
    /// functions that guard uses.
    #[tokio::test]
    async fn the_clip_is_copy_compatible_with_the_capture_it_prepends() {
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
        };
        let dir = tempfile::tempdir().unwrap();
        let segments = vec![write_segment(dir.path(), 0, spec, 0, 96_000)]; // 2 s
        let clip = build_clip(&segments, spec, 1, dir.path())
            .await
            .expect("a clip");
        let clip_info = wav::parse_header(&std::fs::read(&clip.raw_path).unwrap()).unwrap();

        // What the capture engine writes for the same negotiated format.
        let capture_info = wav::parse_header(&wav::header(spec, 0)).unwrap();
        assert!(
            clip_info.copy_compatible_with(&capture_info),
            "clip {clip_info:?} vs capture {capture_info:?}"
        );

        // And the safety net: a buffer that negotiated a DIFFERENT rate (the
        // device changed between `preroll_start` and the record press) is
        // rejected by that same guard rather than corrupting the deliverable.
        let other = wav::parse_header(&wav::header(
            WavSpec {
                channels: 2,
                sample_rate: 44_100,
            },
            0,
        ))
        .unwrap();
        assert!(!clip_info.copy_compatible_with(&other));
        // Same for a channel-count change (mono capture, stereo buffer).
        let mono = wav::parse_header(&wav::header(
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
            },
            0,
        ))
        .unwrap();
        assert!(!clip_info.copy_compatible_with(&mono));
    }

    #[tokio::test]
    async fn harvest_on_an_idle_engine_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let engine = NativePrerollEngine::new(dir.path().to_path_buf());
        assert!(engine.harvest(15).await.is_none());
    }
}
