//! NDI receiver plumbing (R3 P2c) — **STUB**, default-off `ndi` feature.
//!
//! The NDI architecture (per the Electron `src/main/ndi-receiver.ts`) bridges
//! frames from a network NDI source into the streamer's single ffmpeg via a
//! **loopback TCP socket**: libndi receives frames, a TCP server serves the raw
//! bytes, and ffmpeg reads `tcp://127.0.0.1:<port>` with `-f rawvideo`. The PURE
//! parts — the discovered-source model, the FourCC→pixfmt choice, and the
//! `-f rawvideo …` input-arg builder — live in the unit-tested
//! [`sundayrec_core::ndi`].
//!
//! ## Feature flag + STUB
//!
//! Behind the **default-off `ndi`** cargo feature. The real receiver needs the
//! NDI SDK runtime (libndi) + a native FFI binding + an actual NDI source on the
//! LAN — NONE of which are present in this repo or this environment. So even
//! WITH `--features ndi` the seam is a STUB: [`list_sources`] returns an empty
//! list and [`start_receiver`] returns a clear, actionable error pointing at
//! `docs/NEEDS-RICHARD.md`. The default build returns `feature_disabled`.
//!
//! Wiring the real binding (the FFI crate, the loopback TCP server, the
//! grandiose-equivalent frame pump) is the documented needs-Richard step — the
//! pure decision logic it will lean on is already built + tested.

#[cfg(feature = "ndi")]
use std::sync::Mutex;

use sundayrec_core::ndi::{NdiReceiverInfo, NdiSource};

use crate::error::{AppError, AppResult};

/// The real NDI transmit sender (runtime dlopen of `libndi`). Feature-gated; the
/// whole module compiles away in the default build.
#[cfg(feature = "ndi")]
pub mod sender;

/// The clear, stable error every NDI seam call returns until the SDK is bundled.
/// Kept as a constant so the message (and the doc pointer) is identical across
/// entry points and the renderer can match on it.
pub const NDI_NOT_BUNDLED: &str =
    "ndi_not_bundled: NDI SDK not bundled — see docs/NEEDS-RICHARD.md";

#[cfg(not(feature = "ndi"))]
fn disabled<T>(verb: &str) -> AppResult<T> {
    Err(AppError::Validation(format!(
        "feature_disabled: ndi.{verb} requires a build with `--features ndi`"
    )))
}

/// List NDI sources advertising on the LAN. Default build → `feature_disabled`.
#[cfg(not(feature = "ndi"))]
pub async fn list_sources() -> AppResult<Vec<NdiSource>> {
    disabled("listSources")
}

/// List NDI sources. **STUB** (feature-on): the NDI SDK isn't bundled, so there
/// is nothing to discover — returns an empty list rather than erroring, so the
/// overlay UI can show "no NDI sources found" calmly. The real discovery
/// (libndi `find`) is the needs-Richard step.
#[cfg(feature = "ndi")]
pub async fn list_sources() -> AppResult<Vec<NdiSource>> {
    tracing::warn!("[ndi] list_sources called but NDI SDK is not bundled — returning empty");
    Ok(Vec::new())
}

/// Start a loopback-TCP receiver for `source_name`, resolving the frame size +
/// pixel format from the first frame. Default build → `feature_disabled`.
#[cfg(not(feature = "ndi"))]
pub async fn start_receiver(_source_name: &str, _want_alpha: bool) -> AppResult<NdiReceiverInfo> {
    disabled("startReceiver")
}

/// Start a receiver. **STUB** (feature-on): without the NDI SDK there is no
/// libndi to receive frames from, so this returns the clear [`NDI_NOT_BUNDLED`]
/// error pointing at the needs-Richard doc. The real implementation (open the
/// source, bind an ephemeral loopback TCP port, pump frames, resolve the size
/// from the first frame, hand back [`NdiReceiverInfo`] for
/// `sundayrec_core::ndi::build_ndi_input_args`) needs the SDK + a rig.
#[cfg(feature = "ndi")]
pub async fn start_receiver(_source_name: &str, _want_alpha: bool) -> AppResult<NdiReceiverInfo> {
    Err(AppError::Recording(NDI_NOT_BUNDLED.into()))
}

// ── NDI output (transmit) — REAL sender over the runtime ─────────────────────
//
// Broadcasts the selected camera onto the LAN as an NDI source other software
// (vMix, OBS-NDI, ProPresenter, a separate streaming PC) can pick up. ffmpeg
// decodes the camera to raw UYVY422 frames; the dlopen [`sender`] hands each
// frame to libndi. Default-off + SDK/HARDWARE-UNVERIFIED (see `sender`).

/// One running NDI output: the frame-pump task + the flag that stops it.
#[cfg(feature = "ndi")]
struct NdiOutputSession {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Managed state for NDI transmit. At most one output runs at a time. The struct
/// compiles in both feature states so the managed-state type stays stable.
#[derive(Default)]
pub struct NdiOutputEngine {
    #[cfg(feature = "ndi")]
    session: Mutex<Option<NdiOutputSession>>,
}

impl NdiOutputEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Whether the NDI runtime is installed on this machine (so the UI can show the
/// "install NDI" hint instead of letting a start fail). Default build → `false`.
#[cfg(not(feature = "ndi"))]
pub fn output_runtime_available() -> bool {
    false
}

/// Whether the NDI runtime (`libndi`) is installed + loadable.
#[cfg(feature = "ndi")]
pub fn output_runtime_available() -> bool {
    sender::runtime_available()
}

/// Start transmitting `device_token` as an NDI source named `source_name` at
/// `width`×`height`@`fps`. Default build → `feature_disabled`.
#[cfg(not(feature = "ndi"))]
#[allow(clippy::too_many_arguments)]
pub async fn output_start(
    _engine: &NdiOutputEngine,
    _device_token: String,
    _width: u32,
    _height: u32,
    _fps: u32,
    _source_name: String,
) -> AppResult<()> {
    disabled("outputStart")
}

/// Start transmitting the camera as an NDI source. SDK/HARDWARE-UNVERIFIED.
#[cfg(feature = "ndi")]
pub async fn output_start(
    engine: &NdiOutputEngine,
    device_token: String,
    width: u32,
    height: u32,
    fps: u32,
    source_name: String,
) -> AppResult<()> {
    use sundayrec_core::ndi::{build_ndi_rawframe_args, validate_ndi_source_name};

    validate_ndi_source_name(&source_name)
        .map_err(|e| AppError::Validation(format!("invalid_ndi_source_name:{e:?}")))?;

    // Presence check FIRST — no FFI runs if the runtime isn't installed.
    let rt = sender::NdiRuntime::load().ok_or_else(|| {
        AppError::Recording(
            "NDI-kjøretid er ikke installert. Last ned og installer NDI Tools / NDI Runtime, \
             og prøv igjen."
                .into(),
        )
    })?;

    // Stop any previous output cleanly.
    output_stop(engine).await?;

    let args = build_ndi_rawframe_args(
        crate::util::detect_platform(),
        &device_token,
        width,
        height,
        fps,
    );
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("ndi ffmpeg spawn: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Recording("ndi ffmpeg had no stdout".into()))?;

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_t = stop.clone();
    let task = tauri::async_runtime::spawn(async move {
        run_output_pump(rt, source_name, child, stdout, width, height, fps, stop_t).await;
    });

    *crate::util::lock_recover(&engine.session) = Some(NdiOutputSession { stop, task });
    Ok(())
}

/// The frame pump: create the NDI sender, then read each raw UYVY422 frame from
/// ffmpeg and hand it to libndi until stop / EOF. Owns the sender + child so both
/// are torn down (instance destroyed, ffmpeg killed) when the task ends/aborts.
#[cfg(feature = "ndi")]
#[allow(clippy::too_many_arguments)]
async fn run_output_pump(
    rt: std::sync::Arc<sender::NdiRuntime>,
    source_name: String,
    mut child: tokio::process::Child,
    mut stdout: tokio::process::ChildStdout,
    width: u32,
    height: u32,
    fps: u32,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncReadExt;

    let ndi = match sender::NdiSender::create(rt, &source_name) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[ndi] sender create failed: {e}");
            let _ = child.start_kill();
            return;
        }
    };

    let frame_bytes = sundayrec_core::ndi::uyvy_frame_bytes(width, height);
    let mut buf = vec![0u8; frame_bytes];
    tracing::info!("[ndi] transmitting «{source_name}» {width}x{height}@{fps}");

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // Read exactly one frame; ffmpeg emits a frame every ~1/fps s, so the
        // stop flag is checked at least that often.
        match stdout.read_exact(&mut buf).await {
            Ok(_) => ndi.send_uyvy(&buf, width, height, fps, 1),
            Err(_) => break, // EOF / pipe closed → ffmpeg ended
        }
    }

    let _ = child.start_kill();
    let _ = child.wait().await;
    // `ndi` drops here → the NDI source disappears from the network.
}

/// Stop the running NDI output. Idempotent. Default build → `feature_disabled`.
#[cfg(not(feature = "ndi"))]
pub async fn output_stop(_engine: &NdiOutputEngine) -> AppResult<()> {
    disabled("outputStop")
}

/// Stop the running NDI output: signal + abort the pump (which kills ffmpeg and
/// destroys the NDI sender via Drop). Safe to call when nothing is running.
#[cfg(feature = "ndi")]
pub async fn output_stop(engine: &NdiOutputEngine) -> AppResult<()> {
    use std::sync::atomic::Ordering;
    let session = crate::util::lock_recover(&engine.session).take();
    if let Some(s) = session {
        s.stop.store(true, Ordering::SeqCst);
        s.task.abort();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "ndi"))]
    #[tokio::test]
    async fn list_sources_is_disabled_without_the_feature() {
        let err = list_sources().await.unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(err.to_string().contains("feature_disabled"));
    }

    #[cfg(not(feature = "ndi"))]
    #[tokio::test]
    async fn start_receiver_is_disabled_without_the_feature() {
        let err = start_receiver("Studio", false).await.unwrap_err();
        assert!(err.to_string().contains("feature_disabled"));
    }

    #[cfg(feature = "ndi")]
    #[tokio::test]
    async fn stub_list_sources_is_empty_and_start_points_at_needs_richard() {
        assert!(list_sources().await.unwrap().is_empty());
        let err = start_receiver("Studio", false).await.unwrap_err();
        assert!(err.to_string().contains("NDI SDK not bundled"));
        assert!(err.to_string().contains("NEEDS-RICHARD"));
    }
}
