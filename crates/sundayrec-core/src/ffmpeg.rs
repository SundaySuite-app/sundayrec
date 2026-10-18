//! The ffmpeg-facing platform enum.
//!
//! Until C3b this module also built the ffmpeg capture's audio filters — the
//! Windows dshow A/V drift correction (`aresample=async=…`), the `silencedetect`
//! markers and the live `astats` level meter. Every recording is captured
//! natively now: the drift is the conformer's job ([`crate::video_conform`]),
//! silence is detected in-process ([`crate::silence::SilenceDetector`]) and
//! the meters read the native engine's own peaks, so the builders are gone.
//! What is left is the [`Platform`] the encoder choice, the editor and the
//! media helpers still branch on.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Capture host platform. We only branch on it where the underlying OS stack
/// actually forces a difference (the video encoder, the editor's export).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "Platform.ts")]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
}
