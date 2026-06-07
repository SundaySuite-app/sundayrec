//! Windows ASIO capture session — the I/O shell that records from a cpal ASIO
//! input stream by piping its routed PCM into the existing ffmpeg sidecar.
//!
//! ## Why this exists
//!
//! SundayRec records via an ffmpeg sidecar, and ffmpeg CANNOT open an ASIO
//! device. So when the user picks an ASIO interface we capture the audio
//! ourselves with cpal (the only ASIO route in Rust), and pipe the raw PCM into
//! ffmpeg's `stdin` (`-f f32le -i pipe:0`) — ffmpeg still does ALL the
//! encoding/muxing (and, for a video session, the camera via dshow as input 0).
//! This keeps the entire downstream pipeline (codecs, containers, history,
//! preview) unchanged; only the AUDIO SOURCE differs.
//!
//! ## Architecture (mirrors [`crate::recorder::two_process`]'s self-contained shape)
//!
//! ```text
//!   cpal ASIO stream  ──(routed f32 PCM)──►  ringbuf  ──►  writer task  ──►  ffmpeg stdin
//!   (dedicated thread; the Stream is !Send                 (tokio task)        │
//!    so it is built + held on its own thread,                                  ▼
//!    exactly like audio/vu.rs)                                            encode/mux → file
//! ```
//!
//!   - **Stop = EOF on the pipe.** stdin carries PCM, so we CANNOT also send the
//!     `q` graceful-stop nudge the rest of the recorder uses. Instead the writer
//!     drains the ring, drops `ChildStdin` (EOF), and ffmpeg finalises the
//!     container cleanly.
//!   - **Channel routing in the callback** ([`crate::audio::asio::build_route_plan`]):
//!     the callback copies only the chosen channel indices, so the pipe carries
//!     exactly the recorded layout and ffmpeg needs no `pan` filter.
//!
//! ## v1 scope (the rest falls back to the dshow/WASAPI path)
//!
//! Audio-only AND video+ASIO are supported. **Split, reconnect, preroll, live
//! levels and stop-on-silence are NOT** wired on the ASIO path (they assume an
//! ffmpeg-managed input / a `q` stop). Manual-max auto-stop IS honoured (it is a
//! host timer). A cpal stream error ends the session cleanly (finalise what we
//! have) rather than reconnecting — same honest boundary as the two-process path.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED — Windows + ASIO only
//!
//! All of this compiles only under `#[cfg(all(target_os = "windows", feature =
//! "asio"))]` and can only be exercised on a Windows rig with an ASIO driver
//! (ASIO4ALL suffices). The pure parts (arg building, channel routing) live in
//! [`sundayrec_core::capture`] / [`crate::audio::asio`] and ARE unit-tested
//! off-Windows. Off-Windows this module is a stub that signals a clear error
//! (it is never reached — `is_asio_device` is `false` there).

#[cfg(all(target_os = "windows", feature = "asio"))]
pub use imp::run_asio_session;

#[cfg(not(all(target_os = "windows", feature = "asio")))]
use crate::error::AppResult;
#[cfg(not(all(target_os = "windows", feature = "asio")))]
use crate::recorder::engine::RecordingOpts;
#[cfg(not(all(target_os = "windows", feature = "asio")))]
use tauri::AppHandle;

/// Off-Windows / feature-off stub. Never called in practice (the recorder only
/// branches here when [`crate::audio::asio::is_asio_device`] is true, which is
/// always `false` here), but it keeps `start()` reading identically on every
/// platform. Signals a clear error through the ready channel.
#[cfg(not(all(target_os = "windows", feature = "asio")))]
#[allow(clippy::too_many_arguments)]
pub async fn run_asio_session(
    _app: AppHandle,
    _pool: Option<sqlx::SqlitePool>,
    _opts: RecordingOpts,
    _video: Option<sundayrec_core::device_match::FfmpegDevice>,
    _stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready_tx: tokio::sync::oneshot::Sender<AppResult<()>>,
    _last_state: std::sync::Arc<std::sync::Mutex<sundayrec_core::recorder::RecorderState>>,
) {
    let _ = ready_tx.send(Err(crate::error::AppError::Recording(
        "ASIO capture is only available on Windows builds compiled with --features asio".into(),
    )));
}

#[cfg(all(target_os = "windows", feature = "asio"))]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use sqlx::SqlitePool;
    use sundayrec_core::capture::{build_asio_audio_args, build_asio_video_args};
    use sundayrec_core::device_match::FfmpegDevice;
    use sundayrec_core::recorder::RecorderState;
    use tauri::{AppHandle, Emitter};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use crate::audio::asio::{build_route_plan, route_frame, ChannelRoute};
    use crate::db::store::{insert_recording, RecordingRow};
    use crate::error::{AppError, AppResult};
    use crate::media::ffmpeg::spawn_ffmpeg;
    use crate::recorder::engine::{
        RecorderStatePayload, RecordingEvent, RecordingFinished, RecordingOpts, ERROR_EVENT,
        FINISHED_EVENT, STATE_EVENT,
    };

    /// Ring capacity in f32 samples: ~500 ms of stereo audio at 96 kHz
    /// (96000 × 2 × 0.5 ≈ 96k). A generous cushion so a transient writer/pipe
    /// stall never drops samples; on overrun the callback drops the newest block
    /// (and bumps a counter) rather than ever blocking the real-time thread.
    const RING_CAPACITY: usize = 96_000;

    /// Resolve the OUTPUT channel count for a route plan (1 for mono modes, 2 for
    /// stereo) — the number of samples the plan emits per input frame.
    fn out_channels(plan: &[ChannelRoute]) -> u8 {
        plan.len() as u8
    }

    /// Probe an ASIO device's stream config WITHOUT keeping the (`!Send`) handle:
    /// returns the native sample rate, total input-channel count, and sample
    /// format as plain `Copy` values the async side can use to build the ffmpeg
    /// args. Runs on a blocking thread (cpal host calls block).
    fn probe_asio_config(device_name: &str) -> AppResult<(u32, u16, SampleFormat)> {
        let host = cpal::host_from_id(cpal::HostId::Asio)
            .map_err(|e| AppError::Recording(format!("could not open ASIO host: {e}")))?;
        let device = host
            .devices()
            .map_err(|e| AppError::Recording(format!("listing ASIO devices: {e}")))?
            .find(|d| d.name().ok().as_deref() == Some(device_name))
            .ok_or_else(|| AppError::Recording(format!("ASIO device not found: {device_name}")))?;
        let cfg = device
            .default_input_config()
            .map_err(|e| AppError::Recording(format!("querying ASIO input config: {e}")))?;
        Ok((cfg.sample_rate().0, cfg.channels(), cfg.sample_format()))
    }

    /// The cpal ASIO stream thread. Opens the device, builds an input stream whose
    /// callback routes the chosen channels into `prod`, plays it, then parks until
    /// `stop` flips — at which point it drops the stream (stopping capture). The
    /// `!Send` `Stream` never leaves this thread (the same discipline as
    /// `audio/vu.rs`). Reports the build result through `built_tx` exactly once.
    #[allow(clippy::too_many_arguments)]
    fn stream_thread(
        device_name: String,
        sample_rate: u32,
        total_channels: u16,
        sample_format: SampleFormat,
        plan: Vec<ChannelRoute>,
        mut prod: ringbuf::HeapProd<f32>,
        stop: Arc<AtomicBool>,
        dropped: Arc<AtomicU64>,
        built_tx: std::sync::mpsc::Sender<Result<(), String>>,
        err_tx: tokio::sync::mpsc::Sender<String>,
    ) {
        use ringbuf::traits::Producer;

        let build = (|| -> Result<cpal::Stream, String> {
            let host = cpal::host_from_id(cpal::HostId::Asio)
                .map_err(|e| format!("could not open ASIO host: {e}"))?;
            let device = host
                .devices()
                .map_err(|e| format!("listing ASIO devices: {e}"))?
                .find(|d| d.name().ok().as_deref() == Some(device_name.as_str()))
                .ok_or_else(|| format!("ASIO device not found: {device_name}"))?;

            let config = cpal::StreamConfig {
                channels: total_channels,
                sample_rate: cpal::SampleRate(sample_rate),
                buffer_size: cpal::BufferSize::Default,
            };
            let total = total_channels as usize;
            let dropped_cb = Arc::clone(&dropped);
            // On a device error mid-recording (USB pulled, driver reset) cpal calls
            // this — tell the supervisor so it finalises what we have instead of
            // hanging on a pipe that will never get more data.
            let err_fn = move |e: cpal::StreamError| {
                tracing::error!("ASIO input stream error: {e}");
                let _ = err_tx.try_send(e.to_string());
            };

            // Route an interleaved block into the ring. Real-time safe: a reused
            // scratch buffer (allocated once here, never in the callback) and a
            // lock-free push. On overrun we drop the newest block + count it.
            let mut scratch: Vec<f32> = Vec::with_capacity(4096);
            let route_block = move |samples: &[f32],
                                    prod: &mut ringbuf::HeapProd<f32>,
                                    scratch: &mut Vec<f32>| {
                if total == 0 {
                    return;
                }
                scratch.clear();
                for frame in samples.chunks_exact(total) {
                    route_frame(&plan, frame, scratch);
                }
                let pushed = prod.push_slice(scratch);
                if pushed < scratch.len() {
                    dropped_cb.fetch_add((scratch.len() - pushed) as u64, Ordering::Relaxed);
                }
            };

            let stream = match sample_format {
                SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        route_block(data, &mut prod, &mut scratch)
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I32 => {
                    let mut conv: Vec<f32> = Vec::with_capacity(4096);
                    device.build_input_stream(
                        &config,
                        move |data: &[i32], _: &cpal::InputCallbackInfo| {
                            conv.clear();
                            conv.extend(data.iter().map(|&s| s as f32 / i32::MAX as f32));
                            route_block(&conv, &mut prod, &mut scratch)
                        },
                        err_fn,
                        None,
                    )
                }
                SampleFormat::I16 => {
                    let mut conv: Vec<f32> = Vec::with_capacity(4096);
                    device.build_input_stream(
                        &config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            conv.clear();
                            conv.extend(data.iter().map(|&s| s as f32 / i16::MAX as f32));
                            route_block(&conv, &mut prod, &mut scratch)
                        },
                        err_fn,
                        None,
                    )
                }
                other => return Err(format!("unsupported ASIO sample format: {other:?}")),
            }
            .map_err(|e| format!("building ASIO input stream: {e}"))?;

            stream.play().map_err(|e| format!("starting ASIO stream: {e}"))?;
            Ok(stream)
        })();

        match build {
            Ok(stream) => {
                let _ = built_tx.send(Ok(()));
                // Hold the stream alive until stop. Parking (not busy-looping) keeps
                // the thread idle; the audio callback runs on cpal's own thread.
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                drop(stream); // stops capture cleanly
            }
            Err(e) => {
                let _ = built_tx.send(Err(e));
            }
        }
    }

    /// Drain the ring into ffmpeg's stdin as little-endian f32 bytes until stop is
    /// requested AND the ring is empty, then drop stdin (EOF) so ffmpeg finalises.
    async fn writer_task(
        mut cons: ringbuf::HeapCons<f32>,
        mut stdin: tokio::process::ChildStdin,
        stop: Arc<AtomicBool>,
    ) {
        use ringbuf::traits::Consumer;
        let mut samples = vec![0.0f32; 8192];
        let mut bytes: Vec<u8> = Vec::with_capacity(8192 * 4);
        loop {
            let n = cons.pop_slice(&mut samples);
            if n > 0 {
                bytes.clear();
                for &s in &samples[..n] {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                if stdin.write_all(&bytes).await.is_err() {
                    // ffmpeg closed its input (e.g. it died) — nothing more to do.
                    break;
                }
            } else if stop.load(Ordering::Relaxed) {
                break; // stop requested and ring drained
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        }
        let _ = stdin.flush().await;
        drop(stdin); // EOF → ffmpeg flushes + finalises the container
    }

    /// Run an ASIO capture session (audio-only OR video+ASIO-audio). See the
    /// module header for the architecture and scope.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_asio_session(
        app: AppHandle,
        pool: Option<SqlitePool>,
        opts: RecordingOpts,
        video: Option<FfmpegDevice>,
        mut stop_rx: tokio::sync::mpsc::Receiver<()>,
        ready_tx: tokio::sync::oneshot::Sender<AppResult<()>>,
        last_state: Arc<Mutex<RecorderState>>,
    ) {
        // ── Resolve device config + routing (pure once probed) ───────────────
        let device_name = opts.audio_device_name.clone();
        let probe = {
            let name = device_name.clone();
            tokio::task::spawn_blocking(move || probe_asio_config(&name)).await
        };
        let (sample_rate, total_channels, sample_format) = match probe {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
            Err(e) => {
                let _ = ready_tx.send(Err(AppError::Recording(format!(
                    "ASIO probe task failed: {e}"
                ))));
                return;
            }
        };

        let plan = build_route_plan(
            opts.channel_mode,
            opts.input_channel_l,
            opts.input_channel_r,
            total_channels,
        );
        let out_ch = out_channels(&plan);

        // ── Build ffmpeg args (audio-only or video+pipe) ─────────────────────
        let has_video = video.is_some();
        let args: Vec<String> = match &video {
            Some(v) => build_asio_video_args(
                &v.name,
                opts.framerate.max(1),
                sample_rate,
                out_ch,
                &opts.output_path,
                opts.sample_rate,
                opts.bitrate_kbps,
                video_codec_of(&opts),
                None, // live preview wiring deferred for the ASIO path (v1)
            ),
            None => build_asio_audio_args(
                sample_rate,
                out_ch,
                &opts.output_path,
                opts.sample_rate,
                opts.bitrate_kbps,
            ),
        };

        // ── Spawn ffmpeg, take stdin + drain stderr ──────────────────────────
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        tracing::info!(?arg_refs, device = %device_name, sample_rate, out_ch, "recorder: ASIO capture starting");
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
                    "ffmpeg gave no stdin pipe for ASIO audio".into(),
                )));
                return;
            }
        };
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let stderr_log = child.stderr.take().map(|s| {
            let tail = Arc::clone(&stderr_tail);
            tauri::async_runtime::spawn(drain_stderr(s, tail))
        });

        // ── Ring + threads ───────────────────────────────────────────────────
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let rb = ringbuf::HeapRb::<f32>::new(RING_CAPACITY);
        let (prod, cons) = {
            use ringbuf::traits::Split;
            rb.split()
        };

        let (built_tx, built_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        // A device error mid-recording (USB pulled, driver reset) arrives here from
        // cpal's error callback so the supervisor can finalise gracefully.
        let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<String>(1);
        let st_name = device_name.clone();
        let st_plan = plan.clone();
        let st_stop = Arc::clone(&stop);
        let st_dropped = Arc::clone(&dropped);
        let stream_handle = std::thread::Builder::new()
            .name("asio-capture".into())
            .spawn(move || {
                stream_thread(
                    st_name,
                    sample_rate,
                    total_channels,
                    sample_format,
                    st_plan,
                    prod,
                    st_stop,
                    st_dropped,
                    built_tx,
                    err_tx,
                )
            });
        let stream_handle = match stream_handle {
            Ok(h) => h,
            Err(e) => {
                let _ = child.start_kill();
                let _ = ready_tx.send(Err(AppError::Recording(format!(
                    "could not spawn ASIO capture thread: {e}"
                ))));
                return;
            }
        };

        // Wait for the stream to actually build + play before reporting ready, so a
        // bad device fails the Start call instead of silently producing nothing.
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
                    "ASIO capture thread exited before signalling".into(),
                )));
                return;
            }
        }

        // Stream is live → start draining into ffmpeg and report ready.
        let writer = tauri::async_runtime::spawn(writer_task(cons, stdin, Arc::clone(&stop)));
        set_state(&app, &last_state, RecorderState::Recording);
        let _ = ready_tx.send(Ok(()));

        // ── Run until stop / auto-stop / ffmpeg or stream death ──────────────
        let auto_stop = opts.manual_max_minutes;
        let auto_stop_fut = async {
            if auto_stop == 0 {
                std::future::pending::<()>().await
            } else {
                tokio::time::sleep(std::time::Duration::from_secs(u64::from(auto_stop) * 60)).await
            }
        };
        tokio::pin!(auto_stop_fut);

        tokio::select! {
            _ = stop_rx.recv() => tracing::info!("recorder: ASIO — graceful stop requested"),
            _ = &mut auto_stop_fut => tracing::info!("recorder: ASIO — manual-max auto-stop"),
            msg = err_rx.recv() => {
                // The ASIO device errored mid-recording (USB pulled / driver reset).
                // Finalise what we captured and tell the UI plainly.
                let reason = msg.unwrap_or_else(|| "ASIO device error".into());
                tracing::warn!(%reason, "recorder: ASIO — device error, finalising");
                emit_error(&app, "device_disconnected", &reason);
            }
            status = child.wait() => {
                // ffmpeg died on its own — surface a classified error.
                tracing::warn!(?status, "recorder: ASIO — ffmpeg exited unexpectedly");
                let tail = stderr_tail.lock().map(|g| g.clone()).unwrap_or_default();
                emit_error(&app, "ffmpeg_exited", tail.lines().last().unwrap_or("ffmpeg stopped"));
            }
        }

        // ── Tear down: stop stream → writer EOF → ffmpeg finalises ───────────
        set_state(&app, &last_state, RecorderState::Stopping);
        stop.store(true, Ordering::Relaxed); // stream thread drops the Stream; writer drains then EOFs
        let _ = writer.await; // closes stdin (EOF)
        let _ = child.wait().await; // ffmpeg finalises the container
        let _ = stream_handle.join();
        if let Some(h) = stderr_log {
            h.abort();
        }
        let dropped_total = dropped.load(Ordering::Relaxed);
        if dropped_total > 0 {
            tracing::warn!(dropped_total, "recorder: ASIO — ring overran, samples dropped");
        }

        // ── History + finished event ─────────────────────────────────────────
        write_history(&pool, &opts.output_path, &device_name).await;
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
        set_state(&app, &last_state, RecorderState::Stopped);
        tracing::info!("recorder: ASIO session stopped cleanly");
    }

    /// Map the recording opts' video-codec tag to the core enum (H.264 default).
    fn video_codec_of(opts: &RecordingOpts) -> sundayrec_core::editor::VideoCodec {
        if opts.video_codec.eq_ignore_ascii_case("h265") {
            sundayrec_core::editor::VideoCodec::H265
        } else {
            sundayrec_core::editor::VideoCodec::H264
        }
    }

    /// Emit a `recording://state` payload and update the shared last-state mirror so
    /// `recording_status` stays consistent. The ASIO path has no reconnects and no
    /// armed auto-stop deadline to report.
    fn set_state(app: &AppHandle, last_state: &Arc<Mutex<RecorderState>>, to: RecorderState) {
        if let Ok(mut g) = last_state.lock() {
            *g = to;
        }
        let _ = app.emit(
            STATE_EVENT,
            RecorderStatePayload {
                state: to,
                reconnect_count: 0,
                scheduled_stop_ms: None,
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
    async fn write_history(pool: &Option<SqlitePool>, final_path: &str, device_name: &str) {
        let byte_size = tokio::fs::metadata(final_path)
            .await
            .map(|m| m.len() as i64)
            .ok();
        let Some(pool) = pool else { return };
        let row = RecordingRow {
            id: String::new(),
            file_path: final_path.to_string(),
            device_name: Some(device_name.to_string()),
            started_at: 0.0,
            duration_ms: None,
            byte_size,
            created_at: 0.0,
            note: None,
        };
        if let Err(e) = insert_recording(pool, row).await {
            tracing::error!("recorder: ASIO failed to write history row: {e}");
        }
    }

    /// Drain ffmpeg stderr to the log and keep the last ~2 KB so a failure can
    /// report the real reason (mirrors `two_process::drain_stderr`).
    async fn drain_stderr<R>(stderr: R, tail: Arc<Mutex<String>>)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::trace!(target: "asio_ffmpeg", "{line}");
            if let Ok(mut t) = tail.lock() {
                t.push_str(&line);
                t.push('\n');
                if t.len() > 2048 {
                    let mut cut = t.len() - 2048;
                    while cut < t.len() && !t.is_char_boundary(cut) {
                        cut += 1;
                    }
                    *t = t.split_off(cut);
                }
            }
        }
    }
}
