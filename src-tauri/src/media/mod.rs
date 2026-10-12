//! Media subsystem — the bundled ffmpeg/ffprobe sidecar and the async
//! primitives the recorder (Spike B) is built on.
//!
//! `ffmpeg` owns binary resolution (env override → bundled sidecar → PATH) and
//! the `tokio::process` spawn helper used to drive ffmpeg with real-time
//! stderr/stdout streaming and a graceful stdin `q` shutdown. `camera` probes a
//! camera's advertised capture modes for the recorder; `video_probe` is the
//! one-shot diagnose camera check built on the same primitives; `video_encoder`
//! picks (probes and remembers) the hardware video encoder per machine (C0a).

pub mod camera;
pub mod ffmpeg;
pub mod permissions;
pub mod video_encoder;
pub mod video_probe;
