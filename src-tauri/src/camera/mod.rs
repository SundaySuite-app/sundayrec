//! Native camera capture — the replacement for ffmpeg's `-list_devices` and the
//! capture-mode search through ffmpeg's error messages (docs/VIDEOMOTOR.md §C1).
//!
//! Built in three sessions:
//!
//! - **C1a (here):** [`avfoundation`], the macOS backend — Cap's MIT
//!   `cap-camera` / `cap-camera-avfoundation` PORTED to the objc2 bindings this
//!   crate already uses, with Cap's seven production fixes.
//! - **C1b:** the Windows backend (Cap's Media Foundation / DirectShow crates,
//!   taken in under `crates/`).
//! - **C1c:** one API over both (`list()`, `formats(id)`, `start(id, format,
//!   callback)`), the `camera_probe` example, and the diagnosis report on top.
//!
//! Nothing records through this module yet: the recorder still drives ffmpeg's
//! own capture until C3. The pure decisions (which frame duration to pin, a
//! USB camera's vendor/product, host ticks → ns) live in
//! `sundayrec_core::camera_format`, so they are tested on every platform.
//!
//! The backends are compiled only on their own OS — Linux and Windows CI run
//! `clippy -D warnings`, and a macOS-only module there would be all dead code.

#[cfg(target_os = "macos")]
pub mod avfoundation;
