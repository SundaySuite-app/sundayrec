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
//!   - [`capture`]      — unified ffmpeg capture-argument builder (Spike B)
//!   - [`errors`]       — ffmpeg-stderr → stable error-code classification
//!   - [`filename`]     — output-filename construction (sanitise + pattern) (Fase 5)
//!   - [`device_match`] — 5-strategy fuzzy device matching (the device-name moat)
//!   - [`device_enum`]  — pure ffmpeg `-list_devices` stderr parsers (audio + video)
//!   - [`email`]         — error/test alert templates (7-lang) + throttle/dedup gate (PU-1)
//!   - [`tray`]          — tray menu-model (localized items/actions) (PU-2)
//!   - [`mjpeg`]        — MJPEG stdout reassembly (SOI/EOI frame splitter + JPEG dims)
//!   - [`preroll`]      — pre-roll rolling-capture / harvest-trim decision mat (Fase 3.2)
//!   - [`progress`]     — ffmpeg `size=`-progress parsing + one-shot startup resolution
//!   - [`reconnect`]    — watchdog (stuck-progress) + reconnect back-off decisions
//!   - [`recorder`]     — the recorder state machine + session recovery/split policy (Fase 3)
//!   - [`schedule`]     — scheduler recurrence/occurrence/missed-recording decisions (Fase 5)
//!   - [`wake`]         — wake-from-sleep capability/parse/schedule-command decisions (Fase 5)
//!   - [`timeouts`]     — recording-pipeline timeout constants (one source of truth)
//!   - [`two_process`]  — two-process audio+video fallback: per-process capture args + A/V mux/offset (Fase 3.3b)
//!   - [`update`]       — auto-update status model + dev-check guard + semver "is newer" (R7)
//!   - [`silence`]      — the silence-watcher *decision* state machine (no real timers)
//!   - [`levels`]       — pure parser for `astats` per-channel peak-level stderr → live UI meters (B-5)
//!   - [`settings`]     — the typed/validated settings model + defaults (Fase 1)
//!   - [`preflight`]    — the "ready-to-record" finding decisions (Fase 2)
//!   - [`diagnostics`]  — the diagnostics markdown report builder (Fase 2)
//!   - [`editor`]       — non-destructive cut/trim/region planning + export-arg math (PU-7 editor)
//!   - [`mastering`]    — EBU R128 loudness (integrated/range/true-peak) + normalise-gain decisions (PU-7)
//!   - [`audio_analysis`] — peaks/waveform, spectrum (FFT), frame classification (PU-7)
//!   - [`detect`] — the ONE sermon detector over those segments (E9)
//!   - [`whisper`]      — whisper.cpp transcription decisions: model registry, argv/thread heuristic, progress/exit parse, JSON-sidecar normalise, chunk/merge, language map (PU-5)
//!   - [`image_probe`]  — PNG/JPEG/WebP header parsing (format + pixel size) for the episode-image panels (Fase 6)
//!   - [`redact`]       — scrubbing text that leaves the process: user paths out of crash records, credentials out of log lines (E2)
//!   - [`feedback`]     — the record of a human correcting us: the sermon auto-pick, the proposed trim, and the AI companion's suggestions — what to store, what counts as a correction, and what a later one replaces (E8)
//!   - [`trim_feedback`] — how far the operator moved the proposed sermon trim, and the sign convention that makes the deltas readable (E8)
//!   - [`learning_summary`] — folding every recording's feedback file into the counts + trim-direction verdict the transparency screen shows (E8)
//!   - [`local_adaptivity`] — the first thing that ACTS on those corrections: a hard-clamped, opt-in, one-click-resettable offset on the proposed sermon boundaries, derived from this install's own corrections (E10)
//!   - [`vad`]         — the neural voice-activity seam (E9): the 576-sample framing + two-piece per-stream state a Silero-class model needs, and the `VadBackend` trait that keeps ONNX out of this crate. NOT wired into sermon detection yet
//!   - [`telemetry`]    — the opt-in telemetry WIRE CONTRACT: a payload whose types cannot hold audio, paths, names or device names, plus the durable outbox's pure decisions (E3)
//!   - [`tuning`]       — EVERY number the sermon detector decides with, in one documented table: what each means, what moving it does, and honestly which ones nobody can justify. [`audio_analysis`] and [`detect`] re-export from here, so there is one definition of each (E10)

pub mod ab_eval;
pub mod audio;
pub mod audio_analysis;
pub mod capture;
pub mod chapters;
pub mod church_calendar;
pub mod companion;
pub mod detect;
pub mod device_enum;
pub mod device_match;
pub mod diagnostics;
pub mod editor;
pub mod email;
pub mod errors;
pub mod feedback;
pub mod ffmpeg;
pub mod filename;
pub mod history;
pub mod image_probe;
pub mod learning_summary;
pub mod levels;
pub mod local_adaptivity;
pub mod mastering;
pub mod mjpeg;
pub mod notify;
pub mod preflight;
pub mod preroll;
pub mod processing;
pub mod progress;
pub mod reconnect;
pub mod recorder;
pub mod recovery;
pub mod redact;
pub mod schedule;
pub mod selftest;
pub mod settings;
pub mod shadow;
pub mod silence;
pub mod telemetry;
pub mod test_recording;
pub mod timeouts;
pub mod tray;
pub mod trim_feedback;
pub mod tuning;
pub mod two_process;
pub mod update;
pub mod vad;
pub mod wake;
pub mod wav;
pub mod whisper;
