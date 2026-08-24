//! Multi-channel device probing over the REAL capture backend (ffmpeg).
//!
//! The webview's `getUserMedia` caps a device at 2 channels, which hid the
//! channel picker for real digital mixers (2026-07-31: the Qu-5 exposes 32
//! input channels over avfoundation, but the UI thought it had 2 and recorded
//! a near-silent channel). These probes ask the SAME backend the recorder
//! uses, so what the picker shows is exactly what the capture can address:
//!
//!   - [`probe_input_channels`]: open the device for a blink and parse the
//!     input banner's channel count.
//!   - [`scan_channel_peaks`]: capture a few seconds to the null muxer with a
//!     per-channel `astats` pass and report each channel's max peak — "which
//!     of my mixer's channels actually carry the mix?"
//!
//! Both may fail while ANOTHER process holds the device in a different format
//! (avfoundation: "audio format is not supported") — the in-app callers
//! release the renderer's own captures first.

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use ts_rs::TS;

use sundayrec_core::capture::{parse_input_channel_count, MAC_INPUT_QUEUE, MAC_INPUT_RTBUFSIZE};
use sundayrec_core::device_match::find_best_device_match;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::levels::parse_ametadata_peak;

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::spawn_ffmpeg;
use crate::recorder::engine::current_platform;

/// One channel's measured max peak over the scan window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ChannelPeak.ts")]
#[serde(rename_all = "camelCase")]
pub struct ChannelPeak {
    /// 1-based channel number as astats reports it (channel 1 = input 1).
    pub channel: u32,
    /// Max peak (dBFS) seen during the scan; −120 = effectively silent.
    pub peak_db: f64,
}

/// Resolve the configured device to the raw platform input token (`<idx>` on
/// mac/linux — the caller's argv builder adds the `:` — `<name>` on Windows).
async fn resolve_token(device_name: &str) -> AppResult<String> {
    let inv = enumerate_ffmpeg_devices().await?;
    let Some(dev) = find_best_device_match(&inv.audio_inputs, device_name) else {
        return Err(AppError::Validation(format!(
            "fant ikke lydenheten «{device_name}»"
        )));
    };
    Ok(match current_platform() {
        Platform::MacOS | Platform::Linux => dev.index.map(|i| i.to_string()).unwrap_or_default(),
        Platform::Windows => dev.name.clone(),
    })
}

fn input_args(token: &str) -> Vec<String> {
    match current_platform() {
        Platform::MacOS | Platform::Linux => vec![
            "-f".into(),
            "avfoundation".into(),
            "-thread_queue_size".into(),
            MAC_INPUT_QUEUE.into(),
            "-rtbufsize".into(),
            MAC_INPUT_RTBUFSIZE.into(),
            "-i".into(),
            format!(":{token}"),
        ],
        Platform::Windows => vec![
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            format!("audio={token}"),
        ],
    }
}

/// Spawn ffmpeg with `args`, wait bounded, and return the full stderr blob.
async fn run_collect_stderr(args: &[String], bound_secs: u64) -> AppResult<String> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = spawn_ffmpeg(&arg_refs).await?;
    let drain = child.stderr.take().map(|mut stderr| {
        tauri::async_runtime::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    if tokio::time::timeout(std::time::Duration::from_secs(bound_secs), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    Ok(match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    })
}

/// The device's REAL input channel count (a blink-open + banner parse).
pub async fn probe_input_channels(device_name: &str) -> AppResult<u32> {
    let token = resolve_token(device_name).await?;
    let mut args = vec!["-hide_banner".to_string()];
    args.extend(input_args(&token));
    args.extend(
        ["-t", "0.05", "-f", "null", "-"]
            .into_iter()
            .map(String::from),
    );
    let stderr = run_collect_stderr(&args, 15).await?;
    parse_input_channel_count(&stderr).ok_or_else(|| {
        AppError::Recording(format!(
            "kunne ikke lese kanaltall (enheten kan være i bruk i et annet format): {}",
            stderr.lines().last().unwrap_or("")
        ))
    })
}

/// Scan every input channel's peak over `secs` seconds (null muxer — nothing is
/// written to disk). Uses the recorder's own batched astats chain so the scan
/// can never firehose stderr.
pub async fn scan_channel_peaks(device_name: &str, secs: u32) -> AppResult<Vec<ChannelPeak>> {
    let secs = secs.clamp(1, 15);
    let token = resolve_token(device_name).await?;
    let mut args = vec!["-hide_banner".to_string()];
    args.extend(input_args(&token));
    // The recorder's own batched levels chain (correct stderr sink per
    // platform, capped print rate — a scan can never firehose stderr).
    let levels = sundayrec_core::ffmpeg::build_levels_detect_filter(current_platform());
    args.extend(
        ["-af", &levels, "-t", &secs.to_string(), "-f", "null", "-"]
            .into_iter()
            .map(String::from),
    );
    let stderr = run_collect_stderr(&args, u64::from(secs) + 15).await?;

    let mut peaks: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    for line in stderr.lines() {
        if let Some((chan, db)) = parse_ametadata_peak(line) {
            let entry = peaks.entry(u32::from(chan)).or_insert(f64::NEG_INFINITY);
            if db > *entry {
                *entry = db;
            }
        }
    }
    if peaks.is_empty() {
        return Err(AppError::Recording(format!(
            "skann ga ingen kanaldata (enheten kan være i bruk i et annet format): {}",
            stderr.lines().last().unwrap_or("")
        )));
    }
    Ok(peaks
        .into_iter()
        .map(|(channel, peak_db)| ChannelPeak {
            channel,
            peak_db: if peak_db.is_finite() { peak_db } else { -120.0 },
        })
        .collect())
}
