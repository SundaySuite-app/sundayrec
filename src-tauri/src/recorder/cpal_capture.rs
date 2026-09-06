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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sundayrec_core::recorder::Deliverable;
use sundayrec_core::recovery::{AudioEncodeManifest, DeliverableManifest, SessionManifest};

use crate::db::store::RecordingRow;
use crate::recorder::concat::DeliverySpec;
use crate::recorder::engine::{capture_base_path, capture_dir, delivery_encode_for, RecordingOpts};

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

// ─────────────────────────────────────────────────────────────────────────────
//   F2-W4 — the Windows video session is crash-safe now
// ─────────────────────────────────────────────────────────────────────────────
//
// WHAT WAS TRUE: this path pointed ffmpeg straight at the user's `.mp4`. An mp4
// carries its index (`moov`) only after a clean finalise, so ANY other ending —
// Task Manager kill, power cut, a Windows update reboot (F-W1), the console
// window that used to be closable (#237) — left an UNPLAYABLE file. And because
// the session wrote no crash-recovery manifest, `scan_and_recover` on the next
// launch did not even know the file existed. Sunday's video was simply gone.
//
// WHAT IS TRUE NOW: identical shape to the macOS/`run_session` path — capture
// Matroska into `.sundayrec-capture-<session_id>/<stem>.mkv` (playable up to
// whatever instant the machine died), persist the manifest that says how to
// finish it, and remux (`-c copy`, seconds) into the user's mp4 at stop. A
// session that never reaches its stop is finished by the next launch instead,
// through the SAME `finalize_deliverable` the live stop uses, and lands in
// history marked «Gjenopprettet etter uventet avslutning».
//
// (Audio-only cpal — the `classic_ffmpeg_audio` hatch — is left alone: the
// native engine owns audio on both platforms now, and it already captures WAV
// through this same decoupled layout.)

/// The decoupled-capture layout for ONE Windows cpal VIDEO session: where ffmpeg
/// captures, and everything needed to finish that capture — at stop, or on the
/// next launch after a crash.
///
/// Pure data, built by [`plan_video_capture`] before anything touches the disk,
/// so the whole layout decision is unit-testable off Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoCapture {
    /// The per-session hidden folder BESIDE the user's delivery file.
    pub cap_dir: PathBuf,
    /// The Matroska ffmpeg writes — the session's one and only fragment.
    pub capture_path: String,
    /// Manifest filename stem + the recovery scan's session key.
    pub session_id: String,
    /// How the capture becomes the user's file (shared with `run_session`).
    pub delivery: AudioEncodeManifest,
    /// Epoch ms the session started; the recovered row's start time.
    pub started_ms: u64,
    /// Capture device name, for the recovered history row.
    pub device_name: String,
}

impl VideoCapture {
    /// The capture as the concat/finalize layer sees it: one fragment, which IS
    /// the primary. This path has no split and no reconnect (the engine routes a
    /// session that needs either to dshow), so the layout never grows.
    pub(crate) fn deliverable(&self) -> Deliverable {
        Deliverable {
            primary_path: self.capture_path.clone(),
            fragments: vec![self.capture_path.clone()],
            started_at_ms: self.started_ms,
        }
    }

    /// The crash-recovery manifest to persist once the capture is live.
    pub(crate) fn manifest(&self) -> SessionManifest {
        SessionManifest {
            session_id: self.session_id.clone(),
            device_name: self.device_name.clone(),
            session_start_ms: self.started_ms,
            // Pre-roll needs the full `run_session`; the engine sends any session
            // that wants one to dshow (ASIO excepted, where it is logged as
            // inactive), so this path never has a clip to prepend.
            preroll_clip_path: None,
            delivery_encode: Some(self.delivery.clone()),
            deliverables: vec![DeliverableManifest {
                primary_path: self.capture_path.clone(),
                fragments: vec![self.capture_path.clone()],
                started_at_ms: self.started_ms,
            }],
        }
    }

    /// The finalise arguments for the live stop — built from the SAME manifest
    /// the crash recovery would read, through the same constructor, so a
    /// recovered recording cannot come back in a different format than a
    /// cleanly-stopped one.
    pub(crate) fn delivery_spec(&self) -> DeliverySpec {
        DeliverySpec::from_manifest(&self.delivery, &self.capture_path)
    }
}

/// Decide a video session's capture layout. Pure — no directory is created and
/// no manifest is written here.
///
/// `start_ms` doubles as the session id (the engine is a singleton, so a start
/// timestamp never repeats), exactly as `run_session` does it.
pub(crate) fn plan_video_capture(
    opts: &RecordingOpts,
    device_name: &str,
    start_ms: u64,
) -> VideoCapture {
    let session_id = start_ms.to_string();
    let cap_dir = capture_dir(&opts.output_path, &session_id);
    // Matroska, carrying the delivery file's OWN stem so `delivery_path_for`
    // maps it straight back to what the user asked for.
    let capture_path = capture_base_path(&cap_dir, &opts.output_path, "mkv");
    VideoCapture {
        cap_dir,
        capture_path,
        session_id,
        delivery: delivery_encode_for(opts, false),
        started_ms: start_ms,
        device_name: device_name.to_string(),
    }
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

    use super::{history_row, plan_video_capture, writer_task, CpalHostKind, VideoCapture};
    use crate::audio::asio::{build_route_plan, ChannelRoute};
    use crate::db::store::insert_recording;
    use crate::error::{AppError, AppResult};
    use crate::media::ffmpeg::spawn_ffmpeg;
    use crate::recorder::concat::finalize_deliverable;
    use crate::recorder::engine::{
        extract_separate_audio, now_ms, RecordingEvent, RecordingFinished, RecordingLevels,
        RecordingOpts, StateWriter, ERROR_EVENT, FINISHED_EVENT, LEVELS_EVENT,
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
        state: StateWriter,
    ) {
        let label = host_kind.label();
        // The session's start AND its id (a singleton engine never repeats a
        // start timestamp). Stamped before the device probe, like `run_session`
        // does it, because the capture layout — and therefore the crash-recovery
        // manifest — has to exist before the first frame can land.
        let start_ms = now_ms();

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

        // ── Decoupled capture layout (video only) ────────────────────────────
        // A video session captures crash-tolerant Matroska into a hidden
        // per-session folder beside the delivery file and is remuxed to the
        // user's mp4 at stop (F2-W4 — see the block comment above
        // [`VideoCapture`]). Audio-only keeps writing its delivery file directly:
        // that is the `classic_ffmpeg_audio` hatch, and the native engine — which
        // owns audio on both platforms — already has this layout.
        let has_video = video.is_some();
        let capture = has_video.then(|| plan_video_capture(&opts, &device_name, start_ms));
        if let Some(c) = &capture {
            if let Err(e) = tokio::fs::create_dir_all(&c.cap_dir).await {
                // Same verdict as `run_session`: without the capture folder there
                // is no crash-safe recording to make. The engine falls back to
                // dshow, which fails on the same folder with the same message
                // rather than silently recording an unrecoverable mp4.
                tracing::error!(dir = %c.cap_dir.display(), "recorder: cpal failed to create capture dir: {e}");
                let _ = ready_tx.send(Err(AppError::Recording(format!(
                    "kunne ikke opprette opptaksmappe {}: {e}",
                    c.cap_dir.display()
                ))));
                return;
            }
        }
        // What ffmpeg actually writes: the MKV capture (video) or, for audio-only,
        // the user's file itself.
        let ffmpeg_target = capture
            .as_ref()
            .map(|c| c.capture_path.clone())
            .unwrap_or_else(|| opts.output_path.clone());

        // ── Build ffmpeg args (audio-only or video+pipe) ─────────────────────
        let args: Vec<String> = match &video {
            Some(v) => build_cpal_pipe_video_args(
                &v.name,
                sundayrec_core::capture::RECORDING_FRAMERATE,
                sample_rate,
                out_ch,
                &ffmpeg_target,
                opts.sample_rate,
                opts.bitrate_kbps,
                // v0.15: the recording codec is a constant (H.264).
                sundayrec_core::capture::RECORDING_VIDEO_CODEC,
                None, // live preview wiring deferred for the cpal path
            ),
            None => build_cpal_pipe_audio_args(
                sample_rate,
                out_ch,
                &ffmpeg_target,
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
                discard_unstarted_capture(capture.as_ref()).await;
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let _ = child.start_kill();
                discard_unstarted_capture(capture.as_ref()).await;
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
                discard_unstarted_capture(capture.as_ref()).await;
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
                discard_unstarted_capture(capture.as_ref()).await;
                let _ = ready_tx.send(Err(AppError::Recording(e)));
                return;
            }
            _ => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                stop.store(true, Ordering::Relaxed);
                let _ = stream_handle.join();
                discard_unstarted_capture(capture.as_ref()).await;
                let _ = ready_tx.send(Err(AppError::Recording(
                    "cpal capture thread exited before signalling".into(),
                )));
                return;
            }
        }

        // The capture is LIVE → persist the crash-recovery manifest. From this
        // instant on, an app that never reaches its stop is finished by the next
        // launch's `scan_and_recover` instead of leaving an orphaned file nobody
        // knows about. Written here rather than before the spawn because every
        // failure above falls back to dshow, and a manifest for a session that
        // never recorded is litter the startup scan would have to reason about.
        // Best-effort, exactly like `run_session`: it never blocks the recording.
        if let Some(c) = &capture {
            crate::recorder::recovery::write_manifest(&app, &c.manifest()).await;
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
        // run_session — re-pin the timer whenever the watch changes, off the SAME
        // `start_ms` the session id and the manifest carry (F2-W4 moved that stamp
        // to the top of the function; `run_session` has always taken it there).
        let initial_stop = (opts.manual_max_minutes > 0)
            .then(|| start_ms + u64::from(opts.manual_max_minutes) * 60_000);
        state.arm_autostop(initial_stop);
        let mut stop_watch = state.subscribe();
        let mut auto_deadline: Option<u64> = *stop_watch.borrow();
        let remaining = |d: Option<u64>| -> Duration {
            d.map(|d| Duration::from_millis(d.saturating_sub(now_ms())))
                // `None` → idle ~100 years so the guarded arm never fires.
                .unwrap_or_else(|| Duration::from_secs(60 * 60 * 24 * 365 * 100))
        };
        let auto_sleep = tokio::time::sleep(remaining(auto_deadline));
        tokio::pin!(auto_sleep);

        state.set(RecorderState::Recording, 0);
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
                        state.restamp(0, auto_deadline);
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
        state.set(RecorderState::Stopping, 0);
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

        // ── Deliver: MKV capture → the user's mp4 ────────────────────────────
        // For a video session `opts.output_path` does not exist yet — ffmpeg wrote
        // Matroska into the capture folder. Everything downstream (the sidecar
        // extract, the history row, the record→edit hand-off) works on whatever
        // this leaves behind, which on a failed remux is the capture itself.
        let final_path = match &capture {
            None => opts.output_path.clone(),
            Some(c) => finalize_video_capture(&app, c).await,
        };

        // ── Separate-audio sidecar (H2): extract the clean audio next to a video
        // recording, exactly like the dshow path (`engine::extract_separate_audio`). ─
        if has_video && opts.keep_separate_audio {
            if let Some(pool) = &pool {
                let audio = FfmpegDevice::new(device_name.clone(), "cpal", None);
                extract_separate_audio(pool, &final_path, start_ms, duration_ms, &opts, &audio)
                    .await;
            }
        }

        // ── History + finished event ─────────────────────────────────────────
        write_history(&pool, &final_path, &device_name, start_ms, duration_ms).await;
        if tokio::fs::metadata(&final_path)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            let _ = app.emit(
                FINISHED_EVENT,
                RecordingFinished {
                    file_path: final_path.clone(),
                    has_video,
                },
            );
        }
        // The terminal write clears the shared auto-stop deadline itself (inside
        // [`StateWriter::set`]), so a finished recording ships no lingering
        // countdown — and a SUPERSEDED cpal supervisor clears nothing at all.
        state.set(RecorderState::Stopped, 0);
        tracing::info!(host = label, "recorder: cpal session stopped cleanly");
    }

    /// Throw away a capture folder whose session never started. Every failure
    /// between "folder created" and "stream live" hands the recording back to the
    /// engine, which starts a FRESH dshow session with its own folder — so what
    /// this one left (a zero-length mkv ffmpeg may have opened, and the folder)
    /// is litter beside the user's recordings.
    ///
    /// Best-effort and order-dependent: `remove_dir` only removes an EMPTY
    /// directory, so the file goes first. No manifest has been written at any of
    /// these points, which is why nothing here has to think about recovery.
    async fn discard_unstarted_capture(capture: Option<&VideoCapture>) {
        let Some(c) = capture else { return };
        let _ = tokio::fs::remove_file(&c.capture_path).await;
        let _ = tokio::fs::remove_dir(&c.cap_dir).await;
    }

    /// Finish a video session: remux the Matroska capture into the user's mp4
    /// through the SAME [`finalize_deliverable`] the macOS path and the startup
    /// recovery use, and return the file history should point at.
    ///
    /// On success the recovery manifest is deleted (its job is done) and the
    /// now-empty capture folder with it. On FAILURE the manifest deliberately
    /// STAYS: the mkv is a whole, playable recording, and the next launch must
    /// get the chance to retry the remux rather than forfeit it — the same
    /// "a stop is only clean for what actually delivered" rule `run_session`
    /// follows. The history row then points at the capture, so the service is
    /// reachable from the app either way.
    async fn finalize_video_capture(app: &AppHandle, capture: &VideoCapture) -> String {
        let deliverable = capture.deliverable();
        let spec = capture.delivery_spec();
        match finalize_deliverable(&deliverable, None, Some(&spec)).await {
            Ok(delivered) => {
                crate::recorder::recovery::delete_manifest(app, &capture.session_id).await;
                // `remove_dir` only removes it if EMPTY — a delivery that somehow
                // left the capture behind keeps its folder as a recovery source.
                let _ = tokio::fs::remove_dir(&capture.cap_dir).await;
                delivered
            }
            Err(e) => {
                tracing::error!(
                    capture = %capture.capture_path,
                    delivery = %spec.delivery_path,
                    "recorder: cpal — remux to the delivery format failed, keeping the capture \
                     and the recovery manifest for the next launch: {e}"
                );
                capture.capture_path.clone()
            }
        }
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

    // ── The Windows video session's crash-safe layout (F2-W4) ────────────────

    /// A video session's options: the user asked for `/Opptak/gudstjeneste.mp4`.
    fn video_opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: Some("Logitech BRIO".into()),
            output_path: "/Opptak/gudstjeneste.mp4".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            channel_mode: sundayrec_core::settings::ChannelMode::Stereo,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: None,
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

    /// GOLDEN (F2-W4). The Windows video capture lands in the session's hidden
    /// folder as Matroska — NOT in the user's mp4, which has no `moov` atom until
    /// a clean finalise and is therefore unplayable after a kill.
    #[test]
    fn video_capture_targets_the_sessions_mkv_beside_the_delivery_file() {
        let c = plan_video_capture(&video_opts(), "Soundcraft USB Audio", 1_786_179_600_000);
        assert_eq!(c.session_id, "1786179600000");
        assert_eq!(
            c.cap_dir,
            PathBuf::from("/Opptak/.sundayrec-capture-1786179600000")
        );
        assert_eq!(
            c.capture_path, "/Opptak/.sundayrec-capture-1786179600000/gudstjeneste.mkv",
            "the capture keeps the delivery stem so it maps straight back"
        );
        // The layout is the SAME one `run_session` builds — the folder sits
        // beside the delivery file (one volume, so the remux never crosses a
        // filesystem) and is hidden.
        assert_eq!(
            c.cap_dir.parent(),
            std::path::Path::new("/Opptak/gudstjeneste.mp4").parent()
        );
    }

    /// The seam: the argument builder must be handed the CAPTURE path. This is
    /// the assertion that fails if someone re-points ffmpeg at `output_path`.
    #[test]
    fn video_capture_args_are_built_for_the_mkv_not_the_mp4() {
        let c = plan_video_capture(&video_opts(), "Soundcraft USB Audio", 1_786_179_600_000);
        let args = sundayrec_core::capture::build_cpal_pipe_video_args(
            "Logitech BRIO",
            sundayrec_core::capture::RECORDING_FRAMERATE,
            48_000,
            2,
            &c.capture_path,
            None,
            192,
            sundayrec_core::capture::RECORDING_VIDEO_CODEC,
            None,
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(c.capture_path.as_str())
        );
        assert!(
            !args.iter().any(|a| a.ends_with(".mp4")),
            "no ffmpeg argument may still point at the delivery mp4: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "+faststart"),
            "the mkv capture drops faststart — and with it the whole-file \
             rewrite a stop used to pay"
        );
    }

    /// The manifest is what makes a crash survivable: it is the ONLY way
    /// `scan_and_recover` learns the capture exists. One deliverable, one
    /// fragment (this path has neither split nor reconnect), stamped with the
    /// session start, and carrying a REMUX — not an audio encode.
    #[test]
    fn video_capture_manifest_describes_a_remuxable_single_fragment_session() {
        let c = plan_video_capture(&video_opts(), "Soundcraft USB Audio", 1_786_179_600_000);
        let m = c.manifest();
        assert_eq!(m.session_id, "1786179600000");
        assert_eq!(m.device_name, "Soundcraft USB Audio");
        assert_eq!(m.session_start_ms, 1_786_179_600_000);
        assert_eq!(
            m.preroll_clip_path, None,
            "a session that wants a pre-roll is routed to dshow, never here"
        );
        assert_eq!(m.deliverables.len(), 1);
        assert_eq!(m.deliverables[0].primary_path, c.capture_path);
        assert_eq!(m.deliverables[0].fragments, vec![c.capture_path.clone()]);
        assert_eq!(m.deliverables[0].started_at_ms, 1_786_179_600_000);

        let enc = m
            .delivery_encode
            .as_ref()
            .expect("a decoupled capture must say how to finish");
        assert_eq!(
            enc.mode,
            sundayrec_core::recovery::DeliveryMode::RemuxCopy,
            "video is stream-copied into the container, never re-encoded"
        );
        assert_eq!(enc.delivery_dir, "/Opptak");
        assert_eq!(enc.ext, "mp4");
        // The manifest is the whole contract with the next launch — it has to
        // survive a round-trip through the JSON file on disk.
        let json = m.to_json().expect("serialise");
        assert_eq!(
            sundayrec_core::recovery::SessionManifest::from_json(&json).expect("parse"),
            c.manifest()
        );
    }

    /// Live stop and crash recovery must deliver to the SAME place: the file the
    /// user actually asked for. (Built through `DeliverySpec::from_manifest`, the
    /// one constructor all three finalise paths share.)
    #[test]
    fn video_capture_delivers_back_to_exactly_the_users_file() {
        let opts = video_opts();
        let c = plan_video_capture(&opts, "Soundcraft USB Audio", 1_786_179_600_000);
        let spec = c.delivery_spec();
        assert_eq!(spec.delivery_path, opts.output_path);
        assert_eq!(spec.mode, sundayrec_core::recovery::DeliveryMode::RemuxCopy);
        // What the recovery scan would compute from the persisted manifest, with
        // no live session in memory, is the same path.
        let enc = c.manifest().delivery_encode.unwrap();
        assert_eq!(
            sundayrec_core::recovery::delivery_path_for(
                &c.manifest().deliverables[0].primary_path,
                &enc.delivery_dir,
                &enc.ext,
            ),
            opts.output_path
        );
    }

    /// The concat layer sees one fragment and no pre-roll, so `concat_needed` is
    /// false and finalisation is a pure remux — no ffmpeg concat pass, no
    /// second copy of a multi-hour service on disk.
    #[test]
    fn video_capture_deliverable_needs_no_concat_pass() {
        let c = plan_video_capture(&video_opts(), "Mic", 1_786_179_600_000);
        let d = c.deliverable();
        assert_eq!(d.primary_path, c.capture_path);
        assert_eq!(d.fragments, vec![c.capture_path.clone()]);
        assert!(
            !sundayrec_core::recorder::concat_needed(&d.fragments, false),
            "a single fragment with no pre-roll is already the finished capture"
        );
    }

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
