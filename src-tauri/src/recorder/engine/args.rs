//! Pure ffmpeg argument shaping for the recorder: the unified capture argv,
//! the separate-audio extract argv, and the device token. Split out of
//! `engine.rs`; see the parent module docs.

use sundayrec_core::capture::{build_unified_capture_args, CaptureOpts};
use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::settings::ChannelMode;

use super::payloads::RecordingOpts;

/// Map the running OS to the core [`Platform`] enum. Public for the recorder's
/// consumers (e.g. `test_recording`); the logic lives in [`crate::util`].
pub fn current_platform() -> Platform {
    crate::util::detect_platform()
}

/// Build the ffmpeg record arguments for `opts` against a resolved audio device
/// (and optional video device), on `platform`, writing to `output_path`. Pure
/// wrapper over the core builder so argument shaping is unit-tested without a
/// process. `output_path` is passed separately so the supervisor can build args
/// for each reconnect/split segment without mutating `opts`.
pub fn build_record_args(
    platform: Platform,
    audio: &FfmpegDevice,
    video: Option<&FfmpegDevice>,
    opts: &RecordingOpts,
    output_path: &str,
) -> Vec<String> {
    let audio_token = device_token(audio);
    let video_token = video.map(device_token);
    let capture = CaptureOpts {
        stop_on_silence: opts.stop_on_silence,
        silence_threshold_db: opts.silence_threshold_db,
        framerate: sundayrec_core::capture::RECORDING_FRAMERATE,
        channel_mode: opts.channel_mode,
        input_channel_l: opts.input_channel_l,
        input_channel_r: opts.input_channel_r,
        sample_rate: opts.sample_rate,
        bitrate_kbps: opts.bitrate_kbps,
        live_levels: opts.live_levels,
        // Video recordings ALSO write a low-fps preview JPEG (deadlock-proof file
        // sink) the UI polls for a live image while recording.
        preview_jpg: video.map(|_| recording_preview_path().to_string_lossy().into_owned()),
        // The probed camera mode (resolved in `start`); pins a size/rate the
        // device actually advertises so avfoundation opens the camera.
        video_input: opts.video_input,
        // v0.15: codec + encoder are constants (H.264; VideoToolbox where the
        // platform has it — the builder gates `hw_accel` to macOS itself).
        video_codec: sundayrec_core::capture::RECORDING_VIDEO_CODEC,
        hw_accel: sundayrec_core::capture::RECORDING_HW_ACCEL,
    };
    build_unified_capture_args(
        platform,
        video_token.as_deref(),
        &audio_token,
        output_path,
        &capture,
    )
}

/// Shared path of the live in-recording preview JPEG: a single file in the OS temp
/// dir that the recording ffmpeg auto-overwrites (`-update 1`) ~4×/s for video
/// recordings, and the `recording_preview_frame` command reads. One fixed path is
/// fine — at most one recording runs at a time.
pub fn recording_preview_path() -> std::path::PathBuf {
    std::env::temp_dir().join("sundayrec-recording-preview.jpg")
}

/// The addressable token for a device: the avfoundation index (mac) when known,
/// otherwise the dshow name (Windows).
pub(super) fn device_token(d: &FfmpegDevice) -> String {
    match d.index {
        Some(i) => i.to_string(),
        None => d.name.clone(),
    }
}

/// Build the one-shot ffmpeg args that extract a standalone audio file from a
/// finished video container: `ffmpeg -i <src> -vn -map 0:a:0 <audio_encode_args>
/// -y <dst>`. The encode args come from the SHARED [`audio_encode_args`] seam
/// (codec from the sidecar extension, channels from `channel_mode`, sample-rate +
/// bitrate from the recording's opts) so the sidecar matches the recording's
/// chosen audio settings. Pure so the argument shape is unit-tested without a
/// process.
pub(super) fn build_separate_audio_args(src: &str, dst: &str, opts: &RecordingOpts) -> Vec<String> {
    let sep_ext = opts.separate_audio_format.trim_start_matches('.');
    let channels = match opts.channel_mode {
        ChannelMode::Stereo => 2,
        _ => 1,
    };
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        src.to_string(),
        // Drop video, take only the first audio stream.
        "-vn".into(),
        "-map".into(),
        "0:a:0".into(),
    ];
    args.extend(sundayrec_core::capture::audio_encode_args(
        sep_ext,
        channels,
        opts.sample_rate,
        opts.bitrate_kbps,
    ));
    args.push("-y".into());
    args.push(dst.to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: None,
            output_path: "/tmp/rec.m4a".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            channel_mode: ChannelMode::Stereo,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: Some(48_000),
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

    #[test]
    fn recording_preview_path_is_a_stable_temp_jpeg() {
        let p = recording_preview_path();
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("jpg"),
            "the in-recording preview is a JPEG file sink (deadlock-proof, not a pipe)"
        );
        // Stable across calls (one fixed path; at most one recording at a time).
        assert_eq!(p, recording_preview_path());
    }

    #[test]
    fn build_record_args_audio_only_mac_uses_index_token() {
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let args = build_record_args(Platform::MacOS, &audio, None, &opts(), "/tmp/rec.m4a");
        assert!(args.iter().any(|a| a == ":1"), "got: {args:?}");
        assert_eq!(args.last().unwrap(), "/tmp/rec.m4a");
        assert!(
            !args.iter().any(|a| a == "-c:v"),
            "audio-only → no video codec"
        );
    }

    #[test]
    fn build_record_args_uses_passed_output_path_not_opts() {
        // The supervisor builds per-segment args with a fresh path.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &opts(), "/tmp/rec_r1.m4a");
        assert_eq!(args.last().unwrap(), "/tmp/rec_r1.m4a");
    }

    #[test]
    fn build_record_args_windows_uses_device_name_token() {
        let audio = FfmpegDevice::new("Yamaha AG06", "dshow", None);
        let video = FfmpegDevice::new("Logitech BRIO", "dshow", None);
        let args = build_record_args(
            Platform::Windows,
            &audio,
            Some(&video),
            &opts(),
            "/tmp/rec.mp4",
        );
        assert!(args.iter().any(|a| a == "audio=Yamaha AG06"));
        assert!(args.iter().any(|a| a == "video=Logitech BRIO"));
        let af = args
            .iter()
            .position(|a| a == "-af")
            .map(|i| args[i + 1].clone())
            .unwrap();
        assert!(af.contains("aresample=async=1000:first_pts=0"));
    }

    #[test]
    fn device_token_prefers_index_then_name() {
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "avfoundation", Some(2))),
            "2"
        );
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "dshow", None)),
            "Mic"
        );
    }

    #[test]
    fn build_record_args_mono_has_no_stereo_channel_flag() {
        let mut o = opts();
        o.channel_mode = ChannelMode::MonoMix;
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/mono.m4a");
        // Mono maps to `-ac 1`; stereo would request 2 channels.
        let ac = args
            .iter()
            .position(|a| a == "-ac")
            .map(|i| args[i + 1].clone());
        assert_eq!(ac.as_deref(), Some("1"), "got: {args:?}");
    }

    #[test]
    fn build_record_args_stereo_requests_two_channels() {
        let mut o = opts();
        o.channel_mode = ChannelMode::Stereo;
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/st.m4a");
        let ac = args
            .iter()
            .position(|a| a == "-ac")
            .map(|i| args[i + 1].clone());
        assert_eq!(ac.as_deref(), Some("2"), "got: {args:?}");
    }

    #[test]
    fn build_record_args_video_on_mac_uses_combined_index_token() {
        // mac avfoundation addresses video+audio as `<videoIdx>:<audioIdx>`.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let video = FfmpegDevice::new("FaceTime HD", "avfoundation", Some(0));
        let args = build_record_args(
            Platform::MacOS,
            &audio,
            Some(&video),
            &opts(),
            "/tmp/av.mp4",
        );
        assert!(args.iter().any(|a| a == "0:1"), "got: {args:?}");
        // A video session encodes a video stream + the A/V-sync CFR lock.
        assert!(args.iter().any(|a| a == "-c:v"), "got: {args:?}");
        assert!(
            args.windows(2).any(|w| w == ["-fps_mode", "cfr"]),
            "video is CFR-locked; got: {args:?}"
        );
        // The mp4 is the PRIMARY output; a video recording also writes the
        // deadlock-proof preview JPEG (file sink, `-update 1`) as the tail —
        // never a MEDIA output on `pipe:1` (the pipe was what could freeze the
        // capture). The one permitted `pipe:1` is the `-progress` channel's: a
        // global flag, tiny, and drained unconditionally by its own reader task.
        assert!(
            args.iter().any(|a| a == "/tmp/av.mp4"),
            "mp4 present; got: {args:?}"
        );
        assert!(
            args.iter()
                .enumerate()
                .filter(|(_, a)| a.as_str() == "pipe:1")
                .all(|(i, _)| i > 0 && args[i - 1] == "-progress"),
            "no MEDIA output on the pipe; got: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["-update", "1"]),
            "preview file sink"
        );
        assert!(
            args.last().unwrap().ends_with(".jpg"),
            "preview JPEG is the tail output; got: {args:?}"
        );
        let mp4 = args.iter().position(|a| a == "/tmp/av.mp4").unwrap();
        let jpg = args.len() - 1;
        assert!(mp4 < jpg, "mp4 finalises before the preview; got: {args:?}");
    }

    #[test]
    fn build_record_args_passes_silence_threshold_to_filter() {
        let mut o = opts();
        o.stop_on_silence = true;
        o.silence_threshold_db = Some(-45);
        let audio = FfmpegDevice::new("Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/s.m4a");
        // The silencedetect filter must carry the requested threshold.
        let joined = args.join(" ");
        assert!(
            joined.contains("silencedetect=noise=-45dB"),
            "expected the -45 dB threshold in the detector, got: {joined}"
        );
    }

    #[test]
    fn build_record_args_off_silence_uses_the_permissive_warn_threshold() {
        // The detector is ALWAYS in the chain (the warning path needs the markers);
        // with stop-on-silence OFF it falls back to the fixed -55 dB warn level
        // rather than any user threshold.
        let mut o = opts();
        o.stop_on_silence = false;
        o.silence_threshold_db = Some(-45);
        let audio = FfmpegDevice::new("Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/s.m4a");
        let joined = args.join(" ");
        assert!(
            joined.contains("silencedetect=noise=-55dB"),
            "off → fixed -55 dB warn detector, got: {joined}"
        );
        assert!(
            !joined.contains("-45dB"),
            "the user threshold must be ignored when stop-on-silence is off"
        );
    }

    #[test]
    fn separate_audio_args_extract_audio_only_to_chosen_format() {
        // A stereo wav sidecar from an mp4: drop video, take audio stream 0, encode
        // to pcm_s16le (wav), stereo, native rate (no -ar), output last after -y.
        let mut o = opts();
        o.video_device_name = Some("FaceTime HD".into());
        o.channel_mode = ChannelMode::Stereo;
        o.sample_rate = None;
        o.separate_audio_format = "wav".into();
        let args = build_separate_audio_args("/tmp/service.mp4", "/tmp/service.wav", &o);
        // Source in, video dropped, first audio stream mapped.
        assert!(args.windows(2).any(|w| w == ["-i", "/tmp/service.mp4"]));
        assert!(args.iter().any(|a| a == "-vn"), "must drop video");
        assert!(args.windows(2).any(|w| w == ["-map", "0:a:0"]));
        // wav → pcm_s16le, no bitrate, stereo, native (no -ar).
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_s16le"]));
        assert!(!args.iter().any(|a| a == "-b:a"), "pcm takes no bitrate");
        assert!(args.windows(2).any(|w| w == ["-ac", "2"]));
        assert!(!args.iter().any(|a| a == "-ar"), "native rate omits -ar");
        // Overwrite + output path always last.
        let n = args.len();
        assert_eq!(args[n - 2], "-y");
        assert_eq!(args.last().unwrap(), "/tmp/service.wav");
    }

    #[test]
    fn separate_audio_args_honour_format_channels_and_rate() {
        // An mp3 mono sidecar at a forced 44.1 kHz with a 256k bitrate.
        let mut o = opts();
        o.channel_mode = ChannelMode::MonoMix;
        o.sample_rate = Some(44_100);
        o.bitrate_kbps = 256;
        o.separate_audio_format = ".mp3".into(); // leading dot tolerated
        let args = build_separate_audio_args("/tmp/x.mp4", "/tmp/x.mp3", &o);
        assert!(args.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
        assert!(args.windows(2).any(|w| w == ["-b:a", "256k"]));
        assert!(args.windows(2).any(|w| w == ["-ac", "1"]), "mono → -ac 1");
        assert!(args.windows(2).any(|w| w == ["-ar", "44100"]));
    }

    #[test]
    fn build_record_args_native_rate_omits_ar() {
        // The anti-choppiness contract: Auto/native sample rate (None) must NOT
        // emit `-ar`, so ffmpeg captures at the device's own rate instead of
        // resampling (forcing a mismatched rate drops samples → choppy audio).
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let mut o = opts();
        o.sample_rate = None;
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/rec.m4a");
        assert!(
            !args.iter().any(|a| a == "-ar"),
            "native rate must omit -ar; got: {args:?}"
        );
    }

    #[test]
    fn build_record_args_forced_rate_emits_ar() {
        // The escape hatch: an explicit rate is honoured (advanced users / fixed
        // interfaces) — only the DEFAULT is native.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let mut o = opts();
        o.sample_rate = Some(44_100);
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/rec.m4a");
        assert!(
            args.windows(2).any(|w| w == ["-ar", "44100"]),
            "forced rate must emit -ar 44100; got: {args:?}"
        );
    }
}
