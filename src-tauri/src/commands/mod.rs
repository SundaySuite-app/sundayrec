//! Tauri command handlers.
//!
//! Commands are the thin IPC layer the renderer calls via `invoke()`. They
//! delegate to `sundayrec-core` (and, later, the `services` modules) and return
//! `Result<T, AppError>`. Naming convention: `entity_verb` (e.g. `app_info`).

pub mod account;
pub mod app;
pub mod audio;
pub mod bridge;
pub mod bridge_live;
pub mod calendar;
pub mod cloud;
pub mod companion;
pub mod db;
pub mod deeplink;
pub mod diagnostics;
pub mod editor;
pub mod email;
pub mod haptics;
pub mod integrations;
// E2.3 — reveal the log folder / copy its tail. Neither takes a path: the only
// directory they can touch is computed in-process (see the module docs).
pub mod logs;
pub mod media;
pub mod ndi;
pub mod path_guard;
// E1.3 — a TEST-only module: the coverage ratchet that makes it impossible to
// land a new path-taking command without classifying it as guarded or exempt.
// Compiled out of every non-test build by its own inner `#![cfg(test)]`.
mod path_ratchet;
pub mod publish;
pub mod recorder;
pub mod review;
pub mod scheduler;
pub mod settings;
pub mod streaming;
pub mod thumbnail;
pub mod trash;
pub mod update;
pub mod wake;
pub mod whisper;
