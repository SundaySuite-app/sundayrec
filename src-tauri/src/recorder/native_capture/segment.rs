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
//! - stderr error classify    → typed `cpal::ErrorKind` (via the core's
//!   `stream_fault` table: keep recording / reconnect / retry / fatal) and
//!   `WriterErrorKind`
//! - xrun/overrun stderr lines → the OS's own count: cpal 0.18 reports each
//!   CoreAudio/WASAPI glitch as `ErrorKind::Xrun`; the stream's error callback
//!   counts them (`NativeSegment::xruns`) and the runner folds the total into
//!   `RecordingTelemetry::native_xruns` (A3c)
//! - `q` + wait (finalize)    → stop flags + bounded joins (writer patches
//!   the RIFF header and fsyncs)
//!
//! **The optional video track (C3a).** A native VIDEO session passes a
//! [`NativeVideoPlan`]: the segment then also opens the camera and starts a
//! [`VideoTrack`](super::video::VideoTrack) writing `<fragment>.mkv` beside the
//! WAV, fed by a clock tap on this stream's callback. Nothing in the select!
//! loop watches the video — a camera or encoder that dies is logged and
//! counted, and the sound goes on. The stop order is #341's, carried over from
//! `cpal_capture.rs`: the CAPTURES stop before the WRITERS (audio stream, then
//! camera, then the WAV writer, then the video track, which needs the WAV's
//! final frame count), and every wait has a time limit
//! ([`stop_native_bounded`]). A writer that dies without a word is announced
//! (#337's lesson from `two_process.rs`: the sound must never stop silently
//! while the picture keeps going).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::StreamTrait;
use ringbuf::traits::Split;
use sundayrec_core::alerts::AlertText;
use sundayrec_core::levels::SILENCE_FLOOR_DB;
use sundayrec_core::preflight::{
    finalize_reserve_bytes, low_disk_should_stop, min_disk_headroom_bytes,
};
use sundayrec_core::reconnect::{WatchdogState, WatchdogVerdict};
use sundayrec_core::recorder::RecordingSession;
use sundayrec_core::silence::{
    silence_threshold_db, SilenceAction, SilenceDetector, SilenceWatcher,
};
use sundayrec_core::timeouts::RecorderTimeouts;
use sundayrec_core::wav::WavSpec;
use tauri::{AppHandle, Emitter};

use crate::audio::asio::build_route_plan;
use crate::error::AppResult;
use crate::recorder::context::{SegmentCounters, SessionContext};
use crate::recorder::engine::{
    emit_error, emit_warning, error_code_str, now_ms, RecordingLevels, RecordingOpts,
    RecordingProgress, SegmentOutcome, StateWriter, LEVELS_EVENT, PROGRESS_EVENT, SILENCE_EVENT,
    STARTED_EVENT,
};
use crate::recorder::native_capture::stream::{
    build_input_stream_typed, classify_stream_error_counting, find_device_exact_typed,
    find_device_typed, negotiate_typed, open_host, ring_capacity, CpalHostKind, NativeStartError,
    StreamFaultReport, StreamSink,
};
use crate::recorder::native_capture::stream::{CaptureTimeSource, ClockPair, ClockTap};
use crate::recorder::native_capture::video::{
    open_camera, FfmpegSink, NativeVideoPlan, SinkFactory, VideoSource, VideoTrack,
    VideoTrackConfig, VideoTrackStats, ENCODER_EXIT_TIMEOUT, PAIR_RING,
};
use crate::recorder::native_capture::writer::{
    spawn_wav_writer, WriterConfig, WriterErrorKind, WriterEvent, FLUSH_EVERY, SYNC_EVERY,
};
use crate::util::lock_recover;
use sundayrec_core::audio::MeterBanks;
use sundayrec_core::errors::RecordingErrorCode;
use sundayrec_core::stream_fault::{stream_fault_action, StreamFault, StreamFaultAction};

/// How often the levels sampler reads the meters and emits (parity with the
/// VU engine; ~30/s).
const LEVELS_EVERY: Duration = Duration::from_millis(33);

/// A `Sleep` that fires after `d`, or never (a 100-year sleep) when `d` is None.
/// Lets the `select!` arm exist unconditionally; the arm's `if` guard gates it.
/// (Moved here from the deleted `engine/process.rs`, C3b — this loop is its
/// only user now.)
fn sleep_opt(d: Option<Duration>) -> tokio::time::Sleep {
    tokio::time::sleep(d.unwrap_or(Duration::from_secs(60 * 60 * 24 * 365 * 100)))
}

/// Await an optional pinned sleep; when `None`, never resolves. The `select!`
/// arm guards on `is_some()` so the `None` branch is never actually polled to
/// completion.
async fn wait_opt(s: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>) {
    match s {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}

/// Bound on joining the stream thread (it polls its stop flag every 50 ms and
/// then just drops the stream — 5 s only trips on a wedged CoreAudio teardown).
const STREAM_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on opening the camera at a fragment's start, and on stopping it. A
/// camera that takes longer is recorded as black — the audio never waits on it.
const CAMERA_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on the video track finishing: its last slots, then the encoder's own
/// exit (itself bounded by [`ENCODER_EXIT_TIMEOUT`]). Past it the encoder is
/// killed; the MKV is crash-tolerant and keeps what it has.
const VIDEO_FINISH_TIMEOUT: Duration = Duration::from_secs(5 + ENCODER_EXIT_TIMEOUT.as_secs());

/// One live native capture: the `!Send` stream on its own thread, the writer
/// thread, and the channels/counters the runner selects over.
pub struct NativeSegment {
    stream_stop: Arc<AtomicBool>,
    writer_stop: Arc<AtomicBool>,
    stream_join: Option<std::thread::JoinHandle<()>>,
    writer_join: Option<std::thread::JoinHandle<Result<super::writer::WriterSummary, String>>>,
    events: tokio::sync::mpsc::UnboundedReceiver<WriterEvent>,
    stream_err_rx: tokio::sync::mpsc::Receiver<StreamFaultReport>,
    meters: Arc<MeterBanks>,
    /// Samples dropped by the RT callback on ring overrun (whole frames).
    pub overrun: Arc<AtomicU64>,
    /// Xruns the audio system reported through cpal's error callback
    /// (`ErrorKind::Xrun`) while this segment ran — the real glitch count
    /// that replaces the ffmpeg path's stderr phrase scraping (A3c).
    pub xruns: Arc<AtomicU64>,
    /// Header+data bytes on disk — the watchdog/progress/disk-guard truth.
    pub bytes: Arc<AtomicU64>,
    /// Exact frames written — the truth-measurement cross-check.
    pub frames: Arc<AtomicU64>,
    /// The pinned capture format (routed channels + negotiated rate).
    pub spec: WavSpec,
    /// The cpal name of the device this segment actually opened — what a
    /// RECONNECT must find again, exactly, rather than whatever the stored
    /// label fuzzy-matches (`None` only if cpal could not report a name).
    pub device_name: Option<String>,
    /// The video track of a native VIDEO session (C3a); `None` for audio-only.
    video: Option<VideoTrack>,
    /// The video track's counters, read once it has stopped.
    pub video_stats: Option<VideoTrackStats>,
}

/// Open the device, negotiate its format, and start the full native stack
/// (stream thread + ring + writer) writing to `output_path`. Returns once the
/// stream is BUILT AND PLAYING (readiness discipline of `audio/vu.rs`) — a bad
/// device fails the Start call with a TYPED reason ([`NativeStartError`]); there
/// is no ffmpeg reserve to fall back to (A5b).
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
    spawn_native_segment_for(host, opts, output_path, pinned_rate, None, None)
        .await
        .map_err(NativeStartError::into_app_error)
}

/// [`spawn_native_segment`] with the reconnect rule: when `reopen` is
/// `Some(name)`, the device is found by EXACT name (the one the first segment
/// opened) instead of the fuzzy ladder, and a miss fails the spawn so the
/// supervisor's back-off keeps waiting for the real device. `None` is the first
/// start, where the stored label legitimately needs the ladder.
///
/// The failure is typed ([`NativeStartError`]): its code is what the volunteer's
/// sentence is picked by on a first start, and what the reconnect policy reads
/// on a reopen (where a missing device is transient, a refused permission is not
/// — see [`NativeStartError::for_reopen`]).
///
/// `video` is the session's [`NativeVideoPlan`] for a native VIDEO session
/// (C3a): the fragment then also records `<output>.mkv`. `None` = audio-only.
pub async fn spawn_native_segment_for(
    host: CpalHostKind,
    opts: &RecordingOpts,
    output_path: &str,
    pinned_rate: Option<u32>,
    reopen: Option<&str>,
    video: Option<&NativeVideoPlan>,
) -> Result<NativeSegment, NativeStartError> {
    spawn_native_segment_typed(host, opts, output_path, pinned_rate, reopen, video)
        .await
        .map_err(|e| if reopen.is_some() { e.for_reopen() } else { e })
}

async fn spawn_native_segment_typed(
    host: CpalHostKind,
    opts: &RecordingOpts,
    output_path: &str,
    pinned_rate: Option<u32>,
    reopen: Option<&str>,
    video_plan: Option<&NativeVideoPlan>,
) -> Result<NativeSegment, NativeStartError> {
    let exact = reopen.map(str::to_owned);
    let device_name = exact
        .clone()
        .unwrap_or_else(|| opts.audio_device_name.clone());
    let requested_rate = pinned_rate.or(opts.sample_rate);

    // Probe on a blocking thread: the (!Send) device handle never escapes.
    let (negotiated, opened_name) = {
        let name = device_name.clone();
        let exact = exact.is_some();
        tokio::task::spawn_blocking(move || -> Result<_, NativeStartError> {
            let h = open_host(host).map_err(NativeStartError::other)?;
            let device = if exact {
                find_device_exact_typed(&h, &name)?
            } else {
                find_device_typed(&h, &name)?
            };
            let opened = crate::audio::devices::device_name(&device);
            Ok((negotiate_typed(&device, requested_rate)?, opened))
        })
        .await
        .map_err(|e| NativeStartError::other(format!("device probe task failed: {e}")))??
    };
    // A first start that landed on a device whose name is not the stored one
    // (the ladder's substring/word-overlap strategies) is worth a trail: the
    // recording may be from a different microphone than the user picked. A UI
    // warning is deferred; reconnects never get here with a loose match.
    if let Some(opened) = opened_name.as_deref() {
        if !device_name.is_empty()
            && !sundayrec_core::device_match::names_match_exactly(opened, &device_name)
        {
            tracing::warn!(
                requested = %device_name,
                opened = %opened,
                "recorder: stored input device matched loosely, not by exact name"
            );
        }
    }

    // A reconnect (exact match) refuses a pick the returned device no longer
    // has, instead of letting `build_route_plan` clamp it onto the last channel.
    if exact.is_some() {
        crate::audio::asio::check_channel_picks(
            opts.channel_mode,
            opts.input_channel_l,
            opts.input_channel_r,
            negotiated.channels,
        )
        .map_err(|e| {
            NativeStartError::new(
                RecordingErrorCode::DeviceDisconnected,
                format!("input device not back: {e}"),
            )
        })?;
    }
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
    let xruns = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let meters = Arc::new(MeterBanks::new(out_ch as usize));
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<WriterEvent>();
    let (err_tx, stream_err_rx) = tokio::sync::mpsc::channel::<StreamFaultReport>(1);

    // Writer first, so the stream never produces into a ring nobody drains.
    let writer_join = spawn_wav_writer(
        cons,
        WriterConfig {
            path: std::path::PathBuf::from(output_path),
            spec,
            flush_every: FLUSH_EVERY,
            sync_every: SYNC_EVERY,
            silence: SilenceDetector::new(
                silence_threshold_db(opts.stop_on_silence, opts.silence_threshold_db),
                spec.sample_rate,
            ),
            sync_count: None,
        },
        Arc::clone(&writer_stop),
        Arc::clone(&bytes),
        Arc::clone(&frames),
        event_tx,
    )
    .map_err(|e| NativeStartError::other(format!("could not spawn wav writer: {e}")))?;

    // The video track (C3a), before the stream: the stream's callback feeds
    // its clock, and the camera should be delivering by the first slot. A
    // camera that cannot open costs the PICTURE (black slots), never the start.
    let (clock_tap, video) = match video_plan {
        None => (None, None),
        Some(plan) => {
            let (pair_prod, pair_cons) = ringbuf::HeapRb::<ClockPair>::new(PAIR_RING).split();
            let mkv = sundayrec_core::video_track::video_fragment_path(output_path);
            let track = start_video_track(plan, &mkv, spec.sample_rate, pair_cons).await;
            (
                Some(ClockTap::new(
                    pair_prod,
                    CaptureTimeSource::for_host(host),
                    spec.sample_rate,
                )),
                track,
            )
        }
    };

    // Stream thread: reopen host+device (the Stream is !Send), build, play,
    // report readiness over an async oneshot, park until stopped.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), NativeStartError>>();
    let st_stop = Arc::clone(&stream_stop);
    let st_meters = Arc::clone(&meters);
    let st_overrun = Arc::clone(&overrun);
    let st_xruns = Arc::clone(&xruns);
    let st_name = device_name.clone();
    let exact_reopen = exact.is_some();
    let stream_join = std::thread::Builder::new()
        .name("native-capture".into())
        .spawn(move || {
            let build = (|| -> Result<cpal::Stream, NativeStartError> {
                let h = open_host(host).map_err(NativeStartError::other)?;
                let device = if exact_reopen {
                    find_device_exact_typed(&h, &st_name)?
                } else {
                    find_device_typed(&h, &st_name)?
                };
                let config = cpal::StreamConfig {
                    channels: negotiated.channels,
                    sample_rate: negotiated.sample_rate,
                    buffer_size: cpal::BufferSize::Default,
                };
                // Only a fault that ends the stream reaches the runner; the
                // advisories (rerouted, realtime refused) are logged inside
                // `classify_stream_error_counting` and recording carries on.
                // An xrun is additionally COUNTED into `st_xruns`.
                let err_fn = move |e: cpal::Error| {
                    if let Some(report) =
                        classify_stream_error_counting("native capture stream", &e, Some(&st_xruns))
                    {
                        let _ = err_tx.try_send(report);
                    }
                };
                let stream = build_input_stream_typed(
                    &device,
                    config,
                    negotiated.sample_format,
                    negotiated.channels as usize,
                    StreamSink::Capture {
                        meters: st_meters,
                        plan,
                        prod,
                        overrun: st_overrun,
                        clock: clock_tap,
                    },
                    err_fn,
                )?;
                stream
                    .play()
                    .map_err(|e| NativeStartError::from_cpal("starting stream", &e))?;
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
        .map_err(|e| NativeStartError::other(format!("could not spawn capture thread: {e}")))?;

    let mut seg = NativeSegment {
        stream_stop,
        writer_stop,
        stream_join: Some(stream_join),
        writer_join: Some(writer_join),
        events: event_rx,
        stream_err_rx,
        meters,
        overrun,
        xruns,
        bytes,
        frames,
        spec,
        device_name: opened_name,
        video,
        video_stats: None,
    };

    match ready_rx.await {
        Ok(Ok(())) => Ok(seg),
        Ok(Err(e)) => {
            abort_native_segment(&mut seg, output_path).await;
            Err(e)
        }
        Err(_) => {
            abort_native_segment(&mut seg, output_path).await;
            Err(NativeStartError::other(
                "native capture thread exited before signalling",
            ))
        }
    }
}

/// Open the camera (bounded) and start the fragment's video track. `None`
/// only if the track's thread could not be spawned at all; a camera that will
/// not open still gets a track (black slots), so the MKV matches the WAV.
async fn start_video_track(
    plan: &NativeVideoPlan,
    mkv: &str,
    sample_rate: u32,
    pairs: ringbuf::HeapCons<ClockPair>,
) -> Option<VideoTrack> {
    let (name, w, h, fps) = (plan.camera_name.clone(), plan.width, plan.height, plan.fps);
    let opened = tokio::time::timeout(
        CAMERA_TIMEOUT,
        tokio::task::spawn_blocking(move || open_camera(&name, w, h, fps)),
    )
    .await;
    let mut spec = sundayrec_core::video_track::VideoTrackSpec {
        width: plan.width,
        height: plan.height,
        fps_num: plan.fps.num,
        fps_den: plan.fps.den,
        input: sundayrec_core::video_track::RawPixelFormat::Nv12,
        flip: plan.flip,
    };
    let source: Option<Box<dyn VideoSource>> = match opened {
        Ok(Ok(Ok((capture, cam_spec)))) => {
            spec = sundayrec_core::video_track::VideoTrackSpec {
                flip: plan.flip,
                ..cam_spec
            };
            Some(Box::new(capture))
        }
        Ok(Ok(Err(e))) => {
            tracing::error!(camera = %plan.camera_name, "video track: the camera did not open ({e}) — recording black, the sound is unaffected");
            None
        }
        _ => {
            tracing::error!(camera = %plan.camera_name, "video track: the camera did not open in time — recording black, the sound is unaffected");
            None
        }
    };
    let mkv_path = std::path::PathBuf::from(mkv);
    let factory_path = mkv_path.clone();
    let child = std::sync::Arc::new(std::sync::Mutex::new(None));
    let factory_child = std::sync::Arc::clone(&child);
    let make: SinkFactory = Box::new(move |spec, encoder| {
        FfmpegSink::spawn(
            spec,
            encoder,
            &factory_path,
            std::sync::Arc::clone(&factory_child),
            // The recording screen's live picture (C3b; C4 replaces it).
            Some(&crate::recorder::engine::recording_preview_path()),
        )
    });
    match VideoTrack::spawn_with_child(
        VideoTrackConfig {
            mkv_path,
            spec,
            encoder: plan.encoder,
            sample_rate,
        },
        source,
        pairs,
        make,
        child,
    ) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::error!("video track: could not start its thread ({e}) — recording sound only");
            None
        }
    }
}

/// Tear down a stack that must not record (a failed spawn, or a reconnect that
/// came back at an incompatible rate), and remove the WAV when it holds no
/// audio so the next attempt starts from a clean capture dir (and its MKV with
/// it — a video fragment without its audio fragment is never delivered).
pub(crate) async fn abort_native_segment(seg: &mut NativeSegment, output_path: &str) {
    stop_native_bounded(seg).await;
    if seg.frames.load(Ordering::Relaxed) == 0 {
        let _ = tokio::fs::remove_file(output_path).await;
        let _ = tokio::fs::remove_file(sundayrec_core::video_track::video_fragment_path(
            output_path,
        ))
        .await;
    }
}

/// Stop the stack in order — the CAPTURES first (audio stream, then camera:
/// producers gone), then the WRITERS (the WAV writer drains the ring, patches
/// the header, fsyncs; then the video track decides its last slots up to the
/// WAV's final length and closes its encoder) — with a time limit on every
/// wait so a wedged teardown can never freeze the supervisor (the "UI stuck on
/// Stopping" class of bug). This is #341's order from `cpal_capture.rs`.
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
    // The camera next: a capture, so before any writer.
    if let Some(source) = seg.video.as_mut().and_then(VideoTrack::take_source) {
        let stop = tokio::task::spawn_blocking(move || source.stop());
        if tokio::time::timeout(CAMERA_TIMEOUT, stop).await.is_err() {
            tracing::warn!("video track: the camera did not stop in time — continuing");
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
    // Last, the video track: it needs the WAV's final length to write exactly
    // as many slots as the audio has (`slot_count`), so it finishes after the
    // writer has drained.
    if let Some(mut video) = seg.video.take() {
        video.request_finish(seg.frames.load(Ordering::Relaxed));
        if let Some(h) = video.take_join() {
            let join = tokio::task::spawn_blocking(move || h.join());
            let mut join = std::pin::pin!(join);
            if tokio::time::timeout(VIDEO_FINISH_TIMEOUT, &mut join)
                .await
                .is_err()
            {
                // A wedged encoder that stopped reading: kill it, which fails
                // the blocked write, and give the thread a moment to exit.
                tracing::warn!("video track: did not finish in time — killing the encoder");
                video.kill_encoder();
                let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
            }
        }
        let stats = video.stats();
        tracing::info!(?stats, mkv = %video.mkv_path.display(), "video track: fragment finished");
        seg.video_stats = Some(stats);
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
    /// The cpal stream reported a fault that ends it (USB pulled, format
    /// changed, device busy, permission refused) — typed, so the runner can
    /// tell them apart. Advisories never arrive here (see
    /// [`classify_stream_error_counting`]).
    StreamError(StreamFaultReport),
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
    /// Xruns the audio system reported while this capture ran.
    fn xruns(&self) -> u64;
    /// The pinned capture format.
    fn spec(&self) -> WavSpec;
    /// Take-and-reset the peak meter for `channel`, in dBFS.
    fn take_peak_dbfs(&self, channel: usize) -> f32;
    /// Stop the stack with bounded joins (stream first, then writer).
    fn stop(&mut self) -> impl std::future::Future<Output = ()> + Send;
    /// The video track's counters once stopped (C3a); `None` for audio-only.
    fn video_stats(&self) -> Option<VideoTrackStats> {
        None
    }
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
            // A closed channel means the stream (which owns the sender) is
            // gone without a report: unclassified, i.e. the old reconnect.
            msg = self.stream_err_rx.recv() => SegmentSignal::StreamError(
                msg.unwrap_or_else(|| StreamFaultReport {
                    fault: StreamFault::Other,
                    detail: "audio device error".into(),
                }),
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

    fn xruns(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
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

    fn video_stats(&self) -> Option<VideoTrackStats> {
        self.video_stats
    }
}

/// Where the runner's user-visible output goes.
///
/// The production implementation is a `tauri::AppHandle`; the test one is a
/// recording buffer. Without this the loop could not be run at all off a GUI
/// thread — an `AppHandle` cannot be constructed in a unit test.
pub(crate) trait EventSink {
    /// `recording://error` — classified, terminal for this segment. `message`
    /// is already a sentence for the volunteer.
    fn error(&self, code: &str, message: &str);
    /// `recording://error` for a failure whose only words are diagnostics:
    /// the event carries the localized sentence for `code`, `detail` goes to
    /// the log (see [`crate::recorder::engine::emit_failure`]).
    fn failure(&self, code: &str, detail: &str) {
        tracing::error!(code, %detail, "recorder: native capture — terminal failure");
        self.error(
            code,
            &AlertText::for_recording_code(code).text(crate::ui_lang::current()),
        );
    }
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
    /// `true` when this segment is a RECONNECT piece (`_rN`), not the session's
    /// first. A first segment that never opens is a failed start (fatal); a
    /// reconnect segment that is merely slow must not end a session whose
    /// reconnect budget is untouched.
    pub reconnecting: bool,
}

/// The production sink: a real `AppHandle` plus the two pieces of session state
/// the `state` payload carries. The state half goes through the engine's ONE
/// guarded door — this sink cannot reach the shared last-state directly, so a
/// superseded segment cannot re-stamp the live session's countdown.
pub(crate) struct AppEventSink<'a> {
    app: &'a AppHandle,
    writer: &'a StateWriter,
    reconnect_count: u32,
}

impl EventSink for AppEventSink<'_> {
    fn error(&self, code: &str, message: &str) {
        emit_error(self.app, code, message);
    }
    fn failure(&self, code: &str, detail: &str) {
        crate::recorder::engine::emit_failure(self.app, code, detail);
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
        self.writer.restamp(self.reconnect_count, scheduled_stop_ms);
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
///
/// F2-T2: the session half of its inputs (the app handle, the options and the
/// generation-guarded [`StateWriter`]) arrives in `ctx`, and the segment's three
/// counters in [`SegmentCounters`] — the SAME two groups `engine::run_segment`
/// takes, which is the point: the two segment loops now demonstrably read the
/// same state door, rather than each being handed its own set of loose arguments.
pub(crate) async fn run_native_segment(
    ctx: &SessionContext,
    mut seg: NativeSegment,
    session: &RecordingSession,
    counters: SegmentCounters,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
) -> SegmentOutcome {
    let sink = AppEventSink {
        app: &ctx.app,
        writer: &ctx.state,
        reconnect_count: session.reconnect_count(),
    };
    // The capture folder's volume. Probed fresh on every disk tick — the
    // recording folder can live on a drive that is unmounted mid-service.
    let disk_folder = std::path::Path::new(&ctx.opts.output_path)
        .parent()
        .map(|p| p.to_path_buf());
    let free_bytes = nonblocking_probe(move || -> Option<u64> {
        disk_folder
            .as_ref()
            .and_then(|folder| fs4::available_space(folder).ok())
    });
    let env = SegmentEnv {
        now_ms: &now_ms,
        free_bytes: &free_bytes,
        reconnecting: session.reconnect_count() > 0,
    };
    let outcome = drive_native_segment(
        &mut seg, &sink, &env, &ctx.opts, counters, stop_rx, stop_watch,
    )
    .await;
    // C0a, through the video track (C3b): a hardware encoder that failed at
    // the first frame made the track fall back to libx264 — forget the stored
    // choice so the next recording does not try it again.
    if seg.video_stats.is_some_and(|v| v.encoder_fallback) {
        crate::media::video_encoder::note_encoder_fallback(&ctx.app);
    }
    outcome
}

/// Wrap a BLOCKING free-space probe so calling it from the async segment loop
/// never blocks the loop. `fs4::available_space` is a syscall: on a network share
/// or a USB drive that has stalled it can hang for minutes, and inside the
/// `select!` that would freeze the stop request, the watchdogs and the meters
/// with it. Each call starts at most one probe on the blocking pool and answers
/// with the LAST finished result (`None` until the first one lands) — one disk
/// tick (30 s) stale at worst, against a headroom measured in hundreds of MiB.
/// Must be called from inside a tokio runtime.
fn nonblocking_probe(
    probe: impl Fn() -> Option<u64> + Send + Sync + 'static,
) -> impl Fn() -> Option<u64> + Sync {
    use std::sync::atomic::AtomicBool;
    const UNKNOWN: u64 = u64::MAX;
    let probe = Arc::new(probe);
    let last = Arc::new(AtomicU64::new(UNKNOWN));
    let inflight = Arc::new(AtomicBool::new(false));
    move || {
        if !inflight.swap(true, Ordering::AcqRel) {
            let (probe, last, inflight) =
                (Arc::clone(&probe), Arc::clone(&last), Arc::clone(&inflight));
            tokio::task::spawn_blocking(move || {
                last.store(probe().unwrap_or(UNKNOWN), Ordering::Release);
                inflight.store(false, Ordering::Release);
            });
        }
        match last.load(Ordering::Acquire) {
            UNKNOWN => None,
            v => Some(v),
        }
    }
}

/// The segment runner proper: one `select!` loop over every signal and timer a
/// live capture has, generic over the capture ([`SegmentSignals`]), its output
/// ([`EventSink`]) and the free-space probe.
///
/// Everything here used to be inlined in [`run_native_segment`] behind an
/// `AppHandle` and a real cpal stream, which is why none of it had a test.
///
/// It takes [`SegmentCounters`] rather than a [`SessionContext`] (F2-T2) on
/// purpose: this function is the UNIT-TESTED core, and it is testable precisely
/// because it is generic over its capture, its sink and its clock instead of
/// holding a real `AppHandle`. The counters are the part a test does supply.
pub(crate) async fn drive_native_segment<S, E>(
    seg: &mut S,
    sink: &E,
    env: &SegmentEnv<'_>,
    opts: &RecordingOpts,
    counters: SegmentCounters,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
) -> SegmentOutcome
where
    S: SegmentSignals,
    E: EventSink,
{
    let SegmentCounters {
        segment_bytes,
        deliverable_bytes,
        telemetry,
    } = counters;
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
                            sink.failure(code_str, &message);
                        } else {
                            sink.warning(code_str, &message);
                        }
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit { last_error: Some(verdict.code), stream_fault: None };
                    }
                    SegmentSignal::WriterGone => {
                        // Writer gone without an error event — treat as an
                        // unexpected exit; the recovery policy decides. SAY so
                        // (#337, inherited from `two_process.rs`): in a video
                        // session the picture can keep going while the sound
                        // has stopped, and a silent reconnect would leave the
                        // volunteer believing the sound was still recorded.
                        sink.warning(
                            "device_disconnected",
                            "the audio capture stopped unexpectedly — reconnecting",
                        );
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit { last_error: None, stream_fault: None };
                    }
                    // A stream fault mid-session. The class decides (core
                    // `stream_fault` table): unplugged / format changed →
                    // reconnect, busy → retry under its own code, permission →
                    // fatal with the permission sentence. Either way the
                    // fragment is finalized first, so the _rN concat (or the
                    // fail-stop delivery) has a valid piece.
                    SegmentSignal::StreamError(StreamFaultReport { fault, detail }) => {
                        let code = match stream_fault_action(fault) {
                            // Filtered out at the callback; harmless if one
                            // slips through — the stream is still running.
                            StreamFaultAction::KeepRecording => {
                                tracing::warn!(?fault, %detail, "native capture: advisory reached the runner — still recording");
                                continue;
                            }
                            StreamFaultAction::Reconnect(code) => {
                                sink.warning(error_code_str(code), &detail);
                                code
                            }
                            StreamFaultAction::Fatal(code) => {
                                sink.failure(error_code_str(code), &detail);
                                code
                            }
                        };
                        seg.stop().await;
                        break SegmentOutcome::UnexpectedExit {
                            last_error: Some(code),
                            stream_fault: Some(fault),
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
                seg.stop().await;
                if env.reconnecting {
                    // A reconnect piece that is slow to deliver its first block
                    // (a mixer that re-enumerates for >12 s) is NOT a failed
                    // start: `DeviceNotFound` is fatal and would end the session
                    // with the reconnect budget untouched, and `sink.error` is
                    // the terminal channel. Warn and let the recovery policy
                    // retry — `DeviceDisconnected` is transient.
                    sink.warning(
                        "start_timeout",
                        &AlertText::RecordingStartTimeoutMic.text(crate::ui_lang::current()),
                    );
                    break SegmentOutcome::UnexpectedExit {
                        last_error: Some(RecordingErrorCode::DeviceDisconnected),
                        stream_fault: None,
                    };
                }
                sink.error(
                    "start_timeout",
                    &AlertText::RecordingStartTimeoutMic.text(crate::ui_lang::current()),
                );
                break SegmentOutcome::UnexpectedExit {
                    last_error: Some(RecordingErrorCode::DeviceNotFound),
                    stream_fault: None,
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
                        // English reserve, verbatim from the ffmpeg twin —
                        // see `engine.rs`: the code is what the shell renders.
                        &format!(
                            "no progress for {} s — reconnecting",
                            RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                        ),
                    );
                    seg.stop().await;
                    break SegmentOutcome::UnexpectedExit { last_error: None, stream_fault: None };
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
                            &AlertText::RecordingDiskFull.text(crate::ui_lang::current()),
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
    let xruns = seg.xruns();
    if xruns > 0 {
        tracing::warn!(xruns, "native capture: the audio system reported xruns");
    }
    // C3a: the video track's account of this fragment. The telemetry counter
    // (`video.frames_dropped`) is C3c's; until then it is in the log.
    if let Some(v) = seg.video_stats() {
        if v.queue_dropped > 0 || v.encoder_failed {
            tracing::warn!(
                ?v,
                "video track: frames dropped or the encoder failed this fragment"
            );
        }
    }
    {
        let mut t = lock_recover(&telemetry);
        t.ring_overrun_samples = t.ring_overrun_samples.saturating_add(overrun);
        t.native_xruns = t.native_xruns.saturating_add(xruns);
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
    // Production code reaches the telemetry mutex through `SegmentCounters` now;
    // the fakes below still build their own.
    use std::sync::Mutex;
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
    ///
    /// F2-W7 (#231) had this `#[ignore]`d on Windows after it killed the test process
    /// with STATUS_ACCESS_VIOLATION; that was read as a stream-building (or
    /// enumeration) problem, even under `--test-threads=1`. The real cause was
    /// cpal's cached WASAPI enumerator outliving the COM apartment of the
    /// short-lived test thread that created it — libtest gives every test its own
    /// thread, even single-threaded. Fixed at the root in `audio::com_anchor`, and
    /// proven on windows-latest (probe run 35465912824): the sequential cpal tests
    /// died 10/10 on main and 0/10 with the anchor, with this test included.
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
        xruns: Arc<AtomicU64>,
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
        fn xruns(&self) -> u64 {
            self.xruns.load(Ordering::Relaxed)
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
        xruns: Arc<AtomicU64>,
        stops: Arc<AtomicU64>,
        sink: Recorder,
    }

    fn rig() -> Rig {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let frames = Arc::new(AtomicU64::new(0));
        let overrun = Arc::new(AtomicU64::new(0));
        let xruns = Arc::new(AtomicU64::new(0));
        let stops = Arc::new(AtomicU64::new(0));
        Rig {
            seg: Scripted {
                rx,
                bytes: Arc::clone(&bytes),
                frames: Arc::clone(&frames),
                overrun: Arc::clone(&overrun),
                xruns: Arc::clone(&xruns),
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
            xruns,
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

    /// A throwaway set of segment counters: a fresh byte counter, nothing
    /// carried over from a previous fragment, and a telemetry sink the test
    /// discards. What most cases here want.
    fn counters() -> SegmentCounters {
        counters_with(telemetry())
    }

    /// …and the variant for a case that reads the telemetry back afterwards.
    fn counters_with(
        tel: Arc<Mutex<sundayrec_core::selftest::RecordingTelemetry>>,
    ) -> SegmentCounters {
        SegmentCounters {
            segment_bytes: Arc::new(AtomicU64::new(0)),
            deliverable_bytes: 0,
            telemetry: tel,
        }
    }

    /// Drive the runner to completion under paused time. Takes the very same
    /// [`SegmentCounters`] the production caller builds (F2-T2), so the shape
    /// under test is the shape that ships.
    async fn drive(
        rig: &mut Rig,
        opts: &RecordingOpts,
        counters: SegmentCounters,
        free: Option<u64>,
        clock: Arc<AtomicU64>,
        stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
        stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
    ) -> SegmentOutcome {
        drive_as(rig, opts, counters, free, clock, stop_rx, stop_watch, false).await
    }

    /// [`drive`], but for a reconnect (`_rN`) segment when `reconnecting`.
    #[allow(clippy::too_many_arguments)]
    async fn drive_as(
        rig: &mut Rig,
        opts: &RecordingOpts,
        counters: SegmentCounters,
        free: Option<u64>,
        clock: Arc<AtomicU64>,
        stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
        stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
        reconnecting: bool,
    ) -> SegmentOutcome {
        let now = move || clock.load(Ordering::Relaxed);
        let free_fn = move || free;
        let env = SegmentEnv {
            now_ms: &now,
            free_bytes: &free_fn,
            reconnecting,
        };
        drive_native_segment(
            &mut rig.seg,
            &rig.sink,
            &env,
            opts,
            counters,
            stop_rx,
            stop_watch,
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
            counters(),
            Some(u64::MAX / 2),
            clock.handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceNotFound),
                stream_fault: None,
            }
        );
        assert!(r.sink.has(&Ev::Error("start_timeout".into())));
        assert_eq!(
            r.stops.load(Ordering::Relaxed),
            1,
            "the stack must be torn down"
        );
    }

    /// The free-space probe must not block the segment loop: a stalled volume
    /// (a hung network share) would otherwise freeze stop, watchdogs and meters.
    #[tokio::test]
    async fn a_stalled_free_space_probe_does_not_block_the_caller() {
        let started = std::time::Instant::now();
        let probe = nonblocking_probe(|| {
            std::thread::sleep(Duration::from_millis(1500));
            Some(42)
        });
        assert_eq!(probe(), None, "no result yet — and no waiting for one");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the probe call blocked the caller for {:?}",
            started.elapsed()
        );
        // Once the blocking call finishes, its result is served.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if probe() == Some(42) {
                return;
            }
        }
        panic!("the probe result never arrived");
    }

    /// A RECONNECT segment that is slow to deliver its first block is not a
    /// failed start: a warning and a transient code, so the recovery policy
    /// retries instead of ending the session with its budget untouched.
    #[tokio::test(start_paused = true)]
    async fn a_slow_reconnect_segment_warns_and_stays_recoverable() {
        let mut r = rig();
        let (_stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        let opts = test_opts("/tmp/x.wav");
        let out = drive_as(
            &mut r,
            &opts,
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
            true,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceDisconnected),
                stream_fault: None,
            }
        );
        assert!(!sundayrec_core::recorder::is_fatal_reconnect_error(
            RecordingErrorCode::DeviceDisconnected
        ));
        assert!(r.sink.has(&Ev::Warning("start_timeout".into())));
        assert!(
            !r.sink.has(&Ev::Error("start_timeout".into())),
            "a reconnect piece must not fire the terminal error channel"
        );
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
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
            counters(),
            Some(u64::MAX / 2),
            clock.handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        ticker.abort();

        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: None,
                stream_fault: None
            }
        );
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
    /// an unexpected exit so the recovery policy runs. And it must be SAID
    /// (#337): with a video track the picture keeps going, so a silent
    /// reconnect would hide that the sound stopped.
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: None,
                stream_fault: None
            }
        );
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
        assert!(
            r.sink.has(&Ev::Warning("device_disconnected".into())),
            "a dead audio writer is announced, not swallowed"
        );
        assert!(
            !r.sink.has(&Ev::Error("device_disconnected".into())),
            "…as a warning: the session reconnects"
        );
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DiskFull),
                stream_fault: None,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceError),
                stream_fault: None,
            }
        );
        assert!(r.sink.has(&Ev::Warning(
            error_code_str(RecordingErrorCode::DeviceError).into()
        )));
    }

    /// Feed ONE cpal error, exactly as cpal's error callback would deliver it,
    /// through the production classification (`classify_stream_error`) into a
    /// running segment, and return what the segment did. `None` from the
    /// classifier (an advisory) sends nothing, and the segment is then stopped
    /// by the user so the test can tell "kept recording" from "ended".
    async fn run_with_cpal_error(kind: cpal::ErrorKind) -> (SegmentOutcome, Rig) {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        let e = cpal::Error::with_message(kind, "scripted");
        match crate::recorder::native_capture::stream::classify_stream_error("test stream", &e) {
            Some(report) => r.tx.send(SegmentSignal::StreamError(report)).unwrap(),
            None => stop_tx.send(()).await.unwrap(),
        }
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        (out, r)
    }

    /// A3b class 1 — `DeviceNotAvailable` (USB pulled): warn (not error — the
    /// fragment is valid and the supervisor will reconnect) and hand the
    /// recovery policy the transient `DeviceDisconnected`.
    #[tokio::test(start_paused = true)]
    async fn an_unplugged_device_finalises_the_fragment_and_reconnects() {
        let (out, r) = run_with_cpal_error(cpal::ErrorKind::DeviceNotAvailable).await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceDisconnected),
                stream_fault: Some(StreamFault::DeviceNotAvailable),
            }
        );
        assert!(r.sink.has(&Ev::Warning("device_disconnected".into())));
        assert!(!r.sink.seen().iter().any(|e| matches!(e, Ev::Error(_))));
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
    }

    /// A3b class 2 — `StreamInvalidated` (macOS: the device's sample rate
    /// changed): the same reconnect, and the fault travels with the outcome so
    /// the supervisor reopens at the device's NEW rate instead of re-clocking
    /// it back to the pinned one.
    #[tokio::test(start_paused = true)]
    async fn a_rate_change_reconnects_and_releases_the_rate_pin() {
        let (out, r) = run_with_cpal_error(cpal::ErrorKind::StreamInvalidated).await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceDisconnected),
                stream_fault: Some(StreamFault::StreamInvalidated),
            }
        );
        assert!(r.sink.has(&Ev::Warning("device_disconnected".into())));
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
        let SegmentOutcome::UnexpectedExit { stream_fault, .. } = out else {
            unreachable!()
        };
        assert!(!sundayrec_core::stream_fault::reconnect_keeps_rate_pin(
            stream_fault
        ));
    }

    /// A3b class 3 — `DeviceBusy`: a retry under its OWN code (transient, so
    /// the recovery policy retries on its back-off), not "disconnected".
    #[tokio::test(start_paused = true)]
    async fn a_busy_device_is_retried_as_device_busy() {
        let (out, r) = run_with_cpal_error(cpal::ErrorKind::DeviceBusy).await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceBusy),
                stream_fault: Some(StreamFault::DeviceBusy),
            }
        );
        assert!(r.sink.has(&Ev::Warning("device_busy".into())));
        assert!(!r.sink.seen().iter().any(|e| matches!(e, Ev::Error(_))));
        assert!(!sundayrec_core::recorder::is_fatal_reconnect_error(
            RecordingErrorCode::DeviceBusy
        ));
    }

    /// A3b class 4 — `PermissionDenied`: terminal. The segment raises a
    /// `recording://error` under the permission code (whose sentence tells the
    /// volunteer to grant microphone access), and the code it hands the
    /// supervisor is one the recovery policy gives up on.
    #[tokio::test(start_paused = true)]
    async fn a_denied_permission_is_fatal_with_the_permission_message() {
        let (out, r) = run_with_cpal_error(cpal::ErrorKind::PermissionDenied).await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DevicePermissionDenied),
                stream_fault: Some(StreamFault::PermissionDenied),
            }
        );
        assert!(r.sink.has(&Ev::Error("device_permission_denied".into())));
        assert!(!r.sink.seen().iter().any(|e| matches!(e, Ev::Warning(_))));
        assert!(sundayrec_core::recorder::is_fatal_reconnect_error(
            RecordingErrorCode::DevicePermissionDenied
        ));
        assert_eq!(
            AlertText::for_recording_code("device_permission_denied"),
            AlertText::RecordingPermissionDenied
        );
        assert_eq!(r.stops.load(Ordering::Relaxed), 1);
    }

    /// The advisories cpal 0.18 sends through the SAME callback — a glitch, a
    /// reroute, refused real-time priority — must not cost a reconnect: the
    /// segment keeps recording until the user stops it.
    #[tokio::test(start_paused = true)]
    async fn advisories_do_not_end_the_segment() {
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            let (out, r) = run_with_cpal_error(kind).await;
            assert_eq!(out, SegmentOutcome::GracefulStop, "{kind:?}");
            assert!(
                !r.sink
                    .seen()
                    .iter()
                    .any(|e| matches!(e, Ev::Warning(_) | Ev::Error(_))),
                "{kind:?} must not reach the volunteer"
            );
        }
    }

    /// Belt and braces: should an advisory ever reach the runner (the filter
    /// lives in the callback), the runner too keeps recording.
    #[tokio::test(start_paused = true)]
    async fn an_advisory_that_reaches_the_runner_is_ignored() {
        let mut r = rig();
        let (stop_tx, mut stop_rx, _w_tx, mut w_rx) = channels();
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::StreamError(StreamFaultReport {
            fault: StreamFault::Xrun,
            detail: "xrun".into(),
        }))
        .unwrap();
        stop_tx.send(()).await.unwrap();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(out, SegmentOutcome::GracefulStop);
    }

    /// Anything unclassified (a backend error, a kind a later cpal adds) keeps
    /// the pre-A3b behaviour: reconnect as `device_disconnected`.
    #[tokio::test(start_paused = true)]
    async fn an_unclassified_fault_keeps_the_old_reconnect() {
        let (out, r) = run_with_cpal_error(cpal::ErrorKind::BackendError).await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: Some(RecordingErrorCode::DeviceDisconnected),
                stream_fault: Some(StreamFault::Other),
            }
        );
        assert!(r.sink.has(&Ev::Warning("device_disconnected".into())));
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            handle.clone(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(0), // no free space at all
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx2,
            &mut w_rx2,
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
                counters(),
                Some(u64::MAX / 2),
                FakeClock::default().handle(),
                &mut stop_rx,
                &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx2,
            &mut w_rx2,
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
        r.xruns.store(7, Ordering::Relaxed);
        r.frames.store(96_000, Ordering::Relaxed); // 2 s at 48 kHz
        r.tx.send(SegmentSignal::Writer(WriterEvent::Started))
            .unwrap();
        r.tx.send(SegmentSignal::WriterGone).unwrap();
        let tel = telemetry();
        let opts = test_opts("/tmp/x.wav");
        let out = drive(
            &mut r,
            &opts,
            counters_with(tel.clone()),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
        )
        .await;
        assert_eq!(
            out,
            SegmentOutcome::UnexpectedExit {
                last_error: None,
                stream_fault: None
            }
        );
        let t = tel.lock().expect("telemetry");
        assert_eq!(t.ring_overrun_samples, 4_096);
        assert_eq!(t.native_xruns, 7, "the OS-reported xruns reach telemetry");
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
            r.xruns.store(2, Ordering::Relaxed);
            r.frames.store(48_000, Ordering::Relaxed);
            r.tx.send(SegmentSignal::WriterGone).unwrap();
            drive(
                &mut r,
                &opts,
                counters_with(tel.clone()),
                Some(u64::MAX / 2),
                FakeClock::default().handle(),
                &mut stop_rx,
                &mut w_rx,
            )
            .await;
        }
        let t = tel.lock().expect("telemetry");
        assert_eq!(t.ring_overrun_samples, 2_000);
        assert_eq!(t.native_xruns, 4);
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
            counters(),
            Some(u64::MAX / 2),
            FakeClock::default().handle(),
            &mut stop_rx,
            &mut w_rx,
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
            video_flip: false,
        }
    }

    /// Real-device end-to-end: 2 s on the default input → a playable, header-
    /// consistent WAV whose frame count agrees with the wall clock. SELF-
    /// SKIPPING (same pattern as the real-ffmpeg tests in `media/ffmpeg.rs`):
    /// no input device, or a build failure (CI runners, denied mic access) →
    /// the test reports why and passes vacuously.
    ///
    /// F2-W7 (#231) had this `#[ignore]`d on Windows after it killed the test process
    /// with STATUS_ACCESS_VIOLATION; that was read as a stream-building (or
    /// enumeration) problem, even under `--test-threads=1`. The real cause was
    /// cpal's cached WASAPI enumerator outliving the COM apartment of the
    /// short-lived test thread that created it — libtest gives every test its own
    /// thread, even single-threaded. Fixed at the root in `audio::com_anchor`, and
    /// proven on windows-latest (probe run 35465912824): the sequential cpal tests
    /// died 10/10 on main and 0/10 with the anchor, with this test included.
    /// Real-device behaviour stays rig-verified.
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
