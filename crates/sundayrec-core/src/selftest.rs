//! Capture self-test decisions — pure, GUI-free, fs/network-free.
//!
//! Completes the long-deferred "Fase 3" capture verification. Given the **stderr
//! of a short capture run through the REAL recording argv**
//! ([`crate::capture::build_unified_capture_args`]) plus a few facts about the
//! produced file, this module decides `Pass | Warn | Fail` with concrete numbers
//! — so audio health becomes *measurable* and a stutter fix becomes *provable* on
//! the rig instead of a guess. The impure shell (`src-tauri/test_recording`)
//! spawns ffmpeg, stats the file, runs the `astats`/`silencedetect` passes, and
//! feeds the parsed facts into [`selftest_verdict`] here.
//!
//! Everything here is deterministic and unit-tested over canned ffmpeg-stderr
//! blobs — the same pattern as [`crate::progress`] / [`crate::test_recording`].
//!
//! ## Thresholds are a starting point — calibrate on the rig
//!
//! The Pass/Warn/Fail thresholds ([`FAIL_GAP_SEC`] …) are conservative defaults.
//! The first known-good capture on the real Behringer rig is the reference; tune
//! the consts from those numbers. They live as named consts precisely so that
//! re-calibration is a one-line, reviewable change.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::test_recording::{classify_signal, size_is_plausible, TestRecordingSignal};

// ── Stderr stat parsers (pure) ──────────────────────────────────────────────

/// Parse the LAST `<field>N` unsigned-integer value out of an ffmpeg stderr blob
/// (e.g. `drop=`, `dup=`). ffmpeg right-pads these in the progress line
/// (`... dup=0 drop=12 speed=...`); we tolerate leading whitespace and take the
/// leading digit run. Returns `0` when the field never appears (the common case
/// for an audio-only capture, which has no video frame accounting). Mirrors
/// [`crate::progress::parse_size_kb`]'s scan-for-token approach.
fn parse_last_uint_field(stderr: &str, field: &str) -> u64 {
    let mut last: u64 = 0;
    let mut search = stderr;
    while let Some(pos) = search.find(field) {
        let after = &search[pos + field.len()..];
        let digits: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(v) = digits.parse::<u64>() {
            last = v;
        }
        search = &search[pos + field.len()..];
    }
    last
}

/// ffmpeg's `drop=N` — frames it discarded. On an avfoundation/USB-clock
/// overflow this rises; for pure audio it usually stays 0 (no video frame
/// accounting), so it's mainly a video-capture + defense-in-depth signal.
pub fn parse_drop_count(stderr: &str) -> u64 {
    parse_last_uint_field(stderr, "drop=")
}

/// ffmpeg's `dup=N` — duplicated/padded frames (a CFR-padding / clock-mismatch
/// signal). Same audio caveat as [`parse_drop_count`].
pub fn parse_dup_count(stderr: &str) -> u64 {
    parse_last_uint_field(stderr, "dup=")
}

/// Phrases ffmpeg / the macOS+Windows capture backends print when the real-time
/// input buffer overruns or the clock jumps — the direct stutter signal for an
/// audio-only capture. CONSERVATIVE + lowercase-matched; refine against real rig
/// stderr (avfoundation's exact wording is what we calibrate here). Kept a plain
/// reviewable list rather than a regex so additions are obvious in review.
pub const XRUN_PHRASES: &[&str] = &[
    "too full",           // avfoundation: "... buffer ... too full, frame dropped!"
    "frame dropped",      // generic drop notice
    "overrun",            // coreaudio/wasapi overrun
    "underflow",          // buffer underflow
    "buffer is full",     // thread_queue full
    "non-monotonous dts", // timestamp jump / discontinuity
    "discontinuity",      // stream discontinuity
    "rtbufsize",          // "Real-time buffer ... full" mentions rtbufsize
];

/// Count xrun/overrun-class lines in an ffmpeg stderr blob (case-insensitive).
/// Each matching line counts once (a line mentioning two phrases still counts
/// once) so the number tracks *events*, not phrase hits.
pub fn parse_xrun_count(stderr: &str) -> u64 {
    stderr
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            XRUN_PHRASES.iter().any(|p| lower.contains(p))
        })
        .count() as u64
}

/// The phrasings ffmpeg / avfoundation actually print when the CAPTURE side
/// back-pressures or drops samples — the machine-observable signature of the
/// "hakkete" bug (2026-07-31: 15–56 % sample loss registered as a fully CLEAN
/// recording because these lines were logged but never counted; this list and
/// [`XRUN_PHRASES`] were disjoint). Single source of truth — the engine's
/// warning log and the telemetry counter both match against THIS list.
pub const CAPTURE_DROP_PHRASES: &[&str] = &[
    "thread message queue blocking",
    "consider raising the thread_queue_size",
    "audio queue overflow",
    "queue input is backward in time",
    "past duration",
    "non monotonically increasing dts",
    "packets dropped",
];

/// `true` if a PRE-LOWERCASED stderr line is a capture-drop warning.
pub fn is_capture_drop_line(lower: &str) -> bool {
    CAPTURE_DROP_PHRASES.iter().any(|p| lower.contains(p))
}

/// `true` if a PRE-LOWERCASED stderr line is an xrun/overrun-class event.
pub fn is_xrun_line(lower: &str) -> bool {
    XRUN_PHRASES.iter().any(|p| lower.contains(p))
}

/// Parse the LAST `time=HH:MM:SS.ss` value out of an ffmpeg stderr blob into
/// seconds — the captured duration. `time=N/A` (printed before the first frame)
/// is skipped. Returns `None` when no parseable `time=` appears. The continuity
/// check (expected − measured) is the primary audio-dropout signal.
pub fn parse_last_time_secs(stderr: &str) -> Option<f64> {
    const FIELD: &str = "time=";
    let mut last: Option<f64> = None;
    let mut search = stderr;
    while let Some(pos) = search.find(FIELD) {
        let after = &search[pos + FIELD.len()..].trim_start();
        // Leading token up to whitespace, e.g. "00:00:04.00".
        let token: &str = after.split_whitespace().next().unwrap_or("");
        if let Some(secs) = parse_hhmmss(token) {
            last = Some(secs);
        }
        search = &search[pos + FIELD.len()..];
    }
    last
}

/// Parse `HH:MM:SS.ss` (or `MM:SS.ss` / `SS.ss`) into seconds. Returns `None` for
/// `N/A` or anything non-numeric.
fn parse_hhmmss(token: &str) -> Option<f64> {
    if token.is_empty() || token.eq_ignore_ascii_case("n/a") {
        return None;
    }
    let mut secs = 0.0f64;
    for part in token.split(':') {
        let v: f64 = part.parse().ok()?;
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

/// Parse `silence_start: X` / `silence_end: Y` pairs from a `silencedetect`
/// stderr pass into `(start, end)` segments. An unmatched trailing
/// `silence_start` (silence ran to EOF) is closed at `eof_sec` when provided.
/// A take of continuous tone/speech that comes back with interior silence has
/// gaps — dropped audio.
pub fn parse_silence_segments(stderr: &str, eof_sec: Option<f64>) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut open_start: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(v) = field_after(line, "silence_start:") {
            open_start = Some(v);
        } else if let Some(v) = field_after(line, "silence_end:") {
            if let Some(s) = open_start.take() {
                if v > s {
                    out.push((s, v));
                }
            }
        }
    }
    if let (Some(s), Some(eof)) = (open_start, eof_sec) {
        if eof > s {
            out.push((s, eof));
        }
    }
    out
}

/// Total seconds covered by silence segments.
pub fn silence_total_sec(segments: &[(f64, f64)]) -> f64 {
    segments.iter().map(|(s, e)| (e - s).max(0.0)).sum()
}

/// Parse the leading float after a `marker:` in a line (e.g. `silence_start: 3.20`).
fn field_after(line: &str, marker: &str) -> Option<f64> {
    let idx = line.find(marker)?;
    let tail = line[idx + marker.len()..].trim_start();
    let token: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.' || *c == '+')
        .collect();
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

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
#[ts(export, export_to = "../../../src/lib/bindings/SelfTestFacts.ts")]
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
#[ts(export, export_to = "../../../src/lib/bindings/SelfTestVerdict.ts")]
#[serde(rename_all = "lowercase")]
pub enum SelfTestVerdict {
    Pass,
    Warn,
    Fail,
}

/// The self-test result: a verdict, the human reasons, and the flat numbers the
/// diagnose report + the user paste verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/SelfTestReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct SelfTestReport {
    /// `true` unless the verdict is `Fail` — i.e. capture works (possibly with a
    /// warning). Drives the diagnose `capture_ok` tri-state.
    pub ok: bool,
    pub verdict: SelfTestVerdict,
    pub reasons: Vec<String>,
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
    let mut verdict = SelfTestVerdict::Pass;
    let escalate =
        |v: SelfTestVerdict, why: String, reasons: &mut Vec<String>, cur: &mut SelfTestVerdict| {
            reasons.push(why);
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
            format!("Ingen lyd fanget (fil {} B er for liten)", f.size_bytes),
            &mut reasons,
            &mut verdict,
        );
    }
    if signal == TestRecordingSignal::Silent {
        escalate(
            SelfTestVerdict::Fail,
            "Stille opptak — ingen signal (sjekk enhet/gain)".to_string(),
            &mut reasons,
            &mut verdict,
        );
    }
    if gap_sec >= FAIL_GAP_SEC {
        escalate(
            SelfTestVerdict::Fail,
            format!("{gap_sec:.2}s manglende/stille lyd — hakking/dropp"),
            &mut reasons,
            &mut verdict,
        );
    }
    if f.drops >= FAIL_DROPS || f.xruns >= FAIL_XRUNS {
        escalate(
            SelfTestVerdict::Fail,
            format!("Mange dropp ({}) / xruns ({})", f.drops, f.xruns),
            &mut reasons,
            &mut verdict,
        );
    }

    // WARN conditions (only matter if not already Fail).
    if let (Some(forced), Some(native)) = (f.forced_sample_rate, f.native_sample_rate) {
        if forced != native {
            escalate(
                SelfTestVerdict::Warn,
                format!(
                    "Tvunget samplingsrate {forced} Hz ≠ enhetens {native} Hz (resampling kan gi dropp)"
                ),
                &mut reasons,
                &mut verdict,
            );
        }
    }
    if (WARN_GAP_SEC..FAIL_GAP_SEC).contains(&gap_sec) {
        escalate(
            SelfTestVerdict::Warn,
            format!("{gap_sec:.2}s liten gap/stillhet i opptaket"),
            &mut reasons,
            &mut verdict,
        );
    }
    if signal == TestRecordingSignal::Low {
        escalate(
            SelfTestVerdict::Warn,
            "Svakt signal — vurder å øke gain".to_string(),
            &mut reasons,
            &mut verdict,
        );
    }
    if (1..FAIL_DROPS).contains(&f.drops) || (1..FAIL_XRUNS).contains(&f.xruns) {
        escalate(
            SelfTestVerdict::Warn,
            format!("Noen dropp ({}) / xruns ({})", f.drops, f.xruns),
            &mut reasons,
            &mut verdict,
        );
    }

    if verdict == SelfTestVerdict::Pass {
        reasons.push("Jevnt opptak, ingen dropp".to_string());
    }

    SelfTestReport {
        ok: verdict != SelfTestVerdict::Fail,
        verdict,
        reasons,
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
/// passive-logging path. The recorder's stderr reader feeds every line through
/// [`RecordingTelemetry::observe_line`] and flags a starved IPC channel via
/// [`RecordingTelemetry::note_levels_dropped`]; the host stamps `duration_sec`/
/// `timestamp`/`exit_ok` at session end and persists it. Surfaced in the
/// diagnose report so the user pastes a *trend*, not a guess.
///
/// `drops`/`dups` track the max ffmpeg `drop=`/`dup=` seen (cumulative within one
/// ffmpeg process); across a split (a new process) this is the max single
/// segment, not the sum — an acceptable under-report for the common single-take
/// sermon. `xruns`/`levels_dropped` are event counts that accumulate correctly.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/RecordingTelemetry.ts")]
#[serde(rename_all = "camelCase")]
pub struct RecordingTelemetry {
    /// Max ffmpeg `drop=` (discarded frames; an avfoundation/USB-overflow signal).
    pub drops: u64,
    /// Max ffmpeg `dup=` (duplicated frames; a clock-mismatch signal).
    pub dups: u64,
    /// xrun/overrun-class stderr lines seen — the direct stutter signal.
    pub xruns: u64,
    /// Times the live-levels IPC `try_send` hit a FULL channel — the direct
    /// renderer/IPC-starvation signal ("recording mode lags"). A rising count
    /// means the UI couldn't keep up and back-pressure was building.
    pub levels_dropped: u64,
    /// Capture back-pressure / sample-drop warnings from ffmpeg's own stderr
    /// ([`CAPTURE_DROP_PHRASES`]) — the direct "samples were thrown away" count.
    #[serde(default)]
    pub capture_drop_lines: u64,
    /// Non-levels reader messages (Progress/Silence/Error/Started) dropped on a
    /// full channel. Observability loss only — NEVER capture loss (the whole
    /// point of the all-`try_send` reader).
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
    /// (whole frames' worth of f32 samples). The native analogue of the
    /// xrun/capture-drop stderr counters; 0 on the ffmpeg path.
    #[serde(default)]
    pub ring_overrun_samples: u64,
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
    /// Fold one ffmpeg stderr line into the counters. Cheap; call on the
    /// low-rate lines (NOT the high-rate per-frame level lines, which carry no
    /// drop/dup/xrun fields anyway).
    pub fn observe_line(&mut self, line: &str) {
        self.observe_line_prelowered(&line.to_lowercase());
    }

    /// [`Self::observe_line`] for a caller that already lowercased the line —
    /// the recorder's reader classifies every line against several phrase lists
    /// and must pay for AT MOST one allocation per line (it drains the pipe
    /// ffmpeg blocks on; per-line cost is capture-health-critical).
    pub fn observe_line_prelowered(&mut self, lower: &str) {
        self.drops = self.drops.max(parse_drop_count(lower));
        self.dups = self.dups.max(parse_dup_count(lower));
        if is_xrun_line(lower) {
            self.xruns = self.xruns.saturating_add(1);
        }
        if is_capture_drop_line(lower) {
            self.capture_drop_lines = self.capture_drop_lines.saturating_add(1);
        }
    }

    /// Record that a live-levels message was dropped because the IPC channel was
    /// full (the renderer/event loop couldn't drain fast enough).
    pub fn note_levels_dropped(&mut self) {
        self.levels_dropped = self.levels_dropped.saturating_add(1);
    }

    /// Record that a non-levels reader message was dropped on a full channel
    /// (observability loss, deliberately preferred over reader back-pressure).
    pub fn note_msg_dropped(&mut self) {
        self.msgs_dropped = self.msgs_dropped.saturating_add(1);
    }

    /// Whether these counters indicate a degraded recording (dropped audio,
    /// capture back-pressure, measured duration loss, or IPC starvation). Used
    /// to raise a diagnose finding.
    pub fn is_degraded(&self) -> bool {
        self.drops > 0
            || self.xruns > 0
            || self.levels_dropped > 0
            || self.capture_drop_lines > 0
            || self.ring_overrun_samples > 0
            || self.loss_pct >= DURATION_LOSS_FAIL_PCT
    }
}

/// Wall-clock allowance for ffmpeg spawn + device open before the first sample
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
        drops: t.drops,
        dups: t.dups,
        // Capture-drop lines ARE xrun-class events for verdict purposes; a
        // native ring overrun counts as one xrun event (its MAGNITUDE already
        // shows up as duration loss — the overrun samples never reach disk).
        xruns: t
            .xruns
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
    fn drop_dup_parse_last_and_default_zero() {
        let s = "frame=10 dup=0 drop=2 speed=1x\nframe=20 dup=1 drop=5 speed=1x";
        assert_eq!(parse_drop_count(s), 5);
        assert_eq!(parse_dup_count(s), 1);
        // Audio-only capture has no frame accounting → fields absent → 0.
        assert_eq!(parse_drop_count("size=256kB time=00:00:05.00"), 0);
    }

    #[test]
    fn xrun_counts_events_not_phrases() {
        let s = "\
[avfoundation @ 0x1] real-time buffer too full, frame dropped!
normal progress line size=10kB
[avfoundation @ 0x1] Non-monotonous DTS in output stream
clean line";
        // line 1 matches two phrases ("too full" + "frame dropped") → counts once.
        assert_eq!(parse_xrun_count(s), 2);
        assert_eq!(parse_xrun_count("all good here"), 0);
    }

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

    #[test]
    fn time_parses_last_and_skips_na() {
        let s = "time=N/A\nsize=10kB time=00:00:04.00\nsize=20kB time=00:00:09.50";
        assert_eq!(parse_last_time_secs(s), Some(9.5));
        assert_eq!(parse_last_time_secs("no time here"), None);
    }

    #[test]
    fn hhmmss_forms() {
        assert_eq!(parse_hhmmss("00:00:04.00"), Some(4.0));
        assert_eq!(parse_hhmmss("01:02:03"), Some(3723.0));
        assert_eq!(parse_hhmmss("12.5"), Some(12.5));
        assert_eq!(parse_hhmmss("N/A"), None);
    }

    #[test]
    fn silence_segments_pairs_and_eof_close() {
        let s = "\
[silencedetect] silence_start: 2.0
[silencedetect] silence_end: 2.5 | silence_duration: 0.5
[silencedetect] silence_start: 8.0";
        // closed pair (0.5) + open one closed at eof 10.0 (2.0)
        let segs = parse_silence_segments(s, Some(10.0));
        assert_eq!(segs, vec![(2.0, 2.5), (8.0, 10.0)]);
        assert!((silence_total_sec(&segs) - 2.5).abs() < 1e-9);
        // without eof, the trailing open start is dropped
        assert_eq!(parse_silence_segments(s, None), vec![(2.0, 2.5)]);
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
    fn telemetry_observe_line_accumulates() {
        let mut t = RecordingTelemetry::default();
        t.observe_line("frame=10 dup=1 drop=3 speed=1x");
        t.observe_line("frame=20 dup=0 drop=2 speed=1x"); // drop/dup are max, not last
        t.observe_line("[avfoundation] real-time buffer too full, frame dropped!");
        t.observe_line("size=256kB time=00:00:05.00"); // clean progress line
        assert_eq!(t.drops, 3, "max drop across lines");
        assert_eq!(t.dups, 1);
        assert_eq!(t.xruns, 1, "one xrun-class line");
        assert!(t.is_degraded());
    }

    #[test]
    fn telemetry_counts_capture_drop_lines() {
        // The 2026-07-31 blindness: these avfoundation phrasings were logged but
        // never counted — 28 % sample loss registered as a clean recording.
        let mut t = RecordingTelemetry::default();
        t.observe_line("[avfoundation @ 0x0] Thread message queue blocking; consider raising the thread_queue_size option");
        t.observe_line("[aist#0:0] 100 packets dropped");
        t.observe_line("size=256kB time=00:00:05.00");
        assert_eq!(t.capture_drop_lines, 2);
        assert!(t.is_degraded(), "capture drops must degrade the verdict");
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
    fn telemetry_levels_dropped_counts() {
        let mut t = RecordingTelemetry::default();
        assert!(!t.is_degraded());
        t.note_levels_dropped();
        t.note_levels_dropped();
        assert_eq!(t.levels_dropped, 2);
        assert!(t.is_degraded());
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
}

/// Property tests (E5.8) — [`RecordingTelemetry::observe_line`] is on the
/// recorder's hot path: EVERY line ffmpeg writes to stderr for the life of a
/// real recording is folded through it (see its own doc comment on the
/// per-line allocation budget). A panic here would take capture down mid-take;
/// this is the same "stderr is unstructured free text we don't control" case
/// [`crate::progress::parse_size_kb`] guards, just for the drop/dup/xrun/time/
/// silence side of the same stream.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Arbitrary stderr-shaped lines must never panic the always-on telemetry
        /// path — this runs unconditionally on every recording, not just self-tests.
        #[test]
        fn observe_line_never_panics(line in ".{0,500}") {
            let mut t = RecordingTelemetry::default();
            t.observe_line(&line);
        }

        /// [`parse_last_time_secs`] and [`parse_silence_segments`] see the exact
        /// same unstructured stderr; neither may panic on it.
        #[test]
        fn time_and_silence_parsers_never_panic(blob in ".{0,500}") {
            let _ = parse_last_time_secs(&blob);
            let _ = parse_silence_segments(&blob, Some(1.0));
            let _ = parse_silence_segments(&blob, None);
        }

        /// Every segment [`parse_silence_segments`] emits must be a genuine,
        /// positive-duration span (`end > start`) — the same non-negative-gap
        /// invariant the hand-written `silence_start`/`silence_end` pairing
        /// already enforces (`if v > s`), now checked over arbitrary blobs
        /// instead of only the hand-picked fixtures.
        #[test]
        fn silence_segments_are_always_positive_duration(blob in ".{0,500}", eof in prop::option::of(0.0f64..1e6)) {
            for (start, end) in parse_silence_segments(&blob, eof) {
                prop_assert!(end > start);
            }
        }

        /// The always-on drop/dup counters must read identically whichever size
        /// unit spelling (`kB` vs the ffmpeg-7.1-renamed `KiB`) happens to sit
        /// elsewhere on the SAME line — the unit rename that once broke
        /// [`crate::progress::parse_size_kb`] must not have a sibling bug here,
        /// since `observe_line` folds the whole line, not just the `size=` field.
        #[test]
        fn drop_dup_counts_are_unaffected_by_the_size_unit_spelling(
            drop in 0u64..=10_000_000,
            dup in 0u64..=10_000_000,
            unit in prop_oneof![Just("kB"), Just("KiB")],
        ) {
            let line = format!(
                "frame=1 drop={drop} dup={dup} size=  100{unit} time=00:00:01.00 bitrate=1.0kbits/s"
            );
            let mut t = RecordingTelemetry::default();
            t.observe_line(&line);
            prop_assert_eq!(t.drops, drop);
            prop_assert_eq!(t.dups, dup);
        }
    }
}
