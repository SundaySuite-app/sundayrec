//! The recording session's context — one struct instead of a dozen arguments.
//!
//! ## Why this file exists (F2-T2, from the F1 architecture review)
//!
//! Every supervisor and every step of the finalisation pipeline needs the same
//! handful of things: the app handle to emit through, the history pool, the
//! session's options, the resolved devices, the pre-roll clip and — since
//! F1-A5 — the generation-guarded [`StateWriter`]. They used to be threaded
//! through as loose parameters: `run_session` took 12, `run_cpal_session` 8,
//! `run_native_segment` 10, `finalize_pending`/`finalize_one` 10 each, and the
//! repo carried ten `#[allow(clippy::too_many_arguments)]` inside
//! `recorder/**` to say so.
//!
//! That was not a cosmetic problem. It is exactly WHY the cpal path was
//! forgotten when `session_generation` was added (F1-A5): threading one more
//! argument through twelve-to-sixteen positions is enough friction that the
//! second path gets left behind, and nothing in the compiler complains — the
//! forgotten path still compiles, still runs, and only misbehaves on a Sunday
//! when two sessions overlap.
//!
//! So the session's context is ONE value now. Adding a field here reaches every
//! call site that already takes the context, and — because both supervisors
//! account for the fields EXHAUSTIVELY (see [`SessionContext`]'s own doc) —
//! neither path can silently skip a newcomer.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;
use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::selftest::RecordingTelemetry;
use tauri::AppHandle;

use crate::recorder::engine::{CaptureBackend, RecordingOpts, StateWriter};
use crate::recorder::preroll::PrerollClip;

/// Everything ONE recording session is run against, whichever capture path runs
/// it: the ffmpeg supervisor (`engine::run_session`), the Windows cpal-pipe
/// supervisor ([`crate::recorder::cpal_capture::run_cpal_session`]) and the
/// two-process video fallback
/// ([`crate::recorder::two_process::run_two_process_session`]).
///
/// Built exactly ONCE per `RecorderEngine::start`, before the capture path is
/// routed, and handed to whichever supervisor wins the routing. That is what
/// makes the [`StateWriter`] — and therefore the `session_generation` guard
/// inside it — provably the same source on every path: there is one writer, in
/// one field, of one value. Cloning the context (the cpal attempt gets a clone
/// so the originals survive a fall-through to DirectShow) clones the writer,
/// which shares the same `Arc`s and the same generation counter — pinned by
/// `a_cloned_session_context_shares_the_same_generation_guard` in
/// `engine`'s tests.
///
/// ## The exhaustiveness rule
///
/// Both supervisors ACCOUNT FOR EVERY FIELD in a destructuring `let` with no
/// `..` rest pattern. That is deliberate and load-bearing: a field added here
/// stops both paths compiling until each one has said, in writing, what it does
/// with it. Reading `ctx.<field>` alone would not do that — a new field would
/// be merely *available* to a path, which is precisely the state F1-A5 was born
/// in.
///
/// ## What is NOT in here, and why
///
/// The two channels — `stop_rx` (the stop request) and `ready` (the start
/// handshake's reply address) — stay as separate parameters. They are not
/// context but SINGLE-CONSUMER endpoints: `stop_rx` is `&mut`-borrowed by every
/// segment loop and MOVED whole into the two-process fallback, so living in a
/// struct that everything else reads through `&` would make every read site
/// fight the segment loop's borrow; and `ready` is a `oneshot::Sender` consumed
/// exactly once, before the session proper begins, so as a field it would be an
/// `Option` that every later reader has to ignore plus a "have we answered yet?"
/// state to reason about. Keeping them out is the choice with the fewest
/// surprises: the context is what a session READS, the channels are how it
/// talks.
#[derive(Clone)]
pub(crate) struct SessionContext {
    /// The Tauri handle every `recording://*` event is emitted through.
    pub(crate) app: AppHandle,
    /// The history database, when one is open. `None` = no history row.
    pub(crate) pool: Option<SqlitePool>,
    /// The session's options (paths, formats, split/silence/auto-stop, channel
    /// routing). Owned per session: the engine resolves the camera input mode
    /// into it before the session starts.
    pub(crate) opts: RecordingOpts,
    /// The capture host platform, resolved once by `start()`.
    pub(crate) platform: Platform,
    /// Which capture engine is running this session. MUTABLE across the
    /// session: the native backend may demote itself to ffmpeg exactly once, on
    /// a native start failure, and every later spawn/finalise must see the
    /// demotion — which it does, because they read this field.
    pub(crate) backend: CaptureBackend,
    /// The ffmpeg-side audio device. MUTABLE across the session: a reconnect
    /// re-resolves the device index by name (the mixer can come back on a
    /// different avfoundation/dshow index), and every later spawn + the history
    /// row must see the fresh one.
    pub(crate) audio: FfmpegDevice,
    /// The camera, when the session records video. `None` = audio-only.
    pub(crate) video: Option<FfmpegDevice>,
    /// The harvested pre-roll clip, prepended to the FIRST deliverable.
    pub(crate) preroll_clip: Option<PrerollClip>,
    /// The ONE generation-guarded door to the recorder's shared state and
    /// auto-stop countdown. See [`StateWriter`] for what the guard is for.
    pub(crate) state: StateWriter,
    /// `(engine label, fallback reason)` for the diagnose tool — the engine's
    /// own cell, so a backend demotion mid-session is visible to support.
    pub(crate) audio_engine: Arc<Mutex<(Option<String>, Option<String>)>>,
}

/// The three counters ONE segment is driven against.
///
/// They travel together everywhere a segment does — `engine::run_segment`, the
/// native `run_native_segment` and its unit-tested core `drive_native_segment`
/// — and they are the difference between those functions needing an
/// `#[allow(clippy::too_many_arguments)]` and not. Grouped rather than
/// context-ified because their lifetime is the SEGMENT's, not the session's:
/// `segment_bytes` is a fresh counter per fragment, and `deliverable_bytes` is
/// recomputed at every reconnect and reset at every split.
pub(crate) struct SegmentCounters {
    /// Bytes written by THIS fragment, fed by the capture's own progress
    /// reporting and read by the byte-progress watchdog.
    pub(crate) segment_bytes: Arc<AtomicU64>,
    /// Bytes already captured into the current deliverable's PREVIOUS fragments
    /// (`_rN` reconnect pieces) — feeds the RIFF-cap forced split so a
    /// `-c copy`-concatenated deliverable can never cross the 4 GiB WAV ceiling.
    pub(crate) deliverable_bytes: u64,
    /// The session-wide health counters, fed per line/block by the segment's
    /// reader and persisted in the session verdict.
    pub(crate) telemetry: Arc<Mutex<RecordingTelemetry>>,
}
