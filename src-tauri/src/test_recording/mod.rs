//! Test-recording seam (P2b) — **HARDWARE-UNVERIFIED**.
//!
//! The impure half of the "Test mikrofon" button. Every decision (the size
//! floor, the error-kind classifier, the `astats` RMS → signal classifier) lives
//! in the unit-tested [`sundayrec_core::test_recording`]. This module performs
//! the side effects: run a short capture, stat the output, run the astats pass,
//! and clean up.
//!
//! ## The test goes down the recording's road (M1R #7)
//!
//! Since A5b every audio recording is captured by the native engine (cpal →
//! ring → WAV writer), with its own channel routing and rate negotiation. The
//! test used to capture through ffmpeg avfoundation/dshow instead, which has no
//! channel choice and cannot see an ASIO-only device — so «OK» and «fant ikke
//! enheten» described a different capture than the one a service would get.
//! [`run_test_recording`] now opens the same cpal host, with the same channel
//! mode, input-channel picks and sample rate as a recording
//! ([`native_test_opts`]), and judges the signal on the native WAV it wrote —
//! so it measures exactly the channels that would be recorded. ffmpeg is used
//! only for the `astats` level pass over that finished file.
//!
//! It is annotated HARDWARE-UNVERIFIED — the capture path needs a real mic; only
//! the core decisions and the option mapping are proven in the gate. See
//! docs/SMOKE-TEST.md.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use ts_rs::TS;

use sundayrec_core::settings::Settings;
use sundayrec_core::test_recording::{
    build_astats_args, classify_ffmpeg_error, classify_signal, parse_strongest_rms,
    size_is_plausible, TestRecordingError, TestRecordingSignal, TEST_DURATION_SEC,
};

/// Wait for a spawned ffmpeg `child` to exit, bounded by `timeout`. The capture
/// is self-limited by ffmpeg's `-t`, but if the device can't be opened (mic
/// contended, permission stuck) ffmpeg can hang at startup and `wait()` would
/// block the "Test mikrofon" command forever — so on timeout we kill the child
/// and report it as a non-success exit (which the caller classifies). Returns the
/// exit status, or `None` if it timed out / errored.
async fn wait_bounded(
    child: &mut tokio::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => {
            tracing::warn!("test recording: ffmpeg exceeded {timeout:?}; killing");
            let _ = child.kill().await;
            None
        }
    }
}

use crate::error::AppResult;
use crate::media::ffmpeg::spawn_ffmpeg;

/// Precision capture bench — THE proof tool for the zero-loss acceptance
/// criterion (2026-07-31: 15–56 % sample loss that every other instrument
/// missed). Drives the REAL shipping stack (negotiate → cpal stream → ring →
/// WAV writer) for `secs` seconds, then judges the facts with the unit-tested
/// [`sundayrec_core::selftest::selftest_verdict`]. (The ffmpeg bench that used to
/// sit beside this one left with the audio escape hatch in A5b.)
///
/// Run it with signal present (speak/play audio) — a silent room legitimately
/// fails the verdict. Measured duration comes from ffprobe over the produced WAV
/// (the truth), with the writer's exact frame count logged as the capture-side
/// cross-check; the xruns the audio system reported and any ring overrun
/// count as xrun-class events.
///
/// Interior-silence facts are not gathered here (no silencedetect pass), so
/// the verdict focuses on loss + level — the bench's purpose.
pub async fn run_native_capture_bench(
    host: crate::recorder::native_capture::stream::CpalHostKind,
    audio_device_name: &str,
    sample_rate: Option<u32>,
    secs: u32,
) -> AppResult<sundayrec_core::selftest::SelfTestReport> {
    use crate::recorder::native_capture::segment::{spawn_native_segment, stop_native_bounded};
    use sundayrec_core::selftest as st;
    use sundayrec_core::settings::ChannelMode;

    let secs = secs.clamp(5, 600);
    let tmp_dir = std::env::temp_dir().join("sundayrec-bench");
    std::fs::create_dir_all(&tmp_dir)?;
    let out = tmp_dir.join(format!(
        "bench_native_{}.wav",
        crate::db::store::now_ms() as u64
    ));
    let out_str = out.to_string_lossy().into_owned();

    let opts = crate::recorder::engine::RecordingOpts {
        audio_device_name: audio_device_name.to_string(),
        video_device_name: None,
        output_path: out_str.clone(),
        stop_on_silence: false,
        silence_threshold_db: None,
        silence_timeout_minutes: 5,
        channel_mode: ChannelMode::Stereo,
        input_channel_l: None,
        input_channel_r: None,
        sample_rate,
        bitrate_kbps: 192,
        split_minutes: 0,
        manual_max_minutes: 0,
        live_levels: false,
        keep_separate_audio: false,
        separate_audio_format: "wav".into(),
        video_flip: false,
    };

    let mut seg = spawn_native_segment(host, &opts, &out_str, None).await?;
    tokio::time::sleep(Duration::from_secs(u64::from(secs))).await;
    stop_native_bounded(&mut seg).await;

    let measured_sec = crate::media::ffmpeg::probe_duration_secs(&out_str)
        .await
        .unwrap_or(0.0);
    let frames = seg.frames.load(std::sync::atomic::Ordering::Relaxed);
    let frames_sec = frames as f64 / f64::from(seg.spec.sample_rate.max(1));
    let overrun = seg.overrun.load(std::sync::atomic::Ordering::Relaxed);
    // The audio system's own xrun count (cpal `ErrorKind::Xrun`), not a phrase
    // scraped from a log: the native engine reads no stderr.
    let native_xruns = seg.xruns.load(std::sync::atomic::Ordering::Relaxed);
    let size_bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let strongest_rms_db = run_astats_rms(&out_str).await;

    let facts = st::SelfTestFacts {
        expected_sec: f64::from(secs),
        measured_sec,
        drops: 0,
        dups: 0,
        xruns: native_xruns.saturating_add(u64::from(overrun > 0)),
        size_bytes,
        strongest_rms_db,
        silence_total_sec: 0.0,
        native_sample_rate: Some(seg.spec.sample_rate),
        forced_sample_rate: sample_rate,
    };
    let report = st::selftest_verdict(&facts);
    tracing::info!(
        verdict = ?report.verdict,
        expected = facts.expected_sec,
        measured = facts.measured_sec,
        frames_sec,
        ring_overrun_samples = overrun,
        native_xruns,
        rate = seg.spec.sample_rate,
        "native capture bench complete"
    );
    let _ = std::fs::remove_file(&out);
    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The diagnose capture probe (E2.5)
// ─────────────────────────────────────────────────────────────────────────────

/// How long the diagnose probe holds the microphone. Two seconds is enough for
/// the device to open, negotiate and deliver frames; anything longer is time the
/// operator spends staring at a spinner during a pre-service check.
pub const PROBE_SECS: u64 = 2;

/// Absolute ceiling on the whole probe, so a device that hangs on open cannot
/// wedge the diagnose command. Generous against `PROBE_SECS` because a cold
/// USB interface can take several seconds just to enumerate.
pub const PROBE_DEADLINE: Duration = Duration::from_secs(20);

/// The `RecordingOpts` the native bench + probe drive the shipping stack with.
/// Shared so the probe provably runs the SAME configuration as the bench (and
/// therefore the same one a real audio-only recording does).
fn native_probe_opts(
    audio_device_name: &str,
    output_path: &str,
    sample_rate: Option<u32>,
) -> crate::recorder::engine::RecordingOpts {
    use sundayrec_core::settings::ChannelMode;
    crate::recorder::engine::RecordingOpts {
        audio_device_name: audio_device_name.to_string(),
        video_device_name: None,
        output_path: output_path.to_string(),
        stop_on_silence: false,
        silence_threshold_db: None,
        silence_timeout_minutes: 5,
        channel_mode: ChannelMode::Stereo,
        input_channel_l: None,
        input_channel_r: None,
        sample_rate,
        bitrate_kbps: 192,
        split_minutes: 0,
        manual_max_minutes: 0,
        live_levels: false,
        keep_separate_audio: false,
        separate_audio_format: "wav".into(),
        video_flip: false,
    }
}

/// Run a short REAL capture and report whether the microphone produced audio.
///
/// This is what makes the diagnose report's `capture_ok` stop saying "ikke
/// testet". It opens the same cpal host a real audio-only recording would
/// ([`crate::recorder::engine::capture_host`]), so it exercises the same
/// stack — the native engine, which since A5b is the only one.
///
/// ## What it does NOT do
///
/// It deliberately does not apply the bench's
/// [`sundayrec_core::selftest::selftest_verdict`]. That verdict wants signal —
/// a silent room legitimately fails it — and a pre-service check run in an
/// empty sanctuary must not report "capture failed" because nobody was
/// talking. The question here is strictly "did the device open and deliver
/// samples", which is the question `capture_ok` has always asked.
///
/// ## Safety
///
/// Bounded by [`PROBE_DEADLINE`], and the device is always released: the probe
/// stops the stream + writer through the recorder's own `stop_native_bounded`.
/// The caller ([`crate::diagnostics`]) is responsible for not calling this while
/// something else owns the microphone.
pub async fn probe_audio_capture(
    audio_device_name: &str,
    sample_rate: Option<u32>,
) -> AppResult<bool> {
    let host = crate::recorder::engine::capture_host(
        cfg!(target_os = "windows"),
        crate::audio::asio::is_asio_device(audio_device_name),
    );

    let tmp_dir = std::env::temp_dir().join("sundayrec-probe");
    std::fs::create_dir_all(&tmp_dir)?;
    let out = tmp_dir.join(format!("probe_{}.wav", crate::db::store::now_ms() as u64));
    let out_str = out.to_string_lossy().into_owned();

    let result = tokio::time::timeout(
        PROBE_DEADLINE,
        run_audio_probe(host, audio_device_name, sample_rate, &out_str),
    )
    .await;
    // The temp capture has served its purpose whatever happened to it.
    let _ = std::fs::remove_file(&out);

    match result {
        Ok(inner) => inner,
        Err(_) => {
            tracing::warn!("capture probe: exceeded {PROBE_DEADLINE:?} — reporting failure");
            Ok(false)
        }
    }
}

/// The probe body, split out so [`probe_audio_capture`] owns the deadline and
/// the cleanup in one place.
async fn run_audio_probe(
    host: crate::recorder::native_capture::stream::CpalHostKind,
    audio_device_name: &str,
    sample_rate: Option<u32>,
    out_str: &str,
) -> AppResult<bool> {
    use crate::recorder::native_capture::segment::{spawn_native_segment, stop_native_bounded};

    let opts = native_probe_opts(audio_device_name, out_str, sample_rate);
    let mut seg = spawn_native_segment(host, &opts, out_str, None).await?;
    tokio::time::sleep(Duration::from_secs(PROBE_SECS)).await;
    stop_native_bounded(&mut seg).await;
    let frames = seg.frames.load(std::sync::atomic::Ordering::Relaxed);
    tracing::info!(
        frames,
        rate = seg.spec.sample_rate,
        "capture probe (native) complete"
    );
    // Frames, not bytes: a WAV header exists even when nothing was captured, so
    // "the file is non-empty" would be a false green.
    Ok(frames > 0)
}

/// The bench's RMS pass: astats over the finished file (same helper the test
/// recording uses). `None` on any failure — treated as unmeasured, not silent.
async fn run_astats_rms(path: &str) -> Option<f64> {
    let astats_args = build_astats_args(path);
    let astats_refs: Vec<&str> = astats_args.iter().map(String::as_str).collect();
    let mut c = spawn_ffmpeg(&astats_refs).await.ok()?;
    let drain = c.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    wait_bounded(&mut c, Duration::from_secs(60)).await;
    let buf = match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    parse_strongest_rms(&buf)
}

/// The result of a test recording. Mirrors the Electron `TestRecordingResult`
/// (camelCase): on success, the captured file's size + measured signal; on
/// failure, the classified error kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TestRecordingResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct TestRecordingResult {
    /// Whether the test produced a plausible recording.
    pub ok: bool,
    /// The classified failure, when `ok == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TestRecordingError>,
    /// Output file size in bytes, when a file was produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    /// Measured signal strength, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<TestRecordingSignal>,
}

/// The `RecordingOpts` the test capture opens the device with: the ones a real
/// audio-only recording gets from the settings — the same channel mode, the same
/// explicit input-channel picks (a digital mixer's channels 4/5 are tested as
/// 4/5, not as the first pair) and the same resolved sample rate. Nothing about
/// splitting, silence-stop or pre-roll applies to a ten-second test.
fn native_test_opts(
    settings: &Settings,
    output_path: &str,
) -> crate::recorder::engine::RecordingOpts {
    crate::recorder::engine::RecordingOpts {
        channel_mode: settings.channels,
        input_channel_l: settings.input_channel_l,
        input_channel_r: settings.input_channel_r,
        ..native_probe_opts(
            settings.device_name.as_deref().unwrap_or_default(),
            output_path,
            settings.resolved_sample_rate(),
        )
    }
}

/// Why the native capture could not start, in the vocabulary the frontend
/// already has. The native start errors are strings today, so this reuses the
/// substring classifier the ffmpeg path used («not found», «permission») — a
/// device the picker lists but the native host cannot find is
/// [`TestRecordingError::DeviceNotFound`] exactly as before.
fn classify_start_error(message: &str) -> TestRecordingError {
    classify_ffmpeg_error(message)
}

/// Run a ~10 s test capture for the configured mic, returning size + signal.
///
/// The capture goes through the native engine with the recording's own options
/// (see the module header), then the level is measured on the WAV it wrote.
/// HARDWARE-UNVERIFIED: the capture path needs a real device — the option
/// mapping and the classifiers are gate-tested.
///
/// The caller ([`crate::commands::recorder::run_test_recording`]) must have
/// released every other owner of the microphone first (VU meter, pre-roll
/// buffer) and refused while a recording runs.
pub async fn run_test_recording(settings: &Settings) -> AppResult<TestRecordingResult> {
    use crate::recorder::native_capture::segment::{spawn_native_segment, stop_native_bounded};

    let device = settings.device_name.as_deref().unwrap_or_default();
    let host = crate::recorder::engine::capture_host(
        cfg!(target_os = "windows"),
        crate::audio::asio::is_asio_device(device),
    );

    // Capture to a temp file under the OS temp dir (mirrors the Electron
    // `os.tmpdir()/sundayrec-test`). Cleaned up best-effort at the end; a crash
    // mid-test leaves at most one small WAV the OS reaps eventually.
    let tmp_dir = std::env::temp_dir().join("sundayrec-test");
    std::fs::create_dir_all(&tmp_dir)?;
    let out = tmp_dir.join(format!("test_{}.wav", crate::db::store::now_ms() as u64));
    let out_str = out.to_string_lossy().into_owned();
    let opts = native_test_opts(settings, &out_str);

    // 1. Open the device and capture. The open is bounded (a device that hangs
    //    on open must not wedge the command); the stop is bounded by the engine.
    let open_deadline = Duration::from_secs(TEST_DURATION_SEC as u64 + 15);
    let mut seg = match tokio::time::timeout(
        open_deadline,
        spawn_native_segment(host, &opts, &out_str, None),
    )
    .await
    {
        Ok(Ok(seg)) => seg,
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&out);
            return Ok(TestRecordingResult {
                ok: false,
                error: Some(classify_start_error(&e.to_string())),
                size_bytes: None,
                signal: None,
            });
        }
        Err(_) => {
            tracing::warn!("test recording: opening the device exceeded {open_deadline:?}");
            let _ = std::fs::remove_file(&out);
            return Ok(TestRecordingResult {
                ok: false,
                error: Some(TestRecordingError::FfmpegError),
                size_bytes: None,
                signal: None,
            });
        }
    };
    tokio::time::sleep(Duration::from_secs(TEST_DURATION_SEC as u64)).await;
    stop_native_bounded(&mut seg).await;

    // 2. Size sanity floor. A WAV header exists even when no frame arrived, so
    //    an empty capture is a tiny file → NoAudio.
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if !size_is_plausible(size) {
        let _ = std::fs::remove_file(&out);
        return Ok(TestRecordingResult {
            ok: false,
            error: Some(TestRecordingError::NoAudio),
            size_bytes: Some(size),
            signal: None,
        });
    }

    // 3. Measure RMS via astats over the native WAV. A parse failure → Normal
    //    (don't flag a working capture as silent), exactly the core's
    //    `classify_signal(None)` behaviour.
    let signal = classify_signal(run_astats_rms(&out_str).await);

    // Best-effort cleanup — the test file has served its purpose.
    let _ = std::fs::remove_file(&out);

    Ok(TestRecordingResult {
        ok: true,
        error: None,
        size_bytes: Some(size),
        signal: Some(signal),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::settings::ChannelMode;

    /// The test opens the device the way a recording does: the channel mode,
    /// the explicit input-channel picks and the resolved rate come from the
    /// settings, and nothing that only matters to a take (split, silence-stop,
    /// video) leaks in.
    #[test]
    fn the_test_capture_uses_the_recordings_channels_and_rate() {
        let settings = Settings {
            device_name: Some("Behringer X32".into()),
            channels: ChannelMode::MonoL,
            input_channel_l: Some(4),
            input_channel_r: Some(5),
            ..Settings::default()
        };
        let o = native_test_opts(&settings, "/tmp/t.wav");
        assert_eq!(o.audio_device_name, "Behringer X32");
        assert_eq!(o.output_path, "/tmp/t.wav");
        assert_eq!(o.channel_mode, ChannelMode::MonoL);
        assert_eq!((o.input_channel_l, o.input_channel_r), (Some(4), Some(5)));
        assert_eq!(o.sample_rate, settings.resolved_sample_rate());
        assert_eq!(o.split_minutes, 0);
        assert!(!o.stop_on_silence && o.video_device_name.is_none());
    }

    #[test]
    fn an_unset_device_is_the_default_input() {
        let o = native_test_opts(&Settings::default(), "/tmp/t.wav");
        assert_eq!(o.audio_device_name, "");
    }

    /// The start errors the native engine produces keep their frontend codes.
    #[test]
    fn native_start_errors_keep_the_frontend_vocabulary() {
        assert_eq!(
            classify_start_error("recording error: input device not found: Qu-5"),
            TestRecordingError::DeviceNotFound
        );
        assert_eq!(
            classify_start_error("recording error: microphone permission denied"),
            TestRecordingError::DevicePermissionDenied
        );
        assert_eq!(
            classify_start_error("recording error: unsupported input sample format"),
            TestRecordingError::FfmpegError
        );
    }

    /// A child that exits immediately, spawned from a NATIVE image on every
    /// platform.
    ///
    /// `windows-latest` has no native `true`: the name resolves through PATH to
    /// Git for Windows' MSYS coreutils (`C:\Program Files\Git\usr\bin\true.exe`),
    /// and the first MSYS process on a cold runner pays the `msys-2.0.dll` load
    /// plus a Defender scan — seconds of it, all AFTER `spawn()` has returned and
    /// therefore inside the bound this test gives the child. `cmd /C exit 0` runs
    /// a system image that is already resident.
    fn quick_exit_command() -> tokio::process::Command {
        if cfg!(windows) {
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.args(["/C", "exit", "0"]);
            cmd
        } else {
            tokio::process::Command::new("true")
        }
    }

    #[tokio::test]
    async fn wait_bounded_returns_status_for_a_quick_child() {
        // A process that exits immediately yields its status well inside the bound.
        let mut child = quick_exit_command()
            .spawn()
            .expect("spawn the quick-exit child");
        // `Some(status)` that is `success()` IS the deterministic assertion here:
        // `wait_bounded` returns `None` (and kills) on timeout, so this can only
        // pass on the un-timed-out path. 5 s on Unix, unchanged; 30 s on Windows,
        // where CI runners start processes far more slowly (image load and
        // Defender scanning happen after `spawn()` returns).
        let bound = if cfg!(windows) {
            Duration::from_secs(30)
        } else {
            Duration::from_secs(5)
        };
        let status = wait_bounded(&mut child, bound).await;
        assert!(status.map(|s| s.success()).unwrap_or(false));
    }

    #[tokio::test]
    async fn wait_bounded_kills_and_returns_none_on_timeout() {
        // A long-sleeping child must be killed once the deadline passes (modelling
        // a hung device-open) and report `None` rather than blocking forever.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn `sleep`");
        let status = wait_bounded(&mut child, Duration::from_millis(150)).await;
        assert!(
            status.is_none(),
            "a hung capture must time out and be killed"
        );
    }

    /// MACHINE BENCH (the v0.6.0 zero-loss proof): 60 s on the default input
    /// through the REAL native stack, judged by the same verdict engine as a
    /// live session. Ignored in the normal gate (records a full minute from
    /// the machine's microphone) — run explicitly:
    /// `cargo test -p sundayrec native_bench_60s -- --ignored --nocapture`.
    /// Set `SUNDAYREC_BENCH_KEEP=/path/out.wav` to keep the capture for
    /// spectrogram inspection.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "records 60 s from the default microphone — run with --ignored"]
    async fn native_bench_60s_machine_proof() {
        use crate::recorder::native_capture::segment::{spawn_native_segment, stop_native_bounded};
        use crate::recorder::native_capture::stream::CpalHostKind;
        use std::sync::atomic::Ordering;

        let keep = std::env::var("SUNDAYREC_BENCH_KEEP").ok();
        let dir = tempfile::tempdir().expect("tempdir");
        let out = match &keep {
            Some(p) => std::path::PathBuf::from(p),
            None => dir.path().join("bench60.wav"),
        };
        let out_str = out.to_string_lossy().into_owned();

        let opts = crate::recorder::engine::RecordingOpts {
            audio_device_name: String::new(),
            video_device_name: None,
            output_path: out_str.clone(),
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
            live_levels: false,
            keep_separate_audio: false,
            separate_audio_format: "wav".into(),
            video_flip: false,
        };

        let wall_start = std::time::Instant::now();
        let mut seg = spawn_native_segment(CpalHostKind::Default, &opts, &out_str, None)
            .await
            .expect("native capture must start on the bench machine");
        tokio::time::sleep(Duration::from_secs(60)).await;
        stop_native_bounded(&mut seg).await;
        let wall = wall_start.elapsed().as_secs_f64();

        let frames = seg.frames.load(Ordering::Relaxed);
        let frames_sec = frames as f64 / f64::from(seg.spec.sample_rate.max(1));
        let overrun = seg.overrun.load(Ordering::Relaxed);
        // The audio system's own xrun count over the 60 s (A3c). Reported, not
        // asserted: it is a measurement of the machine, and 0 is the expected
        // value on a quiet one, not a law.
        let xruns = seg.xruns.load(Ordering::Relaxed);
        let probed = crate::media::ffmpeg::probe_duration_secs(&out_str)
            .await
            .unwrap_or(0.0);

        eprintln!(
            "BENCH60: wall={wall:.3}s frames_sec={frames_sec:.3}s ffprobe={probed:.3}s \
             rate={} overruns={overrun} xruns={xruns} file={out_str}",
            seg.spec.sample_rate
        );

        assert_eq!(overrun, 0, "zero ring overruns");
        // Zero-loss criterion: captured media within 1 s of the 60 s wall span
        // (device open eats a fraction of the first second), and ffprobe agrees
        // with the writer's exact frame count to within 50 ms.
        assert!(
            frames_sec > 59.0 && frames_sec <= wall + 0.1,
            "frame seconds must track the wall clock (frames_sec={frames_sec:.3}, wall={wall:.3})"
        );
        assert!(
            (probed - frames_sec).abs() < 0.05,
            "ffprobe ({probed:.3}s) must agree with the frame count ({frames_sec:.3}s)"
        );
    }
}
