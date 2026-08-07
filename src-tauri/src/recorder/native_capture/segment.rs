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
    build_input_stream_any, find_device, negotiate, open_host, CpalHostKind, StreamSink,
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

    // ≥1 s of routed audio (96 kHz stereo ⇒ 192 000 f32, <1 MB) so a transient
    // writer stall never drops samples; overrun drops whole frames + counts.
    let ring_capacity = (spec.sample_rate as usize * out_ch as usize).max(96_000);
    let (prod, cons) = ringbuf::HeapRb::<f32>::new(ring_capacity).split();

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

/// Run one native segment to completion. Mirrors `engine::run_segment`'s arms;
/// see the module header for the signal mapping.
///
/// `deliverable_bytes` is the sum of the current deliverable's PREVIOUS
/// fragments (`_rN` reconnect pieces) — the RIFF-cap guard forces a split
/// before the `-c copy`-concatenated deliverable could cross the 4 GiB WAV
/// ceiling.
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
    // Silence watcher + its (host-owned) timers — identical to the ffmpeg path.
    let mut silence = SilenceWatcher::new(opts.stop_on_silence);
    let silence_stop_after =
        Duration::from_secs(u64::from(opts.silence_timeout_minutes.max(1)) * 60);
    let silence_warn_after = Duration::from_millis(RecorderTimeouts::SILENCE_WARN_MS);
    let mut silence_stop: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut silence_warn: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    // Byte-progress watchdog over the writer's own counter.
    let mut wd = WatchdogState::new(RecorderTimeouts::STUCK_PROGRESS_MS, now_ms());
    let mut wd_tick = tokio::time::interval(Duration::from_millis(RecorderTimeouts::STUCK_POLL_MS));
    wd_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Low-disk guard — identical thresholds to the ffmpeg path (audio-only).
    let disk_folder = std::path::Path::new(&opts.output_path)
        .parent()
        .map(|p| p.to_path_buf());
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
        deadline.map(|d| Duration::from_millis(d.saturating_sub(now_ms())))
    };
    let mut auto_deadline: Option<u64> = *stop_watch.borrow();
    let split_sleep = sleep_opt(split_deadline);
    tokio::pin!(split_sleep);
    let auto_sleep = sleep_opt(auto_stop_remaining(auto_deadline));
    tokio::pin!(auto_sleep);

    // Startup watchdog: the writer must report Started (first block on disk)
    // within the same window ffmpeg gets for its first `size=` line.
    let mut started_seen = false;
    let startup_sleep =
        tokio::time::sleep(Duration::from_millis(RecorderTimeouts::STARTUP_TIMEOUT_MS));
    tokio::pin!(startup_sleep);

    // UI cadences: 1 Hz byte counter, 33 ms level meters (when enabled).
    let mut progress_tick = tokio::time::interval(Duration::from_secs(1));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut levels_tick = tokio::time::interval(LEVELS_EVERY);
    levels_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let stereo = seg.spec.channels >= 2;
    let floor = |db: f32| {
        if db.is_finite() {
            f64::from(db)
        } else {
            SILENCE_FLOOR_DB
        }
    };

    let outcome = loop {
        tokio::select! {
            // Writer events: Started / silence / typed errors.
            ev = seg.events.recv() => {
                match ev {
                    Some(WriterEvent::Started) => {
                        started_seen = true;
                        let _ = app.emit(STARTED_EVENT, ());
                    }
                    Some(WriterEvent::Silence(ev)) => {
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
                    Some(WriterEvent::Error { kind, message }) => {
                        // The writer has already stopped; classify directly
                        // (no stderr sniffing) and hand the supervisor the
                        // same fatal/transient split as the ffmpeg path.
                        let code = match kind {
                            WriterErrorKind::DiskFull => RecordingErrorCode::DiskFull,
                            WriterErrorKind::Io => RecordingErrorCode::DeviceError,
                        };
                        // Derive the wire code from the shared table (never a
                        // literal) so it can't drift from the classification the
                        // recovery policy just made — the ffmpeg twin does the same.
                        let code_str = error_code_str(code);
                        if sundayrec_core::recorder::is_fatal_reconnect_error(code) {
                            emit_error(app, code_str, &message);
                        } else {
                            emit_warning(app, code_str, &message);
                        }
                        stop_native_bounded(&mut seg).await;
                        break SegmentOutcome::UnexpectedExit { last_error: Some(code) };
                    }
                    None => {
                        // Writer gone without an error event — treat as an
                        // unexpected exit; the recovery policy decides.
                        stop_native_bounded(&mut seg).await;
                        break SegmentOutcome::UnexpectedExit { last_error: None };
                    }
                }
            }
            // Device error mid-session (USB pulled, format lost): finalize the
            // fragment so the _rN concat has a valid piece, then reconnect.
            msg = seg.stream_err_rx.recv() => {
                let reason = msg.unwrap_or_else(|| "audio device error".into());
                emit_warning(app, "device_disconnected", &reason);
                stop_native_bounded(&mut seg).await;
                break SegmentOutcome::UnexpectedExit {
                    last_error: Some(RecordingErrorCode::DeviceDisconnected),
                };
            }
            // Graceful stop request.
            _ = stop_rx.recv() => {
                stop_native_bounded(&mut seg).await;
                break SegmentOutcome::GracefulStop;
            }
            // Startup watchdog: no first block on disk in time.
            _ = &mut startup_sleep, if !started_seen => {
                emit_error(
                    app,
                    "start_timeout",
                    "Opptaket startet ikke i tide — sjekk at mikrofonen er tilkoblet og at appen har tilgang (Systeminnstillinger → Personvern).",
                );
                stop_native_bounded(&mut seg).await;
                break SegmentOutcome::UnexpectedExit {
                    last_error: Some(RecordingErrorCode::DeviceNotFound),
                };
            }
            // Byte-progress watchdog.
            _ = wd_tick.tick() => {
                if wd.observe(seg.bytes.load(Ordering::Relaxed), now_ms()) == WatchdogVerdict::Stuck {
                    emit_error(
                        app,
                        "stuck_recording",
                        &format!(
                            "Ingen framgang på {} s — kobler til på nytt",
                            RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                        ),
                    );
                    stop_native_bounded(&mut seg).await;
                    break SegmentOutcome::UnexpectedExit { last_error: None };
                }
            }
            // Low-disk guard + RIFF-cap guard (same 30 s cadence; the cap has
            // 500 MiB of headroom below the real 4 GiB ceiling, dwarfing the
            // ~12 MiB a 96 kHz stereo segment grows between ticks).
            _ = disk_tick.tick() => {
                let seg_bytes = seg.bytes.load(Ordering::Relaxed);
                if sundayrec_core::wav::should_force_split(
                    deliverable_bytes.saturating_add(seg_bytes),
                ) {
                    tracing::warn!(
                        deliverable_bytes,
                        seg_bytes,
                        "native capture: deliverable approaching the 4 GiB WAV ceiling — forcing a split"
                    );
                    stop_native_bounded(&mut seg).await;
                    break SegmentOutcome::Split;
                }
                if let Some(folder) = &disk_folder {
                    if let Ok(free) = fs4::available_space(folder) {
                        let reserve = finalize_reserve_bytes(false, seg_bytes);
                        if low_disk_should_stop(free, disk_headroom + reserve) {
                            emit_error(
                                app,
                                "disk_full",
                                "Lite ledig diskplass — stopper opptaket trygt før disken blir full.",
                            );
                            stop_native_bounded(&mut seg).await;
                            break SegmentOutcome::DiskStop;
                        }
                    }
                }
            }
            // Split timer.
            _ = &mut split_sleep, if split_deadline.is_some() => {
                stop_native_bounded(&mut seg).await;
                break SegmentOutcome::Split;
            }
            // Auto-stop deadline reached.
            _ = &mut auto_sleep, if auto_deadline.is_some() => {
                stop_native_bounded(&mut seg).await;
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
                    let _ = app.emit(
                        STATE_EVENT,
                        RecorderStatePayload {
                            state: *lock_recover(last_state),
                            reconnect_count: session.reconnect_count(),
                            scheduled_stop_ms: auto_deadline,
                        },
                    );
                }
            }
            // Stop-on-silence fired.
            () = wait_opt(&mut silence_stop), if silence_stop.is_some() => {
                silence.on_stop_fired();
                stop_native_bounded(&mut seg).await;
                break SegmentOutcome::SilenceStop;
            }
            // Silence warning fired.
            () = wait_opt(&mut silence_warn), if silence_warn.is_some() => {
                silence.on_warn_fired();
                silence_warn = None;
                let _ = app.emit(
                    SILENCE_EVENT,
                    crate::recorder::engine::RecordingEvent {
                        code: "silence_detected".into(),
                        message: "Stillhet oppdaget i lydsignalet".into(),
                    },
                );
            }
            // 1 Hz UI byte counter (also mirrors into the supervisor's atomic).
            _ = progress_tick.tick() => {
                let b = seg.bytes.load(Ordering::Relaxed);
                segment_bytes.store(b, Ordering::Relaxed);
                if started_seen {
                    let _ = app.emit(PROGRESS_EVENT, RecordingProgress { bytes_written: b });
                }
            }
            // 33 ms level meters over the routed banks.
            _ = levels_tick.tick(), if opts.live_levels => {
                let _ = app.emit(
                    LEVELS_EVENT,
                    RecordingLevels {
                        peak_db_left: floor(seg.meters.peak.take_dbfs(0)),
                        peak_db_right: stereo.then(|| floor(seg.meters.peak.take_dbfs(1))),
                    },
                );
            }
        }
    };

    // Fold this segment's health into the session telemetry: overruns are the
    // native drop signal (verdict folds them into the xrun class), and the
    // exact frame count is the capture-side cross-check against ffprobe.
    let overrun = seg.overrun.load(Ordering::Relaxed);
    if overrun > 0 {
        tracing::warn!(overrun, "native capture: ring overran — samples dropped");
    }
    {
        let mut t = lock_recover(&telemetry);
        t.ring_overrun_samples = t.ring_overrun_samples.saturating_add(overrun);
        t.native_frames_sec +=
            seg.frames.load(Ordering::Relaxed) as f64 / f64::from(seg.spec.sample_rate.max(1));
        // E6.3 symmetry with the ffmpeg twin: close this capture process's
        // drop/dup window. The native engine parses no ffmpeg stderr, so the
        // window is always empty and this is a no-op — but a session that mixes
        // the two backends (a native start that fell back to ffmpeg) must not
        // depend on which arm happened to run.
        t.seal_process();
    }
    segment_bytes.store(seg.bytes.load(Ordering::Relaxed), Ordering::Relaxed);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::settings::ChannelMode;

    fn test_opts(out: &str) -> RecordingOpts {
        RecordingOpts {
            audio_device_name: String::new(), // host default input
            video_device_name: None,
            output_path: out.to_string(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            framerate: 30,
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
            video_resolution: String::new(),
            video_codec: String::new(),
            video_encoder: String::new(),
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
