//! Preflight I/O plumbing (F2.2) — gathers the facts the pure core decides on.
//!
//! The *decisions* (which findings to raise, in which order, with the Electron
//! thresholds + messages) live in [`sundayrec_core::preflight`] and carry the
//! tests. This module only does the I/O the core deliberately can't: resolving
//! the save folder, probing it for writability, reading free disk space, and
//! checking the ffmpeg binary. It then hands those facts to
//! [`assemble_findings`](sundayrec_core::preflight::assemble_findings).
//!
//! ## macOS mic/camera permission
//!
//! Read from the same TCC probe the recorder's own `start()` refuses on
//! ([`crate::media::permissions::status`]), so a Sunday-morning preflight and
//! the start it precedes agree: `Denied`/`Restricted` raise the `error/device`
//! finding, `NotDetermined`/`Unknown` do not (opening the device is what shows
//! the OS prompt, and a check that cannot tell must not cry wolf). Off macOS
//! the probe answers `Unknown`, so both flags stay `false`.
//!
//! ## Hardware-unverified
//!
//! [`run_preflight`] itself needs a real machine: a real ffmpeg, a real volume
//! with real free space. The writable-folder probe, the free-space read and the
//! ffmpeg health-check (ffmpeg still encodes every delivery) are exercised here
//! only against whatever the dev box has.
//! The pure decision over the facts is what the tests cover.

use sqlx::SqlitePool;
use sundayrec_core::device_match::{FfmpegDevice, find_best_device_match};
use sundayrec_core::preflight::{
    PreflightFacts, PreflightFinding, assemble_findings, looks_like_onedrive,
    planned_capture_bytes, video_active,
};

use crate::media::ffmpeg::ffmpeg_health;
use crate::media::permissions::{MediaKind, status};
use crate::settings;

/// Probe a folder for writability the way Electron did (`preflight.ts:40-47`):
/// create it (recursively) if missing, write then delete a probe file. Returns
/// `true` only when every step succeeds.
fn folder_writable(folder: &std::path::Path) -> bool {
    if std::fs::create_dir_all(folder).is_err() {
        return false;
    }
    let probe = folder.join(format!(
        ".preflight_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    if std::fs::write(&probe, b"").is_err() {
        return false;
    }
    // Best-effort cleanup; failure to remove doesn't make the folder unwritable.
    let _ = std::fs::remove_file(&probe);
    true
}

/// Free bytes on the volume holding `folder`, or `None` when the platform can't
/// report it (mirrors Electron's `statfs`-unsupported branch — the core then
/// skips the space check rather than fail-stop).
fn free_bytes(folder: &std::path::Path) -> Option<u64> {
    fs4::available_space(folder).ok()
}

/// Whether the OS has explicitly blocked `kind` (macOS Denied/Restricted).
fn permission_blocked(kind: MediaKind) -> bool {
    status(kind).is_blocked()
}

/// Whether the audio device named in settings is among the enumerated inputs.
///
/// Answers `true` for every case where we CANNOT establish absence — no device
/// configured, or the enumeration itself failed (a permission wall, a driver
/// that would not answer). Only a configured name that the same fuzzy matcher
/// the recorder uses ([`find_best_device_match`]) fails to resolve counts as
/// missing. Getting that asymmetry right is the whole safety of this check: a
/// false alarm on a Sunday morning sends a volunteer hunting for a cable that
/// is already plugged in.
///
/// The list is the NATIVE one (C3b) — the cpal inputs, plus ASIO on Windows —
/// the very list the recorder opens the device from
/// ([`crate::commands::audio::enumerate_tagged_inputs`]). It used to be
/// ffmpeg's `-list_devices`, which no recording reads any more: it could miss
/// an ASIO-only interface the recording opens fine.
async fn device_present(configured: Option<&str>) -> bool {
    let Some(name) = configured.map(str::trim).filter(|n| !n.is_empty()) else {
        return true; // nothing configured — the OS default is used, nothing to check
    };
    // The ASIO half LOADS every installed ASIO driver (memoised, but a sweep
    // when cold), so it is asked only for a device that is an ASIO one — the
    // same economy the scheduler's own lookup keeps (rig item w14).
    let wanted = name.to_string();
    let listed = tokio::task::spawn_blocking(move || {
        let include_asio = crate::audio::asio::is_asio_device(&wanted);
        crate::commands::audio::enumerate_tagged_inputs(include_asio)
    })
    .await;
    let Ok(Ok(inputs)) = listed else {
        return true; // could not enumerate — unknown, not absent
    };
    let candidates: Vec<FfmpegDevice> = inputs
        .into_iter()
        .map(|d| FfmpegDevice::new(d.name, "native", None))
        .collect();
    device_listed(&candidates, name)
}

/// The pure half of [`device_present`]: is `name` in `inputs` by the recorder's
/// fuzzy rule? An EMPTY list answers `true` — on a machine that manifestly has
/// a microphone it means the enumeration produced nothing usable, not that every
/// input vanished.
fn device_listed(inputs: &[FfmpegDevice], name: &str) -> bool {
    inputs.is_empty() || find_best_device_match(inputs, name).is_some()
}

/// A preflight run with the raw facts kept, for callers that need to act on a
/// specific one rather than on the rendered findings list.
pub struct PreflightOutcome {
    /// What the core decided (the same list [`run_preflight`] returns).
    pub findings: Vec<PreflightFinding>,
    /// The facts those findings were decided from.
    pub facts: PreflightFacts,
    /// The configured audio-device name that was checked, when one is set. The
    /// scheduler puts this in the `device_missing` warning so the operator is
    /// told WHICH device to go and plug in.
    pub device_name: Option<String>,
}

/// Run the preflight check: load settings, gather the filesystem/ffmpeg/device
/// facts, and let the core decide the findings. `documents_dir` is the OS
/// Documents directory the Tauri command resolves (used only when no
/// `save_folder` is set).
///
/// macOS mic/camera permission is read from the TCC probe — see the module
/// docs. An empty `findings` means "alt klart".
///
/// The device check is about the device in settings. `planned_minutes` is how long the recording this check
/// precedes is planned to run (a scheduled start), so the free-space check can
/// size the disk against it instead of the flat threshold; `None` = no plan.
pub async fn run_preflight_detailed(
    pool: &SqlitePool,
    documents_dir: Option<&std::path::Path>,
    planned_minutes: Option<u32>,
) -> PreflightOutcome {
    let settings = settings::load(pool).await.unwrap_or_default();

    let ffmpeg_missing = !ffmpeg_health().available;

    // The canonical resolver (R3). An unresolvable folder (nothing configured,
    // no Documents dir) is reported as NOT writable — that is exactly the
    // finding the operator needs — instead of probing a relative "." like the
    // pre-R3 command-side fallback did.
    let (writable, free, onedrive) = match sundayrec_core::settings::resolve_save_folder(
        settings.save_folder.as_deref(),
        documents_dir,
    ) {
        Ok(folder) => (
            folder_writable(&folder),
            free_bytes(&folder),
            // F-W9: a resolved-but-unwritable folder can still be inside
            // OneDrive (a permissions quirk is a different problem from a
            // sync risk) — compute this from the same resolve, not gated on
            // `writable`.
            looks_like_onedrive(&folder.to_string_lossy()),
        ),
        Err(_) => (false, None, false),
    };

    // The device in settings, looked up in the enumeration the recorder uses.
    let device_name = settings
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let device_present = device_present(device_name.as_deref()).await;

    let facts = PreflightFacts {
        ffmpeg_missing,
        folder_writable: writable,
        free_bytes: free,
        video_active: video_active(&settings),
        // macOS TCC — see module docs.
        mic_denied: permission_blocked(MediaKind::Microphone),
        cam_denied: permission_blocked(MediaKind::Camera),
        device_present,
        save_folder_onedrive: onedrive,
        planned_capture_bytes: planned_minutes
            .map(|m| planned_capture_bytes(m, settings.resolved_sample_rate())),
    };

    PreflightOutcome {
        findings: assemble_findings(facts),
        facts,
        device_name,
    }
}

/// The findings alone — what every existing caller wants.
pub async fn run_preflight(
    pool: &SqlitePool,
    documents_dir: Option<&std::path::Path>,
) -> Vec<PreflightFinding> {
    run_preflight_detailed(pool, documents_dir, None)
        .await
        .findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_counts_as_missing_only_when_the_list_says_so() {
        let inputs = [
            FfmpegDevice::new("Behringer UMC204HD 192k", "native", None),
            FfmpegDevice::new("MacBook Pro-mikrofon", "native", None),
        ];
        // The recorder's fuzzy rule: a stored label finds its device.
        assert!(device_listed(&inputs, "Behringer UMC204HD"));
        // Not in the list → missing.
        assert!(!device_listed(&inputs, "Allen & Heath Qu-5"));
        // An empty list proves nothing — never a false alarm.
        assert!(device_listed(&[], "Allen & Heath Qu-5"));
    }

    // Save-folder resolution itself is the canonical
    // `sundayrec_core::settings::resolve_save_folder` and is tested there.

    #[test]
    fn folder_writable_true_for_a_real_temp_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(folder_writable(dir.path()));
        // Probe file is cleaned up — directory is empty again.
        let entries = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(entries, 0, "probe file should be removed");
    }

    #[test]
    fn folder_writable_creates_missing_nested_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a/b/c");
        assert!(folder_writable(&nested));
        assert!(nested.is_dir());
    }

    #[test]
    fn free_bytes_reads_a_real_volume() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The temp dir lives on a real volume, so this must report something.
        let bytes = free_bytes(dir.path());
        assert!(bytes.is_some());
        assert!(bytes.unwrap() > 0);
    }

    // With nothing configured there is nothing to be missing — the Sunday path,
    // end to end through the real function. (The sidecar and the save folder
    // are whatever the test box has, so the assertion is about the DEVICE
    // finding only.)
    #[tokio::test]
    async fn with_no_device_configured_no_device_finding_is_raised() {
        use sundayrec_core::preflight::PreflightCode;
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        let findings: Vec<_> = run_preflight_detailed(&pool, Some(dir.path()), None)
            .await
            .findings
            .into_iter()
            .filter(|f| f.code == Some(PreflightCode::DeviceMissing))
            .collect();
        assert!(findings.is_empty(), "{findings:?}");
    }
}
