//! NDI source-discovery model + the pure rawvideo input-arg builder (R3 NDI).
//!
//! Ported from the Electron `src/main/ndi-receiver.ts` + the `buildNdiInputArgs`
//! branch of `overlay.ts`. The NDI architecture bridges frames from a network
//! source into the streamer's single ffmpeg via a **loopback TCP socket**:
//! libndi receives frames, a TCP server serves the raw bytes, and ffmpeg reads
//! `tcp://127.0.0.1:<port>` with `-f rawvideo`.
//!
//! The parts that are PURE — the discovered-source model, picking the ffmpeg
//! pixel format from the delivered FourCC + the alpha request, and building the
//! `-f rawvideo …` input args for a resolved receiver — live here and are unit
//! tested. The libndi binding + the TCP server itself need the NDI runtime + a
//! rig, so they live in the `src-tauri` seam behind the default-off `ndi`
//! feature (a STUB until the SDK is bundled — see docs/NEEDS-RICHARD.md).
//!
//! # NDI *output* (transmit) — the seam and what's still missing
//!
//! Besides receiving, SundayRec can *advertise its own program feed as an NDI
//! source* so a downstream switcher (vMix/OBS/ProPresenter) can pull it over the
//! LAN. ffmpeg transmits NDI through the **`libndi_newtek` muxer** (`-f
//! libndi_newtek`), which is NOT compiled into the bundled sidecar. Making
//! output actually transmit requires BOTH of the following, neither of which
//! ships today:
//!
//! 1. **The NDI runtime/SDK** installed on the machine (the NewTek/Vizrt NDI
//!    runtime provides `libndi.*`, which the muxer dlopens at run time).
//! 2. **An ffmpeg built with `--enable-libndi`** so the `libndi_newtek` muxer
//!    exists. The bundled sidecar is a stock build WITHOUT it; confirm with
//!    [`ffmpeg_supports_ndi_output`] over `ffmpeg -hide_banner -muxers`.
//!
//! Until both are present, the output path MUST refuse to spawn (the `src-tauri`
//! seam returns the same "NDI SDK not bundled" error as the receiver). What is
//! PURE and hardened *here* — so the eventual wiring is small and correct — is:
//!   - source-name **validation/sanitisation** ([`validate_ndi_source_name`],
//!     [`sanitize_ndi_source_name`]) — NDI names must be non-empty, bounded, and
//!     free of control chars, which corrupt the advertised metadata;
//!   - the **output config** ([`NdiOutputOptions`], serde + ts-rs for the UI);
//!   - the **output-arg builder** ([`build_ndi_output_args`]) producing the
//!     correct `-f libndi_newtek -pix_fmt uyvy422 …` argv;
//!   - the **muxer-detection parser** ([`ffmpeg_supports_ndi_output`]) so the
//!     seam can give a precise "rebuild ffmpeg with `--enable-libndi`" message
//!     instead of a raw ffmpeg failure.
//!
//! Wiring checklist (when the SDK + ffmpeg land): in the `src-tauri` `ndi` seam,
//! (a) run `ffmpeg -muxers` once and gate on [`ffmpeg_supports_ndi_output`];
//! (b) validate the user's source name with [`validate_ndi_source_name`];
//! (c) append [`build_ndi_output_args`] as the LAST ffmpeg output (NDI is an
//! additional sink, alongside RTMP/file — it never replaces them).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ffmpeg::Platform;

/// A source advertising on the network, as surfaced to the UI. Mirrors the
/// Electron `NdiSourceInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/NdiSource.ts")]
#[serde(rename_all = "camelCase")]
pub struct NdiSource {
    /// Full advertised name, e.g. `"STUDIO-PC (ProPresenter Output 1)"`.
    pub name: String,
    /// Resolvable `IP:port` (or a LOCAL HOST marker for same-machine sources).
    pub address: String,
}

/// The ffmpeg rawvideo pixel format we read NDI frames as. UYVY is the smaller,
/// no-alpha 4:2:2 format; BGRA carries the alpha channel (for chroma/alpha-key
/// compositing). Mirrors the Electron `'uyvy422' | 'bgra'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/NdiPixFmt.ts")]
#[serde(rename_all = "lowercase")]
pub enum NdiPixFmt {
    Uyvy422,
    Bgra,
}

impl NdiPixFmt {
    /// The ffmpeg `-pix_fmt` token.
    pub fn ffmpeg_token(self) -> &'static str {
        match self {
            NdiPixFmt::Uyvy422 => "uyvy422",
            NdiPixFmt::Bgra => "bgra",
        }
    }
}

/// NDI FourCC codes we recognise (subset of libndi's set). Used by [`pick_pix_fmt`]
/// to choose the ffmpeg pixel format from what a frame actually delivered.
///
/// FourCC = `a | b<<8 | c<<16 | d<<24` over the ASCII chars (the libndi / Windows
/// `MAKEFOURCC` convention), so the little-endian byte order spells the code.
/// These MUST be exact — a real NDI sender stamps the value into the frame header
/// and a receiver decodes the colour plane from it.
pub mod fourcc {
    pub const UYVY: u32 = 0x5956_5955; // bytes U Y V Y
    pub const BGRA: u32 = 0x4152_4742; // bytes B G R A
    pub const BGRX: u32 = 0x5852_4742; // bytes B G R X

    /// Decode a FourCC back to its 4 ASCII bytes (LSB-first) — for tests/debug.
    pub const fn to_ascii(code: u32) -> [u8; 4] {
        code.to_le_bytes()
    }
}

/// Pick the ffmpeg pixel format from the delivered frame's FourCC, falling back
/// to "what we asked for" (alpha→BGRA, else UYVY) when the FourCC isn't one we
/// know — mirrors the Electron `pickPixFmt`. Preferring the actual FourCC over
/// the request avoids a misaligned colour decode when libndi delivers something
/// other than we asked for.
pub fn pick_pix_fmt(delivered_fourcc: u32, want_alpha: bool) -> NdiPixFmt {
    match delivered_fourcc {
        fourcc::BGRA | fourcc::BGRX => NdiPixFmt::Bgra,
        fourcc::UYVY => NdiPixFmt::Uyvy422,
        _ => {
            if want_alpha {
                NdiPixFmt::Bgra
            } else {
                NdiPixFmt::Uyvy422
            }
        }
    }
}

/// A receiver resolved enough to wire ffmpeg: the loopback port it serves on,
/// the pixel format, the frame size (from the first frame), and the framerate.
/// The renderer-facing mirror of the Electron `ReceiverHandle`'s data fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/NdiReceiverInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct NdiReceiverInfo {
    /// Loopback TCP port ffmpeg connects to.
    pub port: u16,
    pub pix_fmt: NdiPixFmt,
    pub width: u32,
    pub height: u32,
    /// Best-effort framerate (libndi reports, or a 30 fallback the seam sets).
    pub framerate: u32,
}

/// Build the ffmpeg `-f rawvideo …` input args for a resolved NDI receiver. Pure
/// — mirrors the Electron `buildNdiInputArgs`. `default_framerate` is the
/// stream's base framerate used when the receiver reports `framerate == 0`.
///
/// ffmpeg reads the loopback TCP server the receiver exposes; the receiver
/// pushes raw bytes in the negotiated pixel format, so we must tell ffmpeg the
/// exact size + pixel format (rawvideo carries no header).
pub fn build_ndi_input_args(rt: &NdiReceiverInfo, default_framerate: u32) -> Vec<String> {
    let fr = if rt.framerate == 0 {
        default_framerate
    } else {
        rt.framerate
    };
    vec![
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        rt.pix_fmt.ffmpeg_token().into(),
        "-s".into(),
        format!("{}x{}", rt.width, rt.height),
        "-framerate".into(),
        fr.to_string(),
        "-i".into(),
        format!("tcp://127.0.0.1:{}", rt.port),
    ]
}

/// Filter discovered sources by a (case-insensitive) name substring, the
/// "best match" helper the overlay UI uses to reconcile a saved source name
/// against what's currently advertising. Exact name match wins; otherwise the
/// first case-insensitive substring hit; otherwise `None`.
pub fn match_source<'a>(sources: &'a [NdiSource], wanted: &str) -> Option<&'a NdiSource> {
    let w = wanted.trim();
    if w.is_empty() {
        return None;
    }
    if let Some(exact) = sources.iter().find(|s| s.name == w) {
        return Some(exact);
    }
    let lw = w.to_lowercase();
    sources.iter().find(|s| s.name.to_lowercase().contains(&lw))
}

// ── NDI output (transmit) — source-name validation ───────────────────────────

/// The maximum length we allow for an advertised NDI source name. NDI itself
/// carries the name as a UTF-8 string with no hard public limit, but switchers
/// truncate long names and the name rides in the mDNS/discovery metadata, so we
/// keep it comfortably bounded. 128 chars is far longer than any real label and
/// well under any practical wire limit.
pub const NDI_SOURCE_NAME_MAX_LEN: usize = 128;

/// Why an NDI source name was rejected. Stable (snake_case) so the renderer can
/// map to a localized message without parsing free text — mirrors the
/// [`crate::streaming::StreamKeyError`] idiom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/NdiSourceNameError.ts")]
#[serde(rename_all = "snake_case")]
pub enum NdiSourceNameError {
    /// Empty or whitespace-only after trimming.
    Empty,
    /// Longer than [`NDI_SOURCE_NAME_MAX_LEN`] characters (after trimming).
    TooLong,
    /// Contained a control character (newline, NUL, …) — these corrupt the
    /// advertised discovery metadata and confuse receivers.
    HasControlChar,
}

/// Validate a user-chosen NDI source name. Pure — no network, no SDK. Rejects
/// the three things that break NDI advertisement: empty names (nothing to
/// discover), over-long names (truncated/garbled in switchers), and embedded
/// control characters (corrupt the discovery string). We deliberately do NOT
/// restrict the printable charset: NDI names routinely contain spaces,
/// parentheses and non-ASCII letters (e.g. `"KIRKE-PC (Hovedkamera)"`).
pub fn validate_ndi_source_name(name: &str) -> Result<(), NdiSourceNameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NdiSourceNameError::Empty);
    }
    // Control chars anywhere (including inside, not just the trimmed edges) are
    // always a corruption risk in the advertised string.
    if name.chars().any(|c| c.is_control()) {
        return Err(NdiSourceNameError::HasControlChar);
    }
    if trimmed.chars().count() > NDI_SOURCE_NAME_MAX_LEN {
        return Err(NdiSourceNameError::TooLong);
    }
    Ok(())
}

/// Best-effort coercion of an arbitrary string into a *valid* NDI source name:
/// strip control chars, collapse the result's surrounding whitespace, and
/// truncate to [`NDI_SOURCE_NAME_MAX_LEN`] characters. Returns `None` only when
/// nothing usable survives (e.g. the input was empty or all control chars).
///
/// Useful at the UI boundary to suggest a cleaned name rather than rejecting the
/// user outright. The result is guaranteed to pass [`validate_ndi_source_name`].
pub fn sanitize_ndi_source_name(name: &str) -> Option<String> {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Truncate on a char boundary, not a byte boundary.
    let out: String = trimmed.chars().take(NDI_SOURCE_NAME_MAX_LEN).collect();
    Some(out)
}

// ── NDI output (transmit) — config + arg builder ─────────────────────────────

/// Options for advertising SundayRec's program feed as an NDI output. The
/// renderer-facing mirror surfaced to the UI; the `src-tauri` seam consumes it
/// to build the ffmpeg output args (once an NDI-capable ffmpeg is present).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/NdiOutputOptions.ts")]
#[serde(rename_all = "camelCase")]
pub struct NdiOutputOptions {
    /// The advertised source name other machines discover. Validate with
    /// [`validate_ndi_source_name`] before use.
    pub source_name: String,
    /// The pixel format frames are transmitted in. `uyvy422` is the standard,
    /// bandwidth-friendly 4:2:2 NDI format; `bgra` carries alpha (rarely needed
    /// for a program feed).
    pub pix_fmt: NdiPixFmt,
}

impl Default for NdiOutputOptions {
    fn default() -> Self {
        Self {
            source_name: "SundayRec".to_string(),
            // UYVY 4:2:2 is the conventional NDI program-feed format — half the
            // bandwidth of BGRA and what receivers expect by default.
            pix_fmt: NdiPixFmt::Uyvy422,
        }
    }
}

/// Build the ffmpeg `-f libndi_newtek …` OUTPUT args for an NDI sink. Pure — it
/// only builds the argv; it never spawns ffmpeg and never touches the SDK.
///
/// The output of `libndi_newtek` is the **source name** (not a path), so the
/// name is the final positional argument. We force the pixel format because NDI
/// transmits a specific raw format; leaving it to ffmpeg's default risks a
/// format the muxer rejects. `-flags +global_header` is harmless for the
/// rawish NDI stream and matches the muxer's documented usage.
///
/// IMPORTANT: this argv is *correct* but will FAIL on the bundled sidecar, which
/// lacks the `libndi_newtek` muxer. Gate on [`ffmpeg_supports_ndi_output`]
/// first. The caller is expected to have validated `opts.source_name` with
/// [`validate_ndi_source_name`]; this builder uses the name verbatim.
pub fn build_ndi_output_args(opts: &NdiOutputOptions) -> Vec<String> {
    vec![
        "-pix_fmt".into(),
        opts.pix_fmt.ffmpeg_token().into(),
        "-f".into(),
        "libndi_newtek".into(),
        opts.source_name.clone(),
    ]
}

// ── NDI output (transmit) — ffmpeg capability detection ──────────────────────

/// The muxer name ffmpeg uses for NDI output. Present only when ffmpeg was built
/// `--enable-libndi`.
pub const NDI_MUXER_NAME: &str = "libndi_newtek";

/// Parse `ffmpeg -muxers` output and report whether the `libndi_newtek` muxer is
/// available — i.e. whether *this* ffmpeg can transmit NDI at all. Pure: the
/// caller runs `ffmpeg -hide_banner -muxers` and passes the captured stdout.
///
/// `ffmpeg -muxers` prints a header block, a ` --` separator line, then one
/// muxer per line as:
/// ```text
///  E libndi_newtek   Network Device Interface (NDI) output
/// ```
/// where the leading flag column is ` E` (muxers are encode-only). We match the
/// muxer *token* in the second whitespace-delimited column so a coincidental
/// mention of the name inside a description never yields a false positive.
pub fn ffmpeg_supports_ndi_output(muxers_output: &str) -> bool {
    muxers_output.lines().any(|line| {
        let mut cols = line.split_whitespace();
        // First column is the flag(s) (e.g. "E"); the muxer name is the second.
        match (cols.next(), cols.next()) {
            (Some(flags), Some(name)) => {
                // The flag column for a real muxer row is short and contains 'E'
                // (encode). This skips the header/separator lines, which either
                // have no second column or don't carry an 'E' flag column.
                flags.len() <= 3 && flags.contains('E') && name == NDI_MUXER_NAME
            }
            _ => false,
        }
    })
}

// ── NDI output (transmit) — the runtime-dlopen SENDER path ───────────────────
//
// The `libndi_newtek` ffmpeg muxer above needs an ffmpeg built `--enable-libndi`,
// which the bundled sidecar is NOT. The robust alternative (what OBS-class apps
// ship) is to talk to the NDI runtime DIRECTLY: have ffmpeg decode the camera to
// raw UYVY422 frames, then hand each frame to `libndi` over FFI. The `src-tauri`
// `ndi/sender` seam dlopens `libndi` at runtime; these are its pure helpers.

/// Bytes in one packed UYVY422 frame: 2 bytes per pixel (4 bytes per 2-pixel
/// macropixel). The sender hands libndi exactly this many bytes per frame, so
/// the math must match the `-pix_fmt uyvy422` ffmpeg produces — a mismatch would
/// misalign the colour plane (or read past the buffer).
pub fn uyvy_frame_bytes(width: u32, height: u32) -> usize {
    width as usize * height as usize * 2
}

/// Ordered candidate paths to `dlopen` for the NDI runtime, given the platform
/// and any `NDI_RUNTIME_DIR_V*` directories the SDK sets in the environment.
///
/// The NDI SDK convention is to read `NDI_RUNTIME_DIR_V6` / `V5` and load the
/// library from there; we also try the conventional install locations and the
/// bare library name (letting the dynamic loader's search path resolve it). The
/// caller tries each in order and keeps the first that loads.
pub fn libndi_library_candidates(platform: Platform, env_dirs: &[String]) -> Vec<String> {
    let (file, fallbacks): (&str, &[&str]) = match platform {
        Platform::MacOS => (
            "libndi.dylib",
            &[
                "/usr/local/lib/libndi.dylib",
                "/usr/local/lib/libndi.4.dylib",
                "libndi.dylib",
            ],
        ),
        Platform::Windows => (
            "Processing.NDI.Lib.x64.dll",
            &["Processing.NDI.Lib.x64.dll"],
        ),
        Platform::Linux => (
            "libndi.so",
            &[
                "/usr/lib/libndi.so",
                "/usr/lib/libndi.so.5",
                "/usr/local/lib/libndi.so",
                "libndi.so",
            ],
        ),
    };
    let mut out: Vec<String> = Vec::new();
    // The SDK-pointed runtime dirs win — most precise.
    for dir in env_dirs.iter().filter(|d| !d.trim().is_empty()) {
        let sep = if dir.ends_with('/') || dir.ends_with('\\') {
            ""
        } else {
            "/"
        };
        out.push(format!("{dir}{sep}{file}"));
    }
    for f in fallbacks {
        out.push((*f).to_string());
    }
    // De-dup while preserving order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.clone()));
    out
}

/// ffmpeg args to decode `device_token`'s camera into raw packed UYVY422 frames
/// on stdout, scaled to EXACTLY `width`×`height` at `fps` — the stream the dlopen
/// NDI sender pumps frame-by-frame. Audio is dropped (NDI audio is a later step).
/// `scale` guarantees the frame byte count equals [`uyvy_frame_bytes`] even if the
/// camera ignores the requested size, so the sender can never read a short/long
/// frame.
pub fn build_ndi_rawframe_args(
    platform: Platform,
    device_token: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let size = format!("{width}x{height}");
    let (fmt, input): (&str, String) = match platform {
        Platform::MacOS => ("avfoundation", device_token.to_string()),
        Platform::Windows => ("dshow", format!("video={}", device_token.trim_matches('"'))),
        Platform::Linux => ("v4l2", device_token.to_string()),
    };
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-f".into(),
        fmt.into(),
        "-framerate".into(),
        fps.to_string(),
        "-video_size".into(),
        size,
        "-i".into(),
        input,
        "-an".into(),
        "-vf".into(),
        format!("scale={width}:{height},fps={fps}"),
        "-pix_fmt".into(),
        "uyvy422".into(),
        "-f".into(),
        "rawvideo".into(),
        "pipe:1".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx(port: u16, fmt: NdiPixFmt, w: u32, h: u32, fr: u32) -> NdiReceiverInfo {
        NdiReceiverInfo {
            port,
            pix_fmt: fmt,
            width: w,
            height: h,
            framerate: fr,
        }
    }

    #[test]
    fn fourcc_constants_decode_to_their_ascii_codes() {
        // The exact bytes matter: a real sender stamps these into the frame header.
        assert_eq!(&fourcc::to_ascii(fourcc::UYVY), b"UYVY");
        assert_eq!(&fourcc::to_ascii(fourcc::BGRA), b"BGRA");
        assert_eq!(&fourcc::to_ascii(fourcc::BGRX), b"BGRX");
    }

    #[test]
    fn uyvy_frame_bytes_is_two_per_pixel() {
        assert_eq!(uyvy_frame_bytes(1280, 720), 1280 * 720 * 2);
        assert_eq!(uyvy_frame_bytes(1920, 1080), 1920 * 1080 * 2);
        assert_eq!(uyvy_frame_bytes(0, 0), 0);
    }

    #[test]
    fn libndi_candidates_prefer_env_dirs_then_fallbacks_no_dupes() {
        let c = libndi_library_candidates(
            Platform::MacOS,
            &["/opt/ndi/lib".into(), "  ".into(), "/usr/local/lib".into()],
        );
        // Env dirs first (blank skipped), joined with the platform file.
        assert_eq!(c[0], "/opt/ndi/lib/libndi.dylib");
        assert_eq!(c[1], "/usr/local/lib/libndi.dylib");
        // Fallbacks follow; the bare name is present for the loader search path.
        assert!(c.iter().any(|x| x == "libndi.dylib"));
        // No duplicate (the env "/usr/local/lib" path == the fallback path).
        let mut sorted = c.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), c.len(), "candidates must be unique: {c:?}");
    }

    #[test]
    fn libndi_candidates_are_platform_specific() {
        assert!(libndi_library_candidates(Platform::Windows, &[])
            .iter()
            .any(|c| c.contains("Processing.NDI.Lib")));
        assert!(libndi_library_candidates(Platform::Linux, &[])
            .iter()
            .any(|c| c.ends_with("libndi.so")));
    }

    #[test]
    fn rawframe_args_pin_size_drop_audio_and_emit_uyvy_rawvideo() {
        let a = build_ndi_rawframe_args(Platform::MacOS, "0", 1280, 720, 30);
        let j = a.join(" ");
        assert!(j.contains("-f avfoundation"));
        assert!(j.contains("-video_size 1280x720"));
        assert!(j.contains("-an"), "NDI raw pump is video-only for now");
        assert!(j.contains("scale=1280:720,fps=30"));
        assert!(j.contains("-pix_fmt uyvy422 -f rawvideo pipe:1"));
        // Windows wraps the device in video=… (quotes stripped).
        let w = build_ndi_rawframe_args(Platform::Windows, "\"Cam\"", 640, 480, 25);
        assert!(w.iter().any(|x| x == "video=Cam"));
    }

    // ── pixel-format selection ──
    #[test]
    fn pix_fmt_follows_delivered_fourcc() {
        assert_eq!(pick_pix_fmt(fourcc::BGRA, false), NdiPixFmt::Bgra);
        assert_eq!(pick_pix_fmt(fourcc::BGRX, false), NdiPixFmt::Bgra);
        assert_eq!(pick_pix_fmt(fourcc::UYVY, true), NdiPixFmt::Uyvy422);
    }

    #[test]
    fn pix_fmt_falls_back_to_alpha_request_for_unknown_fourcc() {
        assert_eq!(pick_pix_fmt(0xDEAD_BEEF, true), NdiPixFmt::Bgra);
        assert_eq!(pick_pix_fmt(0xDEAD_BEEF, false), NdiPixFmt::Uyvy422);
    }

    #[test]
    fn pix_fmt_ffmpeg_tokens() {
        assert_eq!(NdiPixFmt::Uyvy422.ffmpeg_token(), "uyvy422");
        assert_eq!(NdiPixFmt::Bgra.ffmpeg_token(), "bgra");
    }

    // ── input args ──
    #[test]
    fn input_args_match_electron_rawvideo_shape() {
        let args = build_ndi_input_args(&rx(54321, NdiPixFmt::Bgra, 1920, 1080, 50), 30);
        assert_eq!(
            args,
            vec![
                "-f",
                "rawvideo",
                "-pix_fmt",
                "bgra",
                "-s",
                "1920x1080",
                "-framerate",
                "50",
                "-i",
                "tcp://127.0.0.1:54321",
            ]
        );
    }

    #[test]
    fn input_args_fall_back_to_default_framerate_when_zero() {
        let args = build_ndi_input_args(&rx(7000, NdiPixFmt::Uyvy422, 1280, 720, 0), 25);
        assert!(args.windows(2).any(|w| w == ["-framerate", "25"]));
        assert!(args.windows(2).any(|w| w == ["-pix_fmt", "uyvy422"]));
    }

    // ── source matching ──
    #[test]
    fn match_source_prefers_exact_then_substring() {
        let sources = vec![
            NdiSource {
                name: "STUDIO-PC (ProPresenter Output 1)".into(),
                address: "10.0.0.5:5961".into(),
            },
            NdiSource {
                name: "LAPTOP (NDI Scan Converter)".into(),
                address: "10.0.0.7:5962".into(),
            },
        ];
        // exact
        assert_eq!(
            match_source(&sources, "LAPTOP (NDI Scan Converter)")
                .unwrap()
                .address,
            "10.0.0.7:5962"
        );
        // case-insensitive substring
        assert_eq!(
            match_source(&sources, "propresenter").unwrap().address,
            "10.0.0.5:5961"
        );
        // no match
        assert!(match_source(&sources, "OBS").is_none());
        // blank
        assert!(match_source(&sources, "  ").is_none());
    }

    // ── source-name validation ──
    #[test]
    fn validate_name_accepts_realistic_names() {
        assert!(validate_ndi_source_name("SundayRec").is_ok());
        assert!(validate_ndi_source_name("KIRKE-PC (Hovedkamera)").is_ok());
        // Non-ASCII letters are fine (NDI names are UTF-8).
        assert!(validate_ndi_source_name("Søndagsmøte – Programfeed").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty_and_blank() {
        assert_eq!(validate_ndi_source_name(""), Err(NdiSourceNameError::Empty));
        assert_eq!(
            validate_ndi_source_name("   "),
            Err(NdiSourceNameError::Empty)
        );
    }

    #[test]
    fn validate_name_rejects_control_chars() {
        assert_eq!(
            validate_ndi_source_name("Studio\nFeed"),
            Err(NdiSourceNameError::HasControlChar)
        );
        assert_eq!(
            validate_ndi_source_name("Studio\u{0}Feed"),
            Err(NdiSourceNameError::HasControlChar)
        );
        assert_eq!(
            validate_ndi_source_name("Tab\tHere"),
            Err(NdiSourceNameError::HasControlChar)
        );
    }

    #[test]
    fn validate_name_rejects_over_long() {
        let long = "a".repeat(NDI_SOURCE_NAME_MAX_LEN + 1);
        assert_eq!(
            validate_ndi_source_name(&long),
            Err(NdiSourceNameError::TooLong)
        );
        // Exactly at the limit is OK.
        let exact = "a".repeat(NDI_SOURCE_NAME_MAX_LEN);
        assert!(validate_ndi_source_name(&exact).is_ok());
    }

    // ── source-name sanitisation ──
    #[test]
    fn sanitize_strips_control_chars_and_trims() {
        assert_eq!(
            sanitize_ndi_source_name("  Studio\nFeed  ").as_deref(),
            Some("StudioFeed")
        );
    }

    #[test]
    fn sanitize_returns_none_for_unrecoverable_input() {
        assert_eq!(sanitize_ndi_source_name(""), None);
        assert_eq!(sanitize_ndi_source_name("   "), None);
        assert_eq!(sanitize_ndi_source_name("\n\t\u{0}"), None);
    }

    #[test]
    fn sanitize_truncates_to_max_len_on_char_boundary() {
        // Multi-byte chars must be truncated by char count, not byte index.
        let input = "é".repeat(NDI_SOURCE_NAME_MAX_LEN + 50);
        let out = sanitize_ndi_source_name(&input).unwrap();
        assert_eq!(out.chars().count(), NDI_SOURCE_NAME_MAX_LEN);
        // And the result is always valid.
        assert!(validate_ndi_source_name(&out).is_ok());
    }

    #[test]
    fn sanitize_output_always_validates() {
        for raw in ["  ok name  ", "weird\u{7}name", "plain"] {
            if let Some(clean) = sanitize_ndi_source_name(raw) {
                assert!(validate_ndi_source_name(&clean).is_ok());
            }
        }
    }

    // ── output config defaults ──
    #[test]
    fn output_options_default_is_uyvy_named_sundayrec() {
        let d = NdiOutputOptions::default();
        assert_eq!(d.source_name, "SundayRec");
        assert_eq!(d.pix_fmt, NdiPixFmt::Uyvy422);
        // The default must itself be a valid name.
        assert!(validate_ndi_source_name(&d.source_name).is_ok());
    }

    // ── output arg builder ──
    #[test]
    fn output_args_have_libndi_muxer_and_name_last() {
        let opts = NdiOutputOptions {
            source_name: "KIRKE-PC (Program)".into(),
            pix_fmt: NdiPixFmt::Uyvy422,
        };
        let args = build_ndi_output_args(&opts);
        assert_eq!(
            args,
            vec![
                "-pix_fmt",
                "uyvy422",
                "-f",
                "libndi_newtek",
                "KIRKE-PC (Program)",
            ]
        );
        // The muxer must be selected and the source name is the final positional.
        assert!(args.windows(2).any(|w| w == ["-f", "libndi_newtek"]));
        assert_eq!(args.last().unwrap(), "KIRKE-PC (Program)");
    }

    #[test]
    fn output_args_honour_alpha_pix_fmt() {
        let opts = NdiOutputOptions {
            source_name: "AlphaFeed".into(),
            pix_fmt: NdiPixFmt::Bgra,
        };
        let args = build_ndi_output_args(&opts);
        assert!(args.windows(2).any(|w| w == ["-pix_fmt", "bgra"]));
    }

    // ── ffmpeg -muxers capability detection ──
    const MUXERS_WITH_NDI: &str = "\
Muxers:
 D. = Demuxing supported
 .E = Muxing supported
 --
  E mp4             MP4 (MPEG-4 Part 14)
  E libndi_newtek   Network Device Interface (NDI) output
  E flv             FLV (Flash Video)
";

    const MUXERS_WITHOUT_NDI: &str = "\
Muxers:
 --
  E mp4             MP4 (MPEG-4 Part 14)
  E flv             FLV (Flash Video)
  E tee             Multiple muxer tee
";

    #[test]
    fn detects_ndi_muxer_when_present() {
        assert!(ffmpeg_supports_ndi_output(MUXERS_WITH_NDI));
    }

    #[test]
    fn reports_no_ndi_muxer_when_absent() {
        assert!(!ffmpeg_supports_ndi_output(MUXERS_WITHOUT_NDI));
        assert!(!ffmpeg_supports_ndi_output(""));
    }

    #[test]
    fn detection_ignores_name_in_description_only() {
        // The token appears only in a description column, not as the muxer name.
        let tricky = "  E flv             writes a libndi_newtek-like container\n";
        assert!(!ffmpeg_supports_ndi_output(tricky));
    }

    #[test]
    fn detection_skips_header_and_separator_lines() {
        let only_headers = "Muxers:\n D. = Demuxing supported\n --\n";
        assert!(!ffmpeg_supports_ndi_output(only_headers));
    }
}
