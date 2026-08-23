//! The diagnostics markdown report — pure builder.
//!
//! Ported from the Electron main process `src/main/diagnostics.ts` (the
//! behavioural specification). That code gathered facts via I/O (ffmpeg
//! `-version`, device enumeration, capture test) and then assembled a markdown
//! report. Here we keep ONLY the assembly: [`build_report_markdown`] takes the
//! already-gathered facts in [`DiagnosticsInput`] and returns the markdown
//! string, so the formatting is deterministic and fully unit-tested. The
//! `src-tauri` `diagnostics` module performs the actual probing and feeds the
//! results in.
//!
//! ## Secrets cannot leak — by construction
//!
//! The Electron `sanitizeSettings` (`diagnostics.ts:172`) hand-picked which
//! settings fields went into the report, deliberately omitting passwords,
//! e-mail addresses and stream keys ("Innstillinger (alle, unntatt
//! passord/e-post)"). We go one better: [`SettingsSummary`] simply has no field
//! for any secret. There is no code path that can place a password / e-mail /
//! stream key into the report because the input type cannot represent one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::selftest::RecordingTelemetry;

/// A non-secret summary of the user's settings for the report. EVERY field here
/// is safe to print. Secrets (cloud tokens, e-mail/SMTP credentials, stream
/// keys) are intentionally absent — see the module docs: the report cannot leak
/// what the type cannot hold.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SettingsSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct SettingsSummary {
    pub language: Option<String>,
    pub device_name: Option<String>,
    pub channels: String,
    /// The sample-rate MODE that actually drives capture: `"auto"` (native, no
    /// `-ar`) or a forced `"r44100"/"r48000"/"r96000"`. A forced rate that
    /// doesn't match the device resamples and can cause stutter.
    pub sample_rate_mode: String,
    pub format: String,
    pub bitrate: String,
    pub filename_pattern: String,
    pub video_enabled: bool,
    pub video_device_name: Option<String>,
    pub stop_on_silence: bool,
    pub silence_threshold: i32,
    pub split_minutes: i32,
    pub auto_delete_days: i32,
    pub save_folder: Option<String>,
}

impl SettingsSummary {
    /// Project the full [`Settings`](crate::settings::Settings) down to the
    /// non-secret subset. The `Settings` model itself carries no secret fields
    /// (credentials live in the OS keychain), but this projection is the
    /// single, explicit allow-list so adding such fields later cannot
    /// accidentally widen the report.
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            language: s.language.clone(),
            device_name: s.device_name.clone(),
            channels: serde_plain_tag(&s.channels),
            sample_rate_mode: serde_plain_tag(&s.sample_rate_mode),
            format: serde_plain_tag(&s.format),
            bitrate: s.bitrate.clone(),
            filename_pattern: serde_plain_tag(&s.filename_pattern),
            video_enabled: s.video_enabled,
            video_device_name: s.video_device_name.clone(),
            stop_on_silence: s.stop_on_silence,
            silence_threshold: s.silence_threshold,
            split_minutes: s.split_minutes,
            auto_delete_days: s.auto_delete_days,
            save_folder: s.save_folder.clone(),
        }
    }
}

/// Serialise a small serde enum to its bare string tag (e.g. `"mp3"`,
/// `"stereo"`) for the human-readable settings dump. Falls back to `"?"` if the
/// value somehow isn't a JSON string (it always is for our `#[serde(rename_all)]`
/// unit enums).
fn serde_plain_tag<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|val| val.as_str().map(str::to_string))
        .unwrap_or_else(|| "?".to_string())
}

/// Everything the `src-tauri` layer gathered, ready to be formatted. No secrets
/// can appear here (see [`SettingsSummary`] / module docs).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/DiagnosticsInput.ts")]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsInput {
    /// App semver (e.g. `"0.1.0"`).
    pub app_version: String,
    /// Target OS (`std::env::consts::OS` — `"macos"`, `"windows"`, …).
    pub platform: String,
    /// CPU architecture (`std::env::consts::ARCH`).
    pub arch: String,
    /// First line of `ffmpeg -version`, or `None` when ffmpeg did not resolve.
    pub ffmpeg_version: Option<String>,
    /// Audio capture device names ffmpeg / cpal enumerated.
    pub audio_devices: Vec<String>,
    /// Video capture device names ffmpeg enumerated.
    pub video_devices: Vec<String>,
    /// The non-secret settings summary.
    pub settings: SettingsSummary,
    /// Audio capture test result: `Some(true)`=ok, `Some(false)`=failed,
    /// `None`=not tested. E2.5 restored the real probe, so `None` now means the
    /// probe was REFUSED — and [`Self::capture_probe_skipped`] says why.
    pub capture_ok: Option<bool>,
    /// Video capture test result, same tri-state. `None` when not tested /
    /// video disabled.
    pub video_ok: Option<bool>,

    // ── Extended facts (the comprehensive diagnose) ──────────────────────────
    /// Free bytes on the save-folder volume, or `None` if it couldn't be read.
    #[serde(default)]
    #[ts(type = "number | null")]
    pub free_disk_bytes: Option<u64>,
    /// Whether the save folder is writable, or `None` if not probed.
    #[serde(default)]
    pub save_folder_writable: Option<bool>,
    /// Microphone permission: `"authorized"`/`"denied"`/`"not_determined"`/`None`.
    #[serde(default)]
    pub mic_permission: Option<String>,
    /// Camera permission, same vocabulary. `None` when not probed / video off.
    #[serde(default)]
    pub camera_permission: Option<String>,
    /// Which audio engine the recorder LAST used (`"wasapi"`/`"asio"`/
    /// `"directshow"`/`"coreaudio"`), or `None` if it hasn't recorded yet.
    #[serde(default)]
    pub audio_engine: Option<String>,
    /// If the modern engine fell back, WHY (human text). `None` = no fallback.
    #[serde(default)]
    pub audio_engine_fallback: Option<String>,
    /// ASIO devices seen (Windows + `asio` feature). Empty otherwise.
    #[serde(default)]
    pub asio_devices: Vec<String>,
    /// The most recent classified recording error written to `last-error.json`.
    #[serde(default)]
    pub last_error: Option<LastErrorInfo>,
    /// Whether the orphan guard is active this session (Windows: kill-on-close
    /// Job Object; macOS/Linux: the detached sidecar reaper).
    #[serde(default)]
    pub orphan_guard_active: Option<bool>,
    /// Health telemetry of the MOST RECENT recording (drops/xruns/IPC-starvation),
    /// read back from `last-recording.json`. `None` = nothing recorded yet. The
    /// automatic passive-logging signal that lets the report explain stutter/lag.
    #[serde(default)]
    pub last_recording: Option<RecordingTelemetry>,
    /// Recent recordings' telemetry (newest last) for a TREND view — so the user
    /// pastes a pattern, not a one-off snapshot. Capped upstream (~20).
    #[serde(default)]
    pub recording_history: Vec<RecordingTelemetry>,

    // ── E2 observability signals ─────────────────────────────────────────────
    /// Why the capture probe did not run, when it did not. `None` = it ran (and
    /// [`Self::capture_ok`] carries the answer). Some situations make a probe
    /// unsafe — a live recording owns the microphone — and saying WHY beats a
    /// bare "ikke testet" that reads like an unfinished feature.
    #[serde(default)]
    pub capture_probe_skipped: Option<String>,
    /// Persisted panics found in `<app-data>/crashes/`. `None` when the ring
    /// could not be read at all (which is different from "no crashes").
    #[serde(default)]
    pub crashes: Option<CrashSummary>,
    /// Supervised background tasks that had to be restarted this install.
    #[serde(default)]
    pub task_restarts: Option<TaskRestartSummary>,
    /// The rotating file log, if it is running.
    #[serde(default)]
    pub log_file: Option<LogFileInfo>,
}

/// What the crash ring holds — count + newest, not the records themselves. The
/// report is a page a person reads, not a stack-trace archive.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/CrashSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct CrashSummary {
    pub count: usize,
    /// RFC 3339 local timestamp of the newest record, if any.
    pub newest: Option<String>,
    /// The newest record's message, truncated upstream. Lets the report show
    /// WHAT crashed without the user opening a folder.
    pub newest_message: Option<String>,
}

/// Supervised long-lived tasks that died and were restarted (E2.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/TaskRestartSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct TaskRestartSummary {
    pub count: usize,
    pub newest: Option<String>,
    /// Distinct task names, so the report names the SUBSYSTEM that is unstable
    /// (the scheduler missing recordings reads very differently from the trash
    /// sweep failing).
    pub tasks: Vec<String>,
}

/// The rotating file log's state (E2.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/LogFileInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct LogFileInfo {
    /// Absolute path to the live log file.
    pub path: String,
    /// Its current size, or `None` when it does not exist yet.
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    /// Log lines dropped because the writer could not keep up. Non-zero means
    /// the log is INCOMPLETE, which anyone reading it needs to know.
    #[ts(type = "number")]
    pub dropped_lines: u64,
}

/// The most recent recording error, read back from `last-error.json` (written by
/// the recorder on a classified failure). Lets the diagnose tool explain what
/// stopped the previous recording even though it can't see in-process events.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/LastErrorInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct LastErrorInfo {
    pub code: String,
    pub message: String,
    pub timestamp: String,
}

/// Severity of a [`DiagnosticFinding`], driving the UI badge + the support triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/DiagnosticSeverity.ts")]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// Healthy — informational confirmation.
    Ok,
    /// Worth knowing, not blocking (e.g. fell back to a working backend).
    Info,
    /// Likely to cause trouble (low disk, video on but no camera).
    Warning,
    /// Will block / has blocked recording (no device, ffmpeg missing, denied).
    Critical,
}

/// A single diagnose result with a STABLE support code. The `code` (e.g.
/// `"SR-AUDIO-01"`) never changes meaning across versions, so a user can read it
/// out and support knows exactly which situation it is — the "fishing" the user
/// asked for. `detail` carries the specifics (device name, free GB, …) and `hint`
/// the concrete next step.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../../src/lib/bindings/DiagnosticFinding.ts")]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFinding {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub title: String,
    pub detail: String,
    pub hint: String,
}

impl DiagnosticFinding {
    fn new(
        code: &str,
        severity: DiagnosticSeverity,
        title: &str,
        detail: impl Into<String>,
        hint: &str,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity,
            title: title.to_string(),
            detail: detail.into(),
            hint: hint.to_string(),
        }
    }
}

/// Disk headroom (bytes) below which recording is at risk — mirrors the recorder
/// guard's video threshold (4 GB) as the conservative warning line for diagnose.
const DISK_WARN_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Turn the gathered facts into stable-coded findings — the heart of the
/// "feilkode-system". Pure + fully unit-tested; the I/O layer only feeds facts in.
///
/// Codes are grouped by area: `SR-FFMPEG-*`, `SR-AUDIO-*`, `SR-VIDEO-*`,
/// `SR-DISK-*`, `SR-PERM-*`, `SR-ENGINE-*`. Order: criticals first.
pub fn detect_issues(input: &DiagnosticsInput) -> Vec<DiagnosticFinding> {
    use DiagnosticSeverity::*;
    let mut out: Vec<DiagnosticFinding> = Vec::new();

    // ffmpeg binary.
    if input.ffmpeg_version.is_none() {
        out.push(DiagnosticFinding::new(
            "SR-FFMPEG-01",
            Critical,
            "Opptaksmotor (ffmpeg) mangler",
            "ffmpeg-binæren ble ikke funnet eller svarte ikke.",
            "Reinstaller SundayRec — opptaksmotoren følger med appen.",
        ));
    }

    // Audio devices.
    if input.audio_devices.is_empty() && input.asio_devices.is_empty() {
        out.push(DiagnosticFinding::new(
            "SR-AUDIO-01",
            Critical,
            "Ingen lydenhet funnet",
            "Verken Windows-lyd, ASIO eller ffmpeg fant en mikrofon/lydkort.",
            "Sjekk at lydkortet er tilkoblet og driveren installert. På delt PC: er Windows Audio-tjenesten oppe?",
        ));
    } else if let Some(sel) = input.settings.device_name.as_deref() {
        // A device is selected but is nowhere in the enumerated lists.
        let known = input
            .audio_devices
            .iter()
            .chain(input.asio_devices.iter())
            .any(|d| d.eq_ignore_ascii_case(sel) || d.contains(sel) || sel.contains(d.as_str()));
        if !known && !sel.is_empty() {
            out.push(DiagnosticFinding::new(
                "SR-AUDIO-02",
                Warning,
                "Valgt lydenhet ble ikke funnet",
                format!("Innstillingen peker på «{sel}», men den er ikke blant enhetene nå."),
                "Koble til enheten, eller velg en annen under Innstillinger → Lyd.",
            ));
        }
    }

    // Audio-engine fallback (ASIO/WASAPI → DirectShow) — informational.
    if let Some(reason) = input.audio_engine_fallback.as_deref() {
        out.push(DiagnosticFinding::new(
            "SR-AUDIO-10",
            Info,
            "Falt tilbake til DirectShow",
            format!("Moderne lyd-motor (WASAPI/ASIO) startet ikke: {reason}"),
            "Opptak fungerer fortsatt. Vil du tvinge moderne motor, sjekk driver/ASIO og at enheten ikke er opptatt.",
        ));
    }

    // Forced sample rate — a known stutter cause when it doesn't match the
    // device's native rate (ffmpeg then resamples and can drop samples). Only
    // surfaced when the user has moved off the safe "Auto" default.
    if input.settings.sample_rate_mode != "auto" {
        out.push(DiagnosticFinding::new(
            "SR-RATE-01",
            Info,
            "Fast samplingsrate er valgt",
            format!(
                "Innstillingen tvinger {} (ikke «Auto»).",
                rate_label(&input.settings.sample_rate_mode)
            ),
            "Hvis lydkortet kjører en annen rate, resampler ffmpeg og du kan få hakking. Velg «Auto» under Innstillinger → Lyd med mindre du har en konkret grunn.",
        ));
    }

    // Video enabled but no camera.
    if input.settings.video_enabled && input.video_devices.is_empty() {
        out.push(DiagnosticFinding::new(
            "SR-VIDEO-01",
            Warning,
            "Video er på, men ingen kamera funnet",
            "Videoopptak er aktivert, men ingen kameraenhet ble enumerert.",
            "Koble til kameraet, eller slå av video under Innstillinger.",
        ));
    }

    // Disk.
    if let Some(free) = input.free_disk_bytes {
        if free < DISK_WARN_BYTES {
            out.push(DiagnosticFinding::new(
                "SR-DISK-01",
                Warning,
                "Lite ledig diskplass",
                format!("{} ledig på lagringsstedet.", fmt_bytes(free)),
                "Frigjør plass eller velg en annen lagringsmappe før et langt opptak.",
            ));
        }
    }
    if input.save_folder_writable == Some(false) {
        out.push(DiagnosticFinding::new(
            "SR-DISK-02",
            Critical,
            "Kan ikke skrive til lagringsmappen",
            "Lagringsmappen er ikke skrivbar.",
            "Velg en mappe du har skrivetilgang til under Innstillinger → Lagring.",
        ));
    }

    // Permissions.
    if input.mic_permission.as_deref() == Some("denied") {
        out.push(DiagnosticFinding::new(
            "SR-PERM-01",
            Critical,
            "Mikrofontilgang er nektet",
            "Operativsystemet blokkerer mikrofontilgang for SundayRec.",
            "Gi tilgang i Systeminnstillinger → Personvern → Mikrofon, og start appen på nytt.",
        ));
    }
    if input.settings.video_enabled && input.camera_permission.as_deref() == Some("denied") {
        out.push(DiagnosticFinding::new(
            "SR-PERM-02",
            Critical,
            "Kameratilgang er nektet",
            "Operativsystemet blokkerer kameratilgang for SundayRec.",
            "Gi tilgang i Systeminnstillinger → Personvern → Kamera, og start appen på nytt.",
        ));
    }

    // Last recording error.
    if let Some(err) = &input.last_error {
        out.push(DiagnosticFinding::new(
            "SR-ENGINE-01",
            Warning,
            "Forrige opptak endte med en feil",
            format!("[{}] {} ({})", err.code, err.message, err.timestamp),
            "Se feilkoden over. Kjør en test-opptak for å bekrefte at det fungerer nå.",
        ));
    }

    // Last recording HEALTH (automatic telemetry): drops/xruns = stutter,
    // levels_dropped = the UI/IPC couldn't keep up (recording mode lag).
    if let Some(t) = &input.last_recording {
        // Measured duration loss is its OWN, louder finding: the file provably
        // holds less audio than the session lasted (the 2026-07-31 incident:
        // 15–56 % missing while every counter said "clean").
        if t.loss_pct >= crate::selftest::DURATION_LOSS_FAIL_PCT {
            out.push(DiagnosticFinding::new(
                "REC-LOSS",
                Critical,
                "Forrige opptak MANGLER lyd",
                format!(
                    "Forventet ~{:.0} s, fila inneholder {:.0} s — {:.1} % av lyden mangler.",
                    t.expected_sec, t.measured_sec, t.loss_pct
                ),
                "Dette er alvorlig: opptaksprosessen mistet samples underveis. Lukk andre tunge programmer, koble lydkortet direkte (ikke via hub), og meld fra med denne rapporten — tallene her er beviset feilsøkingen trenger.",
            ));
        }
        if t.is_degraded() {
            let mut bits: Vec<String> = Vec::new();
            if t.drops > 0 {
                bits.push(format!("{} dropp", t.drops));
            }
            if t.xruns > 0 {
                bits.push(format!("{} xruns", t.xruns));
            }
            if t.capture_drop_lines > 0 {
                bits.push(format!("{} capture-dropp-varsler", t.capture_drop_lines));
            }
            if t.levels_dropped > 0 {
                bits.push(format!("{} IPC-overbelastninger", t.levels_dropped));
            }
            out.push(DiagnosticFinding::new(
                "SR-CAPTURE-01",
                Warning,
                "Forrige opptak viste tegn til hakking/treghet",
                format!("{} (varighet {:.0} s).", bits.join(", "), t.duration_sec),
                "Lukk andre tunge programmer, sjekk USB-kabel/strøm til lydkortet, og at samplingsrate står på «Auto». Kjør så et nytt opptak og sjekk om tallene faller.",
            ));
        }
    }

    // ── E2 observability signals ─────────────────────────────────────────────

    // A capture probe that RAN and failed is the most direct evidence there is:
    // the device was asked to record and produced nothing. Louder than any
    // enumeration-based guess, and distinct from SR-CAPTURE-01 (which describes
    // a recording that HAPPENED but stuttered).
    if input.capture_ok == Some(false) {
        out.push(DiagnosticFinding::new(
            "SR-CAPTURE-02",
            Critical,
            "Testopptaket fikk ingen lyd",
            "En kort prøve mot den valgte lydenheten ga ingen lyd i det hele tatt.",
            "Sjekk at riktig enhet er valgt under Innstillinger → Lyd, at kabelen sitter i, og at ingen andre programmer holder mikrofonen. Kjør Diagnose på nytt etterpå.",
        ));
    }
    if input.settings.video_enabled && input.video_ok == Some(false) {
        out.push(DiagnosticFinding::new(
            "SR-VIDEO-02",
            Critical,
            "Kameraet ga ingen bilde",
            "Video er på, men en kort prøve mot kameraet ga ingen bilderamme.",
            "Sjekk at kameraet er tilkoblet og ikke i bruk av et annet program (Teams, Zoom), og at appen har kameratilgang.",
        ));
    }

    // Persisted panics. The app crashed; that is worth saying out loud even
    // when everything is fine right now.
    if let Some(c) = &input.crashes {
        if c.count > 0 {
            let when = c.newest.as_deref().unwrap_or("ukjent tidspunkt");
            let what = c
                .newest_message
                .as_deref()
                .map(|m| format!(" Siste: {m}"))
                .unwrap_or_default();
            out.push(DiagnosticFinding::new(
                "SR-CRASH-01",
                Warning,
                "Appen har krasjet",
                format!(
                    "{} krasjrapport(er) ligger lagret; den nyeste er fra {when}.{what}",
                    c.count
                ),
                "Send denne rapporten videre — krasjfilene ligger i app-mappen og er det som gjør feilen mulig å finne.",
            ));
        }
    }

    // Supervised tasks that had to be restarted. One is a hiccup; the count and
    // the NAMES are what turn "it feels flaky" into something actionable.
    if let Some(r) = &input.task_restarts {
        if r.count > 0 {
            let when = r.newest.as_deref().unwrap_or("ukjent tidspunkt");
            out.push(DiagnosticFinding::new(
                "SR-TASK-01",
                Warning,
                "Bakgrunnsoppgaver har måttet startes på nytt",
                format!(
                    "{} omstart(er), sist {when}. Berørt: {}.",
                    r.count,
                    if r.tasks.is_empty() {
                        "ukjent".to_string()
                    } else {
                        r.tasks.join(", ")
                    }
                ),
                "Appen startet dem automatisk igjen, så ingenting stoppet. Men gjentar det seg, ta med denne rapporten — navnene over sier hvilken del som er ustabil.",
            ));
        }
    }

    // No log file means the NEXT problem will be as hard to diagnose as the
    // last one was — worth saying while things are still calm.
    match &input.log_file {
        Some(l) if l.dropped_lines > 0 => out.push(DiagnosticFinding::new(
            "SR-LOG-02",
            Info,
            "Deler av loggen mangler",
            format!(
                "{} loggmeldinger ble forkastet fordi disken ikke holdt følge.",
                l.dropped_lines
            ),
            "Loggen er ufullstendig, men opptaket ble prioritert — det er riktig vei rundt. En treg disk er verdt å se på før neste lange opptak.",
        )),
        Some(_) => {}
        None => out.push(DiagnosticFinding::new(
            "SR-LOG-01",
            Info,
            "Loggfil er ikke aktiv",
            "Denne økten skriver ingen loggfil, så det finnes ingen historikk å sende inn hvis noe skjærer seg.",
            "Vanligvis betyr det at app-mappen ikke kunne opprettes. Start appen på nytt; vedvarer det, sjekk diskplass og rettigheter.",
        )),
    }

    // All clear.
    if out.is_empty() {
        out.push(DiagnosticFinding::new(
            "SR-OK",
            Ok,
            "Ingen problemer oppdaget",
            "Alle sjekker passerte.",
            "Du er klar til å ta opp.",
        ));
    }
    out
}

/// Human label for a `SampleRate` serde tag (`"r48000"` → `"48 kHz"`).
fn rate_label(mode: &str) -> String {
    match mode {
        "r44100" => "44,1 kHz".to_string(),
        "r48000" => "48 kHz".to_string(),
        "r96000" => "96 kHz".to_string(),
        other => other.to_string(),
    }
}

/// Human-friendly byte size (MB/GB) for findings + the report.
fn fmt_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Render a tri-state test result the way the report shows it.
fn render_test(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "✅ OK",
        Some(false) => "❌ Feil",
        None => "ikke testet",
    }
}

/// Build the diagnostics markdown report from the gathered facts. Pure and
/// deterministic — the same input always yields the same string. Sections:
/// System, ffmpeg, Enheter (devices), Innstillinger (settings), Capture-test.
pub fn build_report_markdown(input: DiagnosticsInput) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("# SundayRec Diagnostics".to_string());
    lines.push(String::new());

    // ── Funn (feilkoder) — the actionable summary FIRST ───────────────────────
    let findings = detect_issues(&input);
    lines.push("## Funn".to_string());
    for f in &findings {
        let badge = match f.severity {
            DiagnosticSeverity::Ok => "✅",
            DiagnosticSeverity::Info => "ℹ️",
            DiagnosticSeverity::Warning => "⚠️",
            DiagnosticSeverity::Critical => "🔴",
        };
        lines.push(format!("- {badge} **{}** — {}", f.code, f.title));
        if !f.detail.is_empty() {
            lines.push(format!("  - {}", f.detail));
        }
        if !f.hint.is_empty() {
            lines.push(format!("  - 👉 {}", f.hint));
        }
    }
    lines.push(String::new());

    // ── System ──────────────────────────────────────────────────────────────
    lines.push("## System".to_string());
    lines.push(format!("- **App-versjon:** {}", input.app_version));
    lines.push(format!(
        "- **Plattform:** {} ({})",
        input.platform, input.arch
    ));
    if let Some(active) = input.orphan_guard_active {
        lines.push(format!(
            "- **Orphan-guard (Job Object / reaper):** {}",
            if active { "aktiv" } else { "ikke aktiv" }
        ));
    }
    lines.push(String::new());

    // ── Lyd-motor ─────────────────────────────────────────────────────────────
    if input.audio_engine.is_some() || input.audio_engine_fallback.is_some() {
        lines.push("## Lyd-motor".to_string());
        lines.push(format!(
            "- **Sist brukt:** {}",
            input.audio_engine.as_deref().unwrap_or("ukjent")
        ));
        if let Some(reason) = &input.audio_engine_fallback {
            lines.push(format!("- **Fallback:** {reason}"));
        }
        lines.push(String::new());
    }

    // ── ffmpeg ──────────────────────────────────────────────────────────────
    lines.push("## ffmpeg".to_string());
    match &input.ffmpeg_version {
        Some(v) => lines.push(format!("- **Versjon:** {v}")),
        None => lines.push("- **Versjon:** ikke funnet".to_string()),
    }
    lines.push(String::new());

    // ── Enheter ─────────────────────────────────────────────────────────────
    lines.push("## Enheter".to_string());
    lines.push(format!("### Lydenheter ({})", input.audio_devices.len()));
    if input.audio_devices.is_empty() {
        lines.push("_Ingen funnet_".to_string());
    } else {
        for d in &input.audio_devices {
            lines.push(format!("- `{d}`"));
        }
    }
    if !input.asio_devices.is_empty() {
        lines.push(format!("### ASIO-enheter ({})", input.asio_devices.len()));
        for d in &input.asio_devices {
            lines.push(format!("- `{d}`"));
        }
    }
    lines.push(format!("### Videoenheter ({})", input.video_devices.len()));
    if input.video_devices.is_empty() {
        lines.push("_Ingen funnet_".to_string());
    } else {
        for d in &input.video_devices {
            lines.push(format!("- `{d}`"));
        }
    }
    lines.push(String::new());

    // ── Lagring ───────────────────────────────────────────────────────────────
    lines.push("## Lagring".to_string());
    match input.free_disk_bytes {
        Some(free) => lines.push(format!("- **Ledig plass:** {}", fmt_bytes(free))),
        None => lines.push("- **Ledig plass:** ukjent".to_string()),
    }
    if let Some(w) = input.save_folder_writable {
        lines.push(format!(
            "- **Skrivbar mappe:** {}",
            if w { "ja" } else { "NEI" }
        ));
    }
    lines.push(String::new());

    // ── Tilganger ─────────────────────────────────────────────────────────────
    if input.mic_permission.is_some() || input.camera_permission.is_some() {
        lines.push("## Tilganger".to_string());
        if let Some(m) = &input.mic_permission {
            lines.push(format!("- **Mikrofon:** {m}"));
        }
        if let Some(c) = &input.camera_permission {
            lines.push(format!("- **Kamera:** {c}"));
        }
        lines.push(String::new());
    }

    // ── Siste feil ────────────────────────────────────────────────────────────
    if let Some(err) = &input.last_error {
        lines.push("## Siste opptaksfeil".to_string());
        lines.push(format!("- **Kode:** `{}`", err.code));
        lines.push(format!("- **Melding:** {}", err.message));
        lines.push(format!("- **Tidspunkt:** {}", err.timestamp));
        lines.push(String::new());
    }

    // ── Siste opptak (helse-telemetri, automatisk innsamlet) ────────────────
    if let Some(t) = &input.last_recording {
        lines.push("## Siste opptak (teknisk)".to_string());
        lines.push(format!("- **Varighet:** {:.0} s", t.duration_sec));
        lines.push(format!("- **Dropp (frames):** {}", t.drops));
        lines.push(format!("- **xruns/diskontinuitet:** {}", t.xruns));
        lines.push(format!(
            "- **IPC-overbelastning (tapte nivå-oppdateringer):** {}",
            t.levels_dropped
        ));
        lines.push(format!(
            "- **Avsluttet rent:** {}",
            if t.exit_ok { "ja" } else { "nei" }
        ));
        if !t.timestamp.is_empty() {
            lines.push(format!("- **Tidspunkt:** {}", t.timestamp));
        }
        // Trend across recent recordings (newest first) so a pattern is visible.
        if input.recording_history.len() > 1 {
            lines.push("### Trend (nyeste først)".to_string());
            for h in input.recording_history.iter().rev().take(5) {
                let badge = if h.is_degraded() { "⚠️" } else { "✅" };
                lines.push(format!(
                    "- {badge} {} — dropp {}, xruns {}, ipc {} ({:.0} s)",
                    h.timestamp, h.drops, h.xruns, h.levels_dropped, h.duration_sec
                ));
            }
        }
        lines.push(String::new());
    }

    // ── Capture-test ────────────────────────────────────────────────────────
    lines.push("## Capture-test".to_string());
    lines.push(format!("- **Lyd:** {}", render_test(input.capture_ok)));
    lines.push(format!("- **Video:** {}", render_test(input.video_ok)));
    if let Some(reason) = &input.capture_probe_skipped {
        // "ikke testet" with no reason reads like an unfinished feature — which
        // for four phases it was. Say WHY instead.
        lines.push(format!("- **Ikke kjørt fordi:** {reason}"));
    }
    lines.push(String::new());

    // ── Stabilitet (E2 observability) ────────────────────────────────────────
    lines.push("## Stabilitet".to_string());
    match &input.crashes {
        Some(c) if c.count > 0 => {
            lines.push(format!("- **Krasjrapporter:** {}", c.count));
            if let Some(when) = &c.newest {
                lines.push(format!("  - nyeste: {when}"));
            }
            if let Some(msg) = &c.newest_message {
                lines.push(format!("  - `{msg}`"));
            }
        }
        Some(_) => lines.push("- **Krasjrapporter:** ingen".to_string()),
        None => lines.push("- **Krasjrapporter:** ukjent (kunne ikke leses)".to_string()),
    }
    match &input.task_restarts {
        Some(r) if r.count > 0 => {
            lines.push(format!(
                "- **Omstarter av bakgrunnsoppgaver:** {} ({})",
                r.count,
                r.tasks.join(", ")
            ));
            if let Some(when) = &r.newest {
                lines.push(format!("  - nyeste: {when}"));
            }
        }
        Some(_) => lines.push("- **Omstarter av bakgrunnsoppgaver:** ingen".to_string()),
        None => lines.push("- **Omstarter av bakgrunnsoppgaver:** ukjent".to_string()),
    }
    match &input.log_file {
        Some(l) => {
            lines.push(format!("- **Loggfil:** `{}`", l.path));
            if let Some(size) = l.size_bytes {
                lines.push(format!("  - størrelse: {}", fmt_bytes(size)));
            }
            if l.dropped_lines > 0 {
                lines.push(format!("  - forkastede linjer: {}", l.dropped_lines));
            }
        }
        None => lines.push("- **Loggfil:** ikke aktiv".to_string()),
    }
    lines.push(String::new());

    // ── Innstillinger (non-secret) ──────────────────────────────────────────
    lines.push("## Innstillinger (unntatt passord/e-post)".to_string());
    lines.push("```json".to_string());
    // Pretty JSON of the summary — never contains secrets (type has no field).
    let json = serde_json::to_string_pretty(&input.settings).unwrap_or_else(|_| "{}".to_string());
    lines.push(json);
    lines.push("```".to_string());

    lines.push(String::new());
    lines.push("---".to_string());
    lines.push("_Generert av SundayRec Diagnostics_".to_string());

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn sample_input() -> DiagnosticsInput {
        DiagnosticsInput {
            app_version: "0.1.0".to_string(),
            platform: "macos".to_string(),
            arch: "aarch64".to_string(),
            ffmpeg_version: Some("ffmpeg version 6.0".to_string()),
            audio_devices: vec!["MacBook Pro-mikrofon".to_string()],
            video_devices: vec!["FaceTime HD Camera".to_string()],
            settings: SettingsSummary::from_settings(&Settings::default()),
            capture_ok: None,
            video_ok: None,
            // A HEALTHY machine has a file log, no crashes and no restarts
            // (E2). Leaving these at `Default` would make every fixture below
            // carry an SR-LOG-01 it is not about.
            crashes: Some(CrashSummary::default()),
            task_restarts: Some(TaskRestartSummary::default()),
            log_file: Some(LogFileInfo {
                path: "/tmp/logs/sundayrec.log".into(),
                size_bytes: Some(12_345),
                dropped_lines: 0,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn report_includes_version_platform_and_ffmpeg_line() {
        let md = build_report_markdown(sample_input());
        assert!(md.contains("**App-versjon:** 0.1.0"));
        assert!(md.contains("macos (aarch64)"));
        assert!(md.contains("ffmpeg version 6.0"));
    }

    #[test]
    fn healthy_input_yields_single_ok_finding() {
        let f = detect_issues(&sample_input());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "SR-OK");
        assert_eq!(f[0].severity, DiagnosticSeverity::Ok);
    }

    #[test]
    fn missing_ffmpeg_and_no_devices_are_critical_findings() {
        let mut input = sample_input();
        input.ffmpeg_version = None;
        input.audio_devices.clear();
        let f = detect_issues(&input);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-FFMPEG-01" && x.severity == DiagnosticSeverity::Critical));
        assert!(f
            .iter()
            .any(|x| x.code == "SR-AUDIO-01" && x.severity == DiagnosticSeverity::Critical));
        assert!(!f.iter().any(|x| x.code == "SR-OK"));
    }

    #[test]
    fn selected_device_missing_warns() {
        let mut input = sample_input();
        input.settings.device_name = Some("Soundcraft MADI USB".to_string());
        // audio_devices only has the MacBook mic → selected one is absent.
        let f = detect_issues(&input);
        assert!(f.iter().any(|x| x.code == "SR-AUDIO-02"));
    }

    #[test]
    fn audio_engine_fallback_is_info() {
        let mut input = sample_input();
        input.audio_engine_fallback = Some("driveren var opptatt".to_string());
        let f = detect_issues(&input);
        let e = f
            .iter()
            .find(|x| x.code == "SR-AUDIO-10")
            .expect("fallback finding");
        assert_eq!(e.severity, DiagnosticSeverity::Info);
        assert!(e.detail.contains("driveren var opptatt"));
    }

    #[test]
    fn low_disk_warns_and_unwritable_is_critical() {
        let mut input = sample_input();
        input.free_disk_bytes = Some(1024 * 1024 * 1024); // 1 GB < 4 GB
        input.save_folder_writable = Some(false);
        let f = detect_issues(&input);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-DISK-01" && x.severity == DiagnosticSeverity::Warning));
        assert!(f
            .iter()
            .any(|x| x.code == "SR-DISK-02" && x.severity == DiagnosticSeverity::Critical));
    }

    #[test]
    fn denied_mic_permission_is_critical() {
        let mut input = sample_input();
        input.mic_permission = Some("denied".to_string());
        let f = detect_issues(&input);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-PERM-01" && x.severity == DiagnosticSeverity::Critical));
    }

    #[test]
    fn last_error_surfaces_as_finding_and_report_section() {
        let mut input = sample_input();
        input.last_error = Some(LastErrorInfo {
            code: "device_disconnected".into(),
            message: "USB pulled".into(),
            timestamp: "2026-06-07T12:00:00+02:00".into(),
        });
        assert!(detect_issues(&input)
            .iter()
            .any(|x| x.code == "SR-ENGINE-01"));
        let md = build_report_markdown(input);
        assert!(md.contains("Siste opptaksfeil"));
        assert!(md.contains("device_disconnected"));
    }

    #[test]
    fn report_findings_section_lists_codes() {
        let mut input = sample_input();
        input.ffmpeg_version = None;
        let md = build_report_markdown(input);
        assert!(md.contains("## Funn"));
        assert!(md.contains("SR-FFMPEG-01"));
    }

    #[test]
    fn report_lists_device_names() {
        let md = build_report_markdown(sample_input());
        assert!(md.contains("MacBook Pro-mikrofon"));
        assert!(md.contains("FaceTime HD Camera"));
        assert!(md.contains("Lydenheter (1)"));
        assert!(md.contains("Videoenheter (1)"));
    }

    #[test]
    fn report_shows_no_devices_placeholder() {
        let mut input = sample_input();
        input.audio_devices.clear();
        input.video_devices.clear();
        let md = build_report_markdown(input);
        assert!(md.contains("Lydenheter (0)"));
        assert!(md.contains("_Ingen funnet_"));
    }

    #[test]
    fn missing_ffmpeg_renders_not_found() {
        let mut input = sample_input();
        input.ffmpeg_version = None;
        let md = build_report_markdown(input);
        assert!(md.contains("**Versjon:** ikke funnet"));
        assert!(!md.contains("ffmpeg version"));
    }

    #[test]
    fn capture_tristate_renders_correctly() {
        // None → "ikke testet"
        let md_none = build_report_markdown(sample_input());
        assert!(md_none.contains("**Lyd:** ikke testet"));
        assert!(md_none.contains("**Video:** ikke testet"));

        // Some(true) → OK, Some(false) → Feil
        let mut ok = sample_input();
        ok.capture_ok = Some(true);
        ok.video_ok = Some(false);
        let md = build_report_markdown(ok);
        assert!(md.contains("**Lyd:** ✅ OK"));
        assert!(md.contains("**Video:** ❌ Feil"));
    }

    #[test]
    fn summary_carries_no_secret_fields_even_if_settings_had_them() {
        // The summary type structurally cannot hold a secret. Prove the rendered
        // JSON has only the allow-listed keys and nothing password/email/token-ish.
        let summary = SettingsSummary::from_settings(&Settings {
            device_name: Some("Soundcraft USB".into()),
            ..Default::default()
        });
        let json = serde_json::to_string(&summary).unwrap();
        for forbidden in [
            "password",
            "passord",
            "email",
            "epost",
            "token",
            "streamKey",
            "secret",
        ] {
            assert!(
                !json.to_lowercase().contains(&forbidden.to_lowercase()),
                "summary JSON should not contain `{forbidden}`: {json}"
            );
        }
        // It does carry the safe fields.
        assert!(json.contains("Soundcraft USB"));
        assert!(json.contains("sampleRateMode"));
    }

    #[test]
    fn settings_summary_reflects_enum_tags() {
        let md = build_report_markdown(sample_input());
        // Defaults: mp3 / stereo / date pattern serialise as their Electron tags.
        assert!(md.contains("\"format\": \"mp3\""));
        assert!(md.contains("\"channels\": \"stereo\""));
        assert!(md.contains("\"filenamePattern\": \"date\""));
    }

    #[test]
    fn degraded_last_recording_warns_and_renders_section() {
        let mut input = sample_input();
        input.last_recording = Some(RecordingTelemetry {
            drops: 3,
            xruns: 1,
            levels_dropped: 5,
            duration_sec: 65.0,
            timestamp: "2026-06-15T10:00:00+02:00".into(),
            exit_ok: true,
            ..Default::default()
        });
        let f = detect_issues(&input);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-CAPTURE-01" && x.severity == DiagnosticSeverity::Warning));
        let md = build_report_markdown(input);
        assert!(md.contains("Siste opptak (teknisk)"));
        assert!(md.contains("IPC-overbelastning"));
    }

    #[test]
    fn clean_last_recording_no_capture_finding() {
        let mut input = sample_input();
        input.last_recording = Some(RecordingTelemetry {
            duration_sec: 60.0,
            exit_ok: true,
            ..Default::default()
        });
        let f = detect_issues(&input);
        assert!(!f.iter().any(|x| x.code == "SR-CAPTURE-01"));
        // No degraded recording → still no SR-CAPTURE among findings.
    }

    #[test]
    fn forced_sample_rate_is_info_finding() {
        // Default is "auto" → no finding (the healthy test already asserts that).
        let mut input = sample_input();
        input.settings.sample_rate_mode = "r48000".to_string();
        let e = detect_issues(&input)
            .into_iter()
            .find(|x| x.code == "SR-RATE-01")
            .expect("forced-rate finding");
        assert_eq!(e.severity, DiagnosticSeverity::Info);
        assert!(e.detail.contains("48 kHz"));
    }

    #[test]
    fn auto_sample_rate_has_no_rate_finding() {
        let input = sample_input(); // default mode = auto
        assert!(!detect_issues(&input).iter().any(|x| x.code == "SR-RATE-01"));
    }

    // ── E2.5: the restored capture probe + the observability signals ─────────

    #[test]
    fn a_failed_capture_probe_is_critical_and_distinct_from_a_stuttery_recording() {
        // SR-CAPTURE-01 says "a recording HAPPENED but stuttered".
        // SR-CAPTURE-02 says "we asked the device to record and got nothing" —
        // a different, louder fact, and the one `capture_ok` exists to carry.
        let mut input = sample_input();
        input.capture_ok = Some(false);
        let f = detect_issues(&input);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-CAPTURE-02" && x.severity == DiagnosticSeverity::Critical));
        assert!(!f.iter().any(|x| x.code == "SR-CAPTURE-01"));

        // A probe that PASSED, or one that never ran, says nothing.
        let mut ok = sample_input();
        ok.capture_ok = Some(true);
        assert!(!detect_issues(&ok).iter().any(|x| x.code == "SR-CAPTURE-02"));
        assert!(!detect_issues(&sample_input())
            .iter()
            .any(|x| x.code == "SR-CAPTURE-02"));
    }

    #[test]
    fn a_failed_video_probe_only_matters_when_video_is_on() {
        let mut input = sample_input();
        input.video_ok = Some(false);
        // Video disabled (the default) → a camera that cannot open is not a
        // problem the operator has.
        assert!(!detect_issues(&input)
            .iter()
            .any(|x| x.code == "SR-VIDEO-02"));
        input.settings.video_enabled = true;
        assert!(detect_issues(&input)
            .iter()
            .any(|x| x.code == "SR-VIDEO-02" && x.severity == DiagnosticSeverity::Critical));
    }

    #[test]
    fn a_skipped_probe_says_why_instead_of_a_bare_not_tested() {
        let mut input = sample_input();
        input.capture_probe_skipped = Some("et opptak pågår".to_string());
        let md = build_report_markdown(input);
        assert!(md.contains("**Lyd:** ikke testet"));
        assert!(md.contains("Ikke kjørt fordi:** et opptak pågår"), "{md}");
    }

    #[test]
    fn stored_crashes_surface_with_their_count_and_newest() {
        let mut input = sample_input();
        input.crashes = Some(CrashSummary {
            count: 3,
            newest: Some("2026-08-06T11:00:00+02:00".into()),
            newest_message: Some("called `Option::unwrap()` on a `None` value".into()),
        });
        let e = detect_issues(&input)
            .into_iter()
            .find(|x| x.code == "SR-CRASH-01")
            .expect("crash finding");
        assert_eq!(e.severity, DiagnosticSeverity::Warning);
        assert!(e.detail.contains('3'), "{}", e.detail);
        assert!(
            e.detail.contains("2026-08-06T11:00:00+02:00"),
            "{}",
            e.detail
        );
        assert!(e.detail.contains("Option::unwrap"), "{}", e.detail);

        let md = build_report_markdown(input);
        assert!(md.contains("## Stabilitet"));
        assert!(md.contains("**Krasjrapporter:** 3"));
    }

    #[test]
    fn an_empty_crash_ring_is_silent_and_an_unreadable_one_is_honest() {
        // No crashes → no finding (the healthy fixture already proves this),
        // and the report says so out loud rather than omitting the line.
        assert!(!detect_issues(&sample_input())
            .iter()
            .any(|x| x.code == "SR-CRASH-01"));
        assert!(build_report_markdown(sample_input()).contains("**Krasjrapporter:** ingen"));

        // Unreadable is NOT the same as none, and the report must not pretend.
        let mut unknown = sample_input();
        unknown.crashes = None;
        let md = build_report_markdown(unknown);
        assert!(md.contains("**Krasjrapporter:** ukjent"), "{md}");
    }

    #[test]
    fn supervised_task_restarts_name_the_subsystem() {
        // The count alone would read as "something is flaky"; the NAMES are
        // what make it actionable — a restarting scheduler and a restarting
        // trash sweep are very different news.
        let mut input = sample_input();
        input.task_restarts = Some(TaskRestartSummary {
            count: 11,
            newest: Some("2026-08-06T10:30:00+02:00".into()),
            tasks: vec!["trash::sweep".into(), "scheduler::supervisor".into()],
        });
        let e = detect_issues(&input)
            .into_iter()
            .find(|x| x.code == "SR-TASK-01")
            .expect("restart finding");
        assert_eq!(e.severity, DiagnosticSeverity::Warning);
        assert!(e.detail.contains("11"), "{}", e.detail);
        assert!(e.detail.contains("scheduler::supervisor"), "{}", e.detail);
        assert!(build_report_markdown(input).contains("Omstarter av bakgrunnsoppgaver:** 11"));
    }

    #[test]
    fn a_missing_log_file_is_flagged_and_a_lossy_one_is_flagged_differently() {
        // No log = the NEXT problem is as hard to diagnose as the last one was.
        let mut none = sample_input();
        none.log_file = None;
        let f = detect_issues(&none);
        assert!(f
            .iter()
            .any(|x| x.code == "SR-LOG-01" && x.severity == DiagnosticSeverity::Info));
        assert!(build_report_markdown(none).contains("**Loggfil:** ikke aktiv"));

        // A log with dropped lines is present but INCOMPLETE — anyone reading
        // it needs to know that, and it is a different code.
        let mut lossy = sample_input();
        lossy.log_file = Some(LogFileInfo {
            path: "/tmp/logs/sundayrec.log".into(),
            size_bytes: Some(1024),
            dropped_lines: 42,
        });
        let f = detect_issues(&lossy);
        assert!(f.iter().any(|x| x.code == "SR-LOG-02"));
        assert!(!f.iter().any(|x| x.code == "SR-LOG-01"));
        assert!(build_report_markdown(lossy).contains("forkastede linjer: 42"));
    }

    #[test]
    fn the_stable_code_vocabulary_is_complete_and_unique() {
        // The whole value of `SR-*` is that a user reads a code out and support
        // knows which situation it is. A code that appears in the engine but not
        // here has never been reviewed as part of the vocabulary; a DUPLICATE
        // code would mean two different situations answer to the same name,
        // which silently destroys that value.
        const VOCABULARY: &[&str] = &[
            "SR-OK",
            "SR-FFMPEG-01",
            "SR-AUDIO-01",
            "SR-AUDIO-02",
            "SR-AUDIO-10",
            "SR-RATE-01",
            "SR-VIDEO-01",
            "SR-VIDEO-02",
            "SR-DISK-01",
            "SR-DISK-02",
            "SR-PERM-01",
            "SR-PERM-02",
            "SR-ENGINE-01",
            "SR-CAPTURE-01",
            "SR-CAPTURE-02",
            "SR-CRASH-01",
            "SR-TASK-01",
            "SR-LOG-01",
            "SR-LOG-02",
            "REC-LOSS",
        ];
        let unique: std::collections::BTreeSet<&str> = VOCABULARY.iter().copied().collect();
        assert_eq!(
            unique.len(),
            VOCABULARY.len(),
            "duplicate code in VOCABULARY"
        );

        // Every code the ENGINE can emit must be in the list above. Parsing the
        // module's own source is the only way to assert that without a
        // hand-maintained registry drifting from the code it describes — the
        // same trick `commands::path_ratchet` uses.
        // Everything BEFORE this module's own `#[cfg(test)]` attribute — the
        // first occurrence in the file is the real one, so this cleanly excludes
        // the test source (which contains the needle as a literal).
        let source = include_str!("diagnostics.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default();
        let mut emitted: std::collections::BTreeSet<String> = Default::default();
        for (i, _) in source.match_indices("DiagnosticFinding::new(") {
            let rest = &source[i..];
            let Some(open) = rest.find('"') else { continue };
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            emitted.insert(rest[open + 1..open + 1 + close].to_string());
        }
        assert!(
            emitted.len() >= 15,
            "the source scan found only {} codes — the parser is broken and this \
             assertion would pass vacuously",
            emitted.len()
        );
        for code in &emitted {
            assert!(
                unique.contains(code.as_str()),
                "`{code}` is emitted by detect_issues but is not in VOCABULARY — \
                 add it (and make sure it means something a support reader can act on)"
            );
        }
        for code in &unique {
            assert!(
                emitted.contains(*code),
                "VOCABULARY lists `{code}`, which nothing emits any more — remove \
                 the stale entry rather than leaving a code that can never appear"
            );
        }
    }
}
