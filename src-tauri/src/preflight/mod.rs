//! Preflight I/O plumbing (F2.2) — gathers the facts the pure core decides on.
//!
//! The *decisions* (which findings to raise, in which order, with the Electron
//! thresholds + messages) live in [`sundayrec_core::preflight`] and carry the
//! tests. This module only does the I/O the core deliberately can't: resolving
//! the save folder, probing it for writability, reading free disk space, and
//! checking the ffmpeg binary. It then hands those facts to
//! [`assemble_findings`](sundayrec_core::preflight::assemble_findings).
//!
//! ## macOS mic/camera permission — honestly deferred
//!
//! The Electron build used `systemPreferences.getMediaAccessStatus('microphone'
//! | 'camera')` to raise an `error/device` finding when permission was denied.
//! Tauri 2 has no equivalent clean API, and shelling out to AppleScript / `tccutil`
//! to read the TCC database is fragile and entitlement-sensitive. So the F2.2
//! plumbing leaves `mic_denied`/`cam_denied` as `false` (permission check NOT
//! performed) and defers a proper probe to **Fase 5** (wake/permission), where
//! the macOS permission flow is built. This is an honest gap, not a silent pass:
//! the core path for the finding exists and is tested; only the live probe is
//! absent.
//!
//! ## Hardware-unverified
//!
//! [`run_preflight`] itself needs a real machine: a real ffmpeg, a real volume
//! with real free space. The writable-folder probe, the free-space read and the
//! ffmpeg health-check are exercised here only against whatever the dev box has.
//! The pure decision over the facts is what the tests cover.

use sqlx::SqlitePool;
use sundayrec_core::device_match::find_best_device_match;
use sundayrec_core::preflight::{
    assemble_findings, video_active, PreflightFacts, PreflightFinding,
};

use crate::audio::device_enum::enumerate_ffmpeg_devices_cached;
use crate::media::ffmpeg::ffmpeg_health;
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

/// Whether the audio device named in settings is among the enumerated inputs.
///
/// Answers `true` for every case where we CANNOT establish absence — no device
/// configured, or the enumeration itself failed (no ffmpeg, a permission wall).
/// Only a configured name that the same fuzzy matcher the recorder uses
/// ([`find_best_device_match`]) fails to resolve counts as missing. Getting that
/// asymmetry right is the whole safety of this check: a false alarm on a Sunday
/// morning sends a volunteer hunting for a cable that is already plugged in.
///
/// Uses the SHORT-TTL enumeration cache, so a preflight run right after the
/// device picker (or the record modal's warm-up) costs nothing.
async fn device_present(configured: Option<&str>) -> bool {
    let Some(name) = configured.map(str::trim).filter(|n| !n.is_empty()) else {
        return true; // nothing configured — the OS default is used, nothing to check
    };
    let Ok(inventory) = enumerate_ffmpeg_devices_cached().await else {
        return true; // could not enumerate — unknown, not absent
    };
    if inventory.audio_inputs.is_empty() {
        // An empty list means the enumeration produced nothing usable, which on
        // a machine that manifestly has a microphone means the probe failed, not
        // that every input vanished. The ffmpeg-missing finding covers the real
        // version of this.
        return true;
    }
    find_best_device_match(&inventory.audio_inputs, name).is_some()
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
/// macOS mic/camera permission is NOT probed here — see the module docs
/// (deferred to Fase 5). An empty `findings` means "alt klart".
pub async fn run_preflight_detailed(
    pool: &SqlitePool,
    documents_dir: Option<&std::path::Path>,
) -> PreflightOutcome {
    let settings = settings::load(pool).await.unwrap_or_default();

    let ffmpeg_missing = !ffmpeg_health().available;

    // The canonical resolver (R3). An unresolvable folder (nothing configured,
    // no Documents dir) is reported as NOT writable — that is exactly the
    // finding the operator needs — instead of probing a relative "." like the
    // pre-R3 command-side fallback did.
    let (writable, free) = match sundayrec_core::settings::resolve_save_folder(
        settings.save_folder.as_deref(),
        documents_dir,
    ) {
        Ok(folder) => (folder_writable(&folder), free_bytes(&folder)),
        Err(_) => (false, None),
    };

    let device_name = settings
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);

    let facts = PreflightFacts {
        ffmpeg_missing,
        folder_writable: writable,
        free_bytes: free,
        video_active: video_active(&settings),
        // macOS permission probe deferred to Fase 5 — see module docs.
        mic_denied: false,
        cam_denied: false,
        device_present: device_present(device_name.as_deref()).await,
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
    run_preflight_detailed(pool, documents_dir).await.findings
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
