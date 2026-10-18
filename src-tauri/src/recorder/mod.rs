//! Recorder subsystem — the capture engine and its finalisation pipeline.
//!
//! This is the plumbing layer that turns the pure decision logic in
//! `sundayrec-core` (device matching, capture-argument building, the silence
//! watcher, the watchdog + reconnect schedule, the deliverable/fragment model)
//! into a real recording, and emits the Tauri events the renderer renders.
//!
//! ## One engine (since C3b)
//!
//! Every session captures NATIVELY on both platforms ([`native_capture`]:
//! cpal → ring → our own WAV writer). ffmpeg measurably lost samples under
//! avfoundation, so it left the audio path in v0.6.0 (and its escape hatch in
//! A5b). A video session adds the camera's video track beside each WAV
//! fragment (`native_capture::video`, C3a): the camera is read natively and
//! ffmpeg only ENCODES the frames it is handed. The ffmpeg capture that used to
//! record video sessions — with its Windows cpal-pipe variant, the
//! two-process fallback and the `classic_directshow` hatch — left in C3b.
//!
//! Capture is DECOUPLED from delivery: every session writes a crash-tolerant
//! capture (PCM WAV, plus Matroska video) into a hidden per-session folder, and [`concat`]
//! stitches the fragments and encodes/remuxes them into the user's chosen format
//! at the end. [`recovery`] finishes that job on the next launch when the app
//! died mid-session; [`native_capture::preroll`] supplies the audio captured before the button
//! press.
//!
//! The split mirrors the rest of the codebase: every rule lives, tested, in the
//! core crate; this module only wires devices, processes, tasks, and events.
//!
//! ⚠️ HARDWARE-UNVERIFIED — the capture/stop paths open a real camera + mic and
//! can only be proven on a rig. See [`engine`] for what is verified by tests vs
//! what the manual smoke-test must confirm.

pub mod concat;
// F2-T2: the session context (and the segment counters) every capture path and
// every finalisation step is run against — one struct instead of the 12–16
// loose parameters that made the cpal path easy to forget (F1-A5).
pub(crate) mod context;
pub mod engine;
// v0.15: the opts composition (save folder / filename / formats) the manual
// and scheduled paths share — moved here from the scheduler.
pub mod opts;
// E6.2/E6.4 — the headless long-run harness (forced-split + fault injection).
// Test-only: it re-implements the supervisor's control flow around the REAL
// capture/concat/finalize/history pieces, because those take a `Wry` AppHandle
// that cannot exist in a unit test. See the module docs for exactly which
// production code it does and does not execute.
#[cfg(test)]
mod longrun;
pub mod native_capture;
pub mod recovery;
