//! The native segment runner: one capture-fragment life (device open →
//! stream → ring → writer → finalized WAV), plugged into `run_session`'s
//! supervisor loop as a drop-in sibling of the ffmpeg `run_segment`.
//!
//! The select! arms deliberately MIRROR `engine::run_segment`'s (startup
//! watchdog, byte watchdog, disk guard, split/auto-stop timers, silence
//! timers, stop) rather than being generic over the child — the stop
//! semantics differ (flag+join vs `q`+wait), and the duplication keeps both
//! paths reviewable against each other.
//!
//! Signal mapping vs the ffmpeg path:
//! - `size=` progress lines   → the writer's `segment_bytes` atomic
//! - first `size=` (Started)  → `WriterEvent::Started` (first block flushed)
//! - astats levels lines      → 33 ms sampler over the routed `MeterBanks`
//! - silencedetect markers    → `WriterEvent::Silence` (in-process detector)
//! - stderr error classify    → typed `cpal::StreamError` / `WriterErrorKind`
//! - `q` + wait (finalize)    → stop flags + bounded joins (writer patches
//!   the RIFF header and fsyncs)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::StreamTrait;
use ringbuf::traits::Split;
use sundayrec_core::levels::SILENCE_FLOOR_DB;
use sundayrec_core::preflight::{
    finalize_reserve_bytes, low_disk_should_stop, min_disk_headroom_bytes,
};
use sundayrec_core::reconnect::{WatchdogState, WatchdogVerdict};
use sundayrec_core::recorder::{RecorderState, RecordingSession};
use sundayrec_core::silence::{
    silence_threshold_db, SilenceAction, SilenceDetector, SilenceWatcher,
};
use sundayrec_core::timeouts::RecorderTimeouts;
use sundayrec_core::wav::WavSpec;
use tauri::{AppHandle, Emitter};

use crate::audio::asio::build_route_plan;
use crate::error::{AppError, AppResult};
use crate::recorder::engine::{
    emit_error, emit_warning, error_code_str, now_ms, sleep_opt, wait_opt, RecorderStatePayload,
    RecordingLevels, RecordingOpts, RecordingProgress, SegmentOutcome, LEVELS_EVENT,
    PROGRESS_EVENT, SILENCE_EVENT, STARTED_EVENT, STATE_EVENT,
};
use crate::recorder::native_capture::stream::{
    build_input_stream_any, find_device, negotiate, open_host, ring_capacity, CpalHostKind,
    StreamSink,
};
use crate::recorder::native_capture::writer::{
    spawn_wav_writer, WriterConfig, WriterErrorKind, WriterEvent, FLUSH_EVERY,
};
use crate::util::lock_recover;
use sundayrec_core::audio::MeterBanks;
use sundayrec_core::errors::RecordingErrorCode;

/// How often the levels sampler reads the meters and emits (parity with the
/// ffmpeg path's `LevelMeter::EMIT_EVERY` and the VU engine).
const LEVELS_EVERY: Duration = Duration::from_millis(33);

/// Bound on joining the stream thread (it polls its stop flag every 50 ms and
/// then just drops the stream — 5 s only trips on a wedged CoreAudio teardown).
const STREAM_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// One live native capture: the `!Send` stream on its own thread, the writer
/// thread, and the channels/counters the runner selects over.
pub struct NativeSegment {
    stream_stop: Arc<AtomicBool>,
    writer_stop: Arc<AtomicBool>,
    stream_join: Option<std::thread::JoinHandle<()>>,
    writer_join: Option<std::thread::JoinHandle<Result<super::writer::WriterSummary, String>>>,
    events: tokio::sync::mpsc::UnboundedReceiver<WriterEvent>,
    stream_err_rx: tokio::sync::mpsc::Receiver<String>,
    meters: Arc<MeterBanks>,
    /// Samples dropped by the RT callback on ring overrun (whole frames).
    pub overrun: Arc<AtomicU64>,
    /// Header+data bytes on disk — the watchdog/progress/disk-guard truth.
    pub bytes: Arc<AtomicU64>,
    /// Exact frames written — the truth-measurement cross-check.
    pub frames: Arc<AtomicU64>,
    /// The pinned capture format (routed channels + negotiated rate).
    pub spec: WavSpec,
}

/// Open the device, negotiate its format, and start the full native stack
/// (stream thread + ring + writer) writing to `output_path`. Returns once the
/// stream is BUILT AND PLAYING (readiness discipline of `audio/vu.rs`) — a bad
/// device fails the Start call so the supervisor can fall back to ffmpeg.
///
/// `pinned_rate` is the current deliverable's established rate (a reconnect
/// reopen): it overrides the user's requested rate so the `_rN` fragment can
/// `-c copy`-join its siblings. The supervisor compares the resulting
/// [`NativeSegment::spec`] against the pin and converts a mismatch into a new
/// deliverable.
pub async fn spawn_native_segment(
    host: CpalHostKind,
    opts: &RecordingOpts,
    output_path: &str,
    pinned_rate: Option<u32>,
) -> AppResult<NativeSegment> {
    let device_name = opts.audio_device_name.clone();
    let requested_rate = pinned_rate.or(opts.sample_rate);

    // Probe on a blocking thread: the (!Send) device handle never escapes.
    let negotiated = {
        let name = device_name.clone();
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let h = open_host(host)?;
            let device = find_device(&h, &name)?;
            negotiate(&device, requested_rate)
        })
        .await
        .map_err(|e| AppError::Recording(format!("device probe task failed: {e}")))?
        .map_err(AppError::Recording)?
    };

    let plan = build_route_plan(
        opts.channel_mode,
        opts.input_channel_l,
        opts.input_channel_r,
        negotiated.channels,
    );
    let out_ch = plan.len().max(1) as u16;
    let spec = WavSpec {
        channels: out_ch,
        sample_rate: negotiated.sample_rate,
    };
    tracing::info!(
        device = %device_name,
        host = host.label(),
        rate = spec.sample_rate,
        native_channels = negotiated.channels,
        routed_channels = out_ch,
        format = ?negotiated.sample_format,
        via = negotiated.via,
        out = %output_path,
        "recorder: native capture starting"
    );

    // The shared capture cushion (5 s of routed audio — see `stream::RING_SECONDS`)
    // so a writer stall never drops samples; overrun drops whole frames + counts.
    let (prod, cons) = ringbuf::HeapRb::<f32>::new(ring_capacity(spec.sample_rate, out_ch)).split();

    let stream_stop = Arc::new(AtomicBool::new(false));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let overrun = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let meters = Arc::new(MeterBanks::new(out_ch as usize));
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<WriterEvent>();
    let (err_tx, stream_err_rx) = tokio::sync::mpsc::channel::<String>(1);

    // Writer first, so the stream never produces into a ring nobody drains.
    let writer_join = spawn_wav_writer(
        cons,
        WriterConfig {
            path: std::path::PathBuf::from(output_path),
            spec,
            flush_every: FLUSH_EVERY,
            silence: SilenceDetector::new(
                silence_threshold_db(opts.stop_on_silence, opts.silence_threshold_db),
                spec.sample_rate,
            ),
        },
        Arc::clone(&writer_stop),
        Arc::clone(&bytes),
        Arc::clone(&frames),
        event_tx,
    )
    .map_err(|e| AppError::Recording(format!("could not spawn wav writer: {e}")))?;

    // Stream thread: reopen host+device (the Stream is !Send), build, play,
    // report readiness over an async oneshot, park until stopped.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let st_stop = Arc::clone(&stream_stop);
    let st_meters = Arc::clone(&meters);
    let st_overrun = Arc::clone(&overrun);
    let st_name = device_name.clone();
    let stream_join = std::thread::Builder::new()
        .name("native-capture".into())
        .spawn(move || {
            let build = (|| -> Result<cpal::Stream, String> {
                let h = open_host(host)?;
                let device = find_device(&h, &st_name)?;
                let config = cpal::StreamConfig {
                    channels: negotiated.channels,
                    sample_rate: negotiated.sample_rate,
                    buffer_size: cpal::BufferSize::Default,
                };
                let err_fn = move |e: cpal::StreamError| {
                    tracing::error!("native capture stream error: {e}");
                    let _ = err_tx.try_send(e.to_string());
                };
                let stream = build_input_stream_any(
                    &device,
                    &config,
                    negotiated.sample_format,
                    negotiated.channels as usize,
                    StreamSink::Capture {
                        meters: st_meters,
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
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    drop(stream); // stops capture, releases the device
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| AppError::Recording(format!("could not spawn capture thread: {e}")))?;

    let mut seg = NativeSegment {
        stream_stop,
        writer_stop,
        stream_join: Some(stream_join),
        writer_join: Some(writer_join),
        events: event_rx,
        stream_err_rx,
        meters,
        overrun,
        bytes,
        frames,
        spec,
    };

    match ready_rx.await {
        Ok(Ok(())) => Ok(seg),
        Ok(Err(e)) => {
            abort_native_segment(&mut seg, output_path).await;
            Err(AppError::Recording(e))
        }
        Err(_) => {
            abort_native_segment(&mut seg, output_path).await;
            Err(AppError::Recording(
                "native capture thread exited before signalling".into(),
            ))
        }
    }
}

/// Tear down a stack that must not record (a failed spawn, or a reconnect that
/// came back at an incompatible rate), and remove the WAV when it holds no
/// audio so the next attempt starts from a clean capture dir.
pub(crate) async fn abort_native_segment(seg: &mut NativeSegment, output_path: &str) {
    stop_native_bounded(seg).await;
    if seg.frames.load(Ordering::Relaxed) == 0 {
        let _ = tokio::fs::remove_file(output_path).await;
    }
}

/// Stop the stack in order — stream first (producer gone), then the writer
/// (drains the ring, patches the header, fsyncs) — with bounded joins so a
/// wedged teardown can never freeze the supervisor (the "UI stuck on
/// Stopping" class of bug).
pub(crate) async fn stop_native_bounded(seg: &mut NativeSegment) {
    seg.stream_stop.store(true, Ordering::Relaxed);
    if let Some(h) = seg.stream_join.take() {
        let join = tokio::task::spawn_blocking(move || {
            let _ = h.join();
        });
        if tokio::time::timeout(STREAM_JOIN_TIMEOUT, join)
            .await
            .is_err()
        {
            tracing::warn!("native capture: stream thread did not stop in time — continuing");
        }
    }
    seg.writer_stop.store(true, Ordering::Relaxed);
    if let Some(h) = seg.writer_join.take() {
        let join = tokio::task::spawn_blocking(move || h.join());
        match tokio::time::timeout(
            Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS),
            join,
        )
        .await
        {
            Ok(Ok(Ok(Ok(summary)))) => {
                tracing::info!(
                    data_bytes = summary.data_bytes,
                    frames = summary.frames,
                    "native capture: writer finalized"
                );
            }
            Ok(Ok(Ok(Err(e)))) => {
                tracing::warn!("native capture: writer ended with error: {e}");
            }
            _ => {
                tracing::warn!("native capture: writer did not finalize in time");
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The runner's decisions, lifted out of the select! arms
// ─────────────────────────────────────────────────────────────────────────────

/// What the 30 s disk tick concludes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiskVerdict {
    /// Keep recording.
    Continue,
    /// The deliverable is approaching the 4 GiB WAV ceiling — close this
    /// fragment and open a new deliverable.
    ForceSplit,
    /// The volume is nearly full — stop safely while the finalize still fits.
    DiskStop,
}

/// The disk tick's rule, as a pure function of the numbers it reads.
///
/// **Order matters and is the point of extracting this.** The RIFF cap is
/// checked FIRST: a nearly-full disk and a nearly-4-GiB deliverable can be true
/// at the same moment, and the two verdicts do opposite things — a `ForceSplit`
/// keeps recording into a fresh file, a `DiskStop` ends the service. Getting the
/// order backwards on a long service with a small disk means the recording ends
/// where it should have rolled over. In the select! arm this ordering was an
/// `if` followed by an `if let`, invisible to any test.
///
/// `free` is `None` when the volume could not be probed — an unreadable disk is
/// not evidence of a full one, so we keep recording (the writer's own
/// `DiskFull` error is the backstop that cannot be fooled).
///
/// `split_threshold` is passed IN rather than read from
/// `wav::forced_split_threshold_bytes()` here, because that function consults a
/// process-global environment override (`SUNDAYREC_TEST_SPLIT_BYTES`, debug
/// builds only) that the long-run harness sets while its own tests run. A
/// "pure" function that quietly reads ambient state is not pure, and a table
/// test of it is a coin flip against whatever else happens to be running —
/// which is precisely how this arrived (green locally, red in CI).
pub(crate) fn disk_guard_verdict(
    deliverable_bytes: u64,
    segment_bytes: u64,
    free: Option<u64>,
    headroom: u64,
    split_threshold: u64,
) -> DiskVerdict {
    if deliverable_bytes.saturating_add(segment_bytes) >= split_threshold {
        return DiskVerdict::ForceSplit;
    }
    match free {
        Some(free) => {
            let reserve = finalize_reserve_bytes(false, segment_bytes);
            if low_disk_should_stop(free, headroom.saturating_add(reserve)) {
                DiskVerdict::DiskStop
            } else {
                DiskVerdict::Continue
            }
        }
        None => DiskVerdict::Continue,
    }
}

/// How a typed writer error is classified and announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriterVerdict {
    /// The shared error code — the wire string is derived from it, never typed
    /// as a literal, so it cannot drift from the recovery policy's view.
    pub code: RecordingErrorCode,
    /// `true` → `recording://error` (terminal for the session); `false` →
    /// `recording://warning` (the supervisor will reconnect).
    pub fatal: bool,
}

/// Classify a writer error. The writer has already stopped, so unlike the ffmpeg
/// twin there is no stderr to sniff — the kind IS the classification.
pub(crate) fn writer_error_verdict(kind: WriterErrorKind) -> WriterVerdict {
    let code = match kind {
        WriterErrorKind::DiskFull => RecordingErrorCode::DiskFull,
        WriterErrorKind::Io => RecordingErrorCode::DeviceError,
    };
    WriterVerdict {
        code,
        fatal: sundayrec_core::recorder::is_fatal_reconnect_error(code),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The two seams that make the select! loop testable
// ─────────────────────────────────────────────────────────────────────────────

/// One thing the live capture told the runner.
///
/// Not `Clone`/`PartialEq`: `WriterEvent` carries an owned message and is a
/// one-shot report, so a test asserts on the OUTCOME and the emitted events,
/// never on a copy of the signal it fed in.
#[derive(Debug)]
pub(crate) enum SegmentSignal {
    /// The writer reported something (first block on disk, silence edge, error).
    Writer(WriterEvent),
    /// The writer's channel closed WITHOUT an error event — the writer thread
    /// died. The supervisor decides; there is nothing to classify.
    WriterGone,
    /// The cpal stream reported a device error (USB pulled, format lost).
    StreamError(String),
}

/// Everything [`run_native_segment`] needs from a live capture.
///
/// A trait, not a struct, for one reason: the `select!` loop below is where the
/// startup watchdog, the stuck detector, the silence sequence, the auto-stop
/// extend and the writer-death path all meet, and that meeting point could not
/// be exercised without opening a real microphone. A scripted implementation
/// plus `tokio::time::pause()` turns each of those into a unit test that runs in
/// microseconds — this is the class of seam the audits kept finding bugs in.
///
/// Every method must be CANCEL-SAFE: `select!` drops the losing futures on every
/// iteration, and a `next_signal` that consumed an event before being dropped
/// would silently lose it.
pub(crate) trait SegmentSignals {
    /// The next writer/stream signal. Cancel-safe.
    fn next_signal(&mut self) -> impl std::future::Future<Output = SegmentSignal> + Send;
    /// Header+data bytes on disk — the watchdog/progress/disk-guard truth.
    fn bytes(&self) -> u64;
    /// Exact frames written — the truth-measurement cross-check.
    fn frames(&self) -> u64;
    /// Samples dropped by the RT callback on ring overrun.
    fn overrun(&self) -> u64;
    /// The pinned capture format.
    fn spec(&self) -> WavSpec;
    /// Take-and-reset the peak meter for `channel`, in dBFS.
    fn take_peak_dbfs(&self, channel: usize) -> f32;
    /// Stop the stack with bounded joins (stream first, then writer).
    fn stop(&mut self) -> impl std::future::Future<Output = ()> + Send;
}

impl SegmentSignals for NativeSegment {
    async fn next_signal(&mut self) -> SegmentSignal {
        // Both arms are `mpsc::Receiver::recv`, which is cancel-safe, and
        // `select!` drops the loser without polling it to completion — so this
        // composite is cancel-safe too.
        tokio::select! {
            ev = self.events.recv() => match ev {
                Some(ev) => SegmentSignal::Writer(ev),
                None => SegmentSignal::WriterGone,
            },
            msg = self.stream_err_rx.recv() => SegmentSignal::StreamError(
                msg.unwrap_or_else(|| "audio device error".into()),
            ),
        }
    }

    fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    fn overrun(&self) -> u64 {
        self.overrun.load(Ordering::Relaxed)
    }

    fn spec(&self) -> WavSpec {
        self.spec
    }

    fn take_peak_dbfs(&self, channel: usize) -> f32 {
        self.meters.peak.take_dbfs(channel)
    }

    async fn stop(&mut self) {
        stop_native_bounded(self).await;
    }
}

/// Where the runner's user-visible output goes.
///
/// The production implementation is a `tauri::AppHandle`; the test one is a
/// recording buffer. Without this the loop could not be run at all off a GUI
/// thread — an `AppHandle` cannot be constructed in a unit test.
pub(crate) trait EventSink {
    /// `recording://error` — classified, terminal for this segment.
    fn error(&self, code: &str, message: &str);
    /// `recording://warning` — classified, the session continues.
    fn warning(&self, code: &str, message: &str);
    /// `recording://started` — the first block reached disk.
    fn started(&self);
    /// `recording://silence` — the silence warning fired.
    fn silence(&self, code: &str, message: &str);
    /// `recording://progress` — the 1 Hz byte counter.
    fn progress(&self, bytes_written: u64);
    /// `recording://levels` — the 33 ms meters.
    fn levels(&self, peak_db_left: f64, peak_db_right: Option<f64>);
    /// `recording://state` — re-stamped when the auto-stop deadline moves.
    fn state(&self, scheduled_stop_ms: Option<u64>);
}

/// The ambient facts the runner reads that are neither a signal nor an event:
/// the wall clock and the capture volume's free space.
///
/// Injected rather than called directly so the whole loop can run under
/// `tokio::time::pause()`. `tokio`'s paused clock stops tokio's timers but NOT
/// `SystemTime::now()`, and the stuck-progress watchdog is judged on wall-clock
/// ms — with a real clock in here, a virtual-time test could advance the
/// watchdog's tick a thousand times and never reach its verdict.
pub(crate) struct SegmentEnv<'a> {
    /// Epoch milliseconds.
    pub now_ms: &'a (dyn Fn() -> u64 + Sync),
    /// Free bytes on the capture volume; `None` when it cannot be probed (an
    /// unreadable volume is not evidence of a full one).
    pub free_bytes: &'a (dyn Fn() -> Option<u64> + Sync),
}

/// The production sink: a real `AppHandle` plus the two pieces of session state
/// the `state` payload carries.
pub(crate) struct AppEventSink<'a> {
    app: &'a AppHandle,
    last_state: &'a Arc<Mutex<RecorderState>>,
    reconnect_count: u32,
}

impl EventSink for AppEventSink<'_> {
    fn error(&self, code: &str, message: &str) {
        emit_error(self.app, code, message);
    }
    fn warning(&self, code: &str, message: &str) {
        emit_warning(self.app, code, message);
    }
    fn started(&self) {
        let _ = self.app.emit(STARTED_EVENT, ());
    }
    fn silence(&self, code: &str, message: &str) {
        let _ = self.app.emit(
            SILENCE_EVENT,
            crate::recorder::engine::RecordingEvent {
                code: code.into(),
                message: message.into(),
            },
        );
    }
    fn progress(&self, bytes_written: u64) {
        let _ = self
            .app
            .emit(PROGRESS_EVENT, RecordingProgress { bytes_written });
    }
    fn levels(&self, peak_db_left: f64, peak_db_right: Option<f64>) {
        let _ = self.app.emit(
            LEVELS_EVENT,
            RecordingLevels {
                peak_db_left,
                peak_db_right,
            },
        );
    }
    fn state(&self, scheduled_stop_ms: Option<u64>) {
        let _ = self.app.emit(
            STATE_EVENT,
            RecorderStatePayload {
                state: *lock_recover(self.last_state),
                reconnect_count: self.reconnect_count,
                scheduled_stop_ms,
            },
        );
    }
}

/// Run one native segment to completion. Mirrors `engine::run_segment`'s arms;
/// see the module header for the signal mapping.
///
/// `deliverable_bytes` is the sum of the current deliverable's PREVIOUS
/// fragments (`_rN` reconnect pieces) — the RIFF-cap guard forces a split
/// before the `-c copy`-concatenated deliverable could cross the 4 GiB WAV
/// ceiling.
///
/// This is the thin production wrapper: it binds the real `AppHandle`, the real
/// segment and the real free-space probe, then hands them to
/// [`drive_native_segment`], which holds all the logic and is unit-tested.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_native_segment(
    app: &AppHandle,
    mut seg: NativeSegment,
    opts: &RecordingOpts,
    session: &RecordingSession,
    segment_bytes: Arc<AtomicU64>,
    deliverable_bytes: u64,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    last_state: &Arc<Mutex<RecorderState>>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
    telemetry: Arc<Mutex<sundayrec_core::selftest::RecordingTelemetry>>,
) -> SegmentOutcome {
    let sink = AppEventSink {
        app,
        last_state,
        reconnect_count: session.reconnect_count(),
    };
    // The capture folder's volume. Probed fresh on every disk tick — the
    // recording folder can live on a drive that is unmounted mid-service.
    let disk_folder = std::path::Path::new(&opts.output_path)
        .parent()
        .map(|p| p.to_path_buf());
    let free_bytes = move || -> Option<u64> {
        disk_folder
            .as_ref()
            .and_then(|folder| fs4::available_space(folder).ok())
    };
    let env = SegmentEnv {
        now_ms: &now_ms,
        free_bytes: &free_bytes,
    };
    drive_native_segment(
        &mut seg,
        &sink,
        &env,
        opts,
        segment_bytes,
        deliverable_bytes,
        stop_rx,
        stop_watch,
        telemetry,
    )
    .await
}

/// The segment runner proper: one `select!` loop over every signal and timer a
/// live capture has, generic over the capture ([`SegmentSignals`]), its output
/// ([`EventSink`]) and the free-space probe.
///
/// Everything here used to be inlined in [`run_native_segment`] behind an
/// `AppHandle` and a real cpal stream, which is why none of it had a test.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_native_segment<S, E>(
    seg: &mut S,
    sink: &E,
    env: &SegmentEnv<'_>,
    opts: &RecordingOpts,
    segment_bytes: Arc<AtomicU64>,
    deliverable_bytes: u64,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
    telemetry: Arc<Mutex<sundayrec_core::selftest::RecordingTelemetry>>,
) -> SegmentOutcome
where
    S: SegmentSignals,
    E: EventSink,
{
    // Silence watcher + its (host-owned) timers — identical to the ffmpeg path.
    let mut silence = SilenceWatcher::new(opts.stop_on_silence);
    let silence_stop_after =
        Duration::from_secs(u64::from(opts.silence_timeout_minutes.max(1)) * 60);
    let silence_warn_after = Duration::from_millis(RecorderTimeouts::SILENCE_WARN_MS);
    let mut silence_stop: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut silence_warn: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    // Byte-progress watchdog over the writer's own counter.
    let mut wd = WatchdogState::new(RecorderTimeouts::STUCK_PROGRESS_MS, (env.now_ms)());
    let mut wd_tick = tokio::time::interval(Duration::from_millis(RecorderTimeouts::STUCK_POLL_MS));
    wd_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Low-disk guard — identical thresholds to the ffmpeg path (audio-only).
    let disk_headroom = min_disk_headroom_bytes(false);
    let mut disk_tick = tokio::time::interval(Duration::from_secs(30));
    disk_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Split + auto-stop timers — identical to the ffmpeg path.
    let split_deadline = if opts.split_minutes > 0 {
        Some(Duration::from_secs(u64::from(opts.split_minutes) * 60))
    } else {
        None
    };
    let auto_stop_remaining = |deadline: Option<u64>| -> Option<Duration> {
        deadline.map(|d| Duration::from_millis(d.saturating_sub((env.now_ms)())))
    };
    let mut auto_deadline: Option<u64> = *stop_watch.borrow();
    let split_sleep = sleep_opt(split_deadline);
    tokio::pin!(split_sleep);
    let auto_sleep = sleep_opt(auto_stop_remaining(auto_deadline));
    tokio::pin!(auto_sleep);

    // Startup watchdog: the writer must report Started (first block on disk)
    // within the same window ffmpeg gets for its first progress block.
    let mut started_seen = false;
    let startup_sleep =
        tokio::time::sleep(Duration::from_millis(RecorderTimeouts::STARTUP_TIMEOUT_MS));
    tokio::pin!(startup_sleep);

    // UI cadences: 1 Hz byte counter, 33 ms level meters (when enabled).
    let mut progress_tick = tokio::time::interval(Duration::from_secs(1));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut levels_tick = tokio::time::interval(LEVELS_EVERY);
    levels_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let stereo = seg.spec().channels >= 2;
    let floor = |db: f32| {
        if db.is_finite() {
            f64::from(db)
        } else {
            SILENCE_FLOOR_DB
        }
    };

    let outcome = loop {
        tokio::select! {
            // Writer events: Started / silence / typed errors; and the cpal
            // stream's own device errors.
            signal = seg.next_signal() => {
                match signal {
                    SegmentSignal::Writer(WriterEvent::Started) => {
                        started_seen = true;
                        sink.started();
                    }
                    SegmentSignal::Writer(WriterEvent::Silence(ev)) => {
                        for action in silence.feed(ev) {
                            match action {
                                SilenceAction::ArmStop => {
                                    silence_stop = Some(Box::pin(tokio::time::sleep(silence_stop_after)));
                                }
                                SilenceAction::ArmWarn => {
                                    silence_warn = Some(Box::pin(tokio::time::sleep(silence_warn_after)));
                                }
                                SilenceAction::CancelStop => { silence_stop = None; }
                                SilenceAction::CancelWarn => { silence_warn = None; }
                            }
                        }
                    }
                    SegmentSignal::Writer(WriterEvent::Error { kind, message }) => {
                        // The writer has already stopped; classify directly
                        // (no stderr sniffing) and hand the supervisor the
                        // same fatal/transient split as the ffmpeg path.
                        let verdict = writer_error_verdict(kind);
                        // Derive the wire code from the shared table (never a
                        // literal) so it can't drift from the classification the
                        // recovery policy just made — the ffmpeg twin does the same.
                        let code_str = error_code_str(verdict.code);
                        if verdict.fatal {
                            sink.error(code_str, &message);
                        } else {
                            sink.warning(code_str, &message);
                        }
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit { last_error: Some(verdict.code) };
                    }
                    SegmentSignal::WriterGone => {
                        // Writer gone without an error event — treat as an
                        // unexpected exit; the recovery policy decides.
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit { last_error: None };
                    }
                    // Device error mid-session (USB pulled, format lost): finalize the
                    // fragment so the _rN concat has a valid piece, then reconnect.
                    SegmentSignal::StreamError(reason) => {
                        sink.warning("device_disconnected", &reason);
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit {
                            last_error: Some(RecordingErrorCode::DeviceDisconnected),
                        };
                    }
                }
            }
            // Graceful stop request.
            _ = stop_rx.recv() => {
                seg.stop().await;
                break SegmentOutcome::GracefulStop;
            }
            // Startup watchdog: no first block on disk in time.
            _ = &mut startup_sleep, if !started_seen => {
                sink.error(
                    "start_timeout",
                    "Opptaket startet ikke i tide — sjekk at mikrofonen er tilkoblet og at appen har tilgang (Systeminnstillinger → Personvern).",
                );
                seg.stop().await;
                break SegmentOutcome::UnexpectedExit {
                    last_error: Some(RecordingErrorCode::DeviceNotFound),
                };
            }
            // Byte-progress watchdog.
            _ = wd_tick.tick() => {
                if wd.observe(seg.bytes(), (env.now_ms)()) == WatchdogVerdict::Stuck {
                    // WARNING, not error — the ffmpeg twin's reason verbatim
                    // (`engine.rs`, the same watchdog arm): this breaks to
                    // `UnexpectedExit`, the recovery policy reconnects, and the
                    // session lives. `ERROR_EVENT` is TERMINAL, and firing it
                    // here tore the overlay down and sent a "the recording
                    // failed" e-mail through `notify::wire_failure_sources`
                    // while the capture was coming back.
                    sink.warning(
                        "stuck_recording",
                        &format!(
                            "Ingen framgang på {} s — kobler til på nytt",
                            RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                        ),
                    );
                    seg.stop().await;
                    break SegmentOutcome::UnexpectedExit { last_error: None };
                }
            }
            // Low-disk guard + RIFF-cap guard (same 30 s cadence; the cap has
            // 500 MiB of headroom below the real 4 GiB ceiling, dwarfing the
            // ~12 MiB a 96 kHz stereo segment grows between ticks). The rule
            // and its ORDER live in `disk_guard_verdict`.
            _ = disk_tick.tick() => {
                let seg_bytes = seg.bytes();
                match disk_guard_verdict(
                    deliverable_bytes,
                    seg_bytes,
                    (env.free_bytes)(),
                    disk_headroom,
                    sundayrec_core::wav::forced_split_threshold_bytes(),
                ) {
                    DiskVerdict::Continue => {}
                    DiskVerdict::ForceSplit => {
                        tracing::warn!(
                            deliverable_bytes,
                            seg_bytes,
                            "native capture: deliverable approaching the 4 GiB WAV ceiling — forcing a split"
                        );
                        seg.stop().await;
                        break SegmentOutcome::Split;
                    }
                    DiskVerdict::DiskStop => {
                        sink.error(
                            "disk_full",
                            "Lite ledig diskplass — stopper opptaket trygt før disken blir full.",
                        );
                        seg.stop().await;
                        break SegmentOutcome::DiskStop;
                    }
                }
            }
            // Split timer.
            _ = &mut split_sleep, if split_deadline.is_some() => {
                seg.stop().await;
                break SegmentOutcome::Split;
            }
            // Auto-stop deadline reached.
            _ = &mut auto_sleep, if auto_deadline.is_some() => {
                seg.stop().await;
                break SegmentOutcome::AutoStop;
            }
            // Auto-stop deadline moved/cleared (live extend/cancel).
            changed = stop_watch.changed() => {
                if changed.is_ok() {
                    auto_deadline = *stop_watch.borrow();
                    match auto_stop_remaining(auto_deadline) {
                        Some(rem) => auto_sleep.as_mut().reset(tokio::time::Instant::now() + rem),
                        None => auto_sleep.as_mut().reset(
                            tokio::time::Instant::now()
                                + Duration::from_secs(60 * 60 * 24 * 365 * 100),
                        ),
                    }
                    sink.state(auto_deadline);
                }
            }
            // Stop-on-silence fired.
            () = wait_opt(&mut silence_stop), if silence_stop.is_some() => {
                silence.on_stop_fired();
                seg.stop().await;
                break SegmentOutcome::SilenceStop;
            }
            // Silence warning fired.
            () = wait_opt(&mut silence_warn), if silence_warn.is_some() => {
                silence.on_warn_fired();
                silence_warn = None;
                sink.silence("silence_detected", "Stillhet oppdaget i lydsignalet");
            }
            // 1 Hz UI byte counter (also mirrors into the supervisor's atomic).
            _ = progress_tick.tick() => {
                let b = seg.bytes();
                segment_bytes.store(b, Ordering::Relaxed);
                if started_seen {
                    sink.progress(b);
                }
            }
            // 33 ms level meters over the routed banks.
            _ = levels_tick.tick(), if opts.live_levels => {
                sink.levels(
                    floor(seg.take_peak_dbfs(0)),
                    stereo.then(|| floor(seg.take_peak_dbfs(1))),
                );
            }
        }
    };

    // Fold this segment's health into the session telemetry: overruns are the
    // native drop signal (verdict folds them into the xrun class), and the
    // exact frame count is the capture-side cross-check against ffprobe.
    let overrun = seg.overrun();
    if overrun > 0 {
        tracing::warn!(overrun, "native capture: ring overran — samples dropped");
    }
    {
        let mut t = lock_recover(&telemetry);
        t.ring_overrun_samples = t.ring_overrun_samples.saturating_add(overrun);
        t.native_frames_sec += seg.frames() as f64 / f64::from(seg.spec().sample_rate.max(1));
        // E6.3 symmetry with the ffmpeg twin: close this capture process's
        // drop/dup window. The native engine parses no ffmpeg stderr, so the
        // window is always empty and this is a no-op — but a session that mixes
        // the two backends (a native start that fell back to ffmpeg) must not
        // depend on which arm happened to run.
        t.seal_process();
    }
    segment_bytes.store(seg.bytes(), Ordering::Relaxed);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::settings::ChannelMode;
    use sundayrec_core::silence::SilenceEvent;

    // ── The two verdicts, as tables ──────────────────────────────────────────

    /// A stand-in threshold for the table tests. The real one is a runtime
    /// function of a process-global env override, so the tests below pass their
    /// own — that is the whole reason `disk_guard_verdict` takes it.
    const CAP: u64 = 3 * 1024 * 1024 * 1024 + 500 * 1024 * 1024;

    /// A byte count guaranteed to be BELOW any threshold the production code
    /// can legally use. `forced_split_threshold_bytes` clamps its override into
    /// `MIN_TEST_SPLIT_BYTES ..= FORCED_SPLIT_DELIVERABLE_BYTES`, so anything
    /// under the floor is under every possible threshold — the value the
    /// end-to-end tests below use when they must NOT trigger a rollover.
    fn under_every_possible_threshold() -> u64 {
        sundayrec_core::wav::MIN_TEST_SPLIT_BYTES - 1
    }

    /// …and the mirror: at or above every legal threshold.
    fn over_every_possible_threshold() -> u64 {
        sundayrec_core::wav::FORCED_SPLIT_DELIVERABLE_BYTES
    }

    #[test]
    fn disk_guard_continues_on_a_healthy_volume() {
        assert_eq!(
            disk_guard_verdict(0, 10_000_000, Some(500 * 1024 * 1024 * 1024), 1024, CAP),
            DiskVerdict::Continue
        );
    }

    #[test]
    fn disk_guard_stops_when_the_volume_is_nearly_full() {
        // Zero free bytes cannot be talked out of by any reserve arithmetic.
        assert_eq!(
            disk_guard_verdict(0, 10_000_000, Some(0), min_disk_headroom_bytes(false), CAP),
            DiskVerdict::DiskStop
        );
    }

    #[test]
    fn disk_guard_forces_a_split_before_the_riff_ceiling() {
        assert_eq!(
            disk_guard_verdict(
                0,
                CAP,
                Some(500 * 1024 * 1024 * 1024),
                min_disk_headroom_bytes(false),
                CAP
            ),
            DiskVerdict::ForceSplit
        );
    }

    /// The deliverable's PREVIOUS fragments count towards the ceiling: several
    /// `_rN` pieces concat to more than 4 GiB even though no single fragment is
    /// close.
    #[test]
    fn disk_guard_counts_previous_fragments_towards_the_ceiling() {
        let previous = CAP - 100 * 1024 * 1024;
        assert_eq!(
            disk_guard_verdict(previous, 0, Some(u64::MAX / 2), 1024, CAP),
            DiskVerdict::Continue,
            "just under the cap: keep going"
        );
        assert_eq!(
            disk_guard_verdict(previous, 200 * 1024 * 1024, Some(u64::MAX / 2), 1024, CAP),
            DiskVerdict::ForceSplit,
            "the current fragment pushed the SUM over — the split is the sum's job"
        );
    }

    /// THE ORDERING TEST. Both conditions true at once — a nearly-full disk and
    /// a nearly-4-GiB deliverable. They do opposite things (roll over vs end
    /// the service), so the priority is load-bearing: rolling over into a fresh
    /// file is what keeps the recording valid, and the disk stop still fires on
    /// the very next tick if space really has run out.
    #[test]
    fn the_riff_cap_outranks_the_disk_stop_when_both_fire() {
        assert_eq!(
            disk_guard_verdict(
                0,
                CAP,
                Some(0), // disk also screaming
                min_disk_headroom_bytes(false),
                CAP
            ),
            DiskVerdict::ForceSplit,
            "a 4 GiB deliverable must roll over, not end the service"
        );
    }

    /// An unprobeable volume is not a full one. The writer's own `DiskFull` is
    /// the backstop that cannot be fooled — stopping a service because
    /// `available_space` returned an error would be a self-inflicted outage.
    #[test]
    fn an_unprobeable_volume_does_not_stop_the_recording() {
        assert_eq!(
            disk_guard_verdict(0, 10_000_000, None, min_disk_headroom_bytes(false), CAP),
            DiskVerdict::Continue
        );
        // …but the RIFF cap still applies: it needs no disk probe at all.
        assert_eq!(
            disk_guard_verdict(0, CAP, None, 1024, CAP),
            DiskVerdict::ForceSplit
        );
    }

    /// The production threshold is only ever allowed to move DOWN, into
    /// `MIN_TEST_SPLIT_BYTES ..= FORCED_SPLIT_DELIVERABLE_BYTES`. The
    /// end-to-end tests below lean on that clamp to stay deterministic while
    /// another test is busy setting the override, so it is pinned here.
    #[test]
    fn the_split_threshold_is_always_inside_its_clamp() {
        let t = sundayrec_core::wav::forced_split_threshold_bytes();
        assert!(t >= sundayrec_core::wav::MIN_TEST_SPLIT_BYTES);
        assert!(t <= sundayrec_core::wav::FORCED_SPLIT_DELIVERABLE_BYTES);
        assert!(under_every_possible_threshold() < t);
        assert!(over_every_possible_threshold() >= t);
    }

    #[test]
    fn writer_disk_full_is_fatal_and_io_is_not() {
        let full = writer_error_verdict(WriterErrorKind::DiskFull);
        assert_eq!(full.code, RecordingErrorCode::DiskFull);
        assert!(full.fatal, "retrying will not make room");

        let io = writer_error_verdict(WriterErrorKind::Io);
        assert_eq!(io.code, RecordingErrorCode::DeviceError);
        assert!(!io.fatal, "an I/O blip must get the reconnect budget");
    }

    #[test]
    fn writer_verdict_agrees_with_the_shared_fatal_table() {
        // The guard against the two halves drifting: `fatal` must always be
        // whatever the recovery policy says about `code`, never a second
        // opinion typed out here.
        for kind in [WriterErrorKind::DiskFull, WriterErrorKind::Io] {
            let v = writer_error_verdict(kind);
            assert_eq!(
                v.fatal,
                sundayrec_core::recorder::is_fatal_reconnect_error(v.code),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn writer_verdict_codes_have_wire_strings() {
        assert_eq!(
            error_code_str(writer_error_verdict(WriterErrorKind::DiskFull).code),
            "disk_full"
        );
        assert!(!error_code_str(writer_error_verdict(WriterErrorKind::Io).code).is_empty());
    }

    // ── abort_native_segment's deletion rule ─────────────────────────────────

    /// A spawn that never produced a frame must leave NO file behind, so the
    /// next attempt starts from a clean capture dir (a zero-frame WAV would be
    /// concatenated into the deliverable as a valid-looking empty fragment).
    ///
    /// Driven through the real `spawn_native_segment` failure path: a device
    /// name no host can resolve.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_spawn_leaves_no_capture_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("never-started.wav");
        let out_str = out.to_string_lossy().into_owned();
        let mut opts = test_opts(&out_str);
        opts.audio_device_name = "no-such-device-2f0a1c8e-never-enumerated-by-any-host".to_string();

        let err = match spawn_native_segment(CpalHostKind::Default, &opts, &out_str, None).await {
            Err(e) => e,
            Ok(mut seg) => {
                stop_native_bounded(&mut seg).await;
                panic!("a nonexistent device must not start a segment");
            }
        };
        assert!(
            format!("{err}").to_lowercase().contains("device")
                || format!("{err}").to_lowercase().contains("host"),
            "the error must name the real reason: {err}"
        );
        assert!(
            !out.exists(),
            "a segment that captured nothing left {out:?} behind"
        );
    }

    // ── The select! loop, driven off hardware ────────────────────────────────

    /// One event the runner emitted. Compared by shape, not by prose — the
    /// message texts are localisation, the codes are contract.
    #[derive(Debug, Clone, PartialEq)]
    enum Ev {
        Error(String),
        Warning(String),
        Started,
        Silence,
        Progress(u64),
        Levels(f64, Option<f64>),
        State(Option<u64>),
    }

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<Ev>>,
    }

    impl Recorder {
        fn seen(&self) -> Vec<Ev> {
            self.events.lock().expect("recorder lock").clone()
        }
        fn push(&self, e: Ev) {
            self.events.lock().expect("recorder lock").push(e);
        }
        fn has(&self, e: &Ev) -> bool {
            self.seen().iter().any(|x| x == e)
        }
    }

    impl EventSink for Recorder {
        fn error(&self, code: &str, _m: &str) {
            self.push(Ev::Error(code.into()));
        }
        fn warning(&self, code: &str, _m: &str) {
            self.push(Ev::Warning(code.into()));
        }
        fn started(&self) {
            self.push(Ev::Started);
        }
        fn silence(&self, _c: &str, _m: &str) {
            self.push(Ev::Silence);
        }
        fn progress(&self, b: u64) {
            self.push(Ev::Progress(b));
        }
        fn levels(&self, l: f64, r: Option<f64>) {
            self.push(Ev::Levels(l, r));
        }
        fn state(&self, s: Option<u64>) {
            self.push(Ev::State(s));
        }
    }

    /// A capture whose signals are a script and whose counters the test moves.
    struct Scripted {
        rx: tokio::sync::mpsc::UnboundedReceiver<SegmentSignal>,
        bytes: Arc<AtomicU64>,
        frames: Arc<AtomicU64>,
        overrun: Arc<AtomicU64>,
        peak: f32,
        spec: WavSpec,
        stops: Arc<AtomicU64>,
    }

    impl SegmentSignals for Scripted {
        async fn next_signal(&mut self) -> SegmentSignal {
            match self.rx.recv().await {
                Some(s) => s,
                // The script ran out: a live capture that says nothing more is
                // simply quiet. Parking (rather than reporting `WriterGone`)
                // keeps the loop on its timers, which is what most of these
                // tests are about.
                None => std::future::pending().await,
            }
        }
        fn bytes(&self) -> u64 {
            self.bytes.load(Ordering::Relaxed)
        }
        fn frames(&self) -> u64 {
            self.frames.load(Ordering::Relaxed)
        }
        fn overrun(&self) -> u64 {
            self.overrun.load(Ordering::Relaxed)
        }
        fn spec(&self) -> WavSpec {
            self.spec
        }
        fn take_peak_dbfs(&self, _channel: usize) -> f32 {
            self.peak
        }
        async fn stop(&mut self) {
            self.stops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The scripted rig: a capture, the channel that feeds it, its counters,
    /// and the event recorder.
    struct Rig {
        seg: Scripted,
        tx: tokio::sync::mpsc::UnboundedSender<SegmentSignal>,
        bytes: Arc<AtomicU64>,
        frames: Arc<AtomicU64>,
        overrun: Arc<AtomicU64>,
        stops: Arc<AtomicU64>,
        sink: Recorder,
    }

    fn rig() -> Rig {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let frames = Arc::new(AtomicU64::new(0));
        let overrun = Arc::new(AtomicU64::new(0));
        let stops = Arc::new(AtomicU64::new(0));
        Rig {
            seg: Scripted {
                rx,
                bytes: Arc::clone(&bytes),
                frames: Arc::clone(&frames),
                overrun: Arc::clone(&overrun),
                peak: -12.0,
                spec: WavSpec {
                    channels: 2,
                    sample_rate: 48_000,
                },
                stops: Arc::clone(&stops),
            },
            tx,
            bytes,
            frames,
            overrun,
            stops,
            sink: Recorder::default(),
        }
    }

    /// A monotonic virtual clock the test advances by hand, so the wall-clock
    /// watchdog can be driven alongside tokio's paused timers.
    #[derive(Default)]
    struct FakeClock(Arc<AtomicU64>);
    impl FakeClock {
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::Relaxed);
        }
        fn handle(&self) -> Arc<AtomicU64> {
            Arc::clone(&self.0)
        }
    }

    fn telemetry() -> Arc<Mutex<sundayrec_core::selftest::RecordingTelemetry>> {
        Arc::new(Mutex::new(
            sundayrec_core::selftest::RecordingTelemetry::default(),
        ))
    }

    /// Drive the runner to completion under paused time.
    #[allow(clippy::too_many_arguments)]
    async fn drive(
        rig: &mut Rig,
        opts: &RecordingOpts,
        deliverable_bytes: u64,
        free: Option<u64>,
        clock: Arc<AtomicU64>,
        stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
        stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
        tel: Arc<Mutex<sundayrec_core::selftest::RecordingTelemetry>>,
    ) -> SegmentOutcome {
        let now = move || clock.load(Ordering::Relaxed);
        let free_fn = move || free;
        let env = SegmentEnv {
            now_ms: &now,
            free_bytes: &free_fn,
        };
        drive_native_segment(
            &mut rig.seg,
            &rig.sink,
            &env,
            opts,
            Arc::new(AtomicU64::new(0)),
            deliverable_bytes,
            stop_rx,
            stop_watch,
            tel,
        )
        .await
    }

    /// The stop channel + auto-stop watch a segment runs against.
    type Channels = (
        tokio::sync::mpsc::Sender<()>,
        tokio::sync::mpsc::Receiver<()>,
        tokio::sync::watch::Sender<Option<u64>>,
        tokio::sync::watch::Receiver<Option<u64>>,
    );

    fn channels() -> Channels {
        let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);
        let (w_tx, w_rx) = tokio::sync::watch::channel::<Option<u64>>(None);
        (stop_tx, stop_rx, w_tx, w_rx)
    }

    /// The startup latch: the writer's first block on disk fires
    /// `recording://started`, and the startup watchdog must then NEVER fire —
    /// its arm is guarded on `!started_seen`. (A guard that read the other way
    /// would kill every healthy long recording after the startup window.)
    #[tokio::test(start_paused = true)]
    async fn started_disarms_the_startup_watchdog_for_good() {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let clock = FakeClock::default();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let opts = test_opts("/tmp/x.wav");

        let h = tokio::spawn({
            // Stop the segment long AFTER the startup window would have fired.
            let stop_tx = stop_tx.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(
                    RecorderTimeouts::STARTUP_TIMEOUT_MS * 4,
                ))
                .await;
                let _ = stop_tx.send(()).await;
            }
        });
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            clock.handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        h.await.unwrap();

        assert_eq!(out, SegmentOutcome::GracefulStop);
        assert!(r.sink.has(&Ev::Started));
        assert!(
            !r.sink.has(&Ev::Error("start_timeout".into())),
            "the startup watchdog fired on a recording that had already started"
        );
    }

    /// No first block within the window → an honest `start_timeout` and a
    /// `DeviceNotFound` (fatal — no point burning the reconnect budget on a
    /// device that never opened).
    #[tokio::test(start_paused = true)]
    async fn a_capture_that_never_starts_times_out() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceNotFound)
            }
        );
        assert!(r.sink.has(&Ev::Error("start_timeout".into())));
        assert_eq!(
            r.stops.load(Ordering::Relaxed),
            1,
            "the stack must be torn down"
        );
    }

    /// The stuck detector: bytes frozen past the tolerance while the segment is
    /// otherwise healthy. This is the arm that needs BOTH clocks — tokio's for
    /// the poll tick, wall-clock ms for the verdict.
    #[tokio::test(start_paused = true)]
    async fn frozen_bytes_are_detected_as_stuck() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let clock = FakeClock::default();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.bytes.store(44, Ordering::Relaxed); // a header, then nothing

        let ticker = tokio::spawn({
            let handle = clock.handle();
            async move {
                // Walk both clocks forward together, past the stall tolerance.
                for _ in
                    0..(RecorderTimeouts::STUCK_PROGRESS_MS / RecorderTimeouts::STUCK_POLL_MS + 2)
                {
                    tokio::time::sleep(Duration::from_millis(RecorderTimeouts::STUCK_POLL_MS))
                        .await;
                    handle.fetch_add(RecorderTimeouts::STUCK_POLL_MS, Ordering::Relaxed);
                }
            }
        });
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            clock.handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        ticker.abort();

        assert_eq!(out, SegmentOutcome::UnexpectedExit { last_error: None });
        // The CHANNEL is the assertion, not just the code. A stall the recovery
        // policy answers with `Reconnect` must never reach `recording://error`:
        // that channel is terminal (the overlay comes down) and it is the one
        // `notify::wire_failure_sources` turns into a native alert and an
        // e-mail. Both said "the recording failed" while it was reconnecting.
        assert!(r.sink.has(&Ev::Warning("stuck_recording".into())));
        assert!(
            !r.sink.has(&Ev::Error("stuck_recording".into())),
            "a stall that reconnects must not appear on the terminal channel"
        );
    }

    /// The writer dying WITHOUT an error event — its channel simply closes.
    /// This must not be mistaken for a graceful stop: the supervisor has to see
    /// an unexpected exit so the recovery policy runs.
    #[tokio::test(start_paused = true)]
    async fn a_writer_that_vanishes_is_an_unexpected_exit() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::WriterGone).unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::UnexpectedExit { last_error: None });
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_disk_full_writer_error_is_fatal_and_warned_as_an_error() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Error {
            kind: WriterErrorKind::DiskFull,
            message: "No space left on device".into(),
        }))
        .unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DiskFull)
            }
        );
        assert!(r.sink.has(&Ev::Error("disk_full".into())));
        assert!(
            !r.sink.has(&Ev::Warning("disk_full".into())),
            "a fatal error must not be downgraded to a warning"
        );
    }

    /// A transient I/O error is a WARNING, not an error: the session continues
    /// and the supervisor reconnects. Emitting `recording://error` here would
    /// tear the UI down on a recording that is about to recover.
    #[tokio::test(start_paused = true)]
    async fn a_transient_writer_error_is_a_warning() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Error {
            kind: WriterErrorKind::Io,
            message: "input/output error".into(),
        }))
        .unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceError)
            }
        );
        assert!(r.sink.has(&Ev::Warning(
            error_code_str(RecordingErrorCode::DeviceError).into()
        )));
    }

    /// A cpal device error mid-session: warn (not error — the fragment is
    /// valid and the supervisor will reconnect) and report `DeviceDisconnected`
    /// so the recovery policy treats it as transient.
    #[tokio::test(start_paused = true)]
    async fn a_device_error_finalises_the_fragment_and_asks_for_a_reconnect() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::StreamError("device removed".into()))
            .unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceDisconnected)
            }
        );
        assert!(r.sink.has(&Ev::Warning("device_disconnected".into())));
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
    }

    /// The full silence sequence: silence begins → the warning fires after
    /// `SILENCE_WARN_MS` and the stop timer arms → nothing cancels it → the
    /// segment ends with `SilenceStop`.
    #[tokio::test(start_paused = true)]
    async fn silence_warns_then_stops() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let mut opts = test_opts("/tmp/x.wav");
        opts.stop_on_silence = true;
        // 5 minutes, comfortably past the 60 s warning: the two timers must be
        // ORDERED, and `SILENCE_WARN_MS` happens to equal the one-minute
        // setting, which would make this a coin flip in `select!`.
        opts.silence_timeout_minutes = 5;
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Silence(
            SilenceEvent::Start,
        )))
        .unwrap();

        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::SilenceStop);
        assert!(
            r.sink.has(&Ev::Silence),
            "the operator must be warned BEFORE the recording stops itself"
        );
    }

    /// Sound coming back cancels both silence timers. Without the cancel, a
    /// service with a two-minute quiet prelude would stop itself mid-sermon.
    #[tokio::test(start_paused = true)]
    async fn sound_returning_cancels_the_silence_stop() {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let mut opts = test_opts("/tmp/x.wav");
        opts.stop_on_silence = true;
        opts.silence_timeout_minutes = 1;
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Silence(
            SilenceEvent::Start,
        )))
        .unwrap();
        let tx = r.tx.clone();
        let ender = tokio::spawn(async move {
            // Sound returns well inside the one-minute stop window…
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = tx.send(SegmentSignal::Writer(WriterEvent::Silence(
                SilenceEvent::End,
            )));
            // …and the segment then runs long past where it would have stopped.
            tokio::time::sleep(Duration::from_secs(300)).await;
            let _ = stop_tx.send(()).await;
        });
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        ender.await.unwrap();
        assert_eq!(
            out,
            SegmentOutcome::GracefulStop,
            "the silence stop was not cancelled when sound returned"
        );
    }

    /// Extending the auto-stop deadline while recording must re-pin the timer
    /// AND re-stamp the state event, so the UI countdown follows.
    #[tokio::test(start_paused = true)]
    async fn extending_the_autostop_moves_the_deadline_and_restamps_state() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, w_tx, mut w_rx) = channels();
        let clock = FakeClock::default();
        clock.advance(1_000_000); // a plausible epoch
        let handle = clock.handle();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        // Deadline 10 s out.
        w_tx.send_replace(Some(handle.load(Ordering::Relaxed) + 10_000));

        let extender = tokio::spawn({
            let handle = handle.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                handle.fetch_add(5_000, Ordering::Relaxed);
                // Push it an hour out.
                w_tx.send_replace(Some(handle.load(Ordering::Relaxed) + 3_600_000));
                tokio::time::sleep(Duration::from_secs(60)).await;
                handle.fetch_add(60_000, Ordering::Relaxed);
                // Then bring it back so the test terminates.
                w_tx.send_replace(Some(handle.load(Ordering::Relaxed)));
                tokio::time::sleep(Duration::from_secs(3_600)).await;
            }
        });
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            handle.clone(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        extender.abort();

        assert_eq!(out, SegmentOutcome::AutoStop);
        assert!(
            r.sink
                .seen()
                .iter()
                .any(|e| matches!(e, Ev::State(Some(_)))),
            "the moved deadline was never re-stamped onto the UI"
        );
    }

    /// The RIFF cap reached mid-recording ends the segment as a `Split` (a new
    /// deliverable), not as an error.
    #[tokio::test(start_paused = true)]
    async fn the_riff_cap_ends_the_segment_as_a_split() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.bytes
            .store(over_every_possible_threshold(), Ordering::Relaxed);
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::Split);
        assert!(
            r.sink.seen().iter().all(|e| !matches!(e, Ev::Error(_))),
            "a rollover is not a failure: {:?}",
            r.sink.seen()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_disk_stops_the_segment_with_disk_full() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(0), // no free space at all
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::DiskStop);
        assert!(r.sink.has(&Ev::Error("disk_full".into())));
    }

    /// The split timer, and the guard that a zero `split_minutes` never arms it
    /// (an always-armed `sleep_opt(None)` would rotate the file immediately).
    #[tokio::test(start_paused = true)]
    async fn the_split_timer_fires_only_when_configured() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let mut opts = test_opts("/tmp/x.wav");
        opts.split_minutes = 30;
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::Split);

        // With split off, the same rig must run until something else ends it.
        let mut r2 = rig();
        let (stop_tx2, mut stop_rx2, _w2, mut w_rx2) = channels();
        let off = test_opts("/tmp/x.wav");
        r2.tx
            .send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let ender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60 * 60 * 4)).await;
            let _ = stop_tx2.send(()).await;
        });
        let out2 = drive(
            &mut r2,
            &off,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx2,
            &mut w_rx2,
            telemetry(),
        )
        .await;
        ender.await.unwrap();
        assert_eq!(out2, SegmentOutcome::GracefulStop);
    }

    /// Progress is emitted only AFTER the startup latch: a byte counter ticking
    /// while the UI still says "starter…" is how a never-started recording used
    /// to look alive.
    #[tokio::test(start_paused = true)]
    async fn progress_is_silent_until_the_capture_has_started() {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        // Below every legal split threshold, so a concurrent test that
        // lowers the override cannot turn this into a rollover.
        r.bytes
            .store(under_every_possible_threshold(), Ordering::Relaxed);
        let tx = r.tx.clone();
        let script = tokio::spawn(async move {
            // Five seconds of bytes-on-disk with no `Started` — no progress.
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = tx.send(SegmentSignal::Writer(WriterEvent::Started));
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = stop_tx.send(()).await;
        });
        let opts = test_opts("/tmp/x.wav");
        let before = {
            let out = drive(
                &mut r,
                &opts,
                0,
                Some(u64::MAX / 2),
                FakeClock::default().handle(),
                &mut stop_rx,
                &mut w_rx,
                telemetry(),
            )
            .await;
            script.await.unwrap();
            out
        };
        assert_eq!(before, SegmentOutcome::GracefulStop);
        let seen = r.sink.seen();
        let first_started = seen
            .iter()
            .position(|e| e == &Ev::Started)
            .expect("started");
        let first_progress = seen
            .iter()
            .position(|e| matches!(e, Ev::Progress(_)))
            .expect("progress after start");
        assert!(
            first_started < first_progress,
            "progress was emitted before the capture had started: {seen:?}"
        );
    }

    /// Levels follow the `live_levels` switch, and a stereo capture reports a
    /// right channel while a mono one reports `None` (the UI draws one meter).
    #[tokio::test(start_paused = true)]
    async fn levels_respect_the_switch_and_the_channel_count() {
        // OFF: not a single level event, however long the segment runs.
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let mut off = test_opts("/tmp/x.wav");
        off.live_levels = false;
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let ender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = stop_tx.send(()).await;
        });
        drive(
            &mut r,
            &off,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        ender.await.unwrap();
        assert!(
            !r.sink.seen().iter().any(|e| matches!(e, Ev::Levels(..))),
            "levels were emitted with live_levels off"
        );

        // ON, mono: a left channel and no right.
        let mut r2 = rig();
        r2.seg.spec = WavSpec {
            channels: 1,
            sample_rate: 48_000,
        };
        let (stop_tx2, mut stop_rx2, _w2, mut w_rx2) = channels();
        let on = test_opts("/tmp/x.wav");
        r2.tx
            .send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let ender2 = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = stop_tx2.send(()).await;
        });
        drive(
            &mut r2,
            &on,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx2,
            &mut w_rx2,
            telemetry(),
        )
        .await;
        ender2.await.unwrap();
        assert!(
            r2.sink.has(&Ev::Levels(-12.0, None)),
            "a mono capture must report no right channel: {:?}",
            r2.sink.seen()
        );
    }

    /// Telemetry folding happens on EVERY exit path, not just the graceful one
    /// — a segment that died with overruns must still report them, or the
    /// self-test verdict silently under-counts the drops that killed it.
    #[tokio::test(start_paused = true)]
    async fn telemetry_is_folded_even_when_the_segment_dies() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.overrun.store(4_096, Ordering::Relaxed);
        r.frames.store(96_000, Ordering::Relaxed); // 2 s at 48 kHz
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::WriterGone).unwrap();
        let tel = telemetry();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            tel.clone(),
        )
        .await;
        assert_eq!(out, SegmentOutcome::UnexpectedExit { last_error: None });
        let t = tel.lock().expect("telemetry");
        assert_eq!(t.ring_overrun_samples, 4_096);
        assert!(
            (t.native_frames_sec - 2.0).abs() < 1e-9,
            "frames→seconds used the wrong rate: {}",
            t.native_frames_sec
        );
    }

    /// Overruns ACCUMULATE across the fragments of one session — the supervisor
    /// reuses the telemetry across reconnects, so a second segment must add to
    /// the first, never overwrite it.
    #[tokio::test(start_paused = true)]
    async fn telemetry_accumulates_across_fragments() {
        let tel = telemetry();
        let opts = test_opts("/tmp/x.wav");
        for _ in 0..2 {
            let mut r = rig();
            let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
            r.overrun.store(1_000, Ordering::Relaxed);
            r.frames.store(48_000, Ordering::Relaxed);
            r.tx.send(SegmentSignal::WriterGone).unwrap();
            drive(
                &mut r,
                &opts,
                0,
                Some(u64::MAX / 2),
                FakeClock::default().handle(),
                &mut stop_rx,
                &mut w_rx,
                tel.clone(),
            )
            .await;
        }
        let t = tel.lock().expect("telemetry");
        assert_eq!(t.ring_overrun_samples, 2_000);
        assert!((t.native_frames_sec - 2.0).abs() < 1e-9);
    }

    /// A graceful stop is the one path that must NOT emit an error and must
    /// still tear the stack down exactly once.
    #[tokio::test(start_paused = true)]
    async fn a_graceful_stop_is_quiet() {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let ender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = stop_tx.send(()).await;
        });
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            0,
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            telemetry(),
        )
        .await;
        ender.await.unwrap();
        assert_eq!(out, SegmentOutcome::GracefulStop);
        assert!(r.sink.seen().iter().all(|e| !matches!(e, Ev::Error(_))));
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
    }

    fn test_opts(out: &str) -> RecordingOpts {
        RecordingOpts {
            audio_device_name: String::new(), // host default input
            video_device_name: None,
            output_path: out.to_string(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            channel_mode: ChannelMode::Stereo,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: None, // Auto → device native
            bitrate_kbps: 192,
            split_minutes: 0,
            manual_max_minutes: 0,
            live_levels: true,
            keep_separate_audio: false,
            separate_audio_format: "wav".into(),
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            video_input: None,
        }
    }

    /// Real-device end-to-end: 2 s on the default input → a playable, header-
    /// consistent WAV whose frame count agrees with the wall clock. SELF-
    /// SKIPPING (same pattern as the real-ffmpeg tests in `media/ffmpeg.rs`):
    /// no input device, or a build failure (CI runners, denied mic access) →
    /// the test reports why and passes vacuously.
    #[tokio::test(flavor = "multi_thread")]
    async fn native_capture_records_two_seconds_or_skips() {
        use cpal::traits::HostTrait;
        let Ok(host) = open_host(CpalHostKind::Default) else {
            eprintln!("SKIP: no default cpal host");
            return;
        };
        if host.default_input_device().is_none() {
            eprintln!("SKIP: no default input device on this machine");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("native-selftest.wav");
        let out_str = out.to_string_lossy().into_owned();
        let opts = test_opts(&out_str);

        let mut seg = match spawn_native_segment(CpalHostKind::Default, &opts, &out_str, None).await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("SKIP: native capture could not start here: {e}");
                return;
            }
        };

        tokio::time::sleep(Duration::from_secs(2)).await;
        stop_native_bounded(&mut seg).await;

        let bytes = std::fs::read(&out).expect("capture wav readable");
        let info = sundayrec_core::wav::parse_header(&bytes).expect("valid wav header");
        assert_eq!(
            info.sample_rate, seg.spec.sample_rate,
            "header rate == negotiated"
        );
        assert_eq!(
            info.channels, seg.spec.channels,
            "header channels == routed"
        );
        assert_eq!(info.format_tag, 1, "pcm");
        assert_eq!(info.bits_per_sample, 16, "s16 (the -c copy contract)");

        let frames = seg.frames.load(Ordering::Relaxed);
        let secs = frames as f64 / seg.spec.sample_rate as f64;
        assert!(
            (1.5..=3.0).contains(&secs),
            "captured {secs:.2}s of audio for a 2 s run (frames={frames})"
        );
        // Byte accounting is exact: header + frames×ch×2 == file length,
        // and the header's data field agrees.
        let expect_len =
            sundayrec_core::wav::HEADER_LEN as u64 + frames * seg.spec.bytes_per_frame();
        assert_eq!(bytes.len() as u64, expect_len, "file length matches frames");
        let data_field = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(u64::from(data_field), frames * seg.spec.bytes_per_frame());
        assert_eq!(
            seg.overrun.load(Ordering::Relaxed),
            0,
            "no ring overruns in a 2 s idle capture"
        );
    }
}
