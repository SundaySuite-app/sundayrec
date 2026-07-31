//! Test-recording seam (P2b) — **HARDWARE-UNVERIFIED**.
//!
//! The impure half of the "Test mikrofon" button. Every decision (the ffmpeg
//! argv, the size floor, the stderr error-kind classifier, the `astats` RMS →
//! signal classifier) lives in the unit-tested [`sundayrec_core::test_recording`].
//! This module performs the side effects the Electron `src/main/test-recorder.ts`
//! did: enumerate devices, resolve the configured mic, spawn a short capture via
//! the bundled ffmpeg sidecar, stat the output, run the astats pass, and clean up.
//!
//! No new dependency or cargo feature: it reuses the ffmpeg sidecar the recorder
//! already drives (`crate::media::ffmpeg`). It is annotated HARDWARE-UNVERIFIED —
//! the spawn/stat path needs a real mic + the sidecar binary; only the core
//! decisions are proven in the gate. See docs/SMOKE-TEST.md.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use ts_rs::TS;

use sundayrec_core::device_match::find_best_device_match;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::test_recording::{
    build_astats_args, build_test_args, classify_ffmpeg_error, classify_signal,
    parse_strongest_rms, size_is_plausible, TestRecordingError, TestRecordingSignal,
    TEST_DURATION_SEC,
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

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::spawn_ffmpeg;
use crate::recorder::engine::current_platform;

/// Precision capture bench — THE proof tool for the zero-loss acceptance
/// criterion (2026-07-31: 15–56 % sample loss that every other instrument
/// missed). Runs the REAL recording argv (the exact
/// [`sundayrec_core::capture::build_unified_capture_args`] chain, live levels +
/// silencedetect included) against the configured mic for `secs` seconds,
/// ffprobes the produced WAV, and judges the facts with the unit-tested
/// [`sundayrec_core::selftest::selftest_verdict`].
///
/// `expected_sec == secs` exactly: `-t` limits MEDIA time, so ANY shortfall in
/// the probed duration is dropped samples (no startup allowance needed here,
/// unlike the wall-clock-based live-session measurement). Run it with signal
/// present (speak/play audio) — a silent room legitimately fails the verdict.
pub async fn run_capture_bench(
    audio_device_name: &str,
    sample_rate: Option<u32>,
    secs: u32,
) -> AppResult<sundayrec_core::selftest::SelfTestReport> {
    use sundayrec_core::capture::{build_unified_capture_args, CaptureOpts};
    use sundayrec_core::selftest as st;

    let secs = secs.clamp(5, 600);
    let inv = enumerate_ffmpeg_devices().await?;
    let Some(dev) = find_best_device_match(&inv.audio_inputs, audio_device_name) else {
        return Err(AppError::Validation(format!(
            "benk: fant ikke lydenheten «{audio_device_name}»"
        )));
    };
    // The builder formats the platform input token itself (`:<idx>` on mac,
    // `audio=<name>` on Windows) — pass the raw index/name.
    let token = match current_platform() {
        Platform::MacOS | Platform::Linux => dev.index.map(|i| i.to_string()).unwrap_or_default(),
        Platform::Windows => dev.name.clone(),
    };

    let tmp_dir = std::env::temp_dir().join("sundayrec-bench");
    std::fs::create_dir_all(&tmp_dir)?;
    let out = tmp_dir.join(format!("bench_{}.wav", crate::db::store::now_ms() as u64));
    let out_str = out.to_string_lossy().into_owned();

    let opts = CaptureOpts {
        sample_rate,
        live_levels: true,
        ..CaptureOpts::default()
    };
    let mut args = build_unified_capture_args(current_platform(), None, &token, &out_str, &opts);
    // Inject the bench duration ahead of the trailing `-y <out>` pair.
    let cut = args.len().saturating_sub(2);
    args.splice(cut..cut, ["-t".to_string(), secs.to_string()]);

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = spawn_ffmpeg(&arg_refs).await?;
    let stderr_drain = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    let status = wait_bounded(&mut child, Duration::from_secs(u64::from(secs) + 20)).await;
    let stderr_buf = match stderr_drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.map(|s| s.success()).unwrap_or(false) {
        let _ = std::fs::remove_file(&out);
        return Err(AppError::Recording(format!(
            "benk: ffmpeg feilet — {:?}",
            classify_ffmpeg_error(&stderr_buf)
        )));
    }

    // Facts from the run: measured media duration (ffprobe — the truth), size,
    // drop/xrun/capture-drop counters and interior silence from the capture's
    // own stderr, RMS from a second astats pass.
    let measured_sec = crate::media::ffmpeg::probe_duration_secs(&out_str)
        .await
        .unwrap_or(0.0);
    let size_bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let capture_drop_lines = stderr_buf
        .lines()
        .filter(|l| st::is_capture_drop_line(&l.to_lowercase()))
        .count() as u64;
    let silence = st::parse_silence_segments(&stderr_buf, Some(f64::from(secs)));
    let strongest_rms_db = run_astats_rms(&out_str).await;

    let facts = st::SelfTestFacts {
        expected_sec: f64::from(secs),
        measured_sec,
        drops: st::parse_drop_count(&stderr_buf),
        dups: st::parse_dup_count(&stderr_buf),
        xruns: st::parse_xrun_count(&stderr_buf).saturating_add(capture_drop_lines),
        size_bytes,
        strongest_rms_db,
        silence_total_sec: st::silence_total_sec(&silence),
        native_sample_rate: None,
        forced_sample_rate: sample_rate,
    };
    let report = st::selftest_verdict(&facts);
    tracing::info!(
        verdict = ?report.verdict,
        expected = facts.expected_sec,
        measured = facts.measured_sec,
        "capture bench complete"
    );
    let _ = std::fs::remove_file(&out);
    Ok(report)
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
#[ts(export, export_to = "../../src/lib/bindings/TestRecordingResult.ts")]
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

/// Resolve the platform capture format + the addressable device token for the
/// configured mic name, matching the recorder's avfoundation/dshow model (NOT
/// the Electron wasapi path — the Tauri recorder captures via avfoundation on
/// mac / dshow on Windows). Returns `(format, device)` or `None` when no device
/// matched.
async fn resolve_input(audio_device_name: &str) -> AppResult<Option<(String, String)>> {
    let inv = enumerate_ffmpeg_devices().await?;
    let Some(dev) = find_best_device_match(&inv.audio_inputs, audio_device_name) else {
        return Ok(None);
    };
    let (format, device) = match current_platform() {
        Platform::MacOS | Platform::Linux => {
            // avfoundation audio-only input is ":<index>".
            let idx = dev.index.map(|i| i.to_string()).unwrap_or_default();
            ("avfoundation".to_string(), format!(":{idx}"))
        }
        Platform::Windows => ("dshow".to_string(), format!("audio={}", dev.name)),
    };
    Ok(Some((format, device)))
}

/// Run a ~10 s test capture for the configured mic, returning size + signal.
/// HARDWARE-UNVERIFIED: the spawn/stat/astats path is wired but unproven on a
/// device — only the core argv/classifier decisions are gate-tested.
pub async fn run_test_recording(audio_device_name: &str) -> AppResult<TestRecordingResult> {
    let Some((format, device)) = resolve_input(audio_device_name).await? else {
        return Ok(TestRecordingResult {
            ok: false,
            error: Some(TestRecordingError::DeviceNotFound),
            size_bytes: None,
            signal: None,
        });
    };

    // Capture to a temp file under the OS temp dir (mirrors the Electron
    // `os.tmpdir()/sundayrec-test`). We clean it up best-effort at the end; a
    // crash mid-test leaves at most one small mp3 the OS reaps eventually.
    let tmp_dir = std::env::temp_dir().join("sundayrec-test");
    std::fs::create_dir_all(&tmp_dir)?;
    let out = tmp_dir.join(format!("test_{}.mp3", crate::db::store::now_ms() as u64));
    let out_str = out.to_string_lossy().into_owned();

    // The capture is `-t TEST_DURATION_SEC`; allow a generous margin for device
    // open + encode flush before we treat a non-exit as a hang and kill it.
    let capture_deadline = Duration::from_secs(TEST_DURATION_SEC as u64 + 15);

    // 1. Run the capture. Drain stderr concurrently so a hung device-open can't
    //    wedge the read (stderr only closes when ffmpeg exits) — the bounded wait
    //    kills the child on timeout, which unblocks the drain.
    let args = build_test_args(&format, &device, &out_str);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = spawn_ffmpeg(&arg_refs).await?;
    let stderr_drain = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    let status = wait_bounded(&mut child, capture_deadline).await;
    let stderr_buf = match stderr_drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };

    if !status.map(|s| s.success()).unwrap_or(false) {
        return Ok(TestRecordingResult {
            ok: false,
            error: Some(classify_ffmpeg_error(&stderr_buf)),
            size_bytes: None,
            signal: None,
        });
    }

    // 2. Size sanity floor.
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if !size_is_plausible(size) {
        return Ok(TestRecordingResult {
            ok: false,
            error: Some(TestRecordingError::NoAudio),
            size_bytes: Some(size),
            signal: None,
        });
    }

    // 3. Measure RMS via astats. A parse failure → Normal (don't flag a working
    //    capture as silent), exactly the core's `classify_signal(None)` behaviour.
    let astats_args = build_astats_args(&out_str);
    let astats_refs: Vec<&str> = astats_args.iter().map(String::as_str).collect();
    let signal = match spawn_ffmpeg(&astats_refs).await {
        Ok(mut c) => {
            let drain = c.stderr.take().map(|mut stderr| {
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let _ = stderr.read_to_end(&mut bytes).await;
                    String::from_utf8_lossy(&bytes).into_owned()
                })
            });
            // astats reads a finite file and exits, but bound it anyway so a
            // wedged sidecar can't hang the test command.
            wait_bounded(&mut c, Duration::from_secs(30)).await;
            let buf = match drain {
                Some(h) => h.await.unwrap_or_default(),
                None => String::new(),
            };
            classify_signal(parse_strongest_rms(&buf))
        }
        Err(_) => classify_signal(None),
    };

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

    #[tokio::test]
    async fn wait_bounded_returns_status_for_a_quick_child() {
        // A process that exits immediately yields its status well inside the bound.
        let mut child = tokio::process::Command::new("true")
            .spawn()
            .expect("spawn `true`");
        let status = wait_bounded(&mut child, Duration::from_secs(5)).await;
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
}
