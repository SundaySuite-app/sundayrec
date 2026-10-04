//! Tauri command handlers.
//!
//! Commands are the thin IPC layer the renderer calls via `invoke()`. They
//! delegate to `sundayrec-core` (and, later, the `services` modules) and return
//! `Result<T, AppError>`. Naming convention: `entity_verb` (e.g. `app_info`).

pub mod app;
pub mod audio;
// A2 — places the operator picked in a native dialog RUST opened, held for the
// webview as opaque session tokens (the editor's export folder). No path in.
pub mod chosen_paths;
pub mod db;
pub mod diagnostics;
pub mod editor;
pub mod haptics;
// E2.3 — reveal the log folder / copy its tail. Neither takes a path: the only
// directory they can touch is computed in-process (see the module docs).
pub mod logs;
pub mod media;
pub mod media_filters;
// OS notifications: permission, test, settings page. No path.
pub mod notification;
// One-time notices (the "e-mail alerts were removed" banner). No path.
pub mod notice;
pub mod path_guard;
// «Legg ut» — open the chosen upload page. Takes nothing from the renderer;
// the URL comes from the stored setting via `sundayrec_core::publish`.
pub mod publish;
// E1.3 — a TEST-only module: the coverage ratchet that makes it impossible to
// land a new path-taking command without classifying it as guarded or exempt.
// Compiled out of every non-test build by its own inner `#![cfg(test)]`.
mod path_ratchet;
pub mod recorder;
// The tray's «Åpne opptaksmappen» and «Vis i Finder»: the webview holds no
// `opener:` permission, so these two decide what may be shown. Only
// `recordings_reveal` takes a path (path_guard + three grants, see the module).
pub mod recordings_open;
pub mod scheduler;
pub mod settings;
// E3 — opt-in telemetry: consent, deletion, counters, and the "show me exactly
// what you would send" preview. None takes a path (see the module docs).
pub mod telemetry;
pub mod trash;
pub mod update;
pub mod wake;
