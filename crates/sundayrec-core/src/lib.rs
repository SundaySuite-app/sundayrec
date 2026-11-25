//! SundayRec domain core — pure, GUI-free, Tauri-free.
//!
//! This crate is the *behaviour* of the recorder distilled out of the Electron
//! main process (`src/main/recorder-utils.ts` and friends) into deterministic
//! Rust. The Electron code is the behavioural specification; the structure here
//! is rebuilt clean (see `docs/MIGRATION-TAURI2.md`, §2 "bygg det riktig").
//!
//! Everything here is unit-testable without a display, a device, or a process —
//! the `src-tauri` shell is a thin command/event layer on top.
//!
//! Modules:
//!   - [`audio`]        — pure VU metering mat: block peak/RMS, dBFS, lock-free `PeakMeters`
//!   - [`ffmpeg`]       — pure ffmpeg filter-string builders (drift, silencedetect)
//!   - [`camera_format`] — native camera capture's pure decisions (C1a): which frame duration to pin for the chosen fps, a USB camera's vendor/product, host ticks → ns, when to fall back to compatibility mode
//!   - [`camera_reconnect`] — when a camera has dropped out and when to open it again (C5a): gone vs. stalled, the SAME stable id only, a back-off without an end
//!   - [`capture`]      — the recording's constants, the audio-encode/codec seam, the channel maps, the video-encoder argv and the camera-capability summary (the ffmpeg capture builders left in C3b)
//!   - [`errors`]       — ffmpeg-stderr → stable error-code classification
//!   - [`filename`]     — output-filename construction (sanitise + pattern) (Fase 5)
//!   - [`device_match`] — 5-strategy fuzzy device matching (the device-name moat)
//!   - [`lang`]          — the seven UI languages the backend speaks in (`Lang`), for the native notifications' catalogue
//!   - [`alerts`]        — the sentences the NATIVE notification says, in the 7 languages (F1 A8). One `AlertText` variant per sentence; the `(variant, language)` match is exhaustive, so a new alert cannot ship in Norwegian only
//!   - [`notify`]        — the notification decisions: warning codes, once-semantics for the repeating observers, and the "we already said this" ledger for missed recordings
//!   - [`tray`]          — tray menu-model (localized items/actions) (PU-2)
//!   - [`preroll`]      — pre-roll rolling-capture / harvest-trim decision mat (Fase 3.2)
//!   - [`reconnect`]    — watchdog (stuck-progress) + reconnect back-off decisions
//!   - [`recorder`]     — the recorder state machine + session recovery/split policy (Fase 3)
//!   - [`stream_fault`] — what a live capture-stream fault (cpal 0.18 `ErrorKind`) makes the recorder do: keep recording, reconnect, retry or stop (A3b)
//!   - [`schedule`]     — scheduler recurrence/occurrence/missed-recording decisions (Fase 5)
//!   - [`wake`]         — wake-from-sleep capability/parse/schedule-command decisions (Fase 5)
//!   - [`timeouts`]     — recording-pipeline timeout constants (one source of truth)
//!   - [`av_clock`]     — the audio clock (C2): host-clock instant → audio sample position, a sliding least-squares fit that follows sound-card drift and restarts when the host clock jumps (sleep)
//!   - [`video_conform`] — the conformer (C2): camera frames → a fixed frame rate locked to the audio clock (newest frame per slot, repeat, 2 s hold then black), proptested over 3 simulated hours
//!   - [`video_copy`]   — cut-only video export (B6): which keyframes and clean splits the cuts snap to so the picture can be copied, the concat list, and the refusals that send the export back to a re-encode
//!   - [`video_encoder`] — which video encoder a recording gets: the Windows hardware probe order (nvenc → qsv → amf → mf → libx264), the golden-tested argv per encoder, and the fall-back-on-failure decision (C0a)
//!   - [`update`]       — auto-update status model + dev-check guard + semver "is newer" (R7)
//!   - [`silence`]      — the silence-watcher *decision* state machine (no real timers)
//!   - [`level_watch`]  — D3: the weak-level (muted mixer) state machine, injected clock
//!   - [`levels`]       — the per-channel level value (peak, RMS) the live meters and the editor's channel diagnosis share
//!   - [`settings`]     — the typed/validated settings model + defaults (Fase 1)
//!   - [`preflight`]    — the "ready-to-record" finding decisions (Fase 2)
//!   - [`diagnostics`]  — the diagnostics markdown report builder (Fase 2)
//!   - [`editor`]       — non-destructive cut/trim/region planning + export-arg math (PU-7 editor)
//!   - [`mastering`]    — EBU R128 loudness (integrated/range/true-peak) + normalise-gain decisions (PU-7)
//!   - [`playback`]     — the playback engine's pure half (B5a): the cut plan, constant-power joins, and the reader that turns kept segments into one programme
//!   - [`chain`]        — the podcast recipe and «Tale»'s clean-up as a list of filmcraft effects (B2a)
//!   - [`redact`]       — scrubbing text that leaves the process: user paths out of crash records, credentials out of log lines (E2)
//!   - [`telemetry`]    — the opt-in telemetry WIRE CONTRACT: a payload whose types cannot hold audio, paths, names or device names, plus the durable outbox's pure decisions (E3)

pub mod alerts;
pub mod audio;
pub mod av_clock;
pub mod camera_format;
pub mod camera_reconnect;
pub mod capture;
pub mod chain;
pub mod device_match;
pub mod editor;
pub mod errors;
pub mod ffmpeg;
pub mod filename;
pub mod history;
pub mod lang;
pub mod level_watch;
pub mod levels;
pub mod loudness;
pub mod mastering;
pub mod notify;
pub mod open_read;
pub mod pcm_stream;
pub mod playback;
pub mod preflight;
pub mod preroll;
pub mod processing;
pub mod reconnect;
pub mod recorder;
pub mod recovery;
pub mod redact;
pub mod schedule;
pub mod selftest;
pub mod settings;
pub mod silence;
pub mod stream_fault;
pub mod tale;
pub mod telemetry;
pub mod timeouts;
pub mod tray;
pub mod update;
pub mod video_conform;
pub mod video_copy;
pub mod video_encoder;
pub mod video_track;
pub mod wake;
pub mod wav;
pub mod window;
