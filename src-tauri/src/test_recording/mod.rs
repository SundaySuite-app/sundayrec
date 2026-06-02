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
use crate::error::AppResult;
use crate::media::ffmpeg::spawn_ffmpeg;
use crate::recorder::engine::current_platform;

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
