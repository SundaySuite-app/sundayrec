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
//! `recorder/**` to say so. All ten are gone now (the last one went with
//! `cpal_capture.rs` in C3b).
//!
//! That was not a cosmetic problem. It is exactly WHY the cpal path was
//! forgotten when `session_generation` was added (F1-A5): threading one more
//! argument through twelve-to-sixteen positions is enough friction that the
//! second path gets left behind, and nothing in the compiler complains — the
//! forgotten path still compiles, still runs, and only misbehaves on a Sunday
//! when two sessions overlap.
//!
//! So the session's context is ONE value now. Adding a field here reaches every
//! call site that already takes the context, and — because the supervisor
//! accounts for the fields EXHAUSTIVELY (see [`SessionContext`]'s own doc) — no
//! path can silently skip a newcomer. (There were three supervisors when this
//! was written; since C3b there is one, the native `run_session`.)

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::selftest::RecordingTelemetry;
use tauri::AppHandle;

use crate::recorder::engine::{RecordingOpts, StateWriter};
use crate::recorder::native_capture::lufs::LoudnessLedger;
use crate::recorder::native_capture::preroll::PrerollClip;
use crate::recorder::native_capture::stream::CpalHostKind;

/// Everything ONE recording session is run against: the native supervisor
/// (`engine::supervisor::run_session`) and every finalisation step it calls.
///
/// Built exactly ONCE per `RecorderEngine::start` and handed to the supervisor.
/// That is what makes the [`StateWriter`] — and therefore the
/// `session_generation` guard inside it — provably the same source everywhere:
/// there is one writer, in one field, of one value. Cloning the context clones
/// the writer, which shares the same `Arc`s and the same generation counter —
/// pinned by `a_cloned_state_writer_shares_the_same_generation_guard` in
/// `engine`'s tests.
///
/// ## The exhaustiveness rule
///
/// The supervisor ACCOUNTS FOR EVERY FIELD in a destructuring `let` with no
/// `..` rest pattern. That is deliberate and load-bearing: a field added here
/// stops the supervisor compiling until it has said, in writing, what it does
/// with it. Reading `ctx.<field>` alone would not do that — a new field would
/// be merely *available*, which is precisely the state F1-A5 was born in.
///
/// ## What is NOT in here, and why
///
/// The two channels — `stop_rx` (the stop request) and `ready` (the start
/// handshake's reply address) — stay as separate parameters. They are not
/// context but SINGLE-CONSUMER endpoints: `stop_rx` is `&mut`-borrowed by every
/// segment loop, so living in a struct that everything else reads through `&`
/// would make every read site fight the segment loop's borrow; and `ready` is a `oneshot::Sender` consumed
/// exactly once, before the session proper begins, so as a field it would be an
/// `Option` that every later reader has to ignore plus a "have we answered yet?"
/// state to reason about. Keeping them out is the choice with the fewest
/// surprises: the context is what a session READS, the channels are how it
/// talks.
///
/// Three more things are per-session but still not fields, because nothing
/// hands them DOWN — each supervisor mints its own from what is already here,
/// and a field would only invite a second, disagreeing copy:
///
/// - the keep-awake block (F2-W5). `run_session` takes it from
///   [`session_keep_awake`](crate::recorder::engine::session_keep_awake) as a
///   local binding whose DROP is the release. As a field it would be released
///   whenever the context was, which — since the context is `Clone` — is a
///   thing that happens more than once per session.
/// - the capture folder and the crash-recovery manifest (F1/#246). Both are
///   derived inside the session from `opts.output_path` + the session id, which
///   is itself stamped when the session starts. Deriving beats carrying: there is no
///   window in which the folder in the struct and the folder on disk can differ.
/// - the segment's counters. They live one SEGMENT, not one session — see
///   [`SegmentCounters`].
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
    /// The capture host platform, resolved once by `start()` (the video
    /// track's encoder choice depends on it).
    pub(crate) platform: Platform,
    /// The cpal host the native engine opens — CoreAudio, WASAPI or ASIO —
    /// chosen once by `start()` (`engine::capture_host`). Every session runs on
    /// the native engine since C3b; a start failure is a start error.
    pub(crate) host: CpalHostKind,
    /// The camera, by its listed name, when the session records video (the
    /// native video track beside each WAV fragment). `None` = audio-only.
    pub(crate) camera: Option<String>,
    /// The harvested pre-roll clip, prepended to the FIRST deliverable.
    pub(crate) preroll_clip: Option<PrerollClip>,
    /// The ONE generation-guarded door to the recorder's shared state and
    /// auto-stop countdown. See [`StateWriter`] for what the guard is for.
    pub(crate) state: StateWriter,
    /// One loudness meter per deliverable (D3), measured on the writer thread.
    pub(crate) loudness: LoudnessLedger,
}

/// The three counters ONE segment is driven against.
///
/// They travel together everywhere a segment does — the native
/// `run_native_segment` and its unit-tested core `drive_native_segment` — and
/// they are the difference between those functions needing an
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
