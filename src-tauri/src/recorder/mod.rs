//! Recorder subsystem — the capture engine and its finalisation pipeline.
//!
//! This is the plumbing layer that turns the pure decision logic in
//! `sundayrec-core` (device matching, capture-argument building, the silence
//! watcher, the watchdog + reconnect schedule, the deliverable/fragment model)
//! into a real recording, and emits the Tauri events the renderer renders.
//!
//! ## Which engine captures what (since v0.6.0)
//!
//!   - **Audio-only sessions** capture NATIVELY on both platforms
//!     ([`native_capture`]: cpal → ring → our own WAV writer). ffmpeg measurably
//!     lost samples under avfoundation, so it is no longer in the capture path.
//!   - **Video sessions** still capture through ffmpeg ([`engine::run_session`]),
//!     with [`cpal_capture`] as the Windows variant (cpal audio piped into the
//!     ffmpeg that owns the camera) and [`two_process`] as the fallback when one
//!     ffmpeg cannot hold camera + mic at once.
//!   - The `classic_ffmpeg_audio` / `classic_directshow` hatches force the legacy
//!     ffmpeg paths on any session.
//!
//! Capture is DECOUPLED from delivery: every session writes a crash-tolerant
//! capture (PCM WAV / Matroska) into a hidden per-session folder, and [`concat`]
//! stitches the fragments and encodes/remuxes them into the user's chosen format
//! at the end. [`recovery`] finishes that job on the next launch when the app
//! died mid-session; [`preroll`] supplies the audio captured before the button
//! press.
//!
//! The split mirrors the rest of the codebase: every rule lives, tested, in the
//! core crate; this module only wires devices, processes, tasks, and events.
//!
//! ⚠️ HARDWARE-UNVERIFIED — the capture/stop paths open a real camera + mic and
//! can only be proven on a rig. See [`engine`] for what is verified by tests vs
//! what the manual smoke-test must confirm.

pub mod concat;
pub mod cpal_capture;
pub mod engine;
pub mod native_capture;
pub mod preroll;
pub mod recovery;
pub mod two_process;
