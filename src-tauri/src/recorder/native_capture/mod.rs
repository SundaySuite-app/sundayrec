//! Native capture engine — Rust owns the audio chain end to end.
//!
//! ## Why this exists
//!
//! v0.5.0 removed every backpressure path around ffmpeg's avfoundation capture
//! and added truth measurement — which then *proved* that avfoundation itself
//! silently drops samples below ffmpeg's observability (5.4 % on the built-in
//! mic, 55–100 % on a Qu-5, splice clicks in every spectrogram). The fix is to
//! stop delegating capture: cpal (CoreAudio on macOS, WASAPI/ASIO on Windows)
//! delivers samples straight to us, and we write the capture WAV ourselves.
//!
//! ```text
//!   cpal input stream ──► RT callback ──► SPSC ring ──► writer thread ──► .wav
//!   (device native fmt,   (convert→f32,                 (f32→s16-LE,
//!    ALL channels)         route L/R,                    250 ms flush +
//!                          meter routed chs)             header patch)
//! ```
//!
//! ffmpeg keeps every *offline* job — delivery transcode (WAV→FLAC/mp3), the
//! pre-roll clip, fragment concat, recovery finalise — and, for a video
//! session, ENCODING the picture (never capturing it, and never the sound).
//! The capture fragments land at the exact paths the ffmpeg path used, so the
//! recovery/concat/delivery pipeline is untouched.
//!
//! Split across:
//! - [`stream`] — the consolidated cpal layer (host open, fuzzy device find,
//!   format negotiation, one typed stream-builder set) shared with the VU
//!   engine in `audio/vu.rs`.
//! - [`writer`] — the WAV writer thread (f32→s16, 250 ms flush + in-place
//!   header patching, byte/frame counters, in-process silence feed).
//! - [`segment`] — the segment runner: one capture-fragment life, mirroring
//!   `engine::run_segment`'s select! arms with native stop semantics.
//! - [`preroll`] — the rolling PRE-ROLL buffer: the same stack writing rotating
//!   segment files, doubling as the `vu://levels` emitter while it holds the
//!   device.
//! - [`video`] — the OPTIONAL video track (C3a): the camera's frames, locked to
//!   this engine's audio clock by the core conformer, encoded by a video-only
//!   ffmpeg into an MKV beside each WAV fragment. The sound never depends on
//!   it. Delivery muxes WAV + MKV into the mp4 (`concat`).
//!
//! Video sessions still run on the ffmpeg engine by default; the native one
//! takes them only behind `SUNDAYREC_NATIVE_VIDEO=1` until C3b retires the old
//! paths.

pub mod preroll;
pub mod segment;
pub mod stream;
pub mod video;
pub mod writer;
