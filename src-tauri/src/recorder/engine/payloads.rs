//! The ts-rs payload types the engine puts on the IPC wire, and the
//! [`RecordingOpts`] it is started with (which never comes FROM the wire — see
//! the type). Split out of `engine.rs`; see the parent module docs.

use serde::{Deserialize, Serialize};
use sundayrec_core::levels::ChannelLevels;
use sundayrec_core::recorder::RecorderState;
use sundayrec_core::settings::ChannelMode;
use ts_rs::TS;

// Doc-link targets only. Not `///` link definitions: ts-rs copies doc comments
// into the generated TS bindings verbatim.
#[cfg(doc)]
use super::{RecorderEngine, FINISHED_EVENT};

/// Payload for [`FINISHED_EVENT`] — where the finished recording landed, so the
/// UI's "open in editor" action can load it straight into the editor.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "RecordingFinished.ts")]
pub struct RecordingFinished {
    /// Absolute path to the finished recording file.
    pub file_path: String,
    /// Whether it is a video (mp4) recording.
    pub has_video: bool,
}

/// Options for [`RecorderEngine::start`].
///
/// Minted in Rust ONLY — by `recorder::opts::build_opts_in`, for the
/// scheduler and for a manual start alike — and it never crosses the IPC
/// boundary in either direction any more (so no ts-rs binding either).
///
/// Deliberately NOT `Deserialize`: every `#[tauri::command]` parameter must
/// be `Deserialize`, so without it no command can take these opts, and
/// `output_path` — the file the engine creates its folder for, captures into
/// and finalises over — cannot come from the webview. Until the fix for
/// finding E1 `start_recording` took exactly this struct, and with it a raw
/// renderer string as the place to write. The compile-time assertion right
/// below the struct turns re-adding the derive into a build error.
///
/// Still `Serialize`, for one reader: the golden tests in
/// `commands/recorder.rs` compare its JSON with the bytes main's renderer
/// used to receive and echo back, captured before the fix.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingOpts {
    /// Stored microphone/mixer name to fuzzy-match against the enumerated audio
    /// devices. Empty → first/default device.
    pub audio_device_name: String,
    /// Stored camera name to match against video devices. `None` → audio-only.
    pub video_device_name: Option<String>,
    /// Absolute output file path the (first) segment is written to.
    pub output_path: String,
    /// User opted into stop-on-silence.
    pub stop_on_silence: bool,
    /// Silence threshold in dB (clamped by the core filter builder).
    pub silence_threshold_db: Option<i32>,
    /// Minutes of continuous silence before stop-on-silence fires (1–120).
    pub silence_timeout_minutes: u32,
    // (v0.15: `framerate`, `video_resolution`, `video_codec` and `video_encoder`
    // left these opts with the Video tab's knobs — they are the constants in
    // `sundayrec_core::capture` now, read where the capture args are built.)
    /// Output channel layout / downmix mode (stereo, mono-L, mono-R, mono-mix).
    pub channel_mode: ChannelMode,
    /// Explicit 0-based device input channel → LEFT output (multi-channel mixers).
    /// `None` keeps the `channel_mode` default routing.
    pub input_channel_l: Option<i32>,
    /// Explicit 0-based device input channel → RIGHT output. See `input_channel_l`.
    pub input_channel_r: Option<i32>,
    /// Capture sample rate in Hz, or `None` to capture at the device's NATIVE
    /// rate (omit `-ar` — the anti-resample / anti-choppiness fix). Resolved from
    /// `Settings::sample_rate_mode` via `resolved_sample_rate()`.
    pub sample_rate: Option<u32>,
    /// Output bitrate in kbps for lossy codecs (mp3/aac); ignored by wav/flac.
    pub bitrate_kbps: u32,
    /// Rotate to a fresh segment every N minutes (0 = off).
    pub split_minutes: u32,
    /// Auto-stop the whole session after N minutes (0 = off).
    pub manual_max_minutes: u32,
    /// Emit the live L/R level meters (`astats`) during capture? When `false`,
    /// the levels filter is dropped to keep capture maximally stable.
    pub live_levels: bool,
    /// For a VIDEO recording, also extract a standalone audio sidecar file next to
    /// the finished video. No-op for audio-only recordings (the main file already
    /// is the audio).
    pub keep_separate_audio: bool,
    /// The extension/container for the separate audio sidecar (e.g. `"wav"`),
    /// chosen from `Settings::separate_audio_format`. Drives the extract codec via
    /// the shared `audio_encode_args` seam.
    pub separate_audio_format: String,
    /// Windows escape hatch: force the legacy ffmpeg DirectShow audio path instead
    /// of the modern cpal (WASAPI/ASIO) capture. Default `false`. No effect on macOS.
    pub classic_directshow: bool,
    /// Escape hatch: force the legacy ffmpeg audio capture (avfoundation) instead
    /// of the native cpal engine. Default `false`. See `Settings::classic_ffmpeg_audio`.
    pub classic_ffmpeg_audio: bool,
    /// The camera INPUT mode the recorder probed at start (a size + framerate the
    /// device actually advertises). NOT sent by the frontend — it's resolved
    /// server-side so avfoundation doesn't reject an unsupported size/rate. `None`
    /// → audio-only, or the probe yielded nothing (legacy 720p guess).
    #[serde(skip)]
    pub video_input: Option<sundayrec_core::capture::VideoCaptureMode>,
}

// `RecordingOpts` must NOT be `Deserialize` (see its doc comment): this does
// not compile if it is.
//
// The trick `static_assertions::assert_not_impl_any!` uses, written out so the
// crate does not grow a dependency for one line. Every type gets
// `AmbiguousIfDeserialize<()>`; a type that is ALSO `DeserializeOwned` gets
// `AmbiguousIfDeserialize<Invalid>` as well, and then `<_>` below has two
// candidates and type inference gives up — a compile error that names this
// line. A plain `T: !Deserialize` bound does not exist in stable Rust.
const _: fn() = || {
    trait AmbiguousIfDeserialize<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfDeserialize<()> for T {}
    #[allow(dead_code)]
    struct Invalid;
    impl<T: serde::de::DeserializeOwned> AmbiguousIfDeserialize<Invalid> for T {}
    let _ = <RecordingOpts as AmbiguousIfDeserialize<_>>::some_item;
};

/// A progress heartbeat sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "RecordingProgress.ts")]
pub struct RecordingProgress {
    /// Total bytes ffmpeg has written to the current segment so far.
    #[ts(type = "number")]
    pub bytes_written: u64,
}

/// Live per-channel peak audio levels (dBFS) sent to the renderer, parsed from
/// the recorder's own ffmpeg `astats` telemetry. Drives the L/R meters in the
/// "Opptaksmodus" overlay. `peak_db_right` is `None` for mono sources.
///
/// Field names mirror [`RecordingProgress`] (no serde rename) → the generated TS
/// binding is `peak_db_left` / `peak_db_right`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "RecordingLevels.ts")]
pub struct RecordingLevels {
    /// Peak level (dBFS) of the left / only channel.
    pub peak_db_left: f64,
    /// Peak level (dBFS) of the right channel, or `null` for mono sources.
    pub peak_db_right: Option<f64>,
}

impl From<ChannelLevels> for RecordingLevels {
    fn from(lv: ChannelLevels) -> Self {
        Self {
            peak_db_left: lv.peak_db_left,
            peak_db_right: lv.peak_db_right,
        }
    }
}

/// A classified recorder error / silence / reconnect notice sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "RecordingEvent.ts")]
pub struct RecordingEvent {
    /// Stable code the UI localises (snake_case, e.g. `device_disconnected`,
    /// `stuck_recording`, `silence_detected`).
    pub code: String,
    /// Human-readable detail for logs / a diagnostics surface.
    pub message: String,
}

/// The `recording://state` payload — the current [`RecorderState`] plus the
/// reconnect attempt count so the UI can show "reconnecting (3/20)".
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "RecorderStatePayload.ts")]
pub struct RecorderStatePayload {
    /// The lifecycle state.
    pub state: RecorderState,
    /// How many reconnects have happened so far this session.
    pub reconnect_count: u32,
    /// Absolute epoch-ms the recording will auto-stop at, or `null` for no
    /// auto-stop. Driven by `manual_max_minutes` at start; live extend/cancel
    /// (`recording_extend_autostop` / `recording_cancel_autostop`) move or clear
    /// it and the UI ticks a countdown to it locally.
    #[ts(type = "number | null")]
    pub scheduled_stop_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_progress_serde_roundtrip() {
        let p = RecordingProgress {
            bytes_written: 2_097_152,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: RecordingProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn state_payload_serde_roundtrip() {
        let p = RecorderStatePayload {
            state: RecorderState::Reconnecting,
            reconnect_count: 3,
            scheduled_stop_ms: Some(1_700_000_000_000),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("reconnecting"));
        let back: RecorderStatePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn recording_levels_serde_uses_snake_case() {
        let lv = RecordingLevels {
            peak_db_left: -12.5,
            peak_db_right: Some(-9.3),
        };
        let json = serde_json::to_string(&lv).unwrap();
        assert!(json.contains("\"peak_db_left\""), "got: {json}");
        assert!(json.contains("\"peak_db_right\""), "got: {json}");
        // Mono → right is null.
        let mono = RecordingLevels {
            peak_db_left: -20.0,
            peak_db_right: None,
        };
        let json = serde_json::to_string(&mono).unwrap();
        assert!(json.contains("\"peak_db_right\":null"), "got: {json}");
    }

    #[test]
    fn recording_levels_from_channel_levels() {
        let lv = RecordingLevels::from(ChannelLevels::peaks(-6.0, Some(-7.0)));
        assert_eq!(lv.peak_db_left, -6.0);
        assert_eq!(lv.peak_db_right, Some(-7.0));
    }

    /// The JSON shape the golden tests compare (the one main's renderer used to
    /// receive from `plan_recording_opts` and echo back). There is no way BACK
    /// any more — `RecordingOpts` is not `Deserialize` (see the type).
    #[test]
    fn recording_opts_serialises_with_snake_case_keys() {
        let o = RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: Some("Logitech BRIO".into()),
            output_path: "/tmp/rec.mp4".into(),
            stop_on_silence: true,
            silence_threshold_db: Some(-50),
            silence_timeout_minutes: 7,
            channel_mode: ChannelMode::MonoL,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: Some(44_100),
            bitrate_kbps: 256,
            split_minutes: 30,
            manual_max_minutes: 120,
            live_levels: true,
            keep_separate_audio: true,
            separate_audio_format: "wav".into(),
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            video_input: None,
        };
        let json = serde_json::to_string(&o).unwrap();
        // The wire shape is the struct's default snake_case keys (no rename_all).
        assert!(json.contains("\"audio_device_name\""), "got: {json}");
        assert!(json.contains("\"manual_max_minutes\""), "got: {json}");
        assert!(
            json.contains("\"output_path\":\"/tmp/rec.mp4\""),
            "got: {json}"
        );
        // The probed camera mode is server-side state, never on the wire.
        assert!(!json.contains("video_input"), "got: {json}");
    }

    #[test]
    fn recording_event_serde_round_trips() {
        let e = RecordingEvent {
            code: "device_disconnected".into(),
            message: "Mister kontakt".into(),
        };
        let back: RecordingEvent =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }
}
