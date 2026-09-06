//! Composing [`RecordingOpts`] from the persisted [`Settings`] — the ONE place
//! a recording's save folder, filename and formats are decided, shared by the
//! manual path (`commands::recorder::plan_recording_opts`) and the scheduler.
//!
//! It lived in `scheduler/mod.rs` until v0.15, which made the manual recording
//! path depend on the scheduler module for something that has nothing to do
//! with a schedule. It is here, beside the engine that consumes the opts, so
//! the schedule can become an optional add-on one day without the button on
//! the Home page pulling the scheduler in. Pure composition: no timers, no
//! clock beyond the filename's `now`, no schedule types.

use chrono::Local;
use tauri::AppHandle;

use sundayrec_core::filename::{build_filename, FilenameParams};
use sundayrec_core::settings::{FileFormat, Settings};

use crate::error::AppResult;
use crate::recorder::engine::RecordingOpts;

/// Build [`RecordingOpts`] for a recording — manual OR scheduled — from the
/// persisted settings. Resolves the save folder (creating it), names the file
/// via the core [`build_filename`], and maps the audio-processing settings the
/// slim Tauri `RecordingOpts` carries.
///
/// `custom_name` / `max_minutes` are what the two callers differ in: the
/// manual path passes what the operator typed (and 0 = no auto-stop unless the
/// setting says so); the scheduler passes the slot's name and its stop-derived
/// cap. `video_override` is the Home video toggle (local UI state, not
/// persisted) — `None` means "the setting decides", which is the scheduler's
/// case.
pub(crate) fn build_opts(
    app: &AppHandle,
    settings: &Settings,
    custom_name: Option<&str>,
    max_minutes: u32,
    // `Some(b)` overrides the persisted `video_enabled` (the Home video toggle is
    // local UI state that isn't persisted); `None` uses the setting (scheduler).
    video_override: Option<bool>,
) -> AppResult<RecordingOpts> {
    let folder = crate::save_folder::resolve(app, settings.save_folder.as_deref())?;
    std::fs::create_dir_all(&folder)?;

    // Video is on when the user wants it (override, else the setting) AND a camera
    // is actually configured. When video is on the main file MUST be a video
    // container (mp4) — an audio container like .wav can't hold a video stream, so
    // ffmpeg would drop the camera and silently record audio-only (the ".wav
    // instead of .mp4 / no video" bug). The chosen audio `format` then only
    // governs the SEPARATE audio sidecar; audio-only recordings still use it.
    let camera_configured =
        settings.video_device_name.is_some() || settings.video_device_index.is_some();
    let video_on = video_override.unwrap_or(settings.video_enabled) && camera_configured;
    // Video recordings use THE container (mp4 — v0.15 made it a constant, see
    // `sundayrec_core::capture::RECORDING_VIDEO_CONTAINER`); audio recordings
    // use the chosen audio format.
    let main_ext = if video_on {
        sundayrec_core::capture::RECORDING_VIDEO_CONTAINER
    } else {
        format_ext(settings.format)
    };
    let fname = build_filename(&FilenameParams {
        format: main_ext,
        pattern: settings.filename_pattern,
        custom_name,
        // church-calendar name not ported yet → falls back to "gudstjeneste".
        church_name: None,
        split_timestamp: None,
        now: Local::now().naive_local(),
    });
    let output_path = folder.join(fname).to_string_lossy().into_owned();
    // Never overwrite a same-day recording: bump to `_2`, `_3`, … if the chosen
    // filename already exists on disk (pure suffix logic in core; `Path::exists`
    // is the only I/O seam).
    let output_path = sundayrec_core::filename::make_unique_path(&output_path, |p| {
        std::path::Path::new(p).exists()
    });

    Ok(RecordingOpts {
        audio_device_name: settings.device_name.clone().unwrap_or_default(),
        video_device_name: if video_on {
            settings.video_device_name.clone()
        } else {
            None
        },
        output_path,
        stop_on_silence: settings.stop_on_silence,
        silence_threshold_db: Some(settings.silence_threshold),
        silence_timeout_minutes: settings.silence_timeout_minutes.max(1) as u32,
        channel_mode: settings.channels,
        input_channel_l: settings.input_channel_l,
        input_channel_r: settings.input_channel_r,
        // Auto (native) → None (omit -ar, no resample → no choppiness); explicit
        // modes → Some(hz). The legacy `sample_rate: i32` field is NOT used.
        sample_rate: settings.resolved_sample_rate(),
        bitrate_kbps: settings.bitrate_kbps(),
        split_minutes: settings.split_minutes.max(0) as u32,
        manual_max_minutes: max_minutes,
        // The overlay L/R meters + waveform are driven by THIS backend `astats`
        // telemetry (`recording://levels`) instead of a second getUserMedia mic
        // stream. Opening the mic twice (ffmpeg + getUserMedia) made macOS re-mux
        // the shared device and drop samples → choppy capture; ffmpeg's own astats
        // reads the already-captured signal, so the mic is opened exactly once. The
        // engine reader drains stderr, so the astats lines don't back-pressure the
        // capture. Always on (v0.15): the `showLiveLevels` setting had a reader
        // but no control, so nobody could turn the meters off — the constant
        // says so honestly. The engine's `live_levels` switch itself stays: the
        // test recording and the soak turn it off on purpose.
        live_levels: true,
        keep_separate_audio: settings.keep_separate_audio,
        // The separate audio sidecar follows the ONE audio format the app lets
        // the operator choose (v0.15: `separateAudioFormat` left — it had no
        // control, and the only non-default value in the field came from the
        // legacy import seeding it from this same `format`).
        separate_audio_format: format_ext(settings.format).to_string(),
        // (Resolution / frame rate / codec / encoder: the recording constants in
        // `sundayrec_core::capture`, read by the engine — v0.15.)
        // Windows: force legacy DirectShow audio instead of cpal (WASAPI/ASIO).
        classic_directshow: settings.classic_directshow,
        // Escape hatch: force legacy ffmpeg audio capture instead of the native
        // cpal engine (removable once the rig has verified 0 % loss).
        classic_ffmpeg_audio: settings.classic_ffmpeg_audio,
        // Resolved server-side by the recorder's camera-mode probe at start.
        video_input: None,
    })
}

pub(crate) fn format_ext(f: FileFormat) -> &'static str {
    match f {
        FileFormat::Mp3 => "mp3",
        FileFormat::Wav => "wav",
        FileFormat::Flac => "flac",
        FileFormat::Aac => "aac",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ext_maps_every_variant() {
        assert_eq!(format_ext(FileFormat::Mp3), "mp3");
        assert_eq!(format_ext(FileFormat::Wav), "wav");
        assert_eq!(format_ext(FileFormat::Flac), "flac");
        assert_eq!(format_ext(FileFormat::Aac), "aac");
    }
}
