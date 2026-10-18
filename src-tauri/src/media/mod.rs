//! Media subsystem — the bundled ffmpeg/ffprobe sidecar and the async
//! primitives the recorder (Spike B) is built on.
//!
//! `ffmpeg` owns binary resolution (env override → bundled sidecar → PATH) and
//! the `tokio::process` spawn helper used to drive ffmpeg with real-time
//! stderr/stdout streaming and a graceful stdin `q` shutdown; `video_encoder`
//! picks (probes and remembers) the hardware video encoder per machine (C0a).
//! (C3b removed `camera` — the ffmpeg camera-mode probe — and `video_probe`, the
//! one-shot ffmpeg camera grab: the camera is read natively, `crate::camera`.)

pub mod ffmpeg;
pub mod permissions;
pub mod video_encoder;
