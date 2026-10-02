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
    assemble_findings_for, looks_like_onedrive, video_active, PreflightFacts, PreflightFinding,
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

/// Which audio device a preflight run checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightDevice {
    /// The device in settings, looked up in the enumeration the recorder uses
    /// ([`device_present`]). Every caller, until specials could name their own.
    Settings,
    /// A device the caller has already resolved — the scheduler, for a special
    /// recording with its own device, which has just enumerated the inputs the
    /// start will choose from. Its name, and whether it is there.
    ///
    /// When it is NOT there the finding names it
    /// ([`PreflightCode::SpecialDeviceMissing`](sundayrec_core::preflight::PreflightCode::SpecialDeviceMissing),
    /// `params.device`): this is the special's own device, and the settings
    /// device the generic finding points at is the wrong one to go and find.
    Resolved { name: String, present: bool },
}

/// What a preflight run's device check is about — [`device_target`]'s answer.
#[derive(Debug, PartialEq, Eq)]
struct DeviceTarget {
    /// The device that was checked, when there is one. The scheduler puts it in
    /// the `device_missing` warning so the operator knows what to go and find.
    name: Option<String>,
    /// Whether it is already KNOWN to be there. `None` = not looked up yet, which
    /// is the settings device: [`device_present`] enumerates for it.
    present: Option<bool>,
    /// The device to NAME in the finding, and ONLY a one-off recording's own —
    /// [`PreflightDevice::Resolved`]. `None` for the settings device, so its
    /// finding stays the generic one it always was: a weekly slot (and a special
    /// without a device of its own) must never be told «… for spesialopptaket …»
    /// about a device that is not a special's. It is only READ when the device is
    /// missing; a present one raises no finding either way.
    special: Option<String>,
}

/// Decide [`DeviceTarget`] from the caller's [`PreflightDevice`] and the device
/// named in settings (`settings_name`, as stored — trimmed here).
///
/// Pure, with the settings name as an INPUT, so a test can hold the Sunday
/// invariant to account: even with a device configured in settings, the settings
/// path names nothing as a special's.
fn device_target(device: PreflightDevice, settings_name: Option<&str>) -> DeviceTarget {
    match device {
        PreflightDevice::Settings => DeviceTarget {
            name: settings_name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string),
            present: None,
            special: None,
        },
        PreflightDevice::Resolved { name, present } => DeviceTarget {
            name: Some(name.clone()),
            present: Some(present),
            special: Some(name),
        },
    }
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
///
/// `device` says which audio device the device check is about; see
/// [`PreflightDevice`].
pub async fn run_preflight_detailed(
    pool: &SqlitePool,
    documents_dir: Option<&std::path::Path>,
    device: PreflightDevice,
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

    // Which device the check is about, and whether to NAME it — decided by a
    // pure function (`device_target`) so the Sunday invariant is pinned where it
    // lives: the settings device never carries a name to say.
    let target = device_target(device, settings.device_name.as_deref());
    let device_present = match target.present {
        Some(present) => present,
        // The settings device: look it up in the enumeration the recorder uses.
        None => device_present(target.name.as_deref()).await,
    };
    let device_name = target.name;
    let special_device = target.special;

    let facts = PreflightFacts {
        ffmpeg_missing,
        folder_writable: writable,
        free_bytes: free,
        video_active: video_active(&settings),
        // macOS permission probe deferred to Fase 5 — see module docs.
        mic_denied: false,
        cam_denied: false,
        device_present,
        save_folder_onedrive: onedrive,
    };

    PreflightOutcome {
        findings: assemble_findings_for(facts, special_device.as_deref()),
        facts,
        device_name,
    }
}

/// The findings alone — what every existing caller wants.
pub async fn run_preflight(
    pool: &SqlitePool,
    documents_dir: Option<&std::path::Path>,
) -> Vec<PreflightFinding> {
    run_preflight_detailed(pool, documents_dir, PreflightDevice::Settings)
        .await
        .findings
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

    // ── which device the finding names ───────────────────────────────────────
    //
    // `run_preflight_detailed` is the one place a scheduler decision
    // (`PreflightDevice`) becomes a finding on the wire. The rule «named only
    // for a special» lives in `device_target` and is table-tested there, with a
    // device CONFIGURED in settings (an empty test database cannot show the
    // settings arm leaking a name). The async tests run the real function over a
    // real (empty) settings database; the sidecar and the save folder are
    // whatever the test box has, so every assertion is about the DEVICE findings
    // only, which is all this layer decides.

    use sundayrec_core::preflight::PreflightCode;

    async fn device_findings(device: PreflightDevice) -> Vec<PreflightFinding> {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        run_preflight_detailed(&pool, Some(dir.path()), device)
            .await
            .findings
            .into_iter()
            .filter(|f| {
                matches!(
                    f.code,
                    Some(PreflightCode::DeviceMissing | PreflightCode::SpecialDeviceMissing)
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_missing_resolved_device_is_named_on_its_finding() {
        let findings = device_findings(PreflightDevice::Resolved {
            name: "Zoom H6".into(),
            present: false,
        })
        .await;
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, Some(PreflightCode::SpecialDeviceMissing));
        assert_eq!(
            findings[0].params.get("device").map(String::as_str),
            Some("Zoom H6")
        );
    }

    #[tokio::test]
    async fn a_present_resolved_device_raises_no_device_finding() {
        let findings = device_findings(PreflightDevice::Resolved {
            name: "Zoom H6".into(),
            present: true,
        })
        .await;
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// With nothing configured there is nothing to be missing — the Sunday path,
    /// end to end through the real function. (What pins that the settings device
    /// never gets NAMED when one IS configured is the table below: the enumeration
    /// this would need is whatever the test box has.)
    #[tokio::test]
    async fn with_no_device_configured_the_settings_path_raises_no_device_finding() {
        let findings = device_findings(PreflightDevice::Settings).await;
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// THE Sunday invariant, at the layer that decides it: a device named in
    /// settings is checked and reported, but never NAMED as a special's — a
    /// weekly slot's miss must keep saying «Lydenheten som er valgt i
    /// innstillingene …», not «… for spesialopptaket …». The settings name is an
    /// input here on purpose: with the test database's empty settings the
    /// mutation «the Settings arm passes its name through as `special`» is
    /// invisible.
    #[test]
    fn the_settings_device_is_checked_but_never_named_as_a_specials() {
        for configured in [Some("Behringer X32"), Some("  Zoom H6  ")] {
            let t = device_target(PreflightDevice::Settings, configured);
            assert_eq!(t.special, None, "{configured:?}");
            // Not looked up yet — `device_present` answers for it.
            assert_eq!(t.present, None, "{configured:?}");
            // …but it IS the device that is checked, trimmed.
            assert_eq!(t.name.as_deref(), configured.map(str::trim));
        }
        // Nothing, or only blanks, configured: nothing to check, nothing to name.
        for none in [None, Some(""), Some("   ")] {
            assert_eq!(
                device_target(PreflightDevice::Settings, none),
                DeviceTarget {
                    name: None,
                    present: None,
                    special: None
                },
                "{none:?}"
            );
        }
    }

    /// A resolved device is the special's own: already decided, and named — and
    /// the global device in settings (whatever it is) does not leak in.
    #[test]
    fn a_resolved_device_is_named_and_already_decided() {
        let missing = device_target(
            PreflightDevice::Resolved {
                name: "Zoom H6".into(),
                present: false,
            },
            Some("Behringer X32"),
        );
        assert_eq!(
            missing,
            DeviceTarget {
                name: Some("Zoom H6".into()),
                present: Some(false),
                special: Some("Zoom H6".into()),
            }
        );
        let present = device_target(
            PreflightDevice::Resolved {
                name: "Zoom H6".into(),
                present: true,
            },
            None,
        );
        // Present: never looked up again, and (being present) raises no finding
        // for the name to appear in.
        assert_eq!(present.present, Some(true));
        assert_eq!(present.name.as_deref(), Some("Zoom H6"));
    }
}
