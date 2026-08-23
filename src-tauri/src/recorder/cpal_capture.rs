//! Windows cpal capture session — records from a cpal input stream (WASAPI by
//! default, ASIO for pro interfaces) by piping its routed PCM into the existing
//! ffmpeg sidecar.
//!
//! ## Scope after the 2026-08-01 native rebuild
//!
//! Audio-only sessions on BOTH platforms now run on the native engine
//! ([`crate::recorder::native_capture`]: cpal → ring → direct WAV writer, full
//! split/reconnect/silence support). This module remains for **Windows VIDEO
//! sessions** (camera via dshow + cpal audio piped into one ffmpeg) and as the
//! legacy path behind the `classic_ffmpeg_audio` hatch. It can only be retired
//! when the native engine grows video — so it is HARDENED, not deleted.
//!
//! ## Why this exists
//!
//! SundayRec records via an ffmpeg sidecar, and on Windows ffmpeg's only audio
//! input is **dshow** (DirectShow) — an old API that splits pro multichannel
//! interfaces into stereo pairs and is the source of the "works sometimes"
//! Windows instability. ffmpeg has NO WASAPI input and CANNOT do ASIO. So on
//! Windows we capture the audio ourselves with **cpal** (whose Windows host is
//! WASAPI, plus ASIO when built with `--features asio`) and pipe the raw PCM into
//! ffmpeg's `stdin` (`-f f32le -i pipe:0`) — ffmpeg still does ALL encoding/muxing
//! (and, for a video session, the camera via dshow as input 0). The entire
//! downstream pipeline (codecs, containers, history, preview) is unchanged; only
//! the AUDIO SOURCE moves from dshow to cpal.
//!
//! macOS is untouched: ffmpeg `avfoundation` → Core Audio already exposes the
//! aggregate device as one, so the engine keeps its existing path there.
//!
//! ## Architecture (mirrors [`crate::recorder::two_process`]'s self-contained shape)
//!
//! ```text
//!   cpal stream (WASAPI|ASIO) ─(routed f32 PCM)─► ringbuf ─► writer task ─► ffmpeg stdin
//!   (dedicated thread; the Stream is !Send                  (tokio task)        │
//!    so it is built + held on its own thread,                                   ▼
//!    exactly like audio/vu.rs)                                             encode/mux → file
//! ```
//!
//!   - **Stop = EOF on the pipe.** stdin carries PCM, so we CANNOT also send the
//!     `q` graceful-stop nudge; the writer drains the ring, drops `ChildStdin`
//!     (EOF), and ffmpeg finalises the container cleanly.
//!   - **Channel routing + sample conversion in the callback**: handled by the
//!     shared cpal layer ([`crate::recorder::native_capture::stream`]), which
//!     converts ANY sample format to f32 and copies only the chosen channel
//!     indices, so the pipe carries exactly the recorded layout and ffmpeg needs
//!     no `pan` filter.
//!
//! ## Shared with the native engine (nothing here is a second copy)
//!
//! Host opening, fuzzy device resolution, format dispatch, the frame-aligned
//! ring push and the routed metering all come from
//! [`crate::recorder::native_capture::stream`] — the module that was created to
//! de-duplicate exactly this file. The stderr tail comes from
//! [`crate::recorder::stderr_tail`]. What remains here is only what is genuinely
//! specific to the pipe-into-ffmpeg shape: the writer task and the session
//! supervisor.
//!
//! ## Scope (the rest falls back to the dshow path)
//!
//! Audio-only AND video+cpal-audio are supported. Live L/R **levels ARE** wired
//! (the callback meters the ROUTED signal into a peak-hold that a 33 ms sampler
//! emits as `recording://levels`), and manual-max auto-stop is honoured through
//! the shared `scheduled_stop` watch. **Split, reconnect, preroll and
//! stop-on-silence are NOT** wired here — they assume an ffmpeg-managed input /
//! a `q` stop, so `engine::start` routes a session needing them to dshow (ASIO
//! excepted: dshow can't open it, so the feature is logged as inactive). A cpal
//! stream error ends the session cleanly (finalise what we have) rather than
//! reconnecting — same honest boundary as the two-process path. When cpal can't
//! START, the engine falls back to the dshow capture automatically (see
//! `engine::start`).
//!
//! ## It compiles everywhere now (2026-08-10)
//!
//! This file used to be one `#[cfg(windows)]` block with an off-Windows stub, so
//! **no macOS or Linux build — including CI — ever type-checked a line of it**,
//! and it could not hold a single test. Once the duplicated cpal layer moved out
//! to `native_capture::stream`, nothing left in here was actually
//! Windows-specific: the platform difference lives entirely inside
//! `stream::open_host`, which returns a clear `Err` for the WASAPI/ASIO host ids
//! off-Windows. So the gate is gone. The module compiles and is linted on every
//! platform, and [`run_cpal_session`] fails honestly off-Windows for exactly the
//! same reason the stub used to — the host cannot be opened — instead of because
//! a hand-written stub said so.
//!
//! The engine still only ROUTES here on Windows (`use_cpal` is `cfg!(windows) &&
//! …`), so this changes no behaviour; it changes what the compiler can see.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED — Windows only
//!
//! The live capture can only be exercised on a Windows rig (ASIO host-open needs
//! the extra `feature = "asio"` on top). The decision-shaped parts — device
//! resolution, the history row, the writer's drain/EOF contract, the ring size,
//! the stderr tail — are pure or `AsyncWrite`-generic and ARE unit-tested off
//! Windows (below). What remains unverified is the real WASAPI/ASIO stream and
//! the real ffmpeg pipe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::db::store::RecordingRow;

/// Which cpal host to capture through. WASAPI is the default Windows path
/// (replaces dshow for normal devices); ASIO is the pro-interface path.
///
/// Re-exported from the shared cpal layer rather than redeclared: this file used
/// to carry its own two-variant twin of that enum, so the two capture paths
/// could not be handed the same value without a conversion nobody wrote.
pub use crate::recorder::native_capture::stream::CpalHostKind;

// ─────────────────────────────────────────────────────────────────────────────
//   The platform-independent halves — unit-tested on every platform
// ─────────────────────────────────────────────────────────────────────────────
//
// Split out of the session supervisor so they can be driven directly by a test:
// untestable is how this file got to 817 lines with zero tests.

/// How many f32 samples the writer moves per drain pass. One page-ish block: big
/// enough that the `write_all` syscall cost is amortised, small enough that stop
/// latency stays inside a couple of milliseconds.
const WRITER_BLOCK_SAMPLES: usize = 8192;

/// How long the writer parks when the ring is empty and no stop is pending.
const WRITER_IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(2);

/// Build the history row for a finished cpal recording.
///
/// Pure, and split out for one specific reason: this row used to ship
/// `started_at: 0.0` and a 0 ms duration, which sorted every cpal recording to
/// **1 January 1970** in every start-time-ordered view and made the sidecar
/// duration meaningless. The values are epoch MILLISECONDS carried in REAL
/// columns (`db::store::now_ms`'s convention) — the same shape
/// `engine::finalize_one` writes.
///
/// `id` and `created_at` are left empty/zero on purpose: `insert_recording`
/// stamps both.
pub(crate) fn history_row(
    final_path: &str,
    device_name: &str,
    started_ms: u64,
    duration_ms: f64,
    byte_size: Option<i64>,
) -> RecordingRow {
    RecordingRow {
        id: String::new(),
        file_path: final_path.to_string(),
        device_name: Some(device_name.to_string()),
        started_at: started_ms as f64,
        duration_ms: Some(duration_ms),
        byte_size,
        created_at: 0.0,
        note: None,
    }
}

/// Drain the ring into `sink` as little-endian f32 bytes until stop is requested
/// AND the ring is empty, then drop the sink so ffmpeg sees EOF and finalises.
///
/// Generic over the sink (rather than taking `tokio::process::ChildStdin`) so
/// the drain/EOF contract can be driven by `tokio::io::duplex` in a test: that
/// contract is the whole stop semantics of this path — stdin carries PCM, so we
/// cannot ALSO send ffmpeg the `q` nudge, and a writer that exits with samples
/// still in the ring silently truncates the recording.
///
/// The sink is consumed (not borrowed) because dropping it IS the stop signal.
async fn writer_task<W>(mut cons: ringbuf::HeapCons<f32>, mut sink: W, stop: Arc<AtomicBool>)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use ringbuf::traits::Consumer;
    use tokio::io::AsyncWriteExt;

    let mut samples = vec![0.0f32; WRITER_BLOCK_SAMPLES];
    let mut bytes: Vec<u8> = Vec::with_capacity(WRITER_BLOCK_SAMPLES * 4);
    loop {
        let n = cons.pop_slice(&mut samples);
        if n > 0 {
            bytes.clear();
            for &s in &samples[..n] {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            if sink.write_all(&bytes).await.is_err() {
                break; // ffmpeg closed its input (e.g. it died)
            }
        } else if stop.load(Ordering::Relaxed) {
            break; // stop requested and ring drained
        } else {
            tokio::time::sleep(WRITER_IDLE_POLL).await;
        }
    }
    let _ = sink.flush().await;
    drop(sink); // EOF → ffmpeg flushes + finalises the container
}

pub use imp::run_cpal_session;

mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use std::time::Duration;

    use cpal::traits::StreamTrait;
    use cpal::SampleFormat;
    use sqlx::SqlitePool;
    use sundayrec_core::audio::MeterBanks;
    use sundayrec_core::capture::{build_cpal_pipe_audio_args, build_cpal_pipe_video_args};
    use sundayrec_core::device_match::FfmpegDevice;
    use sundayrec_core::recorder::RecorderState;
    use tauri::{AppHandle, Emitter};

    use super::{history_row, writer_task, CpalHostKind};
    use crate::audio::asio::{build_route_plan, ChannelRoute};
    use crate::db::store::insert_recording;
    use crate::error::{AppError, AppResult};
    use crate::media::ffmpeg::spawn_ffmpeg;
    use crate::recorder::engine::{
        extract_separate_audio, now_ms, RecorderStatePayload, RecordingEvent, RecordingFinished,
        RecordingLevels, RecordingOpts, ERROR_EVENT, FINISHED_EVENT, LEVELS_EVENT, STATE_EVENT,
    };
    use crate::recorder::native_capture::stream::{
        build_input_stream_any, find_device, open_host, ring_capacity, StreamSink,
    };
    use crate::recorder::stderr_tail;

    /// Probe a device's stream config WITHOUT keeping the (`!Send`) handle:
    /// returns the native sample rate, total input-channel count, and sample
    /// format as plain `Copy` values for building the ffmpeg args. Runs on a
    /// blocking thread.
    ///
    /// Deliberately `default_input_config()` rather than the native engine's
    /// range-walk negotiation: this path's ffmpeg args are built from the probe
    /// BEFORE the stream exists, so probe and stream must agree by construction.
    #[allow(deprecated)] // cpal 0.17 deprecates `name()`; still the human device name.
    fn probe_config(
        host_kind: CpalHostKind,
        device_name: &str,
    ) -> AppResult<(u32, u16, SampleFormat)> {
        use cpal::traits::DeviceTrait;
        let host = open_host(host_kind).map_err(AppError::Recording)?;
        let device = find_device(&host, device_name).map_err(AppError::Recording)?;
        let cfg = device
            .default_input_config()
            .map_err(|e| AppError::Recording(format!("querying input config: {e}")))?;
        Ok((cfg.sample_rate(), cfg.channels(), cfg.sample_format()))
    }

    /// The cpal stream thread. Reopens the host (the `!Send` `Stream`/`Device`
    /// never leave this thread, exactly like `audio/vu.rs`), builds + plays the
    /// stream through the SHARED typed builder, then parks until `stop` flips and
    /// drops it. Reports the build result through `built_tx` exactly once.
    #[allow(clippy::too_many_arguments)]
    fn stream_thread(
        host_kind: CpalHostKind,
        device_name: String,
        sample_rate: u32,
        total_channels: u16,
        sample_format: SampleFormat,
        plan: Vec<ChannelRoute>,
        prod: ringbuf::HeapProd<f32>,
        stop: Arc<AtomicBool>,
        overrun: Arc<AtomicU64>,
        meters: Arc<MeterBanks>,
        built_tx: std::sync::mpsc::Sender<Result<(), String>>,
        err_tx: tokio::sync::mpsc::Sender<String>,
    ) {
        let build = (|| -> Result<cpal::Stream, String> {
            let host = open_host(host_kind)?;
            let device = find_device(&host, &device_name)?;
            let config = cpal::StreamConfig {
                channels: total_channels,
                sample_rate, // cpal 0.17: SampleRate is a plain u32
                buffer_size: cpal::BufferSize::Default,
            };
            // On a device error mid-recording (USB pulled, driver reset) cpal calls
            // this — tell the supervisor so it finalises instead of hanging on a
            // pipe that will never get more data.
            let err_fn = move |e: cpal::StreamError| {
                tracing::error!("cpal input stream error: {e}");
                let _ = err_tx.try_send(e.to_string());
            };
            let stream = build_input_stream_any(
                &device,
                &config,
                sample_format,
                total_channels as usize,
                StreamSink::Capture {
                    meters,
                    plan,
                    prod,
                    overrun,
                },
                err_fn,
            )?;
            stream.play().map_err(|e| format!("starting stream: {e}"))?;
            Ok(stream)
        })();

        match build {
            Ok(stream) => {
                let _ = built_tx.send(Ok(()));
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(stream); // stops capture cleanly
            }
            Err(e) => {
                let _ = built_tx.send(Err(e));
            }
        }
    }

    /// Run a cpal capture session (audio-only OR video+cpal-audio) over the given
    /// host. See the module header for architecture and scope.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_cpal_session(
        host_kind: CpalHostKind,
        app: AppHandle,
        pool: Option<SqlitePool>,
        opts: RecordingOpts,
        video: Option<FfmpegDevice>,
        mut stop_rx: tokio::sync::mpsc::Receiver<()>,
        ready_tx: tokio::sync::oneshot::Sender<AppResult<()>>,
        last_state: Arc<Mutex<RecorderState>>,
        scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
    ) {
        let label = host_kind.label();

        // ── Resolve device config + routing (pure once probed) ───────────────
        let device_name = opts.audio_device_name.clone();
        let probe = {
            let name = device_name.clone();
            tokio::task::spawn_blocking(move || probe_config(host_kind, &name)).await
        };
        let (sample_rate, total_channels, sample_format) = match probe {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
            Err(e) => {
                let _ = ready_tx.send(Err(AppError::Recording(format!("probe task failed: {e}"))));
                return;
            }
        };

        let plan = build_route_plan(
            opts.channel_mode,
            opts.input_channel_l,
            opts.input_channel_r,
            total_channels,
        );
        let out_ch = plan.len() as u8;

        // ── Build ffmpeg args (audio-only or video+pipe) ─────────────────────
        let has_video = video.is_some();
        let args: Vec<String> = match &video {
            Some(v) => build_cpal_pipe_video_args(
                &v.name,
                sundayrec_core::capture::RECORDING_FRAMERATE,
                sample_rate,
                out_ch,
                &opts.output_path,
                opts.sample_rate,
                opts.bitrate_kbps,
                // v0.15: the recording codec is a constant (H.264).
                sundayrec_core::capture::RECORDING_VIDEO_CODEC,
                None, // live preview wiring deferred for the cpal path
            ),
            None => build_cpal_pipe_audio_args(
                sample_rate,
                out_ch,
                &opts.output_path,
                opts.sample_rate,
                opts.bitrate_kbps,
            ),
        };

        // ── Spawn ffmpeg, take stdin + drain stderr ──────────────────────────
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        tracing::info!(?arg_refs, host = label, device = %device_name, sample_rate, out_ch, ?sample_format, "recorder: cpal capture starting");
        let mut child = match spawn_ffmpeg(&arg_refs).await {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let _ = child.start_kill();
                let _ = ready_tx.send(Err(AppError::Recording(
                    "ffmpeg gave no stdin pipe for cpal audio".into(),
                )));
                return;
            }
        };
        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_log = child.stderr.take().map(|s| {
            let tail = Arc::clone(&tail);
            tauri::async_runtime::spawn(stderr_tail::drain_stderr(s, "cpal", tail))
        });

        // ── Ring + threads ───────────────────────────────────────────────────
        let stop = Arc::new(AtomicBool::new(false));
        let overrun = Arc::new(AtomicU64::new(0));
        // Per-output-channel meters for the live VU (H1). Shared between the cpal
        // callback (observe, via `StreamSink::Capture`) and the sampler below.
        let meters = Arc::new(MeterBanks::new(out_ch.max(1) as usize));
        let (prod, cons) = {
            use ringbuf::traits::Split;
            ringbuf::HeapRb::<f32>::new(ring_capacity(sample_rate, u16::from(out_ch))).split()
        };

        let (built_tx, built_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<String>(1);
        let st_name = device_name.clone();
        let st_plan = plan.clone();
        let st_stop = Arc::clone(&stop);
        let st_overrun = Arc::clone(&overrun);
        let st_meters = Arc::clone(&meters);
        let stream_handle = std::thread::Builder::new()
            .name("cpal-capture".into())
            .spawn(move || {
                stream_thread(
                    host_kind,
                    st_name,
                    sample_rate,
                    total_channels,
                    sample_format,
                    st_plan,
                    prod,
                    st_stop,
                    st_overrun,
                    st_meters,
                    built_tx,
                    err_tx,
                )
            });
        let stream_handle = match stream_handle {
            Ok(h) => h,
            Err(e) => {
                let _ = child.start_kill();
                let _ = ready_tx.send(Err(AppError::Recording(format!(
                    "could not spawn cpal capture thread: {e}"
                ))));
                return;
            }
        };

        // Wait for the stream to actually build + play before reporting ready, so a
        // bad device fails the Start call (→ engine falls back to dshow) instead of
        // silently producing nothing.
        match tokio::task::spawn_blocking(move || built_rx.recv()).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = stream_handle.join();
                let _ = ready_tx.send(Err(AppError::Recording(e)));
                return;
            }
            _ => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                stop.store(true, Ordering::Relaxed);
                let _ = stream_handle.join();
                let _ = ready_tx.send(Err(AppError::Recording(
                    "cpal capture thread exited before signalling".into(),
                )));
                return;
            }
        }

        // Stream is live → start draining into ffmpeg and report ready.
        let writer = tauri::async_runtime::spawn(writer_task(cons, stdin, Arc::clone(&stop)));

        // Live VU meters (H1): sample the per-channel peak-hold ~30×/s and emit
        // `recording://levels` so the in-recording meters work on the cpal path too.
        let levels_task = {
            let app = app.clone();
            let meters = Arc::clone(&meters);
            let stop = Arc::clone(&stop);
            let stereo = out_ch >= 2;
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(33));
                // Silence is NEG_INFINITY dBFS; clamp to a finite floor the UI renders.
                let floor = |db: f32| {
                    if db.is_finite() {
                        f64::from(db)
                    } else {
                        sundayrec_core::levels::SILENCE_FLOOR_DB
                    }
                };
                while !stop.load(Ordering::Relaxed) {
                    tick.tick().await;
                    let _ = app.emit(
                        LEVELS_EVENT,
                        RecordingLevels {
                            peak_db_left: floor(meters.peak.take_dbfs(0)),
                            peak_db_right: stereo.then(|| floor(meters.peak.take_dbfs(1))),
                        },
                    );
                }
            })
        };

        // Live auto-stop (H3): arm the SHARED absolute deadline so the UI countdown
        // and recording_extend_autostop/cancel work on the cpal path. Mirrors
        // run_session — re-pin the timer whenever the watch changes.
        let start_ms = now_ms();
        let initial_stop = (opts.manual_max_minutes > 0)
            .then(|| start_ms + u64::from(opts.manual_max_minutes) * 60_000);
        scheduled_stop.send_replace(initial_stop);
        let mut stop_watch = scheduled_stop.subscribe();
        let mut auto_deadline: Option<u64> = *stop_watch.borrow();
        let remaining = |d: Option<u64>| -> Duration {
            d.map(|d| Duration::from_millis(d.saturating_sub(now_ms())))
                // `None` → idle ~100 years so the guarded arm never fires.
                .unwrap_or_else(|| Duration::from_secs(60 * 60 * 24 * 365 * 100))
        };
        let auto_sleep = tokio::time::sleep(remaining(auto_deadline));
        tokio::pin!(auto_sleep);

        set_state(&app, &last_state, RecorderState::Recording, auto_deadline);
        let _ = ready_tx.send(Ok(()));

        // ── Run until stop / auto-stop / device or ffmpeg death ──────────────
        loop {
            tokio::select! {
                _ = stop_rx.recv() => {
                    tracing::info!("recorder: cpal — graceful stop requested");
                    break;
                }
                _ = &mut auto_sleep, if auto_deadline.is_some() => {
                    tracing::info!("recorder: cpal — auto-stop deadline reached");
                    break;
                }
                changed = stop_watch.changed() => {
                    if changed.is_ok() {
                        auto_deadline = *stop_watch.borrow();
                        auto_sleep
                            .as_mut()
                            .reset(tokio::time::Instant::now() + remaining(auto_deadline));
                        let _ = app.emit(
                            STATE_EVENT,
                            RecorderStatePayload {
                                state: last_state
                                    .lock()
                                    .map(|g| *g)
                                    .unwrap_or(RecorderState::Recording),
                                reconnect_count: 0,
                                scheduled_stop_ms: auto_deadline,
                            },
                        );
                    }
                }
                msg = err_rx.recv() => {
                    let reason = msg.unwrap_or_else(|| "audio device error".into());
                    tracing::warn!(%reason, "recorder: cpal — device error, finalising");
                    emit_error(&app, "device_disconnected", &reason);
                    break;
                }
                status = child.wait() => {
                    tracing::warn!(?status, "recorder: cpal — ffmpeg exited unexpectedly");
                    let t = stderr_tail::snapshot(&tail);
                    emit_error(&app, "ffmpeg_exited", t.lines().last().unwrap_or("ffmpeg stopped"));
                    break;
                }
            }
        }

        // ── Tear down: stop stream → writer EOF → ffmpeg finalises ───────────
        set_state(&app, &last_state, RecorderState::Stopping, auto_deadline);
        stop.store(true, Ordering::Relaxed);
        levels_task.abort();
        let _ = writer.await; // closes stdin (EOF)
        let _ = child.wait().await; // ffmpeg finalises the container
        let _ = stream_handle.join();
        if let Some(h) = stderr_log {
            h.abort();
        }
        let overrun_total = overrun.load(Ordering::Relaxed);
        if overrun_total > 0 {
            tracing::warn!(
                overrun_total,
                "recorder: cpal — ring overran, samples dropped"
            );
        }

        // The session's real span: captured from the first sample to the finished
        // container. Both history rows below are stamped with it — they used to
        // ship `started_at: 0.0` (sorting the recording to 1970 in every
        // start-time-ordered view) and a 0 ms sidecar duration.
        let ended_ms = now_ms();
        let duration_ms = ended_ms.saturating_sub(start_ms) as f64;

        // ── Separate-audio sidecar (H2): extract the clean audio next to a video
        // recording, exactly like the dshow path (`engine::extract_separate_audio`). ─
        if has_video && opts.keep_separate_audio {
            if let Some(pool) = &pool {
                let audio = FfmpegDevice::new(device_name.clone(), "cpal", None);
                extract_separate_audio(
                    pool,
                    &opts.output_path,
                    start_ms,
                    duration_ms,
                    &opts,
                    &audio,
                )
                .await;
            }
        }

        // ── History + finished event ─────────────────────────────────────────
        write_history(
            &pool,
            &opts.output_path,
            &device_name,
            start_ms,
            duration_ms,
        )
        .await;
        if tokio::fs::metadata(&opts.output_path)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            let _ = app.emit(
                FINISHED_EVENT,
                RecordingFinished {
                    file_path: opts.output_path.clone(),
                    has_video,
                },
            );
        }
        // Clear the shared auto-stop deadline so a finished recording ships no
        // lingering countdown, then announce the terminal state.
        scheduled_stop.send_replace(None);
        set_state(&app, &last_state, RecorderState::Stopped, None);
        tracing::info!(host = label, "recorder: cpal session stopped cleanly");
    }

    /// Emit a `recording://state` payload and update the shared last-state mirror,
    /// stamping the current auto-stop deadline so the UI countdown stays in sync.
    /// The cpal path has no reconnects (always 0).
    fn set_state(
        app: &AppHandle,
        last_state: &Arc<Mutex<RecorderState>>,
        to: RecorderState,
        scheduled_stop_ms: Option<u64>,
    ) {
        if let Ok(mut g) = last_state.lock() {
            *g = to;
        }
        let _ = app.emit(
            STATE_EVENT,
            RecorderStatePayload {
                state: to,
                reconnect_count: 0,
                scheduled_stop_ms,
            },
        );
    }

    /// Emit a classified error to the renderer (mirrors `engine::emit_error`).
    fn emit_error(app: &AppHandle, code: &str, message: &str) {
        let _ = app.emit(
            ERROR_EVENT,
            RecordingEvent {
                code: code.to_string(),
                message: message.to_string(),
            },
        );
    }

    /// Best-effort history row for the finished file (None pool / DB error = no-op).
    /// The row itself is built by the tested [`history_row`].
    async fn write_history(
        pool: &Option<SqlitePool>,
        final_path: &str,
        device_name: &str,
        started_ms: u64,
        duration_ms: f64,
    ) {
        let byte_size = tokio::fs::metadata(final_path)
            .await
            .map(|m| m.len() as i64)
            .ok();
        let Some(pool) = pool else { return };
        let row = history_row(final_path, device_name, started_ms, duration_ms, byte_size);
        if let Err(e) = insert_recording(pool, row).await {
            tracing::error!("recorder: cpal failed to write history row: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    // ── The history row (the 1970 bug site) ──────────────────────────────────

    /// GOLDEN. These five values are the whole row; the ones that were wrong
    /// were `started_at` (0.0 → epoch 1970) and `duration_ms` (0 ms).
    #[test]
    fn history_row_is_stamped_with_the_real_session_span() {
        // 2026-08-10T09:00:00Z, a 92-minute service.
        let started_ms = 1_786_179_600_000_u64;
        let duration_ms = 92.0 * 60_000.0;
        let row = history_row(
            "/Opptak/gudstjeneste.mp3",
            "Allen & Heath Qu-5",
            started_ms,
            duration_ms,
            Some(132_451_200),
        );
        assert_eq!(row.file_path, "/Opptak/gudstjeneste.mp3");
        assert_eq!(row.device_name.as_deref(), Some("Allen & Heath Qu-5"));
        assert_eq!(row.started_at, started_ms as f64);
        assert_eq!(row.duration_ms, Some(5_520_000.0));
        assert_eq!(row.byte_size, Some(132_451_200));
        // `insert_recording` stamps these — the row must not pre-empt it.
        assert_eq!(row.id, "");
        assert_eq!(row.created_at, 0.0);
    }

    #[test]
    fn history_row_never_reports_a_1970_start() {
        // The regression guard for the original bug: `started_at` must carry the
        // epoch-ms the caller measured, NOT 0.
        let row = history_row("/x.mp3", "Mic", 1_786_179_600_000, 1.0, None);
        assert!(
            row.started_at > 1_000_000_000_000.0,
            "started_at {} would sort the recording to 1970",
            row.started_at
        );
    }

    #[test]
    fn history_row_tolerates_an_unmeasurable_file() {
        // `metadata()` can fail (the file was moved between finalise and stat);
        // the row must still be written, just without a size.
        let row = history_row("/x.mp3", "Mic", 42, 0.0, None);
        assert_eq!(row.byte_size, None);
        assert_eq!(row.duration_ms, Some(0.0));
        assert_eq!(row.started_at, 42.0);
    }

    // ── The writer's drain / EOF contract ────────────────────────────────────

    fn ring(cap: usize) -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
        ringbuf::HeapRb::<f32>::new(cap).split()
    }

    /// Every sample that reached the ring must reach the sink, little-endian.
    #[tokio::test]
    async fn writer_pipes_every_sample_as_little_endian_f32() {
        let (mut prod, cons) = ring(1024);
        let samples: Vec<f32> = vec![0.0, 1.0, -1.0, 0.5, -0.25];
        assert_eq!(prod.push_slice(&samples), samples.len());
        let stop = Arc::new(AtomicBool::new(true)); // stop already requested: drain + exit

        let (sink, mut reader) = tokio::io::duplex(64 * 1024);
        let w = tokio::spawn(writer_task(cons, sink, stop));

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.expect("read");
        w.await.expect("writer joined");

        let expect: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(got, expect);
    }

    /// The stop contract: the writer must DRAIN the ring before exiting. A
    /// writer that exited on the stop flag with samples still queued would
    /// silently truncate the tail of every recording.
    #[tokio::test]
    async fn writer_drains_the_ring_before_honouring_stop() {
        let (mut prod, cons) = ring(8192);
        // More than one drain block, so the loop must go round several times.
        let samples: Vec<f32> = (0..5_000).map(|i| i as f32).collect();
        assert_eq!(prod.push_slice(&samples), samples.len());
        let stop = Arc::new(AtomicBool::new(true));

        let (sink, mut reader) = tokio::io::duplex(1024 * 1024);
        let w = tokio::spawn(writer_task(cons, sink, stop));
        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.expect("read");
        w.await.expect("writer joined");

        assert_eq!(
            got.len(),
            samples.len() * 4,
            "the writer dropped {} samples on stop",
            samples.len() - got.len() / 4
        );
    }

    /// Dropping the sink is the ONLY stop signal ffmpeg gets on this path (stdin
    /// carries PCM, so there is no `q` nudge). If the writer returned without
    /// dropping it, ffmpeg would wait forever and never finalise the container.
    #[tokio::test]
    async fn writer_closes_the_sink_so_ffmpeg_sees_eof() {
        let (_prod, cons) = ring(64);
        let stop = Arc::new(AtomicBool::new(true));
        let (sink, mut reader) = tokio::io::duplex(1024);
        let w = tokio::spawn(writer_task(cons, sink, stop));

        let mut got = Vec::new();
        // `read_to_end` only returns once the write half is dropped — this
        // assertion IS the EOF proof.
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut got))
            .await
            .expect("writer never closed the pipe — ffmpeg would hang")
            .expect("read");
        assert_eq!(n, 0);
        w.await.expect("writer joined");
    }

    /// Samples that arrive AFTER the writer started must still be picked up: the
    /// writer polls, it does not snapshot the ring once.
    #[tokio::test]
    async fn writer_keeps_draining_until_stop_is_raised() {
        let (mut prod, cons) = ring(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let (sink, mut reader) = tokio::io::duplex(64 * 1024);
        let w = tokio::spawn(writer_task(cons, sink, Arc::clone(&stop)));

        // Feed after the task is already looping on an empty ring.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(prod.push_slice(&[1.0f32, 2.0, 3.0]), 3);
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.store(true, Ordering::Relaxed);

        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut got))
            .await
            .expect("writer never finished")
            .expect("read");
        w.await.expect("writer joined");
        assert_eq!(got.len(), 3 * 4, "late samples were lost");
    }

    /// ffmpeg dying mid-recording closes the pipe. The writer must NOTICE and
    /// return — spinning on a broken pipe would leak the task for the life of
    /// the app and (on the tear-down path) hang the `writer.await`.
    #[tokio::test]
    async fn writer_gives_up_when_the_sink_dies() {
        let (mut prod, cons) = ring(64 * 1024);
        let samples = vec![0.25f32; 32_768];
        prod.push_slice(&samples);
        let stop = Arc::new(AtomicBool::new(false)); // NEVER stopped

        let (sink, reader) = tokio::io::duplex(16);
        drop(reader); // ffmpeg died
        let w = tokio::spawn(writer_task(cons, sink, stop));

        tokio::time::timeout(Duration::from_secs(5), w)
            .await
            .expect("writer spun forever on a broken pipe")
            .expect("writer joined");
    }

    #[tokio::test]
    async fn writer_on_an_empty_ring_writes_nothing_and_still_closes() {
        let (_prod, cons) = ring(64);
        let stop = Arc::new(AtomicBool::new(true));
        let (sink, mut reader) = tokio::io::duplex(1024);
        let w = tokio::spawn(writer_task(cons, sink, stop));
        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.expect("read");
        w.await.expect("writer joined");
        assert!(got.is_empty());
    }

    // ── Ring sizing on this path ─────────────────────────────────────────────

    /// This path used to size its ring with a hand-written `96_000` constant
    /// ("~500 ms of stereo at 96 kHz") while the native engine used a computed
    /// one — two cushions, drifting apart. It now goes through the same
    /// `stream::ring_capacity`, so the 2026-08-10 raise applies here too.
    #[test]
    fn the_pipe_path_uses_the_shared_ring_size() {
        use crate::recorder::native_capture::stream::ring_capacity;
        // What this file used to allocate, at the format it named.
        let old_flat = 96_000;
        assert!(
            ring_capacity(96_000, 2) > old_flat,
            "the pipe path must no longer get the old half-second ring"
        );
        assert_eq!(ring_capacity(48_000, 2), 480_000);
    }
}
