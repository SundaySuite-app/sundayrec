//! The preflight "ready-to-record" check — pure decision logic.
//!
//! Ported from the Electron main process `src/main/preflight.ts` (the
//! behavioural specification). That code interleaved I/O (statfs, mkdir-probe,
//! `getMediaAccessStatus`, device resolution) with the *decisions* about which
//! findings to raise. Here we keep ONLY the decisions: every function takes the
//! already-gathered facts (free bytes, writable?, video active?, …) and returns
//! the [`PreflightFinding`]s. The `src-tauri` `preflight` module does the actual
//! filesystem/device I/O and feeds the results in, so this stays deterministic
//! and fully unit-tested without a disk, a device, or a process.
//!
//! The serde tags mirror the Electron string unions EXACTLY (`severity`
//! `"warn"`/`"error"`, `category` `"cloud" | "preroll" | "wake" | "disk" |
//! "device"`) so the same renderer logic / log shapes carry across the
//! migration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::alerts::AlertText;

/// How serious a finding is. Serialised lowercase to match the Electron
/// `'warn' | 'error'` union (`preflight.ts:21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PreflightSeverity.ts")]
#[serde(rename_all = "lowercase")]
pub enum PreflightSeverity {
    /// The recording can still proceed, but something is off.
    Warn,
    /// Recording will (likely) fail — needs the user's attention.
    Error,
}

/// Which part of the pipeline a finding is about. Serialised lowercase to match
/// the Electron `'preroll' | 'wake' | 'disk' | 'device'` union
/// (`preflight.ts:21`, minus `'cloud'`, which left with cloud backup).
/// `Preroll`/`Wake` are reserved for their later phases (pre-roll buffer,
/// wake-from-sleep) and are not raised by the F2.2 plumbing yet — they exist so
/// the type already matches Electron.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PreflightCategory.ts")]
#[serde(rename_all = "lowercase")]
pub enum PreflightCategory {
    /// Pre-roll buffer readiness (Fase 5).
    Preroll,
    /// Wake-from-sleep for scheduled jobs (Fase 5).
    Wake,
    /// Disk: writable save folder + free space.
    Disk,
    /// Capture device / ffmpeg binary.
    Device,
}

/// What the preflight check found, as a STABLE CODE.
///
/// Was six hardcoded Norwegian sentences (F2-I18N-R2). The findings are
/// rendered verbatim in the app's preflight card AND in the native
/// notification the scheduler fires half an hour before a service, so the
/// prose was the app's own voice in exactly one of its seven languages.
///
/// The code is the contract now, and it has TWO catalogues because it has two
/// surfaces — which is the rule, not an exception to it:
///
///   • the card: `status.preflightCode.<code>` in `legacy/locales/*.json`,
///     rendered by `app/state/preflight.ts`;
///   • the native notification: [`AlertText`] (see [`Self::alert`]), because
///     Rust cannot reach the renderer's catalogue and a notification is not a
///     place to be silent.
///
/// [`Self::as_str`] is the ENGLISH reserve — the log line, and the text field
/// a shell older than the code would fall back on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PreflightCode.ts")]
#[serde(rename_all = "camelCase")]
pub enum PreflightCode {
    /// The bundled ffmpeg sidecar is not there.
    FfmpegMissing,
    /// The configured audio device is not among the enumerated inputs.
    DeviceMissing,
    /// The save folder exists but cannot be written to.
    FolderNotWritable,
    /// Free space is below the mode's threshold. Carries `{gb}`.
    DiskLow,
    /// macOS is blocking microphone access.
    MicDenied,
    /// macOS is blocking camera access, and video is on.
    CameraDenied,
}

impl PreflightCode {
    /// English reserve, with `{gb}` unfilled where the code carries it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FfmpegMissing => "The ffmpeg binary is missing. SundayRec must be installed again.",
            Self::DeviceMissing => "The audio device selected in settings is not connected.",
            Self::FolderNotWritable => "The save folder cannot be written to.",
            Self::DiskLow => {
                "Only {gb} GB free on the save disk — perhaps not enough for a whole recording."
            }
            Self::MicDenied => {
                "Microphone access has not been granted. Open System Settings → Privacy → Microphone."
            }
            Self::CameraDenied => "Camera access has not been granted.",
        }
    }

    /// The native notification's sentence for this code — the seven-language
    /// catalogue Rust CAN reach.
    pub fn alert(self) -> AlertText {
        match self {
            Self::FfmpegMissing => AlertText::PreflightFfmpegMissing,
            Self::DeviceMissing => AlertText::PreflightDeviceMissing,
            Self::FolderNotWritable => AlertText::PreflightFolderNotWritable,
            Self::DiskLow => AlertText::PreflightDiskLow,
            Self::MicDenied => AlertText::PreflightMicDenied,
            Self::CameraDenied => AlertText::PreflightCameraDenied,
        }
    }
}

/// A single thing the preflight check found. Mirrors the Electron
/// `PreflightFinding` interface field-for-field, plus the [`PreflightCode`] the
/// two localised surfaces render on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PreflightFinding.ts")]
#[serde(rename_all = "camelCase")]
pub struct PreflightFinding {
    pub severity: PreflightSeverity,
    pub category: PreflightCategory,
    /// The code the app localises on. `None` for a finding the SHELL built
    /// itself (`app/lib/status/health-findings.ts` raises two of its own from
    /// the permission probes) — those already carry text in the user's
    /// language, and inventing a code for them here would be a second answer
    /// to a question that already has one.
    #[serde(default)]
    pub code: Option<PreflightCode>,
    /// The engine's own wording: ENGLISH, with `params` already filled in.
    /// Used verbatim by a reader that does not know [`Self::code`].
    pub message: String,
    /// Interpolation values for the localised sentence (`{gb}`). Same shape and
    /// same reason as [`crate::notify::BackendWarning::params`].
    #[serde(default)]
    pub params: HashMap<String, String>,
}

impl PreflightFinding {
    /// An error finding for `code`, with the English reserve as its message.
    fn error(category: PreflightCategory, code: PreflightCode) -> Self {
        Self {
            severity: PreflightSeverity::Error,
            category,
            code: Some(code),
            message: code.as_str().to_string(),
            params: HashMap::new(),
        }
    }

    /// Attach one interpolation value — and fill it into `message`, so the
    /// English reserve is a finished sentence and not a template with a hole
    /// in it. That is the whole difference between a reserve and a bug.
    fn param(mut self, key: &str, value: impl Into<String>) -> Self {
        let value = value.into();
        self.message = self.message.replace(&format!("{{{key}}}"), &value);
        self.params.insert(key.to_string(), value);
        self
    }
}

/// 500 MB — comfortable headroom for a ~1.5 h MP3. Matches Electron
/// `MIN_DISK_AUDIO_BYTES` (`preflight.ts:26`).
pub const MIN_DISK_AUDIO_BYTES: u64 = 500 * 1024 * 1024;
/// 4 GB — comfortable headroom for a ~1.5 h video. Matches Electron
/// `MIN_DISK_VIDEO_BYTES` (`preflight.ts:27`).
pub const MIN_DISK_VIDEO_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The disk headroom for the given capture mode — the same threshold the
/// pre-flight check uses, reused by the during-recording guard.
pub fn min_disk_headroom_bytes(video_active: bool) -> u64 {
    if video_active {
        MIN_DISK_VIDEO_BYTES
    } else {
        MIN_DISK_AUDIO_BYTES
    }
}

/// DURING-recording guard: should the recorder stop NOW because free space has
/// fallen below `headroom_bytes`? Stopping gracefully here finalises a playable
/// file BEFORE ffmpeg hits `ENOSPC` and leaves a corrupt container. Pure so the
/// threshold logic is unit-tested; the engine owns the periodic `fs4` probe.
pub fn low_disk_should_stop(free_bytes: u64, headroom_bytes: u64) -> bool {
    free_bytes < headroom_bytes
}

/// Extra headroom the FINALISE step needs, on top of the steady-state
/// [`min_disk_headroom_bytes`], so a graceful low-disk stop doesn't itself leave
/// too little room for the decoupled-capture delivery step to complete. The
/// delivery step briefly needs BOTH the still-present capture AND its new
/// delivery file on disk at once — the disk-vacuum-during-recording guard was
/// previously blind to this and could stop "safely" with too little margin for
/// the delivery encode/remux to actually finish.
///
/// `captured_bytes` is the CURRENT (open) segment's capture size — a deliberately
/// simple estimate (not a full-session sum across every already-finalised
/// deliverable, which need no reserve since they're already delivered):
///   - Video (`RemuxCopy`, since the video decoupling): the delivery is a
///     lossless `-c copy` stream copy, so it lands at roughly the SAME size as
///     the MKV capture — reserve the full `captured_bytes`.
///   - Audio (`AudioEncode`): the delivery is lossy-encoded from lossless PCM
///     WAV, landing at roughly a SIXTH of the capture's size for typical
///     settings (WAV ≈ 1.5 Mbps vs. a 192–256 kbps delivery) — `/5` leaves
///     margin without over-reserving on a long capture.
pub fn finalize_reserve_bytes(video_active: bool, captured_bytes: u64) -> u64 {
    if video_active {
        captured_bytes
    } else {
        captured_bytes / 5
    }
}

/// Bytes-per-GB the Electron message used for its `.toFixed(1)` GB string
/// (`1_073_741_824` = 1024³, see `preflight.ts:55`).
const BYTES_PER_GB: f64 = 1_073_741_824.0;

/// Decide whether the free disk space warrants a finding.
///
/// Direct port of the Electron disk-space branch (`preflight.ts:52-61`): the
/// threshold is 4 GB when video is active, else 500 MB; below it we raise an
/// `error`/`disk` finding whose message includes the free space as GB with one
/// decimal (the `.toFixed(1)` formatting). At or above the threshold there is no
/// finding.
pub fn disk_space_finding(free_bytes: u64, video_active: bool) -> Option<PreflightFinding> {
    let min = if video_active {
        MIN_DISK_VIDEO_BYTES
    } else {
        MIN_DISK_AUDIO_BYTES
    };
    if free_bytes < min {
        Some(
            PreflightFinding::error(PreflightCategory::Disk, PreflightCode::DiskLow)
                .param("gb", format_gb(free_bytes)),
        )
    } else {
        None
    }
}

/// Format a byte count as GB with one decimal place, matching JS
/// `(bytes / 1_073_741_824).toFixed(1)`.
fn format_gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / BYTES_PER_GB)
}

/// Whether a recording will actually capture video, mirroring the Electron
/// `videoActive` predicate (`preflight.ts:52`): video is enabled AND a camera is
/// selected (by name OR by index).
pub fn video_active(settings: &crate::settings::Settings) -> bool {
    settings.video_enabled
        && (settings.video_device_name.is_some() || settings.video_device_index.is_some())
}

/// The already-gathered facts the `src-tauri` I/O layer hands to
/// [`assemble_findings`]. Keeping this a plain value struct means the whole
/// decision is testable without touching a disk or a device.
#[derive(Debug, Clone, Copy)]
pub struct PreflightFacts {
    /// The bundled ffmpeg binary could not be resolved/run.
    pub ffmpeg_missing: bool,
    /// The save folder exists and a probe file could be written + removed.
    pub folder_writable: bool,
    /// Free bytes on the save-folder volume, when the platform could report it.
    /// `None` skips the space check (Electron's `statfs`-unsupported branch).
    pub free_bytes: Option<u64>,
    /// Whether the recording will capture video (raises the GB threshold).
    pub video_active: bool,
    /// macOS microphone permission was explicitly denied/restricted. `false` on
    /// platforms / builds where we cannot query it (best-effort — see the
    /// `src-tauri` `preflight` module note about the deferred permission probe).
    pub mic_denied: bool,
    /// macOS camera permission denied/restricted (only relevant when
    /// [`Self::video_active`]).
    pub cam_denied: bool,
    /// The audio device named in settings was found among the enumerated inputs.
    ///
    /// **`true` also means "nothing to check"** — no device configured, or the
    /// enumeration itself failed. The shell can only ever positively establish
    /// ABSENCE (a configured name that no enumerated input matches), and a check
    /// that cannot run must never cry wolf: a false "your mixer is gone" on a
    /// Sunday morning is worse than no check at all, because it sends a
    /// volunteer hunting for a cable that is already plugged in.
    pub device_present: bool,
}

/// Assemble the preflight findings from the gathered facts, in the SAME order
/// the Electron `runPreflight` produced them (`preflight.ts:33-95`):
///   1. ffmpeg binary missing            → error/device
///   2. save folder not writable         → error/disk
///   3. low free space                   → error/disk  (via [`disk_space_finding`])
///   4. mic permission denied (macOS)    → error/device
///   5. cam permission denied (macOS)    → error/device  (only when video active)
///
/// The cloud-connectivity and device-name-mismatch findings the Electron build
/// also raised need live I/O (an HTTP probe / a device resolve) that belongs to
/// later phases; they are deliberately NOT synthesised here so the function
/// stays a pure decision over the facts we actually have in F2.2.
pub fn assemble_findings(facts: PreflightFacts) -> Vec<PreflightFinding> {
    let mut findings = Vec::new();

    if facts.ffmpeg_missing {
        findings.push(PreflightFinding::error(
            PreflightCategory::Device,
            PreflightCode::FfmpegMissing,
        ));
    }

    // The minimal half of the device-name-mismatch finding the Electron build
    // had and the note above said was deferred: we now at least say when the
    // CONFIGURED device is not among the enumerated inputs. Which device it is
    // (and what to plug in instead) is the `device_missing` backend warning's
    // job — it carries the name as a parameter. Full mismatch synthesis (did the
    // user mean this other, similarly-named input?) is still deferred.
    if !facts.device_present {
        findings.push(PreflightFinding::error(
            PreflightCategory::Device,
            PreflightCode::DeviceMissing,
        ));
    }

    if !facts.folder_writable {
        findings.push(PreflightFinding::error(
            PreflightCategory::Disk,
            PreflightCode::FolderNotWritable,
        ));
    }

    if let Some(free) = facts.free_bytes {
        if let Some(finding) = disk_space_finding(free, facts.video_active) {
            findings.push(finding);
        }
    }

    if facts.mic_denied {
        findings.push(PreflightFinding::error(
            PreflightCategory::Device,
            PreflightCode::MicDenied,
        ));
    }

    if facts.video_active && facts.cam_denied {
        findings.push(PreflightFinding::error(
            PreflightCategory::Device,
            PreflightCode::CameraDenied,
        ));
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn all_clear() -> PreflightFacts {
        PreflightFacts {
            ffmpeg_missing: false,
            folder_writable: true,
            free_bytes: Some(MIN_DISK_VIDEO_BYTES * 2),
            video_active: false,
            mic_denied: false,
            cam_denied: false,
            device_present: true,
        }
    }

    // ── during-recording low-disk guard ─────────────────────────────────────

    #[test]
    fn low_disk_should_stop_below_headroom() {
        let audio = min_disk_headroom_bytes(false);
        assert_eq!(audio, MIN_DISK_AUDIO_BYTES);
        assert!(low_disk_should_stop(audio - 1, audio), "just under → stop");
        assert!(
            !low_disk_should_stop(audio, audio),
            "exactly at → keep going"
        );
        assert!(
            !low_disk_should_stop(audio * 10, audio),
            "plenty → keep going"
        );
        // Video raises the bar to 4 GB.
        assert_eq!(min_disk_headroom_bytes(true), MIN_DISK_VIDEO_BYTES);
        assert!(low_disk_should_stop(
            MIN_DISK_AUDIO_BYTES,
            min_disk_headroom_bytes(true)
        ));
    }

    // ── finalize_reserve_bytes ───────────────────────────────────────────────

    #[test]
    fn finalize_reserve_video_is_the_full_capture_size() {
        // Video's decoupled delivery is a lossless -c copy remux → the delivery
        // lands at roughly the SAME size as the MKV capture.
        assert_eq!(finalize_reserve_bytes(true, 10_000_000_000), 10_000_000_000);
        assert_eq!(finalize_reserve_bytes(true, 0), 0);
    }

    #[test]
    fn finalize_reserve_audio_is_a_fifth_of_the_capture_size() {
        // Audio's WAV capture is lossy-encoded down to roughly a sixth of its
        // size for typical settings; /5 leaves margin.
        assert_eq!(finalize_reserve_bytes(false, 1_000_000_000), 200_000_000);
        assert_eq!(finalize_reserve_bytes(false, 0), 0);
    }

    #[test]
    fn low_disk_should_stop_accounts_for_the_finalize_reserve() {
        // Without the reserve, `free` clears the steady-state headroom — but the
        // finalise step needs more than that to actually complete. Feeding the
        // reserve into the SAME low_disk_should_stop call (headroom + reserve) is
        // how the engine's disk-tick uses this.
        let headroom = min_disk_headroom_bytes(false); // 500 MB
        let reserve = finalize_reserve_bytes(false, 2_000_000_000); // 400 MB
        let free = headroom + 100_000_000; // 600 MB — clears headroom alone
        assert!(
            !low_disk_should_stop(free, headroom),
            "sanity: headroom alone would NOT stop"
        );
        assert!(
            low_disk_should_stop(free, headroom + reserve),
            "the finalize reserve pushes the guard to stop first"
        );
    }

    // ── disk_space_finding ───────────────────────────────────────────────────

    #[test]
    fn disk_audio_under_threshold_warns() {
        let f = disk_space_finding(MIN_DISK_AUDIO_BYTES - 1, false).expect("finding");
        assert_eq!(f.severity, PreflightSeverity::Error);
        assert_eq!(f.category, PreflightCategory::Disk);
        assert_eq!(f.code, Some(PreflightCode::DiskLow));
    }

    #[test]
    fn disk_audio_at_or_over_threshold_is_clear() {
        assert!(disk_space_finding(MIN_DISK_AUDIO_BYTES, false).is_none());
        assert!(disk_space_finding(MIN_DISK_AUDIO_BYTES + 1, false).is_none());
    }

    #[test]
    fn disk_video_under_threshold_warns() {
        // 1 GB free is plenty for audio but under the 4 GB video bar.
        let one_gb = 1024 * 1024 * 1024;
        assert!(disk_space_finding(one_gb, false).is_none());
        let f = disk_space_finding(one_gb, true).expect("video threshold is higher");
        assert_eq!(f.category, PreflightCategory::Disk);
    }

    #[test]
    fn disk_video_at_or_over_threshold_is_clear() {
        assert!(disk_space_finding(MIN_DISK_VIDEO_BYTES, true).is_none());
        assert!(disk_space_finding(MIN_DISK_VIDEO_BYTES + 1, true).is_none());
    }

    #[test]
    fn disk_message_formats_gb_with_one_decimal() {
        // 1.5 GiB exactly → "1.5 GB" (matches JS toFixed(1) on 1024³ base).
        let one_and_half = (1.5 * BYTES_PER_GB) as u64;
        let f = disk_space_finding(one_and_half, true).expect("finding");
        assert!(
            f.message.contains("1.5 GB"),
            "expected 1.5 GB in: {}",
            f.message
        );
    }

    #[test]
    fn format_gb_rounds_like_tofixed() {
        // 250 MB → 0.2 GB; 0 bytes → 0.0 GB.
        assert_eq!(format_gb(250 * 1024 * 1024), "0.2");
        assert_eq!(format_gb(0), "0.0");
    }

    // ── video_active ─────────────────────────────────────────────────────────

    #[test]
    fn video_active_all_combinations() {
        // disabled → never active, even with a device selected.
        let s = Settings {
            video_enabled: false,
            video_device_name: Some("Cam".into()),
            video_device_index: Some(0),
            ..Default::default()
        };
        assert!(!video_active(&s));

        // enabled but no device → not active.
        let s = Settings {
            video_enabled: true,
            video_device_name: None,
            video_device_index: None,
            ..Default::default()
        };
        assert!(!video_active(&s));

        // enabled + name only → active.
        let s = Settings {
            video_enabled: true,
            video_device_name: Some("Cam".into()),
            video_device_index: None,
            ..Default::default()
        };
        assert!(video_active(&s));

        // enabled + index only → active.
        let s = Settings {
            video_enabled: true,
            video_device_name: None,
            video_device_index: Some(2),
            ..Default::default()
        };
        assert!(video_active(&s));

        // enabled + both → active.
        let s = Settings {
            video_enabled: true,
            video_device_name: Some("Cam".into()),
            video_device_index: Some(2),
            ..Default::default()
        };
        assert!(video_active(&s));
    }

    // ── assemble_findings ────────────────────────────────────────────────────

    #[test]
    fn assemble_all_clear_is_empty() {
        assert!(assemble_findings(all_clear()).is_empty());
    }

    #[test]
    fn assemble_orders_findings_like_electron() {
        // Trip every branch at once; assert the exact order ffmpeg → device →
        // folder → disk → mic → cam.
        let facts = PreflightFacts {
            ffmpeg_missing: true,
            folder_writable: false,
            free_bytes: Some(0),
            video_active: true,
            mic_denied: true,
            cam_denied: true,
            device_present: false,
        };
        let findings = assemble_findings(facts);
        assert_eq!(findings.len(), 6);
        assert_eq!(findings[0].category, PreflightCategory::Device); // ffmpeg
        assert_eq!(findings[0].code, Some(PreflightCode::FfmpegMissing));
        assert_eq!(findings[1].category, PreflightCategory::Device); // device gone
        assert_eq!(findings[1].code, Some(PreflightCode::DeviceMissing));
        assert_eq!(findings[2].category, PreflightCategory::Disk); // folder
        assert_eq!(findings[2].code, Some(PreflightCode::FolderNotWritable));
        assert_eq!(findings[3].category, PreflightCategory::Disk); // free space
        assert_eq!(findings[3].code, Some(PreflightCode::DiskLow));
        assert_eq!(findings[4].category, PreflightCategory::Device); // mic
        assert_eq!(findings[4].code, Some(PreflightCode::MicDenied));
        assert_eq!(findings[5].category, PreflightCategory::Device); // cam
        assert_eq!(findings[5].code, Some(PreflightCode::CameraDenied));
    }

    #[test]
    fn a_missing_configured_device_is_an_error_finding_on_its_own() {
        let findings = assemble_findings(PreflightFacts {
            device_present: false,
            ..all_clear()
        });
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, PreflightSeverity::Error);
        assert_eq!(findings[0].category, PreflightCategory::Device);
    }

    #[test]
    fn an_unanswerable_device_check_raises_nothing() {
        // `device_present: true` is also what "no device configured" and "the
        // enumeration failed" report. Neither may produce a finding: sending a
        // volunteer to re-seat a cable that is already seated, on a Sunday
        // morning, is worse than saying nothing.
        assert!(assemble_findings(all_clear()).is_empty());
    }

    #[test]
    fn assemble_skips_disk_check_when_free_bytes_unknown() {
        // None free_bytes mirrors Electron's statfs-unsupported branch: no
        // disk-space finding, but the other checks still run.
        let facts = PreflightFacts {
            free_bytes: None,
            ffmpeg_missing: true,
            ..all_clear()
        };
        let findings = assemble_findings(facts);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Some(PreflightCode::FfmpegMissing));
    }

    #[test]
    fn assemble_cam_finding_only_when_video_active() {
        // cam_denied but video NOT active → no camera finding.
        let facts = PreflightFacts {
            cam_denied: true,
            video_active: false,
            ..all_clear()
        };
        assert!(assemble_findings(facts).is_empty());

        // cam_denied AND video active → camera finding.
        let facts = PreflightFacts {
            cam_denied: true,
            video_active: true,
            // bump free space over the video bar so disk doesn't also fire.
            free_bytes: Some(MIN_DISK_VIDEO_BYTES + 1),
            ..all_clear()
        };
        let findings = assemble_findings(facts);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Some(PreflightCode::CameraDenied));
    }

    #[test]
    fn assemble_writable_folder_with_low_space_raises_only_space() {
        let facts = PreflightFacts {
            folder_writable: true,
            free_bytes: Some(MIN_DISK_AUDIO_BYTES - 1),
            ..all_clear()
        };
        let findings = assemble_findings(facts);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Some(PreflightCode::DiskLow));
    }

    #[test]
    fn severity_and_category_serialise_to_electron_strings() {
        assert_eq!(
            serde_json::to_string(&PreflightSeverity::Warn).unwrap(),
            "\"warn\""
        );
        assert_eq!(
            serde_json::to_string(&PreflightSeverity::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&PreflightCategory::Disk).unwrap(),
            "\"disk\""
        );
        assert_eq!(
            serde_json::to_string(&PreflightCategory::Device).unwrap(),
            "\"device\""
        );
        assert_eq!(
            serde_json::to_string(&PreflightCategory::Wake).unwrap(),
            "\"wake\""
        );
        // Finding keys are camelCase, matching the Electron interface.
        let f = PreflightFinding::error(PreflightCategory::Disk, PreflightCode::DiskLow);
        let v = serde_json::to_value(&f).unwrap();
        let obj = v.as_object().unwrap();
        assert!(obj.contains_key("severity"));
        assert!(obj.contains_key("category"));
        assert!(obj.contains_key("message"));
        assert!(obj.contains_key("code"));
        assert!(obj.contains_key("params"));
        assert_eq!(obj["code"], "diskLow");
    }

    // ── F2-I18N-R2: koder, ikke prosa ───────────────────────────────────────

    /// Every code says something, in English, and no two say the same thing —
    /// two findings that read alike are two findings nobody can tell apart.
    #[test]
    fn every_preflight_code_has_a_distinct_english_reserve() {
        let all = [
            PreflightCode::FfmpegMissing,
            PreflightCode::DeviceMissing,
            PreflightCode::FolderNotWritable,
            PreflightCode::DiskLow,
            PreflightCode::MicDenied,
            PreflightCode::CameraDenied,
        ];
        let texts: Vec<&str> = all.iter().map(|c| c.as_str()).collect();
        assert!(texts.iter().all(|t| !t.trim().is_empty()));
        let uniq: std::collections::HashSet<_> = texts.iter().collect();
        assert_eq!(uniq.len(), texts.len());
        assert!(
            !texts.iter().any(|t| t.contains(['æ', 'ø', 'å'])),
            "the English reserve is not English"
        );
        // Each code names its own notification arm — a shared arm would make
        // two different problems produce the same notification.
        let alerts: std::collections::HashSet<_> = all.iter().map(|c| c.alert()).collect();
        assert_eq!(alerts.len(), all.len());
    }

    /// `param()` fills the reserve as it records the value. A reserve that
    /// still reads "Only {gb} GB free" is not a reserve.
    #[test]
    fn the_disk_reserve_is_a_finished_sentence() {
        // 1.5 GB is over the audio bar and under the video one.
        let f = disk_space_finding(1_610_612_736, true).unwrap();
        assert_eq!(f.code, Some(PreflightCode::DiskLow));
        assert_eq!(f.params.get("gb").map(String::as_str), Some("1.5"));
        assert!(f.message.contains("1.5 GB"), "{}", f.message);
        assert!(!f.message.contains('{'), "{}", f.message);
    }
}
