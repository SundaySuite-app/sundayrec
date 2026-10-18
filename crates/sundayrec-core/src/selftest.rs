//! Capture self-test decisions — pure, GUI-free, fs/network-free.
//!
//! Completes the long-deferred "Fase 3" capture verification. Given the facts
//! of a capture — the native engine's own counts (frames, ring overruns, the
//! xruns the audio system reported) and the produced file (ffprobe's duration,
//! size, astats level) — this module decides `Pass | Warn | Fail` with concrete
//! numbers, so audio health becomes *measurable* and a stutter fix becomes
//! *provable* on the rig instead of a guess. The impure shell
//! (`src-tauri/test_recording`, the recorder's session end) gathers the facts
//! and feeds them into [`selftest_verdict`] here.
//!
//! (Until C3b this module also parsed ffmpeg's capture stderr — `drop=`/`dup=`,
//! xrun phrases, `time=`, `silencedetect` — for the ffmpeg capture. That capture
//! is gone, and the parsers with it; [`RecordingTelemetry`] keeps the fields
//! they filled so recording history written by older versions still reads.)
//!
//! Everything here is deterministic and unit-tested.
//!
//! ## Thresholds are a starting point — calibrate on the rig
//!
//! The Pass/Warn/Fail thresholds ([`FAIL_GAP_SEC`] …) are conservative defaults.
//! The first known-good capture on the real Behringer rig is the reference; tune
//! the consts from those numbers. They live as named consts precisely so that
//! re-calibration is a one-line, reviewable change.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::telemetry::QualityReason;
use crate::test_recording::{classify_signal, size_is_plausible, TestRecordingSignal};

// ── Verdict ─────────────────────────────────────────────────────────────────

/// A continuous-take gap (missing duration + interior silence) at/above this is a
/// real dropout → `Fail`.
pub const FAIL_GAP_SEC: f64 = 1.0;
/// Any gap at/above this but below [`FAIL_GAP_SEC`] → `Warn`.
pub const WARN_GAP_SEC: f64 = 0.3;
/// Drop/xrun counts at/above these → `Fail`; ≥1 but below → `Warn`.
pub const FAIL_DROPS: u64 = 10;
pub const FAIL_XRUNS: u64 = 5;

/// The facts the impure shell feeds in after running the capture + analysis.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "SelfTestFacts.ts")]
#[serde(rename_all = "camelCase")]
pub struct SelfTestFacts {
    /// The `-t N` duration we asked ffmpeg to capture.
    pub expected_sec: f64,
    /// Actual captured duration (last `time=`, or ffprobe of the file).
    pub measured_sec: f64,
    pub drops: u64,
    pub dups: u64,
    pub xruns: u64,
    /// Output file size in bytes (the "did it capture anything" floor).
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// Strongest `astats` RMS dB over the take, or `None` if unmeasured.
    pub strongest_rms_db: Option<f64>,
    /// Total interior-silence seconds from a `silencedetect` pass over the take.
    pub silence_total_sec: f64,
    /// The device's native sample rate (if known).
    pub native_sample_rate: Option<u32>,
    /// The sample rate the settings FORCE (`Some` ⇒ ffmpeg resamples; a mismatch
    /// with `native_sample_rate` is a known choppiness cause).
    pub forced_sample_rate: Option<u32>,
}

/// Pass/Warn/Fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "SelfTestVerdict.ts")]
#[serde(rename_all = "lowercase")]
pub enum SelfTestVerdict {
    Pass,
    Warn,
    Fail,
}

/// The self-test result: a verdict, the human reasons, and the flat numbers the
/// diagnose report + the user paste verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "SelfTestReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct SelfTestReport {
    /// `true` unless the verdict is `Fail` — i.e. capture works (possibly with a
    /// warning). Drives the diagnose `capture_ok` tri-state.
    pub ok: bool,
    pub verdict: SelfTestVerdict,
    /// WHY, in ENGLISH prose, rendered verbatim by whoever shows the report.
    ///
    /// ⚠️ These sentences are written HERE, in Rust, and the quality banner
    /// showed them word for word in all seven UI languages — a German volunteer
    /// read «Svakt signal — vurder å øke gain». Rust cannot translate them
    /// (the UI language lives in the settings row, and this function is pure),
    /// so the fix was to stop asking it to: [`Self::reason_codes`] carries the
    /// same list as machine-readable codes for the renderer to localise
    /// (`QUALITY_REASON_KEYS` in `app/pages/record/record-core.ts`).
    ///
    /// The prose is the RESERVE, and F2-I18N-R2 made it English: it lands in
    /// the diagnose report — one long support artefact whose reader is whoever
    /// maintains the app, not the volunteer — and in a shell that meets a code
    /// its catalogue predates. A stable sentence is the point; which language
    /// it is stable in is settled by who reads it.
    pub reasons: Vec<String>,
    /// The same conditions as [`Self::reasons`], as codes — one per prose line,
    /// in the same order, set by the same branch.
    ///
    /// Not re-derived from the numbers afterwards (that is
    /// [`crate::telemetry::derive_reason_codes`]'s job, and a second reading is
    /// a second thing to drift): every `escalate` here pushes the sentence and
    /// its code together, so "the codes follow the prose 1:1" is true by
    /// construction and asserted over a fact matrix by
    /// `every_prose_reason_carries_its_code`.
    ///
    /// `#[serde(default)]` so a report stored before the field existed still
    /// deserialises — as an empty list, which is honestly "this payload predates
    /// codes" rather than "nothing was wrong".
    #[serde(default)]
    pub reason_codes: Vec<QualityReason>,
    pub drops: u64,
    pub dups: u64,
    pub xruns: u64,
    pub expected_sec: f64,
    pub measured_sec: f64,
    /// Combined gap = missing duration + interior silence.
    pub gap_sec: f64,
    pub rms_db: Option<f64>,
    /// `silence_total_sec / measured_sec` (0 when nothing captured).
    pub silence_ratio: f64,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub native_sample_rate: Option<u32>,
    pub forced_sample_rate: Option<u32>,
}

/// Decide the verdict from the facts. Pure + fully unit-tested.
///
/// Fails on: nothing captured (size floor), a silent take, a ≥[`FAIL_GAP_SEC`]
/// gap, or ≥[`FAIL_DROPS`]/[`FAIL_XRUNS`]. Warns on: a forced≠native sample rate,
/// a ≥[`WARN_GAP_SEC`] gap, a low signal, or any drops/xruns below the fail line.
pub fn selftest_verdict(f: &SelfTestFacts) -> SelfTestReport {
    let duration_shortfall = (f.expected_sec - f.measured_sec).max(0.0);
    let gap_sec = duration_shortfall + f.silence_total_sec;
    let silence_ratio = if f.measured_sec > 0.0 {
        (f.silence_total_sec / f.measured_sec).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let signal = classify_signal(f.strongest_rms_db);

    let mut reasons: Vec<String> = Vec::new();
    let mut codes: Vec<QualityReason> = Vec::new();
    let mut verdict = SelfTestVerdict::Pass;
    // The prose and its code are pushed by the SAME call, so the two lists
    // cannot fall out of step — the banner's translation depends on the code at
    // index i meaning the sentence at index i.
    let escalate = |v: SelfTestVerdict,
                    why: String,
                    code: QualityReason,
                    reasons: &mut Vec<String>,
                    codes: &mut Vec<QualityReason>,
                    cur: &mut SelfTestVerdict| {
        reasons.push(why);
        codes.push(code);
        // Fail dominates Warn dominates Pass.
        if v == SelfTestVerdict::Fail
            || (v == SelfTestVerdict::Warn && *cur == SelfTestVerdict::Pass)
        {
            *cur = v;
        }
    };

    // FAIL conditions.
    if !size_is_plausible(f.size_bytes) {
        escalate(
            SelfTestVerdict::Fail,
            format!("no audio captured (file {} B is too small)", f.size_bytes),
            QualityReason::NoAudioCaptured,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }
    if signal == TestRecordingSignal::Silent {
        escalate(
            SelfTestVerdict::Fail,
            "silent recording — no signal (check the device/gain)".to_string(),
            QualityReason::SilentTake,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }
    if gap_sec >= FAIL_GAP_SEC {
        escalate(
            SelfTestVerdict::Fail,
            format!("{gap_sec:.2}s of missing/silent audio — stutter/drops"),
            QualityReason::LargeGap,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }
    if f.drops >= FAIL_DROPS || f.xruns >= FAIL_XRUNS {
        escalate(
            SelfTestVerdict::Fail,
            format!("many drops ({}) / xruns ({})", f.drops, f.xruns),
            QualityReason::ManyDrops,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }

    // WARN conditions (only matter if not already Fail).
    if let (Some(forced), Some(native)) = (f.forced_sample_rate, f.native_sample_rate) {
        if forced != native {
            escalate(
                SelfTestVerdict::Warn,
                format!(
                    "forced sample rate {forced} Hz != the device's {native} Hz (resampling can cause drops)"
                ),
                QualityReason::ForcedRateMismatch,
                &mut reasons,
                &mut codes,
                &mut verdict,
            );
        }
    }
    if (WARN_GAP_SEC..FAIL_GAP_SEC).contains(&gap_sec) {
        escalate(
            SelfTestVerdict::Warn,
            format!("{gap_sec:.2}s small gap/silence in the recording"),
            QualityReason::SmallGap,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }
    if signal == TestRecordingSignal::Low {
        escalate(
            SelfTestVerdict::Warn,
            "weak signal — consider raising the gain".to_string(),
            QualityReason::LowSignal,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }
    if (1..FAIL_DROPS).contains(&f.drops) || (1..FAIL_XRUNS).contains(&f.xruns) {
        escalate(
            SelfTestVerdict::Warn,
            format!("some drops ({}) / xruns ({})", f.drops, f.xruns),
            QualityReason::SomeDrops,
            &mut reasons,
            &mut codes,
            &mut verdict,
        );
    }

    if verdict == SelfTestVerdict::Pass {
        reasons.push("Jevnt opptak, ingen dropp".to_string());
        codes.push(QualityReason::Clean);
    }

    SelfTestReport {
        ok: verdict != SelfTestVerdict::Fail,
        verdict,
        reasons,
        reason_codes: codes,
        drops: f.drops,
        dups: f.dups,
        xruns: f.xruns,
        expected_sec: f.expected_sec,
        measured_sec: f.measured_sec,
        gap_sec,
        rms_db: f.strongest_rms_db,
        silence_ratio,
        size_bytes: f.size_bytes,
        native_sample_rate: f.native_sample_rate,
        forced_sample_rate: f.forced_sample_rate,
    }
}

// ── Always-on recording telemetry ───────────────────────────────────────────

/// Health counters accumulated AUTOMATICALLY during a real recording — the
/// passive-logging path. The native engine folds each segment's counts in at
/// the segment's end (ring overruns, the xruns the audio system reported, the
/// writer's exact frame count); the finaliser adds the expected and measured
/// seconds per deliverable; the host stamps `duration_sec`/`timestamp`/
/// `exit_ok` at session end and persists it. Surfaced in the diagnose report
/// so the user pastes a *trend*, not a guess.
///
/// ## The ffmpeg-era fields (C3b)
///
/// `drops`, `dups`, `xruns`, `levels_dropped`, `capture_drop_lines` and
/// `msgs_dropped` were filled from the ffmpeg capture's stderr and its reader
/// channel. That capture left in C3b, so a new session leaves them at 0 — but
/// they stay: the history ring on disk (`recording-telemetry-history.json`)
/// holds sessions recorded by older versions, and the diagnose trend and the
/// verdict must keep reading those exactly as they were read.
///
/// The live ones — `native_xruns`, `ring_overrun_samples`, `native_frames_sec`,
/// `expected_sec`, `measured_sec` — are event counts and sums the engine adds
/// per segment / per deliverable, so they accumulate correctly across any
/// number of splits and reconnects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "RecordingTelemetry.ts")]
#[serde(rename_all = "camelCase")]
pub struct RecordingTelemetry {
    /// Frames the ffmpeg capture DISCARDED (summed `drop=` over its capture
    /// processes). ffmpeg-era: 0 since C3b; older rows keep theirs.
    pub drops: u64,
    /// Frames the ffmpeg capture DUPLICATED (summed `dup=`). ffmpeg-era: 0
    /// since C3b; older rows keep theirs.
    pub dups: u64,
    /// xrun/overrun-class lines in the ffmpeg capture's stderr. ffmpeg-era: 0
    /// since C3b (see [`Self::native_xruns`]); older rows keep theirs.
    pub xruns: u64,
    /// Xruns the audio system itself reported to the NATIVE engine (A3c): cpal
    /// 0.18 delivers a CoreAudio/WASAPI buffer glitch through the stream's
    /// error callback as `ErrorKind::Xrun`, and the engine counts each one.
    /// A real count from the OS, not a phrase scraped from a log.
    #[serde(default)]
    pub native_xruns: u64,
    /// Times the ffmpeg reader's live-levels `try_send` hit a FULL channel.
    /// ffmpeg-era: 0 since C3b; older rows keep theirs.
    pub levels_dropped: u64,
    /// Sample-drop warnings in the ffmpeg capture's stderr. ffmpeg-era: 0
    /// since C3b; older rows keep theirs.
    #[serde(default)]
    pub capture_drop_lines: u64,
    /// ffmpeg reader messages dropped on a full channel (observability loss
    /// only). ffmpeg-era: 0 since C3b; older rows keep theirs.
    #[serde(default)]
    pub msgs_dropped: u64,
    /// Wall-clock seconds the session SHOULD have captured (sum of segment
    /// spans, host-stamped at finalize). 0 until the truth-measurement runs.
    #[serde(default)]
    pub expected_sec: f64,
    /// Media seconds actually present in the delivered file(s) (ffprobe at
    /// finalize). THE truth the 2026-07-31 incident lacked: wall clock said
    /// 46.6 s while the file held 20.4 s and everything reported "clean".
    #[serde(default)]
    pub measured_sec: f64,
    /// Percent of expected audio MISSING from the delivery (startup-allowance
    /// adjusted). ≥ [`DURATION_LOSS_FAIL_PCT`] ⇒ degraded + user-visible.
    #[serde(default)]
    pub loss_pct: f64,
    /// Samples the NATIVE engine's real-time callback dropped on ring overrun
    /// (whole frames' worth of f32 samples).
    #[serde(default)]
    pub ring_overrun_samples: u64,
    /// Camera frames the video track threw away this session because the
    /// encoder could not keep up (C3c; summed per fragment, so it carries across
    /// splits and reconnects). The same number the `video.frames_dropped`
    /// counter adds, and the one the recording overlay shows as a calm note.
    /// Audio is never touched by it. 0 for audio-only sessions and old rows.
    #[serde(default)]
    pub video_frames_dropped: u64,
    /// Media seconds implied by the native writer's EXACT frame count
    /// (frames/rate, summed per segment). Capture-side cross-check against the
    /// ffprobe-measured `measured_sec`: disagreement localizes a fault to
    /// capture vs delivery instantly. 0.0 on the ffmpeg path.
    #[serde(default)]
    pub native_frames_sec: f64,
    /// The Pass/Warn/Fail verdict computed at session end (None on legacy rows).
    #[serde(default)]
    pub report: Option<SelfTestReport>,
    /// Wall-clock length of the recording (host-stamped at session end).
    pub duration_sec: f64,
    /// ISO-8601 local timestamp of session end (host-stamped).
    pub timestamp: String,
    /// Whether the session ended cleanly (Stopped) vs Failed (host-stamped).
    pub exit_ok: bool,
}

impl RecordingTelemetry {
    /// Every xrun the session saw: the native engine's OS-reported
    /// [`Self::native_xruns`] plus, on a row recorded before C3b, the ffmpeg
    /// capture's stderr-derived [`Self::xruns`]. Use this wherever the number
    /// is shown to a person.
    pub fn xruns_total(&self) -> u64 {
        self.xruns.saturating_add(self.native_xruns)
    }

    /// Whether these counters indicate a degraded recording (dropped audio,
    /// capture back-pressure, measured duration loss, or IPC starvation). Used
    /// to raise a diagnose finding.
    pub fn is_degraded(&self) -> bool {
        self.drops > 0
            || self.xruns > 0
            || self.native_xruns > 0
            || self.levels_dropped > 0
            || self.capture_drop_lines > 0
            || self.ring_overrun_samples > 0
            || self.loss_pct >= DURATION_LOSS_FAIL_PCT
    }
}

/// Wall-clock allowance for the capture's device open before the first sample
/// lands (measured 0.3–2 s on the rig). Subtracted from the wall-clock span
/// before loss is computed so a healthy recording is 0 % loss, not 0.5–2 s
/// "missing".
pub const CAPTURE_STARTUP_ALLOWANCE_SEC: f64 = 2.0;

/// Missing ≥ this percent of the expected audio ⇒ degraded + user-visible
/// warning. (The 2026-07-31 incident was 15–56 %.)
pub const DURATION_LOSS_FAIL_PCT: f64 = 1.0;

/// Percent of `expected_sec` MISSING from `measured_sec` (0 when expected ≤ 0 —
/// a session shorter than the startup allowance can't be judged).
pub fn duration_loss_pct(expected_sec: f64, measured_sec: f64) -> f64 {
    if expected_sec <= 0.0 {
        return 0.0;
    }
    ((expected_sec - measured_sec).max(0.0) / expected_sec * 100.0).clamp(0.0, 100.0)
}

/// Build [`SelfTestFacts`] from a real recording's telemetry so the SAME
/// unit-tested [`selftest_verdict`] engine judges live sessions (its first real
/// caller — it sat unwired while recordings lost 15–56 % of their samples).
/// RMS/silence facts are unmeasured on the live path (`None`/0), which
/// [`crate::test_recording::classify_signal`] treats as not-silent.
pub fn facts_from_recording(t: &RecordingTelemetry, size_bytes: u64) -> SelfTestFacts {
    SelfTestFacts {
        expected_sec: (t.expected_sec - CAPTURE_STARTUP_ALLOWANCE_SEC).max(0.0),
        measured_sec: t.measured_sec,
        // ffmpeg-era counters: 0 for every session since C3b, but an older
        // history row keeps judging the way it was judged.
        drops: t.drops,
        dups: t.dups,
        // Capture-drop lines ARE xrun-class events for verdict purposes; the
        // native engine's OS-reported xruns are the same class, counted for
        // real; a native ring overrun counts as one xrun event (its MAGNITUDE
        // already shows up as duration loss — the overrun samples never reach
        // disk).
        xruns: t
            .xruns
            .saturating_add(t.native_xruns)
            .saturating_add(t.capture_drop_lines)
            .saturating_add(u64::from(t.ring_overrun_samples > 0)),
        size_bytes,
        strongest_rms_db: None,
        silence_total_sec: 0.0,
        native_sample_rate: None,
        forced_sample_rate: None,
    }
}

/// Push `item` onto a newest-last ring, trimming from the front to keep at most
/// `cap` entries. Pure helper for the rolling recording-telemetry history.
pub fn push_capped<T>(ring: &mut Vec<T>, item: T, cap: usize) {
    ring.push(item);
    if cap == 0 {
        ring.clear();
    } else if ring.len() > cap {
        let overflow = ring.len() - cap;
        ring.drain(0..overflow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_telemetry_fields_default_and_degrade() {
        // Old persisted rows (no native fields) must still deserialize.
        let legacy = r#"{"drops":0,"dups":0,"xruns":0,"levelsDropped":0,
            "durationSec":10.0,"timestamp":"t","exitOk":true}"#;
        let t: RecordingTelemetry = serde_json::from_str(legacy).expect("legacy row parses");
        assert_eq!(t.ring_overrun_samples, 0);
        assert_eq!(t.native_frames_sec, 0.0);
        assert!(!t.is_degraded());

        // A native ring overrun degrades the session and counts as one
        // xrun-class event in the verdict facts.
        let mut t = t;
        t.ring_overrun_samples = 4800;
        assert!(t.is_degraded());
        let facts = facts_from_recording(&t, 1_000_000);
        assert_eq!(facts.xruns, 1);
    }

    fn good_facts() -> SelfTestFacts {
        SelfTestFacts {
            expected_sec: 15.0,
            measured_sec: 15.0,
            drops: 0,
            dups: 0,
            xruns: 0,
            size_bytes: 400_000,
            strongest_rms_db: Some(-18.0),
            silence_total_sec: 0.0,
            native_sample_rate: Some(48_000),
            forced_sample_rate: None,
        }
    }

    #[test]
    fn clean_capture_passes() {
        let r = selftest_verdict(&good_facts());
        assert_eq!(r.verdict, SelfTestVerdict::Pass);
        assert!(r.ok);
        assert!((r.gap_sec - 0.0).abs() < 1e-9);
    }

    #[test]
    fn no_audio_fails() {
        let mut f = good_facts();
        f.size_bytes = 100; // below MIN_TEST_SIZE_BYTES
        let r = selftest_verdict(&f);
        assert_eq!(r.verdict, SelfTestVerdict::Fail);
        assert!(!r.ok);
    }

    #[test]
    fn silent_take_fails() {
        let mut f = good_facts();
        f.strongest_rms_db = Some(-60.0);
        assert_eq!(selftest_verdict(&f).verdict, SelfTestVerdict::Fail);
    }

    #[test]
    fn big_gap_fails_small_gap_warns() {
        let mut f = good_facts();
        f.silence_total_sec = 1.5; // ≥ FAIL_GAP_SEC
        assert_eq!(selftest_verdict(&f).verdict, SelfTestVerdict::Fail);

        let mut f2 = good_facts();
        f2.measured_sec = 14.6; // 0.4s shortfall → warn band
        let r2 = selftest_verdict(&f2);
        assert_eq!(r2.verdict, SelfTestVerdict::Warn);
        assert!(r2.ok, "warn still counts as working");
    }

    #[test]
    fn many_drops_fail_few_warn() {
        let mut f = good_facts();
        f.drops = 20;
        assert_eq!(selftest_verdict(&f).verdict, SelfTestVerdict::Fail);

        let mut f2 = good_facts();
        f2.xruns = 2; // below FAIL_XRUNS
        assert_eq!(selftest_verdict(&f2).verdict, SelfTestVerdict::Warn);
    }

    #[test]
    fn forced_rate_mismatch_warns() {
        let mut f = good_facts();
        f.forced_sample_rate = Some(48_000);
        f.native_sample_rate = Some(44_100);
        let r = selftest_verdict(&f);
        assert_eq!(r.verdict, SelfTestVerdict::Warn);
        assert!(r.reasons.iter().any(|x| x.contains("44100")));
    }

    #[test]
    fn silence_ratio_computed() {
        let mut f = good_facts();
        f.measured_sec = 10.0;
        f.silence_total_sec = 2.0;
        let r = selftest_verdict(&f);
        assert!((r.silence_ratio - 0.2).abs() < 1e-9);
    }

    #[test]
    fn native_xruns_degrade_and_reach_the_verdict() {
        let t = RecordingTelemetry {
            expected_sec: 62.0,
            measured_sec: 60.0,
            native_xruns: 3,
            ..Default::default()
        };
        assert!(t.is_degraded(), "an OS-reported xrun is a glitch");
        assert_eq!(t.xruns_total(), 3);
        // An older (ffmpeg-era) row's stderr count still adds up with it.
        let both = RecordingTelemetry {
            xruns: 2,
            native_xruns: 3,
            ..Default::default()
        };
        assert_eq!(both.xruns_total(), 5);
        let f = facts_from_recording(&t, 1_000_000);
        assert_eq!(f.xruns, 3);
    }

    /// C3b: a history row written by an ffmpeg-era version (with the stderr
    /// counters it carried) still reads, and is still judged the same way.
    #[test]
    fn an_ffmpeg_era_history_row_still_reads_and_still_degrades() {
        let row: RecordingTelemetry = serde_json::from_str(
            r#"{"drops":40,"dups":4,"xruns":2,"levelsDropped":1,"captureDropLines":3,
                "msgsDropped":0,"expectedSec":62.0,"measuredSec":60.0,"lossPct":0.0,
                "durationSec":62.0,"timestamp":"2026-08-01T11:00:00+02:00","exitOk":true}"#,
        )
        .expect("an old row deserialises");
        assert_eq!(row.drops, 40);
        assert!(row.is_degraded());
        let f = facts_from_recording(&row, 1_000_000);
        assert_eq!(f.drops, 40);
        assert_eq!(f.xruns, 2 + 3, "stderr xruns + capture-drop lines");
    }

    #[test]
    fn duration_loss_pct_basics() {
        assert_eq!(duration_loss_pct(100.0, 100.0), 0.0);
        let bibeltime = duration_loss_pct(100.0, 72.0); // the incident's 28 %
        assert!((bibeltime - 28.0).abs() < 1e-9, "got {bibeltime}");
        assert_eq!(duration_loss_pct(100.0, 110.0), 0.0); // longer than expected ≠ loss
        assert_eq!(duration_loss_pct(0.0, 5.0), 0.0); // too short to judge
        assert_eq!(duration_loss_pct(-1.0, 5.0), 0.0);
    }

    #[test]
    fn loss_pct_at_threshold_degrades() {
        let t = RecordingTelemetry {
            loss_pct: DURATION_LOSS_FAIL_PCT,
            ..Default::default()
        };
        assert!(t.is_degraded());
        let ok = RecordingTelemetry {
            loss_pct: DURATION_LOSS_FAIL_PCT - 0.1,
            ..Default::default()
        };
        assert!(!ok.is_degraded());
    }

    #[test]
    fn facts_from_recording_applies_startup_allowance_and_folds_drop_lines() {
        let t = RecordingTelemetry {
            expected_sec: 62.0,
            measured_sec: 59.9,
            xruns: 1,
            capture_drop_lines: 2,
            ..Default::default()
        };
        let f = facts_from_recording(&t, 1_000_000);
        assert_eq!(f.expected_sec, 62.0 - CAPTURE_STARTUP_ALLOWANCE_SEC);
        assert_eq!(f.measured_sec, 59.9);
        assert_eq!(f.xruns, 3, "capture-drop lines are xrun-class events");
        // 3 xruns (below FAIL_XRUNS=5) on an otherwise-clean take ⇒ Warn.
        let report = selftest_verdict(&f);
        assert_eq!(
            report.verdict,
            SelfTestVerdict::Warn,
            "{:?}",
            report.reasons
        );
        // At/above FAIL_XRUNS ⇒ Fail.
        let mut worse = f.clone();
        worse.xruns = FAIL_XRUNS;
        assert_eq!(selftest_verdict(&worse).verdict, SelfTestVerdict::Fail);
    }

    #[test]
    fn healthy_recording_facts_pass_the_verdict() {
        // classify_signal(None) must not flag a live recording (RMS unmeasured)
        // as silent, and a measured duration within the allowance is 0 % loss.
        let t = RecordingTelemetry {
            expected_sec: 3600.0,
            measured_sec: 3598.5,
            ..Default::default()
        };
        let f = facts_from_recording(&t, 500_000_000);
        let report = selftest_verdict(&f);
        assert_eq!(
            report.verdict,
            SelfTestVerdict::Pass,
            "{:?}",
            report.reasons
        );
        assert!(duration_loss_pct(f.expected_sec, f.measured_sec) < DURATION_LOSS_FAIL_PCT);
    }

    #[test]
    fn push_capped_keeps_newest() {
        let mut ring: Vec<u32> = Vec::new();
        for i in 0..5 {
            push_capped(&mut ring, i, 3);
        }
        assert_eq!(ring, vec![2, 3, 4], "newest-last, capped at 3");
        // cap 0 → always empty
        push_capped(&mut ring, 99, 0);
        assert!(ring.is_empty());
    }

    // ── The codes beside the prose ──────────────────────────────────────────

    /// Facts that trip every condition `selftest_verdict` knows, one per row,
    /// plus the clean case — the matrix the two lists are compared over.
    fn every_condition() -> Vec<(&'static str, SelfTestFacts)> {
        let clean = SelfTestFacts {
            expected_sec: 10.0,
            measured_sec: 10.0,
            silence_total_sec: 0.0,
            strongest_rms_db: Some(-20.0),
            size_bytes: 4_000_000,
            drops: 0,
            dups: 0,
            xruns: 0,
            native_sample_rate: Some(48_000),
            forced_sample_rate: Some(48_000),
        };
        vec![
            ("clean", clean.clone()),
            (
                "no audio captured",
                SelfTestFacts {
                    size_bytes: 0,
                    ..clean.clone()
                },
            ),
            (
                "silent take",
                SelfTestFacts {
                    strongest_rms_db: None,
                    ..clean.clone()
                },
            ),
            (
                "large gap",
                SelfTestFacts {
                    measured_sec: 2.0,
                    ..clean.clone()
                },
            ),
            (
                "many drops",
                SelfTestFacts {
                    drops: FAIL_DROPS,
                    ..clean.clone()
                },
            ),
            (
                "many xruns",
                SelfTestFacts {
                    xruns: FAIL_XRUNS,
                    ..clean.clone()
                },
            ),
            (
                "forced rate mismatch",
                SelfTestFacts {
                    forced_sample_rate: Some(44_100),
                    ..clean.clone()
                },
            ),
            (
                "small gap",
                SelfTestFacts {
                    silence_total_sec: (WARN_GAP_SEC + FAIL_GAP_SEC) / 2.0,
                    ..clean.clone()
                },
            ),
            (
                "low signal",
                SelfTestFacts {
                    strongest_rms_db: Some(-55.0),
                    ..clean.clone()
                },
            ),
            (
                "some drops",
                SelfTestFacts {
                    drops: 1,
                    ..clean.clone()
                },
            ),
            (
                "everything at once",
                SelfTestFacts {
                    size_bytes: 10,
                    measured_sec: 1.0,
                    strongest_rms_db: None,
                    drops: FAIL_DROPS,
                    xruns: FAIL_XRUNS,
                    forced_sample_rate: Some(44_100),
                    ..clean
                },
            ),
        ]
    }

    #[test]
    fn every_prose_reason_carries_its_code() {
        // The banner rendered these Norwegian sentences verbatim in all seven UI
        // languages. The codes are what the renderer translates instead — so
        // there must be exactly one per sentence, always, or the banner would
        // silently drop or invent a line.
        for (name, facts) in every_condition() {
            let r = selftest_verdict(&facts);
            assert_eq!(
                r.reasons.len(),
                r.reason_codes.len(),
                "{name}: {:?} vs {:?}",
                r.reasons,
                r.reason_codes
            );
            assert!(
                !r.reason_codes.is_empty(),
                "{name}: a verdict always says why"
            );
        }
    }

    #[test]
    fn the_codes_are_the_same_reading_the_wire_makes() {
        // `derive_reason_codes` re-reads the SAME conditions from the numbers
        // for telemetry. Two readings of one rule is exactly the seam this
        // codebase keeps finding bugs in, so it is pinned rather than hoped
        // for: if either side gains a condition the other lacks, this fails.
        for (name, facts) in every_condition() {
            let r = selftest_verdict(&facts);
            assert_eq!(
                r.reason_codes,
                crate::telemetry::derive_reason_codes(&r),
                "{name}"
            );
        }
    }

    #[test]
    fn a_clean_take_says_so_in_both_lists() {
        let (_, clean) = every_condition().remove(0);
        let r = selftest_verdict(&clean);
        assert_eq!(r.verdict, SelfTestVerdict::Pass);
        assert_eq!(r.reason_codes, vec![QualityReason::Clean]);
        assert_eq!(r.reasons.len(), 1);
    }

    #[test]
    fn an_older_stored_report_deserialises_without_codes() {
        // `reason_codes` is additive: a payload written before it existed must
        // still load, and must not claim a condition it never carried.
        let json = serde_json::json!({
            "ok": true,
            "verdict": "pass",
            "reasons": ["Jevnt opptak, ingen dropp"],
            "drops": 0,
            "dups": 0,
            "xruns": 0,
            "expectedSec": 10.0,
            "measuredSec": 10.0,
            "gapSec": 0.0,
            "rmsDb": -20.0,
            "silenceRatio": 0.0,
            "sizeBytes": 4_000_000,
            "nativeSampleRate": 48_000,
            "forcedSampleRate": 48_000,
        });
        let r: SelfTestReport = serde_json::from_value(json).unwrap();
        assert!(r.reason_codes.is_empty());
        assert_eq!(r.reasons.len(), 1);
    }
}
