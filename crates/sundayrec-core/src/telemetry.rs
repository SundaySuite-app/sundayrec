//! The opt-in telemetry payload — pure, and secret-free BY TYPE.
//!
//! This module owns the WIRE CONTRACT for the anonymous, opt-in quality
//! reporting SundayRec offers ([`TelemetryPayload`], schema
//! [`TELEMETRY_SCHEMA`]). It builds nothing on its own and sends nothing at all:
//! the `src-tauri` `telemetry` seam gathers the facts, this module shapes them,
//! and a later phase's sender moves the bytes. Everything here is deterministic
//! and unit-tested without a device, a database or a socket.
//!
//! ## The discipline: the payload cannot carry what the type cannot hold
//!
//! [`crate::diagnostics`] made the same argument for the diagnose report (module
//! docs :12-19): the Electron build hand-picked which fields to print, we went
//! one better and gave the input type no field a secret could sit in. The
//! telemetry payload is a stricter case, because it leaves the machine — so the
//! rule here is sharpened into a classification every field must satisfy:
//!
//!   1. **a number or a bool** — `u64`, `f64`, `i64`, `bool`. A counter, a
//!      duration, a timestamp, a flag. These cannot hold text at all, so most of
//!      the payload is unconditionally safe. This is why every timestamp on the
//!      wire is unix milliseconds (UTC) rather than the RFC 3339 local strings
//!      the crash ring and the telemetry history keep on disk: an `i64` cannot
//!      smuggle anything, and it drops the timezone offset as a bonus.
//!   2. **a closed enum** — [`TelemetryOs`], [`CrashKind`], [`QualityReason`],
//!      [`CounterName`], and the reused [`crate::selftest::SelfTestVerdict`] /
//!      [`crate::diagnostics::DiagnosticSeverity`] / [`crate::settings`] tags.
//!      The set of possible values is written down in this file; no runtime
//!      input can widen it.
//!   3. **a String produced by exactly ONE sanitizer in this module** —
//!      [`sanitize_install_id`], [`sanitize_version`], [`sanitize_language`],
//!      [`sanitize_code`], [`sanitize_token`], [`sanitize_free_text`]. Each has a
//!      shape it enforces and a hard character cap.
//!
//! There is no fourth class. `WIRE_FIELDS` (test-only, at the bottom) lists every
//! field on the wire with its classification, and a test walks a maximal
//! serialised payload and fails when the two disagree — so a field added without
//! being classified is a failing test, not a review miss. That is the E1.3
//! ratchet idiom applied to privacy instead of to path guards.
//!
//! ## What is deliberately NOT here
//!
//! Audio, transcripts, sermon text, names, e-mail addresses, filesystem paths,
//! device names, church name, responsible person. Not filtered out — simply
//! unrepresentable: there is no field of any type for them.
//!
//! Two judgement calls worth stating out loud, because both are fields a reader
//! would expect to find:
//!
//!   - **Backtraces are not sent in v1.** The crash ring keeps them
//!     ([`crash::CrashRecord::backtrace`] in `src-tauri`, path-scrubbed at write
//!     time), and the diagnose report can hand them over when the operator
//!     chooses to. But a backtrace is the single largest and least predictable
//!     free-text field we have: 8000 characters naming every source file the
//!     stack walked through, including checkouts under roots the scrubber's list
//!     does not know (a network share, an external volume, a corporate profile
//!     root). Twenty of them is 160 kB per payload for information that
//!     [`CrashReport::message`] + [`CrashReport::location`] + [`CrashReport::kind`]
//!     already identify well enough to group crashes. So the wire carries
//!     [`CrashReport::backtrace_present`] — a bool — and a future schema 2 may add
//!     a symbol-only frame list once there is a reason to.
//!   - **Self-test reasons are sent as CODES, never as their sentences.**
//!     [`crate::selftest::SelfTestReport::reasons`] is a `Vec<String>` of
//!     Norwegian sentences built with `format!`. Today every interpolation is a
//!     number, so nothing leaks — but the type permits a future reason to say
//!     "Enheten Qu-5 forsvant", and nobody adding that reason would think of this
//!     module. [`derive_reason_codes`] therefore re-derives WHICH conditions
//!     fired from the report's NUMBERS, and the wire carries
//!     [`QualityReason`] variants. A test asserts the derivation produces exactly
//!     as many codes as `selftest_verdict` produced reasons across a matrix of
//!     inputs, so a new reason without a new code fails the build.
//!
//! ## The other half of the contract (stated here so it is one story)
//!
//!   - **Data controller: "Sunday Suite".**
//!   - **Fully anonymous.** [`TelemetryPayload::install_id`] is a random UUID v7
//!     minted on the machine, never derived from hardware, e-mail, church name or
//!     a Sunday Account, and never linked to one. Regenerating it (the app's
//!     "delete my data") makes every future payload a new, unrelated install.
//!   - **Retention: 90 days for raw payloads, aggregates kept indefinitely.**
//!     That is a property of the receiving endpoint, not of this struct, but it
//!     belongs in the same place the fields are documented — a contract split
//!     across two repositories is a contract nobody reads.
//!   - **Consent defaults to off.** Nothing in this module is reachable without
//!     it; see the `src-tauri` `telemetry::consent` state machine.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::diagnostics::{DiagnosticFinding, DiagnosticSeverity};
use crate::redact::{redact_secrets, scrub_paths, USER_PLACEHOLDER};
use crate::selftest::{
    RecordingTelemetry, SelfTestReport, SelfTestVerdict, FAIL_DROPS, FAIL_GAP_SEC, FAIL_XRUNS,
    WARN_GAP_SEC,
};
use crate::settings::{ChannelMode, FileFormat, FilenamePattern, SampleRate, Settings};
use crate::test_recording::{classify_signal, size_is_plausible, TestRecordingSignal};
use crate::wake::{WakeFailureEntry, WakeFailureKind};

pub mod consent;
pub mod corrections;
pub mod queue;

pub use corrections::{CorrectionReport, MAX_CORRECTIONS};

/// The payload schema version. Bumped when a field changes MEANING (a new
/// optional field does not need it); the receiving endpoint keys its parsing off
/// this, and the consent record carries its own separate version so a change in
/// SCOPE re-asks the user rather than silently widening what is sent.
pub const TELEMETRY_SCHEMA: u32 = 1;

// ── Caps ────────────────────────────────────────────────────────────────────
//
// Every collection and every string on the wire is bounded. A payload built
// from a machine that crashed 500 times must still be a few kB, both because the
// endpoint pays for what it receives and because an unbounded field is an
// unbounded leak.

/// Crash summaries per payload. Matches the crash ring's own cap, so one payload
/// can carry everything the ring holds and nothing is silently dropped.
pub const MAX_CRASHES: usize = 20;
/// Quality records per payload. Matches the recording-telemetry history's cap.
pub const MAX_QUALITY: usize = 20;
/// Diagnose findings per payload (code + severity only — the whole SR-* space is
/// far smaller than this).
pub const MAX_FINDINGS: usize = 40;
/// Wake-failure records per payload. Matches the `wake_failure` table's cap.
pub const MAX_WAKE_FAILURES: usize = 20;

/// Cap for a panic message. The crash ring keeps 2000 characters for a human
/// reading the file locally; the wire takes the first 200, which is where a
/// panic's IDENTITY lives (`called \`Option::unwrap()\` on a \`None\` value`,
/// `index out of bounds: the len is 3 but the index is 7`) and past which a
/// message is a formatted dump.
pub const MESSAGE_MAX_CHARS: usize = 200;
/// Cap for a `file:line:col`.
pub const LOCATION_MAX_CHARS: usize = 120;
/// Cap for a supervised task's name (all of them are `&'static str` today).
pub const TASK_MAX_CHARS: usize = 64;
/// Cap for a version string.
pub const VERSION_MAX_CHARS: usize = 32;
/// Cap for a language subtag (`"no"`, `"nb"`, `"pt-br"` collapses to `"pt"`).
pub const LANGUAGE_MAX_CHARS: usize = 8;
/// Cap for a stable support code (`SR-CAPTURE-02`, `REC-LOSS`).
pub const CODE_MAX_CHARS: usize = 24;
/// Cap for a snake_case reason token (`no_resume`, `on_battery`).
pub const TOKEN_MAX_CHARS: usize = 32;

/// The install id used when none exists yet — the nil UUID. The preview command
/// shows this before consent is granted, because minting a real id for a user who
/// has not said yes would itself be collection.
pub const NIL_INSTALL_ID: &str = "00000000-0000-0000-0000-000000000000";

// ── Class 2: closed enums ───────────────────────────────────────────────────

/// The operating system, as a CLOSED set. `std::env::consts::OS` is a `&str`
/// with no compile-time guarantee about its contents, so it is mapped here
/// rather than forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TelemetryOs.ts")]
#[serde(rename_all = "lowercase")]
pub enum TelemetryOs {
    Macos,
    Windows,
    Linux,
    Other,
}

impl TelemetryOs {
    /// Map an `std::env::consts::OS`-shaped string onto the closed set.
    pub fn from_consts(os: &str) -> Self {
        match os {
            "macos" => Self::Macos,
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            _ => Self::Other,
        }
    }

    /// This build's OS.
    pub fn current() -> Self {
        Self::from_consts(std::env::consts::OS)
    }
}

/// The CPU architecture, as a CLOSED set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TelemetryArch.ts")]
#[serde(rename_all = "lowercase")]
pub enum TelemetryArch {
    #[serde(rename = "x86_64")]
    X86_64,
    Aarch64,
    Other,
}

impl TelemetryArch {
    /// Map an `std::env::consts::ARCH`-shaped string onto the closed set.
    pub fn from_consts(arch: &str) -> Self {
        match arch {
            "x86_64" => Self::X86_64,
            "aarch64" | "arm64" => Self::Aarch64,
            _ => Self::Other,
        }
    }

    /// This build's architecture.
    pub fn current() -> Self {
        Self::from_consts(std::env::consts::ARCH)
    }
}

/// What kind of crash-adjacent event a [`CrashReport`] describes. Mirrors the
/// `src-tauri` crash ring's `kind` field, as a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CrashKind.ts")]
#[serde(rename_all = "snake_case")]
pub enum CrashKind {
    /// The process panic hook fired.
    Panic,
    /// A watched `JoinHandle` resolved with a panic.
    TaskPanic,
    /// A supervised long-lived task was restarted.
    TaskRestart,
    /// A record whose `kind` this version does not know (a downgrade reading a
    /// newer ring). Kept rather than dropped: the COUNT still matters.
    Other,
}

impl CrashKind {
    /// Map the crash ring's `kind` string onto the closed set.
    pub fn from_record(kind: &str) -> Self {
        match kind {
            "panic" => Self::Panic,
            "task_panic" => Self::TaskPanic,
            "task_restart" => Self::TaskRestart,
            _ => Self::Other,
        }
    }
}

/// WHY a recording's self-test reached its verdict — the machine-readable
/// counterpart of [`crate::selftest::SelfTestReport::reasons`], which is prose.
///
/// One variant per `escalate(...)` call in
/// [`crate::selftest::selftest_verdict`], in the same order, so
/// [`derive_reason_codes`] is a faithful re-reading of that function's decisions
/// from the numbers it recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/QualityReason.ts")]
#[serde(rename_all = "kebab-case")]
pub enum QualityReason {
    /// The delivered file is below the "did anything land" size floor.
    NoAudioCaptured,
    /// The take is silent — no signal at all.
    SilentTake,
    /// A dropout at or above the fail threshold.
    LargeGap,
    /// Drops/xruns at or above the fail thresholds.
    ManyDrops,
    /// A forced sample rate that does not match the device's native rate.
    ForcedRateMismatch,
    /// A gap above the warn threshold but below the fail one.
    SmallGap,
    /// Signal present but weak.
    LowSignal,
    /// Some drops/xruns, below the fail thresholds.
    SomeDrops,
    /// Nothing wrong — the take was clean.
    Clean,
}

/// Every counter name that may appear on the wire.
///
/// A CLOSED enum, not a string, and that is the point: a counter name is the one
/// place a free-form label could sneak user data into an otherwise numeric
/// payload ("export.Gudstjeneste 6. april"). The renderer-facing
/// `telemetry_count` command validates against exactly this list and rejects
/// anything else, so the IPC boundary cannot widen it either.
///
/// The wire strings are dotted namespaces (`area.thing.variant`) so the endpoint
/// can aggregate by prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CounterName.ts")]
pub enum CounterName {
    // ── Recording ────────────────────────────────────────────────────────────
    /// A recording the operator started by hand.
    #[serde(rename = "recording.started.manual")]
    RecordingStartedManual,
    /// A recording the scheduler started.
    #[serde(rename = "recording.started.scheduled")]
    RecordingStartedScheduled,
    /// A recording that was stopped by the operator or the schedule.
    #[serde(rename = "recording.stopped")]
    RecordingStopped,
    /// The pre-roll buffer was armed.
    #[serde(rename = "recording.preroll.started")]
    RecordingPrerollStarted,
    /// A test recording / capture self-test was run.
    #[serde(rename = "recording.selftest")]
    RecordingSelftest,

    // ── Editor ───────────────────────────────────────────────────────────────
    /// The editor was opened on a recording.
    #[serde(rename = "editor.opened")]
    EditorOpened,
    /// An export finished, by delivered format.
    #[serde(rename = "editor.export.mp3")]
    EditorExportMp3,
    #[serde(rename = "editor.export.wav")]
    EditorExportWav,
    #[serde(rename = "editor.export.flac")]
    EditorExportFlac,
    #[serde(rename = "editor.export.video")]
    EditorExportVideo,
    #[serde(rename = "editor.export.other")]
    EditorExportOther,
    /// Mastering was applied to a recording.
    #[serde(rename = "editor.master.applied")]
    EditorMasterApplied,
    /// Chapters were auto-detected.
    #[serde(rename = "editor.chapters.detected")]
    EditorChaptersDetected,
    // (v0.15: `transcribe.run` and `companion.build` left the vocabulary with
    // whisper transcription and the AI companion. Same rule as v0.14 below:
    // removing the SENDER is enough.)

    // ── Files ────────────────────────────────────────────────────────────────
    /// A recording was moved to the trash.
    #[serde(rename = "trash.moved")]
    TrashMoved,
    /// A recording was restored from the trash.
    #[serde(rename = "trash.restored")]
    TrashRestored,

    // ── System ───────────────────────────────────────────────────────────────
    // (v0.14: `streaming.started` left the vocabulary with the live-streaming
    // feature. Removing the SENDER is enough — the Worker treats counter names
    // as opaque strings, and an old client still sending it is simply a name
    // this enum no longer parses.)
    /// The diagnose report was run.
    #[serde(rename = "diagnose.run")]
    DiagnoseRun,
    /// An update was downloaded and installed.
    #[serde(rename = "update.installed")]
    UpdateInstalled,
}

/// Every [`CounterName`], in wire order. The single source of truth for the
/// command's allow-list, the persisted map's key set, and the tests.
pub const ALL_COUNTERS: &[CounterName] = &[
    CounterName::RecordingStartedManual,
    CounterName::RecordingStartedScheduled,
    CounterName::RecordingStopped,
    CounterName::RecordingPrerollStarted,
    CounterName::RecordingSelftest,
    CounterName::EditorOpened,
    CounterName::EditorExportMp3,
    CounterName::EditorExportWav,
    CounterName::EditorExportFlac,
    CounterName::EditorExportVideo,
    CounterName::EditorExportOther,
    CounterName::EditorMasterApplied,
    CounterName::EditorChaptersDetected,
    CounterName::TrashMoved,
    CounterName::TrashRestored,
    CounterName::DiagnoseRun,
    CounterName::UpdateInstalled,
];

impl CounterName {
    /// This counter's wire string (`"editor.export.mp3"`). Goes through serde so
    /// the mapping can never drift from the `#[serde(rename)]` above.
    pub fn as_wire(self) -> &'static str {
        // Every variant has an explicit rename to a `&'static str`, so the match
        // below is the same table serde uses — written out once rather than
        // allocating a String on every call.
        match self {
            Self::RecordingStartedManual => "recording.started.manual",
            Self::RecordingStartedScheduled => "recording.started.scheduled",
            Self::RecordingStopped => "recording.stopped",
            Self::RecordingPrerollStarted => "recording.preroll.started",
            Self::RecordingSelftest => "recording.selftest",
            Self::EditorOpened => "editor.opened",
            Self::EditorExportMp3 => "editor.export.mp3",
            Self::EditorExportWav => "editor.export.wav",
            Self::EditorExportFlac => "editor.export.flac",
            Self::EditorExportVideo => "editor.export.video",
            Self::EditorExportOther => "editor.export.other",
            Self::EditorMasterApplied => "editor.master.applied",
            Self::EditorChaptersDetected => "editor.chapters.detected",
            Self::TrashMoved => "trash.moved",
            Self::TrashRestored => "trash.restored",
            Self::DiagnoseRun => "diagnose.run",
            Self::UpdateInstalled => "update.installed",
        }
    }

    /// Parse a wire string back to a counter, or `None` if it is not one of
    /// ours. THE allow-list check: `telemetry_count("anything else")` is
    /// rejected here, so an untrusted renderer cannot invent a counter name.
    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_COUNTERS.iter().copied().find(|c| c.as_wire() == s)
    }
}

// ── Class 3: the sanitizers ─────────────────────────────────────────────────
//
// Every String that reaches the wire passes through exactly one of these. Each
// enforces a SHAPE (not a blocklist) and a hard character cap, and each is
// idempotent, so re-sanitising a value is always safe.

/// A UUID-shaped install id, or [`NIL_INSTALL_ID`] when the input is not one.
///
/// Shape-checked rather than trusted: the id is read back from a database the
/// user can edit, and a "random id" that is actually `ola@menighet.no` would
/// defeat the entire anonymity claim.
pub fn sanitize_install_id(raw: &str) -> String {
    let ok = raw.len() == 36
        && raw.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    if ok {
        raw.to_ascii_lowercase()
    } else {
        NIL_INSTALL_ID.to_string()
    }
}

/// A version string: the LEADING run of version-shaped characters (ASCII
/// alphanumerics, `.`, `-`, `+`), capped at [`VERSION_MAX_CHARS`].
///
/// Takes a leading run rather than FILTERING, and the difference matters: a
/// filter turns `"0.10.0 (built by Kari)"` into `"0.10.0builtbyKari"` — the
/// separators vanish and the name survives, which is the opposite of the
/// intent. Stopping at the first character a version cannot contain yields
/// `"0.10.0"` and nothing else.
pub fn sanitize_version(raw: &str) -> String {
    raw.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .take(VERSION_MAX_CHARS)
        .collect()
}

/// The PRIMARY language subtag only, lowercased: `"nb-NO"` → `"nb"`,
/// `"pt-BR"` → `"pt"`. `None` when unset or not alphabetic.
///
/// The region subtag is deliberately discarded. Language answers a real product
/// question (which translations are used); region narrows an anonymous install
/// towards a place, which is not what was asked for.
pub fn sanitize_language(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    let primary: String = raw
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_alphabetic)
        .take(LANGUAGE_MAX_CHARS)
        .collect::<String>()
        .to_ascii_lowercase();
    if primary.is_empty() {
        None
    } else {
        Some(primary)
    }
}

/// A stable support code (`SR-CAPTURE-02`, `REC-LOSS`): uppercase ASCII, digits
/// and dashes, capped at [`CODE_MAX_CHARS`]. `None` when nothing survives.
pub fn sanitize_code(raw: &str) -> Option<String> {
    let out: String = raw
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '-')
        .take(CODE_MAX_CHARS)
        .collect();
    (!out.is_empty()).then_some(out)
}

/// A snake_case reason token (`no_resume`, `on_battery`): lowercase ASCII,
/// digits and underscores, capped at [`TOKEN_MAX_CHARS`]. `None` when nothing
/// survives.
///
/// Used for the wake-failure `reason`, which is typed as a free-form `String` in
/// [`crate::wake::WakeFailureEntry`] even though every value it is ever given
/// comes from a fixed vocabulary. The sanitizer makes the wire match the
/// vocabulary rather than the type.
pub fn sanitize_token(raw: &str) -> Option<String> {
    let out: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .take(TOKEN_MAX_CHARS)
        .collect();
    (!out.is_empty()).then_some(out)
}

/// What an absolute path is replaced with in wire-bound free text. Not empty,
/// for the same reason [`crate::redact`] uses visible placeholders: a reader
/// should see that something was removed rather than read a mangled sentence.
pub const PATH_PLACEHOLDER: &str = "<path>";

/// Characters that STRUCTURE a formatted message rather than name a file:
/// quotes, brackets, the Debug/Display punctuation a `format!` puts around a
/// value, and the list separators prose uses.
///
/// They serve two jobs at once, which is why there is one set and not two:
///
///   - a path may START right after one (`Some("/Users/…`, `open(/Users/…`),
///     which is exactly the case the old whitespace tokeniser missed; and
///   - a path ENDS at one (`…x.wav")`, `…x.wav)`), so the closing punctuation
///     and the rest of the sentence survive the replacement.
///
/// Everything NOT in here and not whitespace is treated as part of a path. A
/// filename that does contain one of these loses the tail of its name to the
/// next `<path>` rather than keeping it, which errs safe.
///
/// A SPACE is the exception, and it is a real gap rather than a tidy one — see
/// [`strip_absolute_paths`].
const PATH_DELIMITERS: &[char] = &[
    '"', '\'', '`', '(', ')', '[', ']', '{', '}', '<', '>', '«', '»', ',', ';', '=', '|',
];

/// Whether `c` can sit INSIDE a path — i.e. neither whitespace nor structure.
///
/// `:` counts, which is deliberate and load-bearing twice over: it keeps
/// `C:\Users\…` a single run, and it means the `/` in `https://host/x` is NOT
/// preceded by a boundary, so a URL is left intact rather than mangled into
/// `https:<path>`. A URL is a statement about a server, not about someone's
/// disk, and it is worth keeping in a crash message.
fn is_path_body(c: char) -> bool {
    !c.is_whitespace() && !PATH_DELIMITERS.contains(&c)
}

/// Roots that name an absolute location ON THEIR OWN, matched anywhere in the
/// text — no left boundary required.
///
/// These need no boundary because they cannot mean anything else: a `/Users/`
/// in a message came from a filesystem. That covers the shapes where the
/// character to the left is itself path-ish and so would suppress the anchored
/// rule below — `path:/Users/kari/x`, `file:///Users/kari/x`.
///
/// Lowercase, and matched against a lowercased copy: `C:\USERS\Ola` comes back
/// from Windows APIs in any casing, and macOS is case-insensitive too.
const ABSOLUTE_ROOTS: &[&str] = &[
    "/users/",
    "/home/",
    "/private/",
    "/tmp/",
    "/var/",
    "/volumes/",
    "/applications/",
    "/library/",
    "/opt/",
    "/etc/",
    "/usr/",
    "/mnt/",
    "/media/",
    "/root/",
    "/srv/",
    "\\users\\",
];

/// Whether an absolute path begins at byte offset `i`.
///
/// `prev` is the character immediately to the left (`None` at the start of the
/// text). `lower` is an ASCII-lowercased copy of `text`; lowercasing ASCII never
/// changes a byte's length, so offsets into the two are interchangeable — the
/// same trick [`scrub_paths`] already relies on.
///
/// Two tiers:
///
///   - the [`ABSOLUTE_ROOTS`] and `~/` — unmistakable, so they match anywhere;
///   - a bare `/`, a UNC `\\`, a `X:\` drive — ambiguous, so they match only at
///     a LEFT BOUNDARY, meaning the character to their left is not path-ish.
///
/// The boundary is what keeps RELATIVE paths intact. `src/recorder/engine.rs`
/// is this repository's own layout, identical on every machine, and the most
/// useful thing a crash location carries: its `/` sits after `c`, so no rule
/// fires. An absolute path is a statement about one person's disk; a relative
/// one is a statement about this source tree.
fn path_starts_at(text: &str, lower: &str, i: usize, prev: Option<char>) -> bool {
    let rest = &text[i..];
    if ABSOLUTE_ROOTS.iter().any(|r| lower[i..].starts_with(r)) {
        return true;
    }
    if rest.starts_with("~/") || rest.starts_with("~\\") {
        return true;
    }
    // Anchored shapes below this line.
    if prev.is_some_and(is_path_body) {
        return false;
    }
    let b = rest.as_bytes();
    // `X:\` or `X:/` — a Windows drive. One letter only, which is why a URL
    // scheme (`https:`) can never be mistaken for one.
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/') {
        return true;
    }
    // `\\NAS\share` — a UNC share. Requires something after the two
    // backslashes, so a lone `\\` in prose is not a path.
    if let Some(after) = rest.strip_prefix("\\\\") {
        return after
            .chars()
            .next()
            .is_some_and(|c| c != '\\' && is_path_body(c));
    }
    // A bare POSIX root. Requires a following body character, so the `/` in
    // "og / eller" stays a slash.
    if let Some(after) = rest.strip_prefix('/') {
        return after.chars().next().is_some_and(is_path_body);
    }
    false
}

/// Byte offset one past the end of the path run starting at `from`.
///
/// Runs to the first whitespace or [`PATH_DELIMITERS`] character — EXCEPT that
/// a literal [`USER_PLACEHOLDER`] is swallowed whole. [`scrub_paths`] has
/// already run by this point and may have spliced `<user>` into the middle of
/// the path; without this the `>` would end the run and leave the rest of the
/// path (`\Opptak`, the folder name we are here to remove) on the wire.
fn path_run_end(text: &str, from: usize) -> usize {
    let mut j = from;
    while j < text.len() {
        let rest = &text[j..];
        if rest.starts_with(USER_PLACEHOLDER) {
            j += USER_PLACEHOLDER.len();
            continue;
        }
        let Some(c) = rest.chars().next().filter(|c| is_path_body(*c)) else {
            break;
        };
        j += c.len_utf8();
    }
    j
}

/// Replace every absolute path in `text` with [`PATH_PLACEHOLDER`], whether it
/// stands alone between spaces or sits EMBEDDED in a formatted value.
///
/// The embedded case is why this is not a whitespace tokeniser any more. A
/// tokeniser sees `Some("/Users/kari/Opptak/gudstjeneste.wav")` as one token
/// that starts with `S`, decides it is not a path, and lets the folder and the
/// service name through — while [`scrub_paths`] running before it DOES reach
/// inside, so the operator's name went and everything else stayed. That is the
/// worst of both: the message still carried a path, so the endpoint's own
/// validator rejected it with a 400 and the whole crash report was dropped.
///
/// Scans for path STARTS instead, so structure is not required to sit at a
/// space. What survives is the structure around the path — `Some("<path>")`,
/// `open(<path>)` — which is the part that says what the code was doing.
///
/// One residual, named honestly and NOT fixed here. A path is cut at the first
/// whitespace, so a filename containing a space leaves its tail behind:
///
///   `kunne ikke åpne ~/Opptak/Gudstjeneste 9. november.wav`
///     → `kunne ikke åpne <path> 9. november.wav`
///
/// That tail is a name someone typed. The generated `filenamePattern` names are
/// date/time only, but `FilenameParams::custom_name` — a special recording's
/// title — goes through `sanitize_filename`, which replaces the path-illegal
/// characters and trims the ends but keeps INTERIOR spaces. So this is reachable
/// in practice, not theoretical.
///
/// It is left alone deliberately, because every way to close it guesses. A path
/// run cannot simply eat spaces: `kunne ikke åpne /tmp/x fordi disken er full`
/// would swallow the whole Norwegian sentence, and losing the message loses the
/// only thing that tells two crashes apart. Anything smarter (peek ahead for a
/// token ending in an audio extension) is a heuristic in the one place that must
/// be obviously correct. Closing it properly belongs where the name is, not
/// where the text is — which is what [`telemetry_path`] now does: OUR OWN
/// format sites render a path as `<path>`/`<path:ext>` at insertion, so the
/// messages this app writes are born clean and this scanner is only the safety
/// net for text we do not control. The tail behaviour itself is pinned, not
/// assumed, by `the_spaced_filename_tail_is_a_pinned_boundary` in the tests.
///
/// Idempotent: `<path>` holds no separator, drive or `~`, so a second pass
/// finds no start inside it.
fn strip_absolute_paths(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut prev: Option<char> = None;
    while i < text.len() {
        if path_starts_at(text, &lower, i, prev) {
            out.push_str(PATH_PLACEHOLDER);
            i = path_run_end(text, i);
            // The placeholder ends in `>`, a delimiter — so anything directly
            // after it is at a boundary, as it would have been after the path.
            prev = Some('>');
            continue;
        }
        // `i` always sits on a character boundary: it is advanced either by a
        // whole `char` here or to a run end, which is itself found by walking
        // whole `char`s. Norwegian text is never cut mid-character.
        let c = text[i..].chars().next().expect("i is a char boundary");
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

/// The only route by which developer-authored free text reaches the wire — a
/// panic message, a panic location, a task name.
///
/// Four passes, the first two belt-and-braces over work the crash ring already
/// did at write time (E2.1 scrubs before the record lands on disk, precisely so
/// the file is safe to hand over later):
///
///   1. [`scrub_paths`] — the operator's name out of any absolute path;
///   2. [`redact_secrets`] — anything sitting under a credential-shaped name, in
///      case someone ever formats a token into a panic message;
///   3. [`strip_absolute_paths`] — and then the path itself. Scrubbing turns
///      `/Users/kari/Opptak/gudstjeneste.wav` into `~/Opptak/gudstjeneste.wav`,
///      which no longer names the OPERATOR but still names a folder and a
///      service. The local crash file wants that detail; the wire does not, and
///      the owner scope excludes file paths outright — so on the way out the
///      whole path becomes `<path>`, embedded in a formatted value
///      (`Some("<path>")`) just as much as standing alone;
///   4. a hard character cap (chars, not bytes — a Norwegian message must not be
///      cut mid-character).
///
/// The residual risk is honest and worth naming: a panic message is a `format!`
/// string a developer wrote, and a developer can interpolate anything into one,
/// including a device name that is not path-shaped. The cap bounds it, the three
/// passes cover the categories that actually recur, and the endpoint's 90-day raw
/// retention bounds it in time. The alternative — sending no message at all —
/// would leave a crash report unable to tell two crashes apart, which is the
/// entire reason to collect one.
pub fn sanitize_free_text(raw: &str, home: Option<&str>, max: usize) -> String {
    let cleaned = strip_absolute_paths(&redact_secrets(&scrub_paths(raw, home)));
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    let mut out: String = cleaned.chars().take(max).collect();
    out.push('…');
    out
}

/// File extensions [`telemetry_path`] is allowed to keep. A CLOSED set, in the
/// house style of every other vocabulary in this module: an extension in this
/// list is one the APP chose (its capture containers, its delivery formats, its
/// own sidecar files), so keeping it can never keep a word a person typed. An
/// extension outside it might be the tail of a custom recording title
/// (`Møte.Privat` has the extension `Privat`), so it is dropped, not trusted.
const TELEMETRY_PATH_EXTENSIONS: &[&str] = &[
    "wav", "flac", "mp3", "m4a", "aac", "ogg", "opus", "mkv", "mp4", "mov", "json", "sqlite",
    "txt", "log", "srt", "vtt", "tmp", "part",
];

/// Render a filesystem path for a message that can reach telemetry: `<path>`,
/// or `<path:ext>` when the extension is one the app itself produces.
///
/// ## Why this exists — the two-layer design
///
/// Path hygiene on the wire has two layers, and this helper is the FIRST:
///
///   1. **Insertion-site hygiene (this function, primary).** A message that a
///      panic or a setup error can turn into a crash report must be BORN
///      without the path, at the `format!` that writes it. A message born clean
///      has nothing to leak, whatever the scrubber's blind spots are.
///   2. **[`strip_absolute_paths`] (the safety net).** It washes text we do NOT
///      control — third-party panic messages, library error `Display`s, code
///      that forgot layer 1. It is a scanner over free text, and a scanner has
///      an inherent blind spot: a path run ends at whitespace, so a filename
///      containing a space leaves its tail behind (`~/Opptak/gudstjeneste
///      9. november.wav` → `<path> 9. november.wav` — a service date, on the
///      wire). That tail cannot be closed in the scrubber without guessing at
///      sentence boundaries; it CAN be closed here, by never inserting the name
///      at all.
///
/// So: when formatting a path into a message that can end up in a panic, a
/// setup error, or anything else the crash ring records, use this — and keep
/// the full path in a `tracing::` line if the local log needs it (the log stays
/// on the machine; the ring goes on the wire).
///
/// The kept extension says what KIND of file was involved (often the whole
/// diagnosis: `.sqlite` vs `.wav`), and comes from the closed
/// [`TELEMETRY_PATH_EXTENSIONS`] vocabulary so it can never carry user content.
/// The output is fixed-shape, survives [`sanitize_free_text`] unchanged, and is
/// accepted by the endpoint's path validator — both proven by test.
pub fn telemetry_path(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| TELEMETRY_PATH_EXTENSIONS.contains(&e.as_str()));
    match ext {
        Some(e) => format!("<path:{e}>"),
        None => PATH_PLACEHOLDER.to_string(),
    }
}

// ── The payload ─────────────────────────────────────────────────────────────

/// One crash-ring record, projected onto the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CrashReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    /// Which of the ring's three kinds this is.
    pub kind: CrashKind,
    /// Unix ms (UTC) the record was written.
    #[ts(type = "number")]
    pub at: i64,
    /// The app version that crashed — often NOT the version reading the ring,
    /// which is exactly why it travels with the record.
    pub app_version: String,
    /// The OS that crashed.
    pub os: TelemetryOs,
    /// The panic message: scrubbed, redacted, capped at [`MESSAGE_MAX_CHARS`].
    pub message: String,
    /// `file:line:col`, same treatment. `None` for the task-* kinds.
    pub location: Option<String>,
    /// The supervised task's name, for the task-* kinds.
    pub task: Option<String>,
    /// Whether a backtrace exists ON THE USER'S MACHINE. The backtrace itself is
    /// not sent — see the module docs for why.
    pub backtrace_present: bool,
}

/// One finished recording's health, projected onto the wire.
///
/// Numbers and enums only: no filename, no folder, no device. What a support
/// engineer needs from a recording is whether the audio arrived, and these are
/// the numbers that answer it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/QualityReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    /// Unix ms (UTC) the session ended.
    #[ts(type = "number")]
    pub at: i64,
    /// Wall-clock length of the session.
    pub duration_sec: f64,
    /// Seconds the session should have captured.
    pub expected_sec: f64,
    /// Seconds actually present in the delivered file(s).
    pub measured_sec: f64,
    /// Percent of expected audio missing from the delivery.
    pub loss_pct: f64,
    /// Combined gap (missing duration + interior silence).
    pub gap_sec: f64,
    #[ts(type = "number")]
    pub drops: u64,
    #[ts(type = "number")]
    pub dups: u64,
    #[ts(type = "number")]
    pub xruns: u64,
    /// Live-levels IPC messages dropped on a full channel.
    #[ts(type = "number")]
    pub levels_dropped: u64,
    /// ffmpeg capture back-pressure warnings seen.
    #[ts(type = "number")]
    pub capture_drop_lines: u64,
    /// Non-levels reader messages dropped on a full channel.
    #[ts(type = "number")]
    pub msgs_dropped: u64,
    /// Samples the native engine's callback dropped on ring overrun.
    #[ts(type = "number")]
    pub ring_overrun_samples: u64,
    /// Whether the session ended cleanly.
    pub exit_ok: bool,
    /// The Pass/Warn/Fail verdict, when one was computed.
    pub verdict: Option<SelfTestVerdict>,
    /// WHICH conditions fired — codes, never the Norwegian sentences.
    pub reasons: Vec<QualityReason>,
}

/// One diagnose finding, projected onto the wire: the STABLE code and its
/// severity, and nothing else.
///
/// [`DiagnosticFinding::detail`] is deliberately absent. It is where the useful
/// specifics live — and where a device name ("Fant ikke «Qu-5»"), a free-GB
/// figure for a named volume, or a save folder ends up. The code alone is enough
/// to count how often each situation occurs across installs, which is the only
/// question aggregate telemetry can honestly answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/FindingReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct FindingReport {
    pub code: String,
    pub severity: DiagnosticSeverity,
}

/// One wake-failure outcome, projected onto the wire.
///
/// [`WakeFailureEntry::label`] (the slot name — user-authored free text, e.g.
/// "Gudstjeneste Nordstrand") and `scheduled_at` (a local ISO string carrying the
/// congregation's service time) are both absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/WakeFailureReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct WakeFailureReport {
    pub kind: WakeFailureKind,
    /// Unix ms (UTC) the outcome was recorded.
    #[ts(type = "number")]
    pub at: i64,
    /// The fixed-vocabulary reason token, sanitised.
    pub reason: Option<String>,
    /// Observed-vs-expected delta for a test wake.
    #[ts(type = "number | null")]
    pub delta_sec: Option<i64>,
}

/// One named counter and its value since the last successful send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CounterReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct CounterReport {
    pub name: CounterName,
    #[ts(type = "number")]
    pub value: u64,
}

/// The wire-safe projection of the user's settings.
///
/// [`crate::diagnostics::SettingsSummary`] is NOT reusable here: it carries
/// `device_name` and `save_folder` because a LOCAL report a person reads needs
/// them. This is a strictly narrower type for the strictly stricter context, and
/// the exclusions are the point:
///
///   - `deviceName` / `videoDeviceName` — a device name is chosen by the user
///     and routinely contains a person's or a room's name ("Kari sin mikrofon").
///   - `saveFolder` / `editorIntroPath` / `editorOutroPath` — filesystem paths.
///   - `churchName` / `responsiblePerson` — the two fields that would deanonymise
///     an install outright.
///   - `emailAddress` / `emailSmtp*` — addresses and endpoints.
///   - `slots` / `specialRecordings` — user-authored labels and a congregation's
///     weekly rhythm. Only their COUNTS travel.
///
/// What IS here is configuration shape: the choices that explain a quality
/// record. `filenamePattern` is included because it is a four-variant enum, not
/// free text — the `Church` variant says "this install names files after its
/// congregation", never what that congregation is called.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/WireSettings.ts")]
#[serde(rename_all = "camelCase")]
pub struct WireSettings {
    pub channels: ChannelMode,
    /// The sample-rate MODE that actually drives capture.
    pub sample_rate_mode: SampleRate,
    /// The resolved forced rate in Hz, or `None` for native capture.
    pub sample_rate: Option<u32>,
    pub format: FileFormat,
    pub bitrate_kbps: u32,
    pub filename_pattern: FilenamePattern,
    pub input_volume: i32,
    pub video_enabled: bool,
    pub stop_on_silence: bool,
    pub silence_threshold: i32,
    pub split_minutes: i32,
    pub auto_delete_days: i32,
    pub trim_silence: bool,
    /// Whether a pre-roll buffer is armed at all (not how long it is).
    pub preroll_enabled: bool,
    pub show_live_levels: bool,
    /// The escape hatch back to ffmpeg audio capture. Whether anyone still needs
    /// it decides when the legacy path can be deleted.
    pub classic_ffmpeg_audio: bool,
    /// The Windows DirectShow escape hatch, same question.
    pub classic_directshow: bool,
    /// Automatic updates on/off. (An update CHANNEL — stable/beta — exists too,
    /// see [`crate::settings::UpdateChannel`], but it does not travel here.
    /// Note before adding it: the beta ring is small, so "beta" on a report
    /// narrows who sent it — that is a schema decision, not a field to slip in.)
    pub auto_update: bool,
    pub launch_at_login: bool,
    pub wake_from_sleep: bool,
    /// How many weekly slots are configured — not what they are called.
    pub slot_count: u32,
    /// How many one-off special recordings are configured.
    pub special_count: u32,
}

impl WireSettings {
    /// Project the full [`Settings`] down to the wire-safe subset. The single,
    /// explicit allow-list: a field added to `Settings` is absent here until
    /// someone writes it in, which is the safe default.
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            channels: s.channels,
            sample_rate_mode: s.sample_rate_mode,
            sample_rate: s.resolved_sample_rate(),
            format: s.format,
            bitrate_kbps: s.bitrate_kbps(),
            filename_pattern: s.filename_pattern,
            input_volume: s.input_volume,
            video_enabled: s.video_enabled,
            stop_on_silence: s.stop_on_silence,
            silence_threshold: s.silence_threshold,
            split_minutes: s.split_minutes,
            auto_delete_days: s.auto_delete_days,
            trim_silence: s.trim_silence,
            preroll_enabled: s.pre_roll_seconds > 0,
            show_live_levels: s.show_live_levels,
            classic_ffmpeg_audio: s.classic_ffmpeg_audio,
            classic_directshow: s.classic_directshow,
            auto_update: s.auto_update,
            launch_at_login: s.launch_at_login,
            wake_from_sleep: s.wake_from_sleep,
            slot_count: s.slots.len() as u32,
            special_count: s.special_recordings.len() as u32,
        }
    }
}

impl Default for WireSettings {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

/// The complete telemetry payload — the ONLY thing that ever leaves the machine.
///
/// See the module docs for the three-class field discipline, the two judgement
/// calls (no backtraces, reason codes not sentences), and the controller /
/// retention / anonymity terms this contract is one half of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TelemetryPayload.ts")]
#[serde(rename_all = "camelCase")]
pub struct TelemetryPayload {
    /// [`TELEMETRY_SCHEMA`] at build time.
    pub schema: u32,
    /// The consent version this payload was collected under. Lets the endpoint
    /// prove, per row, which scope the user actually agreed to.
    pub consent_version: u32,
    /// The random, machine-local install id. Never derived from anything.
    pub install_id: String,
    /// Unix ms (UTC) the payload was built.
    #[ts(type = "number")]
    pub built_at: i64,
    pub app_version: String,
    pub os: TelemetryOs,
    pub arch: TelemetryArch,
    /// UI language, primary subtag only. `None` = following the OS.
    pub language: Option<String>,
    pub settings: WireSettings,
    pub counters: Vec<CounterReport>,
    pub crashes: Vec<CrashReport>,
    pub quality: Vec<QualityReport>,
    pub findings: Vec<FindingReport>,
    pub wake_failures: Vec<WakeFailureReport>,
    /// Editor corrections, as coarse bands and counts — see
    /// [`corrections`] for the boundaries and the argument for them.
    ///
    /// Collected under consent v2 and no earlier. The endpoint accepts this
    /// field as OPTIONAL and always will: an install still running a build from
    /// before it existed sends a payload without it, and a 400 there would be
    /// dropped without retry and never surface anywhere. See
    /// `sunday-telemetry/src/schema.ts` `OPTIONAL_PAYLOAD_KEYS`, which states
    /// the same rule from the receiving side and the deploy order it implies.
    pub corrections: Vec<CorrectionReport>,
    // (v0.15: `companionOutcomes` left the wire with the AI companion. It was
    // INDEPENDENTLY optional on the receiving side — `sunday-telemetry/src/
    // schema.ts` `OPTIONAL_PAYLOAD_KEYS` — so a payload without it is accepted
    // by the Worker exactly as a pre-v0.12 payload always was. No schema bump:
    // dropping an optional field changes no field's meaning.)
}

impl TelemetryPayload {
    /// An empty payload for `install_id` — the header every payload carries, with
    /// no records in it. The `src-tauri` builder fills the collections in.
    pub fn new(install_id: &str, consent_version: u32, app_version: &str, built_at: i64) -> Self {
        Self {
            schema: TELEMETRY_SCHEMA,
            consent_version,
            install_id: sanitize_install_id(install_id),
            built_at,
            app_version: sanitize_version(app_version),
            os: TelemetryOs::current(),
            arch: TelemetryArch::current(),
            language: None,
            settings: WireSettings::default(),
            counters: Vec::new(),
            crashes: Vec::new(),
            quality: Vec::new(),
            findings: Vec::new(),
            wake_failures: Vec::new(),
            corrections: Vec::new(),
        }
    }

    /// Whether this payload carries anything worth sending. A payload with no
    /// crashes, no quality records, no findings, no wake failures, no banded
    /// correction and no non-zero counter is just a
    /// header — sending it would be a ping, and a ping is not what the user
    /// consented to.
    ///
    /// Every collection must be consulted here, and
    /// `every_record_collection_counts_towards_is_empty` fails if one is not:
    /// this is what «vis hva som sendes» captions the preview with, so a
    /// collection missing from this list would be printed on screen underneath
    /// the words "ingenting å sende akkurat nå".
    pub fn is_empty(&self) -> bool {
        self.crashes.is_empty()
            && self.quality.is_empty()
            && self.findings.is_empty()
            && self.wake_failures.is_empty()
            && self.counters.iter().all(|c| c.value == 0)
            // Zero-count entries are empty for the same reason zero-valued
            // counters are: the accumulator never emits one, so a payload
            // holding only zeroes is a header wearing a collection.
            && self.corrections.iter().all(|c| c.count == 0)
    }

    /// Trim every collection to its cap, keeping the NEWEST records (the ends of
    /// the newest-last rings the sources use). Called by the builder after the
    /// collections are filled.
    pub fn truncate_to_caps(&mut self) {
        keep_last(&mut self.crashes, MAX_CRASHES);
        keep_last(&mut self.quality, MAX_QUALITY);
        keep_last(&mut self.findings, MAX_FINDINGS);
        keep_last(&mut self.wake_failures, MAX_WAKE_FAILURES);
        // Bounded by construction rather than by policy — the collection is
        // keyed by three closed enums, so it cannot hold more entries than their
        // product. Trimmed anyway, because "cannot happen" is the wrong thing to
        // rest a size cap on when the endpoint rejects an oversized array.
        keep_last(&mut self.corrections, MAX_CORRECTIONS);
    }
}

/// What `telemetry_preview_payload` hands the settings UI: the real payload as
/// pretty JSON, plus the two facts the UI needs to label it honestly.
///
/// The label matters. "Vis hva som sendes" has two different truthful answers
/// depending on consent, and a screen that shows the same words for both is
/// lying in one of them:
///
///   - consent ACTIVE — [`Self::is_next_payload`] is `true` and the JSON is
///     literally the next payload, built from the live watermarks;
///   - consent OFF — there IS no next payload, so the JSON is built from the
///     machine's whole local history instead. That shows the user their own
///     crashes and their own quality numbers in the real shape, which is what
///     the question is actually asking. An empty shell would be technically
///     accurate and tell them nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TelemetryPreview.ts")]
#[serde(rename_all = "camelCase")]
pub struct TelemetryPreview {
    /// The payload, pretty-printed. The REAL one — not a mock, not a sample.
    pub json: String,
    /// `true` when this is literally what would be sent next; `false` when
    /// telemetry is off and the preview was filled from local history to show
    /// the shape. See the struct docs.
    pub is_next_payload: bool,
    /// Whether the payload carries no records at all
    /// ([`TelemetryPayload::is_empty`]) — so the UI can say "ingenting å sende
    /// akkurat nå" rather than render a header and let the user wonder.
    pub is_empty: bool,
}

/// Keep at most `cap` items, dropping from the FRONT (oldest).
fn keep_last<T>(items: &mut Vec<T>, cap: usize) {
    if items.len() > cap {
        items.drain(..items.len() - cap);
    }
}

// ── Projections ─────────────────────────────────────────────────────────────

/// Project a crash-ring record onto the wire.
///
/// Takes the record's parts rather than the `CrashRecord` type itself, which
/// lives in `src-tauri` (it is deliberately not a shared DTO — see its docs), so
/// this stays a pure core function.
#[allow(clippy::too_many_arguments)]
pub fn crash_report(
    kind: &str,
    at: i64,
    app_version: &str,
    os: &str,
    message: &str,
    location: Option<&str>,
    task: Option<&str>,
    has_backtrace: bool,
    home: Option<&str>,
) -> CrashReport {
    CrashReport {
        kind: CrashKind::from_record(kind),
        at,
        app_version: sanitize_version(app_version),
        os: TelemetryOs::from_consts(os),
        message: sanitize_free_text(message, home, MESSAGE_MAX_CHARS),
        location: location.map(|l| sanitize_free_text(l, home, LOCATION_MAX_CHARS)),
        task: task.map(|t| sanitize_free_text(t, home, TASK_MAX_CHARS)),
        backtrace_present: has_backtrace,
    }
}

/// Project one [`RecordingTelemetry`] onto the wire. `at` is supplied by the
/// caller because the record's own `timestamp` is a LOCAL RFC 3339 string and
/// converting it is the shell's job (it owns `chrono`'s local offset).
pub fn quality_report(t: &RecordingTelemetry, at: i64) -> QualityReport {
    QualityReport {
        at,
        duration_sec: t.duration_sec,
        expected_sec: t.expected_sec,
        measured_sec: t.measured_sec,
        loss_pct: t.loss_pct,
        gap_sec: t.report.as_ref().map(|r| r.gap_sec).unwrap_or(0.0),
        drops: t.drops,
        dups: t.dups,
        xruns: t.xruns,
        levels_dropped: t.levels_dropped,
        capture_drop_lines: t.capture_drop_lines,
        msgs_dropped: t.msgs_dropped,
        ring_overrun_samples: t.ring_overrun_samples,
        exit_ok: t.exit_ok,
        verdict: t.report.as_ref().map(|r| r.verdict),
        reasons: t
            .report
            .as_ref()
            .map(derive_reason_codes)
            .unwrap_or_default(),
    }
}

/// Project one diagnose finding onto the wire — code + severity, never `detail`.
/// `None` when the code does not survive [`sanitize_code`] (which would mean a
/// finding with no code at all, and a bare severity is not worth a row).
pub fn finding_report(f: &DiagnosticFinding) -> Option<FindingReport> {
    Some(FindingReport {
        code: sanitize_code(&f.code)?,
        severity: f.severity,
    })
}

/// Project one wake-failure entry onto the wire — kind, time, reason token and
/// delta; never the label or the scheduled local time.
pub fn wake_failure_report(e: &WakeFailureEntry) -> WakeFailureReport {
    WakeFailureReport {
        kind: e.kind,
        at: e.timestamp,
        reason: e.reason.as_deref().and_then(sanitize_token),
        delta_sec: e.delta_sec,
    }
}

/// Re-derive WHICH self-test conditions fired, from the numbers the report
/// carries — so the wire never has to quote the Norwegian sentence.
///
/// This mirrors [`crate::selftest::selftest_verdict`] condition for condition and
/// in the same order, which is what makes the correspondence testable: a reason
/// added there without a code added here changes the two lists' lengths, and
/// `reason_codes_stay_in_step_with_the_selftest` fails.
pub fn derive_reason_codes(r: &SelfTestReport) -> Vec<QualityReason> {
    let mut out = Vec::new();
    let signal = classify_signal(r.rms_db);

    // FAIL conditions, in selftest_verdict's order.
    if !size_is_plausible(r.size_bytes) {
        out.push(QualityReason::NoAudioCaptured);
    }
    if signal == TestRecordingSignal::Silent {
        out.push(QualityReason::SilentTake);
    }
    if r.gap_sec >= FAIL_GAP_SEC {
        out.push(QualityReason::LargeGap);
    }
    if r.drops >= FAIL_DROPS || r.xruns >= FAIL_XRUNS {
        out.push(QualityReason::ManyDrops);
    }

    // WARN conditions.
    if let (Some(forced), Some(native)) = (r.forced_sample_rate, r.native_sample_rate) {
        if forced != native {
            out.push(QualityReason::ForcedRateMismatch);
        }
    }
    if (WARN_GAP_SEC..FAIL_GAP_SEC).contains(&r.gap_sec) {
        out.push(QualityReason::SmallGap);
    }
    if signal == TestRecordingSignal::Low {
        out.push(QualityReason::LowSignal);
    }
    if (1..FAIL_DROPS).contains(&r.drops) || (1..FAIL_XRUNS).contains(&r.xruns) {
        out.push(QualityReason::SomeDrops);
    }

    if r.verdict == SelfTestVerdict::Pass {
        out.push(QualityReason::Clean);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::{selftest_verdict, SelfTestFacts};
    use serde_json::Value;

    // ─────────────────────────────────────────────────────────────────────────
    //   The field inventory — the privacy ratchet (the E1.3 idiom)
    // ─────────────────────────────────────────────────────────────────────────

    /// EVERY key that may appear anywhere in a serialised payload, with the class
    /// (see the module docs) that makes it safe. A new field is a FAILING TEST
    /// until a human writes it down here with a justification — the point being
    /// that "did anyone think about whether this can carry a device name?" stops
    /// being a question a reviewer has to remember to ask.
    ///
    /// Class: `num` = number/bool, `enum` = closed enum, `san:<fn>` = String
    /// through that sanitizer, `nested` = a sub-object of classified fields.
    const WIRE_FIELDS: &[(&str, &str)] = &[
        // ── header ───────────────────────────────────────────────────────────
        ("schema", "num — the payload version"),
        (
            "consentVersion",
            "num — which consent scope this was collected under",
        ),
        (
            "installId",
            "san:sanitize_install_id — random UUID, never derived",
        ),
        ("builtAt", "num — unix ms UTC"),
        ("appVersion", "san:sanitize_version"),
        ("os", "enum TelemetryOs"),
        ("arch", "enum TelemetryArch"),
        (
            "language",
            "san:sanitize_language — primary subtag only, no region",
        ),
        ("settings", "nested WireSettings"),
        ("counters", "nested CounterReport[]"),
        ("crashes", "nested CrashReport[]"),
        ("quality", "nested QualityReport[]"),
        ("findings", "nested FindingReport[]"),
        ("wakeFailures", "nested WakeFailureReport[]"),
        (
            "corrections",
            "nested CorrectionReport[] — bands and counts, never seconds",
        ),
        // ── CrashReport ──────────────────────────────────────────────────────
        ("kind", "enum CrashKind / WakeFailureKind"),
        ("at", "num — unix ms UTC"),
        (
            "message",
            "san:sanitize_free_text — scrubbed, redacted, capped 200",
        ),
        (
            "location",
            "san:sanitize_free_text — file:line:col, capped 120",
        ),
        (
            "task",
            "san:sanitize_free_text — supervised task name, capped 64",
        ),
        (
            "backtracePresent",
            "num(bool) — the backtrace itself is NOT sent",
        ),
        // ── QualityReport ────────────────────────────────────────────────────
        ("durationSec", "num"),
        ("expectedSec", "num"),
        ("measuredSec", "num"),
        ("lossPct", "num"),
        ("gapSec", "num"),
        ("drops", "num"),
        ("dups", "num"),
        ("xruns", "num"),
        ("levelsDropped", "num"),
        ("captureDropLines", "num"),
        ("msgsDropped", "num"),
        ("ringOverrunSamples", "num"),
        ("exitOk", "num(bool)"),
        ("verdict", "enum SelfTestVerdict"),
        (
            "reasons",
            "enum QualityReason[] — codes, never the sentences",
        ),
        // ── FindingReport ────────────────────────────────────────────────────
        (
            "code",
            "san:sanitize_code — the stable SR-* code, never `detail`",
        ),
        ("severity", "enum DiagnosticSeverity"),
        // ── WakeFailureReport ────────────────────────────────────────────────
        (
            "reason",
            "san:sanitize_token — fixed-vocabulary snake_case token",
        ),
        ("deltaSec", "num — observed-vs-expected wake delta"),
        // ── CounterReport ────────────────────────────────────────────────────
        ("name", "enum CounterName — the closed allow-list"),
        ("value", "num"),
        // ── CorrectionReport ─────────────────────────────────────────────────
        //
        // Four fields, three of them closed enums and one a count. What is NOT
        // here is the point of the whole collection: no `deltaSec`, no `at`, no
        // recording. A band is a magnitude with the precision taken out of it
        // (`corrections::CORRECTION_BAND_EDGES_SEC`), and a wall-clock time is
        // never sent as part of a correction — a time of day next to a duration
        // identifies one service at one church.
        (
            "signal",
            "enum CorrectionSignal — which guess was corrected",
        ),
        (
            "direction",
            "enum CorrectionDirection — earlier|later, never a sign bit",
        ),
        (
            "band",
            "enum CorrectionBand — a coarse bucket, never a number of seconds",
        ),
        ("count", "num — how many records had that shape"),
        // ── WireSettings ─────────────────────────────────────────────────────
        ("channels", "enum ChannelMode"),
        ("sampleRateMode", "enum SampleRate"),
        ("sampleRate", "num — resolved Hz, or null for native"),
        ("format", "enum FileFormat"),
        ("bitrateKbps", "num"),
        (
            "filenamePattern",
            "enum FilenamePattern — 4 tags, not free text",
        ),
        ("inputVolume", "num"),
        ("videoEnabled", "num(bool)"),
        ("stopOnSilence", "num(bool)"),
        ("silenceThreshold", "num"),
        ("splitMinutes", "num"),
        ("autoDeleteDays", "num"),
        ("trimSilence", "num(bool)"),
        ("prerollEnabled", "num(bool)"),
        ("showLiveLevels", "num(bool)"),
        ("classicFfmpegAudio", "num(bool)"),
        ("classicDirectshow", "num(bool)"),
        ("autoUpdate", "num(bool)"),
        ("launchAtLogin", "num(bool)"),
        ("wakeFromSleep", "num(bool)"),
        ("slotCount", "num — how many slots, never their labels"),
        (
            "specialCount",
            "num — how many specials, never their labels",
        ),
    ];

    /// Key names that must NEVER appear on the wire. Exact keys, not substrings:
    /// `filenamePattern` legitimately contains both "file" and "name", and a
    /// substring rule would either miss `deviceName` or ban the enum.
    const FORBIDDEN_KEYS: &[&str] = &[
        "deviceName",
        "videoDeviceName",
        "audioDevices",
        "videoDevices",
        "saveFolder",
        "filePath",
        "path",
        "folder",
        "dir",
        "churchName",
        "church",
        "responsiblePerson",
        "email",
        "emailAddress",
        "emailSmtp",
        "emailSmtpUser",
        "password",
        "token",
        "apiKey",
        "streamKey",
        "webhookUrl",
        "url",
        "backtrace",
        "transcript",
        "text",
        "title",
        "detail",
        "hint",
        "label",
        "note",
        "slots",
        "specialRecordings",
        "scheduledAt",
        "filename",
        "user",
        "username",
        "hostname",
        "machineId",
        "ip",
    ];

    /// A payload with EVERY field populated — the fixture the structural tests
    /// walk. Marker values are deliberately boring; the hostile-input test below
    /// is where the nasty strings go.
    fn maximal_payload() -> TelemetryPayload {
        TelemetryPayload {
            schema: TELEMETRY_SCHEMA,
            consent_version: 1,
            install_id: "0198f0a0-1111-7222-8333-444455556666".into(),
            built_at: 1_800_000_000_000,
            app_version: "0.10.0".into(),
            os: TelemetryOs::Macos,
            arch: TelemetryArch::Aarch64,
            language: Some("nb".into()),
            settings: WireSettings::default(),
            counters: ALL_COUNTERS
                .iter()
                .map(|&name| CounterReport { name, value: 3 })
                .collect(),
            crashes: vec![CrashReport {
                kind: CrashKind::Panic,
                at: 1_799_000_000_000,
                app_version: "0.9.0".into(),
                os: TelemetryOs::Windows,
                message: "marker-message".into(),
                location: Some("src/lib.rs:1:1".into()),
                task: Some("marker-task".into()),
                backtrace_present: true,
            }],
            quality: vec![QualityReport {
                at: 1_799_500_000_000,
                duration_sec: 3600.0,
                expected_sec: 3598.0,
                measured_sec: 3597.5,
                loss_pct: 0.01,
                gap_sec: 0.5,
                drops: 1,
                dups: 2,
                xruns: 3,
                levels_dropped: 4,
                capture_drop_lines: 5,
                msgs_dropped: 6,
                ring_overrun_samples: 7,
                exit_ok: true,
                verdict: Some(SelfTestVerdict::Warn),
                reasons: vec![QualityReason::SmallGap, QualityReason::SomeDrops],
            }],
            findings: vec![FindingReport {
                code: "SR-CAPTURE-02".into(),
                severity: DiagnosticSeverity::Warning,
            }],
            wake_failures: vec![WakeFailureReport {
                kind: WakeFailureKind::Missed,
                at: 1_798_000_000_000,
                reason: Some("no_resume".into()),
                delta_sec: Some(-12),
            }],
            corrections: vec![
                CorrectionReport::new(
                    corrections::CorrectionKey {
                        signal: corrections::CorrectionSignal::SermonStart,
                        direction: corrections::CorrectionDirection::Earlier,
                        band: corrections::CorrectionBand::From30To60s,
                    },
                    2,
                ),
                CorrectionReport::new(
                    corrections::CorrectionKey {
                        signal: corrections::CorrectionSignal::SermonPickEnd,
                        direction: corrections::CorrectionDirection::Later,
                        band: corrections::CorrectionBand::Over120s,
                    },
                    1,
                ),
            ],
        }
    }

    /// Every object key anywhere in a JSON value.
    fn all_keys(v: &Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    out.insert(k.clone());
                    all_keys(child, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|i| all_keys(i, out)),
            _ => {}
        }
    }

    /// Every string VALUE anywhere in a JSON value.
    fn all_strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => map.values().for_each(|c| all_strings(c, out)),
            Value::Array(items) => items.iter().for_each(|i| all_strings(i, out)),
            Value::String(s) => out.push(s.clone()),
            _ => {}
        }
    }

    #[test]
    fn every_wire_field_is_classified() {
        // THE RATCHET. Add a field to any payload struct and this fails until the
        // field is written into WIRE_FIELDS with the class that makes it safe.
        let json = serde_json::to_value(maximal_payload()).expect("serialise");
        let mut keys = std::collections::BTreeSet::new();
        all_keys(&json, &mut keys);

        let classified: std::collections::BTreeSet<String> =
            WIRE_FIELDS.iter().map(|(k, _)| (*k).to_string()).collect();

        let unclassified: Vec<&String> = keys.difference(&classified).collect();
        assert!(
            unclassified.is_empty(),
            "these payload fields are not classified in WIRE_FIELDS — add each one \
             with the class (num / enum / san:<fn>) that makes it safe to send: {unclassified:?}"
        );

        // And the other direction: a stale entry means the inventory is fiction.
        let stale: Vec<&String> = classified.difference(&keys).collect();
        assert!(
            stale.is_empty(),
            "WIRE_FIELDS lists fields the payload no longer has: {stale:?}"
        );

        // Every entry names one of the THREE classes from the module docs.
        // There is no fourth class, and "it's probably fine" is not one.
        for (field, class) in WIRE_FIELDS {
            assert!(
                class.starts_with("num")
                    || class.starts_with("enum")
                    || class.starts_with("san:")
                    || class.starts_with("nested"),
                "{field} is classified {class:?}, which is not one of \
                 num / enum / san:<fn> / nested"
            );
        }
    }

    #[test]
    fn no_excluded_key_can_appear_on_the_wire() {
        let json = serde_json::to_value(maximal_payload()).expect("serialise");
        let mut keys = std::collections::BTreeSet::new();
        all_keys(&json, &mut keys);
        for forbidden in FORBIDDEN_KEYS {
            assert!(
                !keys.contains(*forbidden),
                "the payload carries a `{forbidden}` field — the owner scope excludes it"
            );
        }
    }

    #[test]
    fn hostile_inputs_cannot_reach_the_wire() {
        // Feed every projection the worst thing its source could hold and assert
        // none of it survives into the JSON. This is the test that would have
        // caught reusing `SettingsSummary` (deviceName + saveFolder) wholesale.
        let mut p = TelemetryPayload::new(
            // An install id that is not a UUID at all — someone's e-mail in the
            // database column.
            "ola.nordmann@menighet.no",
            1,
            "0.10.0 (built by Kari)",
            1_800_000_000_000,
        );
        p.language = sanitize_language(Some("nb-NO"));
        p.settings = WireSettings::from_settings(&Settings {
            device_name: Some("Kari sin Qu-5".into()),
            video_device_name: Some("Ola sitt kamera".into()),
            save_folder: Some("/Users/kari/Menigheten/Opptak".into()),
            church_name: "Nordstrand menighet".into(),
            responsible_person: "Kari Nordmann".into(),
            email_address: "kari@menighet.no".into(),
            editor_intro_path: Some("/Users/kari/intro.wav".into()),
            ..Default::default()
        });
        p.crashes.push(crash_report(
            "panic",
            1_799_000_000_000,
            "0.9.0",
            "macos",
            "kunne ikke åpne /Users/kari/Opptak/gudstjeneste.wav (token=sbp_abc123)",
            Some("/Users/kari/dev/sundayrec/src/lib.rs:12:3"),
            Some("scheduler-loop"),
            true,
            Some("/Users/kari"),
        ));
        p.wake_failures.push(wake_failure_report(&WakeFailureEntry {
            timestamp: 1_798_000_000_000,
            scheduled_at: "2026-08-09T11:00:00+02:00".into(),
            kind: WakeFailureKind::Missed,
            label: "Gudstjeneste Nordstrand".into(),
            reason: Some("no_resume".into()),
            delta_sec: None,
        }));
        p.findings.push(
            finding_report(&DiagnosticFinding {
                code: "SR-AUDIO-02".into(),
                severity: DiagnosticSeverity::Warning,
                title: "Valgt lydenhet finnes ikke".into(),
                detail: "«Kari sin Qu-5» ble ikke funnet".into(),
                hint: "Velg enheten på nytt".into(),
            })
            .expect("a well-formed code survives"),
        );

        let text = serde_json::to_string(&p).expect("serialise");
        for needle in [
            "Kari",
            "kari",
            "Ola",
            "ola",
            "Nordstrand",
            "menighet",
            "Qu-5",
            "Opptak",
            "/Users/",
            "sbp_abc123",
            "gudstjeneste.wav",
            "intro.wav",
            "Gudstjeneste",
            "@",
            "NO", // the region subtag of nb-NO
        ] {
            assert!(
                !text.contains(needle),
                "hostile input {needle:?} survived into the payload:\n{text}"
            );
        }
        // The install id fell back to the nil UUID rather than carrying the
        // e-mail that was sitting in its slot.
        assert_eq!(p.install_id, NIL_INSTALL_ID);
        // …and the useful parts DID survive.
        assert_eq!(p.crashes[0].kind, CrashKind::Panic);
        assert!(p.crashes[0].message.contains("kunne ikke åpne"));
        assert!(p.crashes[0].backtrace_present);
        assert_eq!(p.findings[0].code, "SR-AUDIO-02");
    }

    #[test]
    fn no_string_value_has_a_pii_shape() {
        // Even with well-formed inputs, assert the SHAPES that would mean a leak
        // never occur: an absolute path, a Windows path, an e-mail, a URL.
        let json = serde_json::to_value(maximal_payload()).expect("serialise");
        let mut strings = Vec::new();
        all_strings(&json, &mut strings);
        assert!(!strings.is_empty(), "the fixture must exercise strings");
        for s in &strings {
            for shape in ["/Users/", "/home/", "\\Users\\", "C:\\", "@", "://"] {
                assert!(
                    !s.contains(shape),
                    "the string {s:?} has the shape {shape:?}, which reads as PII"
                );
            }
        }
    }

    // ── The sanitizers ───────────────────────────────────────────────────────

    #[test]
    fn install_ids_must_look_like_uuids() {
        let good = "0198f0a0-1111-7222-8333-444455556666";
        assert_eq!(sanitize_install_id(good), good);
        // Uppercase is normalised, not rejected.
        assert_eq!(sanitize_install_id(&good.to_uppercase()), good);
        for bad in [
            "",
            "not-a-uuid",
            "ola@menighet.no",
            "0198f0a0-1111-7222-8333-44445555666", // too short
            "0198f0a0-1111-7222-8333-4444555566667", // too long
            "0198f0a0_1111_7222_8333_444455556666", // wrong separators
            "0198f0a0-1111-7222-8333-44445555666z", // non-hex
        ] {
            assert_eq!(sanitize_install_id(bad), NIL_INSTALL_ID, "{bad}");
        }
    }

    #[test]
    fn versions_stop_at_the_first_non_version_character() {
        assert_eq!(sanitize_version("0.10.0"), "0.10.0");
        assert_eq!(
            sanitize_version("1.2.3-beta.1+build7"),
            "1.2.3-beta.1+build7"
        );
        // The bug this shape catches: FILTERING would have produced
        // "0.10.0Karisinmaskin" — separators gone, the name intact.
        assert_eq!(sanitize_version("0.10.0 (Kari sin maskin)"), "0.10.0");
        assert_eq!(sanitize_version("  0.10.0  "), "0.10.0");
        assert_eq!(sanitize_version("/Users/kari"), "");
        assert!(sanitize_version(&"9".repeat(100)).chars().count() <= VERSION_MAX_CHARS);
        // Idempotent.
        let once = sanitize_version("0.10.0 (Kari)");
        assert_eq!(sanitize_version(&once), once);
    }

    #[test]
    fn languages_lose_their_region() {
        assert_eq!(sanitize_language(Some("nb-NO")).as_deref(), Some("nb"));
        assert_eq!(sanitize_language(Some("pt_BR")).as_deref(), Some("pt"));
        assert_eq!(sanitize_language(Some("EN")).as_deref(), Some("en"));
        assert_eq!(sanitize_language(Some("  no  ")).as_deref(), Some("no"));
        assert_eq!(sanitize_language(None), None);
        assert_eq!(sanitize_language(Some("")), None);
        assert_eq!(sanitize_language(Some("123")), None);
    }

    #[test]
    fn codes_and_tokens_keep_only_their_shape() {
        assert_eq!(
            sanitize_code("SR-CAPTURE-02").as_deref(),
            Some("SR-CAPTURE-02")
        );
        assert_eq!(sanitize_code("rec-loss").as_deref(), Some("REC-LOSS"));
        assert_eq!(
            sanitize_code("SR-AUDIO-01 «Kari sin Qu-5»").as_deref(),
            Some("SR-AUDIO-01KARISINQU-5"),
            "prose is stripped to code characters, and the cap bounds the rest"
        );
        assert_eq!(sanitize_code("«»"), None);
        assert!(sanitize_code(&"A".repeat(200)).unwrap().chars().count() <= CODE_MAX_CHARS);

        assert_eq!(sanitize_token("no_resume").as_deref(), Some("no_resume"));
        assert_eq!(sanitize_token("On Battery").as_deref(), Some("onbattery"));
        assert_eq!(sanitize_token("///"), None);
    }

    #[test]
    fn free_text_is_scrubbed_redacted_stripped_and_capped() {
        // Scrubbing alone leaves `~/Opptak/gudstjeneste.wav` — a folder and a
        // service name. The wire gets neither.
        assert_eq!(
            sanitize_free_text(
                "kunne ikke åpne /Users/kari/Opptak/gudstjeneste.wav",
                Some("/Users/kari"),
                200
            ),
            "kunne ikke åpne <path>"
        );
        assert_eq!(
            sanitize_free_text("failed with token=sbp_secret", None, 200),
            "failed with token=***"
        );
        // A foreign user root goes too, even with no HOME to match.
        assert_eq!(
            sanitize_free_text("at /home/bob/app.rs", None, 200),
            "at <path>"
        );
        // Windows and UNC shapes.
        assert_eq!(
            sanitize_free_text("wrote C:\\Users\\Kari\\rec.wav ok", None, 200),
            "wrote <path> ok"
        );
        assert_eq!(
            sanitize_free_text("share \\\\NAS\\opptak", None, 200),
            "share <path>"
        );
        // A RELATIVE source path is this repo's own layout and stays — it is the
        // most useful thing a crash location carries.
        assert_eq!(
            sanitize_free_text("src/recorder/engine.rs:412:9", None, 200),
            "src/recorder/engine.rs:412:9"
        );
        // Whitespace between tokens survives, including newlines.
        assert_eq!(sanitize_free_text("a\n/tmp/x b", None, 200), "a\n<path> b");
        // Capped at a CHARACTER boundary — a Norwegian message must not be cut
        // mid-character.
        let long = "æ".repeat(MESSAGE_MAX_CHARS + 50);
        let cut = sanitize_free_text(&long, None, MESSAGE_MAX_CHARS);
        assert_eq!(cut.chars().count(), MESSAGE_MAX_CHARS + 1);
        assert!(cut.ends_with('…'));
        // Idempotent.
        let once = sanitize_free_text("/Users/kari/x token=abc", Some("/Users/kari"), 200);
        assert_eq!(sanitize_free_text(&once, Some("/Users/kari"), 200), once);
    }

    /// The scrubber's boundary, PINNED rather than assumed: exactly what a
    /// filename containing spaces does and does not lose on the way out.
    ///
    /// A path run ends at the first whitespace ([`strip_absolute_paths`]'s
    /// documented residual), so the tail of a spaced filename — user content, a
    /// service date — survives BOTH this scrubber and the endpoint's own
    /// validator (asserted below with the mirrored Worker regex). Neither layer
    /// downstream catches it. That is precisely why the fix for OUR OWN
    /// messages is [`telemetry_path`] at the insertion site, and why this test
    /// exists: if the scrubber's behaviour on these shapes ever changes, the
    /// boundary moved and the division of labour must be re-argued, not
    /// silently re-assumed.
    #[test]
    fn the_spaced_filename_tail_is_a_pinned_boundary() {
        // (input, home, exact output) — every case a `format!` in this app or a
        // library could realistically produce.
        let cases: &[(&str, Option<&str>, &str)] = &[
            // Tilde path, spaced Norwegian date name: the head goes, the tail stays.
            (
                "kunne ikke åpne ~/Opptak/Gudstjeneste 9. november.wav",
                None,
                "kunne ikke åpne <path> 9. november.wav",
            ),
            // Same through the home branch of `scrub_paths`.
            (
                "kunne ikke åpne /Users/kari/Opptak/gudstjeneste 9. november.wav",
                Some("/Users/kari"),
                "kunne ikke åpne <path> 9. november.wav",
            ),
            // Windows, with a SPACED user name too: the operator's name is
            // consumed (scrub → `<user>`, and the run swallows the placeholder
            // whole), only the date tail remains.
            (
                "finalize C:\\Users\\Kirke Vert\\Opptak\\gudstjeneste 9. november.wav feilet",
                None,
                "finalize <path> 9. november.wav feilet",
            ),
            // Quotes around the path: the closing quote survives with the tail.
            (
                "kunne ikke åpne \"/Users/kari/Opptak/gudstjeneste 9. november.wav\"",
                Some("/Users/kari"),
                "kunne ikke åpne \"<path> 9. november.wav\"",
            ),
            (
                "sti 'C:\\Users\\Kirke Vert\\Opptak\\gudstjeneste 9. november.wav' mangler",
                None,
                "sti '<path> 9. november.wav' mangler",
            ),
            // æøå are path-body characters: a spaceless Norwegian name is
            // swallowed WHOLE — no tail, nothing mid-character.
            (
                "slette ~/Opptak/påskegudstjeneste-søndag.wav gikk galt",
                None,
                "slette <path> gikk galt",
            ),
            // A spaceless path at the very end of the string: fully consumed.
            (
                "kunne ikke slette ~/Opptak/opptak.wav",
                None,
                "kunne ikke slette <path>",
            ),
            // The shape that motivated the insertion-site fix in `lib.rs`: the
            // OS's own app-data dir has a space (`Application Support`), so a
            // setup error formatted with `.display()` leaves this tail. Not a
            // person's data — but a path fragment on the wire all the same,
            // which is why that message is now born clean instead.
            (
                "Failed to setup app: opening database at \
                 ~/Library/Application Support/no.sundaysuite.sundayrec/sundayrec.sqlite: \
                 unable to open database file",
                None,
                "Failed to setup app: opening database at \
                 <path> Support/no.sundaysuite.sundayrec/sundayrec.sqlite: \
                 unable to open database file",
            ),
        ];

        let re = regex::Regex::new(WORKER_ABSOLUTE_PATH_RE).expect("the mirror must compile");
        for (raw, home, want) in cases {
            let got = sanitize_free_text(raw, *home, MESSAGE_MAX_CHARS);
            assert_eq!(&got, want, "input: {raw}");
            // The boundary itself: what leaked here would ALSO pass the
            // endpoint's validator — no layer downstream catches the tail.
            assert!(
                !re.is_match(&got),
                "the endpoint accepts this output, tail and all: {got}"
            );
            // Idempotent on the pinned shapes: a second pass changes nothing.
            assert_eq!(sanitize_free_text(&got, *home, MESSAGE_MAX_CHARS), got);
            // And in every case: no user name, no folder name, no path root.
            for needle in [
                "kari", "Kirke", "Vert", "Opptak", "/Users", "\\Users", "~/", "C:\\",
            ] {
                assert!(!got.contains(needle), "{needle:?} survived in: {got}");
            }
        }
    }

    // ── telemetry_path: insertion-site hygiene ───────────────────────────────

    #[test]
    fn a_telemetry_path_is_born_clean() {
        use std::path::Path;

        // A known extension is kept — it names a format the APP chose, and it
        // is often the diagnosis (`.sqlite` vs `.wav`).
        assert_eq!(
            telemetry_path(Path::new("/Users/kari/Opptak/gudstjeneste 9. november.wav")),
            "<path:wav>"
        );
        assert_eq!(
            telemetry_path(Path::new(
                "/Users/kari/Library/Application Support/no.sundaysuite.sundayrec/sundayrec.sqlite"
            )),
            "<path:sqlite>"
        );
        // Case-insensitive: a FAT volume shouting `.WAV` is still a wav.
        assert_eq!(telemetry_path(Path::new("C:\\Opptak\\X.WAV")), "<path:wav>");
        // No extension → the bare placeholder.
        assert_eq!(telemetry_path(Path::new("/tmp/x")), "<path>");
        // An extension OUTSIDE the closed set may be the tail of a title
        // someone typed (`Møte.Privat` → extension `Privat`): dropped, never
        // trusted.
        assert_eq!(
            telemetry_path(Path::new("/Users/kari/Opptak/Møte.Privat")),
            "<path>"
        );
        // A dotted date with a trailing word is not a keepable extension
        // either — `9. november` has the "extension" ` november`.
        assert_eq!(
            telemetry_path(Path::new("~/Opptak/gudstjeneste 9. november")),
            "<path>"
        );

        // The whole point: a message BUILT with the helper passes the wire
        // pipeline unchanged (nothing to scrub) and the endpoint's validator
        // (nothing to reject) — proven against both, not assumed.
        let re = regex::Regex::new(WORKER_ABSOLUTE_PATH_RE).expect("the mirror must compile");
        let msg = format!(
            "opening database at {}: unable to open database file",
            telemetry_path(Path::new(
                "/Users/kari/Library/Application Support/no.sundaysuite.sundayrec/sundayrec.sqlite"
            ))
        );
        assert_eq!(
            sanitize_free_text(&msg, Some("/Users/kari"), MESSAGE_MAX_CHARS),
            msg,
            "a message born clean must survive the safety net unchanged"
        );
        assert!(!re.is_match(&msg));
    }

    /// The messages this app can actually produce with a path in them, in the
    /// shapes a `format!` puts one in. Used by both tests below.
    ///
    /// Every one of these is a REAL shape: `{:?}` on an `Option<PathBuf>`, a
    /// newtype, a function call written into an error string, a config dump.
    /// The home directory is `/Users/kari`, the folder is `Opptak`, the service
    /// is `gudstjeneste.wav`.
    const EMBEDDED_PATH_CASES: &[&str] = &[
        // Standing alone between spaces — the case that always worked.
        "kunne ikke åpne /Users/kari/Opptak/gudstjeneste.wav",
        // Embedded — the case that did not. No whitespace before the path, so
        // the old whitespace tokeniser never saw it as a path token, while
        // `scrub_paths` DID reach inside and turned it into `~/Opptak/…`: the
        // operator's name gone, the folder and the service name still there.
        r#"kunne ikke åpne Some("/Users/kari/Opptak/gudstjeneste.wav")"#,
        r#"AudioPath("/Users/kari/Opptak/x.wav") finnes ikke"#,
        "open(/Users/kari/Opptak/x.wav) feilet",
        r#"cfg{path:"/Users/kari/x"} er ugyldig"#,
        // Windows and UNC, embedded the same way.
        "sti <C:\\Users\\kari\\Opptak> mangler",
        "listen(\\\\NAS\\opptak\\x.wav) feilet",
        // A path reached by a route that leaves no boundary to its left.
        "cfg path:/Users/kari/Opptak/x.wav mangler",
        "file:///Users/kari/Opptak/x.wav gav 404",
    ];

    /// Every crash report [`EMBEDDED_PATH_CASES`] can produce, with a HOME and
    /// without one (a service account, or a profile the app could not resolve —
    /// the two take different branches through [`scrub_paths`]).
    fn embedded_case_messages() -> Vec<(&'static str, Option<&'static str>, String)> {
        let mut out = Vec::new();
        for raw in EMBEDDED_PATH_CASES {
            for home in [Some("/Users/kari"), None] {
                let c = crash_report(
                    "panic",
                    1_800_000_000_000,
                    "0.10.0",
                    "macos",
                    raw,
                    Some(raw),
                    Some(raw),
                    false,
                    home,
                );
                out.push((*raw, home, c.message));
                out.push((*raw, home, c.location.expect("location was given")));
                out.push((*raw, home, c.task.expect("task was given")));
            }
        }
        out
    }

    #[test]
    fn an_embedded_path_loses_every_component_not_just_the_user_name() {
        // PRIVACY.md makes two promises about this field: that file paths from
        // your machine are never sent, and that the name you gave a recording is
        // never sent. `~/Opptak/gudstjeneste.wav` breaks both. Scrubbing the
        // user name out of a path is NOT enough — the folder and the file name
        // are the recording's identity.
        //
        // Fails on main: there the embedded cases come out as
        // `Some("~/Opptak/gudstjeneste.wav")` and `<C:\Users\<user>\Opptak>`.
        let needles = [
            "Opptak",
            "opptak",
            "gudstjeneste",
            "kari",
            "Kari",
            "NAS",
            "x.wav",
            "/Users",
            "/users",
            "\\Users",
            "C:\\",
            "C:/",
            "~/",
            "~\\",
            "\\\\",
            "/home/",
        ];
        for (raw, home, got) in embedded_case_messages() {
            for needle in needles {
                assert!(
                    !got.contains(needle),
                    "the path component {needle:?} survived.\n  in:   {raw}\n  home: {home:?}\n  out:  {got}"
                );
            }
        }

        // …and the SENTENCE survived, which is the whole point of sending a
        // message at all: two crashes must still be distinguishable.
        let shape = |raw: &str, home| {
            crash_report(
                "panic",
                1_800_000_000_000,
                "0.10.0",
                "macos",
                raw,
                None,
                None,
                false,
                home,
            )
            .message
        };
        assert_eq!(
            shape(
                r#"kunne ikke åpne Some("/Users/kari/Opptak/gudstjeneste.wav")"#,
                Some("/Users/kari")
            ),
            r#"kunne ikke åpne Some("<path>")"#
        );
        assert_eq!(
            shape("open(/Users/kari/Opptak/x.wav) feilet", Some("/Users/kari")),
            "open(<path>) feilet"
        );
        assert_eq!(
            shape(r#"AudioPath("/Users/kari/Opptak/x.wav") finnes ikke"#, None),
            r#"AudioPath("<path>") finnes ikke"#
        );
        assert_eq!(
            shape("listen(\\\\NAS\\opptak\\x.wav) feilet", None),
            "listen(<path>) feilet"
        );

        // Idempotent on the embedded shapes too — running the scrubber on its
        // own output must not change it further.
        for (_, home, got) in embedded_case_messages() {
            assert_eq!(
                sanitize_free_text(&got, home, MESSAGE_MAX_CHARS),
                got,
                "not idempotent: {got}"
            );
        }
    }

    /// The endpoint's own rejection rule, MIRRORED.
    ///
    /// Copied verbatim (modulo Rust escaping) from `ABSOLUTE_PATH_RE` in
    /// `sunday-telemetry/src/schema.ts`, where a string field that matches it is
    /// rejected with `unscrubbed_path` — a 400, which this client drops without
    /// retrying. **The two must be changed together.** If you loosen this
    /// mirror, loosen the Worker; if you tighten the Worker, tighten this.
    ///
    /// This is the seam that had no test, which is why the bug survived: both
    /// repos were internally consistent and disagreed at the boundary, so every
    /// crash report naming a home-relative path vanished silently. Same shape as
    /// the truncation defect fixed in `sunday-telemetry/test/truncation.test.ts`.
    const WORKER_ABSOLUTE_PATH_RE: &str = r#"(^|[\s"'(<\[])(/(Users|home|var|tmp|private|Volumes)/|[A-Za-z]:[\\/]|\\\\[^\s\\]+\\|~[\\/])"#;

    #[test]
    fn scrubbed_free_text_is_accepted_by_the_endpoints_own_validator() {
        let re = regex::Regex::new(WORKER_ABSOLUTE_PATH_RE).expect("the mirror must compile");

        // The mirror is LIVE, not vacuous: it flags exactly what the client
        // used to emit. If this block ever stops matching, the mirror has
        // drifted from the Worker and the test below proves nothing.
        for was_sent in [
            r#"kunne ikke åpne Some("~/Opptak/gudstjeneste.wav")"#,
            "open(~/Opptak/x.wav) feilet",
            "sti <C:\\Users\\<user>\\Opptak> mangler",
            "listen(\\\\NAS\\opptak\\x.wav) feilet",
            "/Users/kari/x",
        ] {
            assert!(
                re.is_match(was_sent),
                "the mirror should reject {was_sent:?} — it is an unscrubbed path"
            );
        }

        // Nothing the fixed client produces is rejected.
        for (raw, home, got) in embedded_case_messages() {
            assert!(
                !re.is_match(&got),
                "the endpoint would answer 400 unscrubbed_path and this client \
                 would drop the report without retrying.\n  in:   {raw}\n  home: {home:?}\n  out:  {got}"
            );
        }

        // Every string in a maximal payload, not only the free-text ones.
        let json = serde_json::to_value(maximal_payload()).expect("serialise");
        let mut strings = Vec::new();
        all_strings(&json, &mut strings);
        assert!(!strings.is_empty(), "the fixture must exercise strings");
        for s in &strings {
            assert!(!re.is_match(s), "the endpoint would reject the field {s:?}");
        }

        // And the thing the report is FOR still gets through: a source location
        // inside this crate is identical on every machine and is not a path on
        // anyone's disk.
        let loc = sanitize_free_text("src/recorder/engine.rs:412:9", None, 200);
        assert_eq!(loc, "src/recorder/engine.rs:412:9");
        assert!(!re.is_match(&loc));
    }

    // ── The closed enums ─────────────────────────────────────────────────────

    #[test]
    fn os_and_arch_are_closed_sets() {
        assert_eq!(TelemetryOs::from_consts("macos"), TelemetryOs::Macos);
        assert_eq!(TelemetryOs::from_consts("windows"), TelemetryOs::Windows);
        assert_eq!(TelemetryOs::from_consts("linux"), TelemetryOs::Linux);
        assert_eq!(TelemetryOs::from_consts("freebsd"), TelemetryOs::Other);
        // A hostile "OS" string lands in Other rather than on the wire.
        assert_eq!(
            TelemetryOs::from_consts("macos (Kari sin maskin)"),
            TelemetryOs::Other
        );
        assert_eq!(TelemetryArch::from_consts("x86_64"), TelemetryArch::X86_64);
        assert_eq!(
            TelemetryArch::from_consts("aarch64"),
            TelemetryArch::Aarch64
        );
        assert_eq!(TelemetryArch::from_consts("riscv64"), TelemetryArch::Other);
    }

    #[test]
    fn crash_kinds_map_the_rings_vocabulary() {
        assert_eq!(CrashKind::from_record("panic"), CrashKind::Panic);
        assert_eq!(CrashKind::from_record("task_panic"), CrashKind::TaskPanic);
        assert_eq!(
            CrashKind::from_record("task_restart"),
            CrashKind::TaskRestart
        );
        assert_eq!(CrashKind::from_record("something-new"), CrashKind::Other);
    }

    #[test]
    fn counter_names_round_trip_and_reject_anything_else() {
        for &c in ALL_COUNTERS {
            assert_eq!(
                CounterName::from_wire(c.as_wire()),
                Some(c),
                "{}",
                c.as_wire()
            );
            // `as_wire` must agree with serde, or the persisted map and the wire
            // would use different strings.
            let via_serde = serde_json::to_value(c).unwrap();
            assert_eq!(via_serde.as_str(), Some(c.as_wire()));
        }
        for bad in [
            "",
            "editor.export",
            "recording.started",
            "../../etc/passwd",
            "export.Gudstjeneste 6. april",
            "EDITOR.OPENED",
        ] {
            assert_eq!(CounterName::from_wire(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_counter_list_is_complete_and_unique() {
        // ALL_COUNTERS is the allow-list; a variant missing from it would be
        // uncountable, and a duplicate wire string would merge two counters.
        let wires: std::collections::BTreeSet<&str> =
            ALL_COUNTERS.iter().map(|c| c.as_wire()).collect();
        assert_eq!(
            wires.len(),
            ALL_COUNTERS.len(),
            "duplicate counter wire name"
        );
        // R1 of «Frivilligen først» retired the review/publish/cloud counters
        // with their features, R2 the transcribe/companion/chapter ones (the
        // Worker treats names as opaque strings, so a sender that stops sending
        // one costs nothing); the floor follows.
        assert!(
            ALL_COUNTERS.len() >= 15,
            "the seam coverage target is ~15 counters, found {}",
            ALL_COUNTERS.len()
        );
        // Every name is a dotted, lowercase namespace — the endpoint aggregates
        // by prefix, and a stray capital or space would break that silently.
        for c in ALL_COUNTERS {
            let w = c.as_wire();
            assert!(w.contains('.'), "{w} is not namespaced");
            assert!(
                w.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.'),
                "{w} has characters outside [a-z0-9.]"
            );
        }
    }

    // ── The projections ──────────────────────────────────────────────────────

    #[test]
    fn reason_codes_stay_in_step_with_the_selftest() {
        // The correspondence that makes codes-instead-of-sentences honest: for
        // every combination of facts, `derive_reason_codes` must produce exactly
        // as many codes as `selftest_verdict` produced reasons. A new reason
        // without a new code fails HERE, at the moment it is added.
        let mut checked = 0;
        for size_bytes in [0u64, 5_000_000] {
            for rms in [None, Some(-80.0), Some(-45.0), Some(-12.0)] {
                for gap in [0.0f64, 0.5, 2.0] {
                    for drops in [0u64, 3, 20] {
                        for xruns in [0u64, 2, 9] {
                            for rates in [
                                (None, None),
                                (Some(48_000u32), Some(48_000u32)),
                                (Some(48_000), Some(44_100)),
                            ] {
                                let facts = SelfTestFacts {
                                    expected_sec: 10.0,
                                    measured_sec: 10.0 - gap,
                                    drops,
                                    dups: 0,
                                    xruns,
                                    size_bytes,
                                    strongest_rms_db: rms,
                                    silence_total_sec: 0.0,
                                    native_sample_rate: rates.1,
                                    forced_sample_rate: rates.0,
                                };
                                let report = selftest_verdict(&facts);
                                let codes = derive_reason_codes(&report);
                                assert_eq!(
                                    codes.len(),
                                    report.reasons.len(),
                                    "codes {codes:?} vs reasons {:?} for {facts:?}",
                                    report.reasons
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 100, "the matrix must actually cover something");
    }

    #[test]
    fn a_clean_take_reports_exactly_one_clean_code() {
        let facts = SelfTestFacts {
            expected_sec: 60.0,
            measured_sec: 60.0,
            size_bytes: 5_000_000,
            strongest_rms_db: Some(-12.0),
            ..Default::default()
        };
        let report = selftest_verdict(&facts);
        assert_eq!(report.verdict, SelfTestVerdict::Pass);
        assert_eq!(derive_reason_codes(&report), vec![QualityReason::Clean]);
    }

    #[test]
    fn a_quality_report_carries_the_numbers_and_no_names() {
        let facts = SelfTestFacts {
            expected_sec: 60.0,
            measured_sec: 40.0,
            drops: 40,
            size_bytes: 5_000_000,
            strongest_rms_db: Some(-12.0),
            ..Default::default()
        };
        let t = RecordingTelemetry {
            drops: 40,
            dups: 1,
            xruns: 2,
            levels_dropped: 3,
            capture_drop_lines: 4,
            msgs_dropped: 5,
            ring_overrun_samples: 6,
            expected_sec: 60.0,
            measured_sec: 40.0,
            loss_pct: 33.3,
            duration_sec: 60.0,
            timestamp: "2026-08-06T12:00:00+02:00".into(),
            exit_ok: true,
            report: Some(selftest_verdict(&facts)),
            ..Default::default()
        };
        let q = quality_report(&t, 1_800_000_000_000);
        assert_eq!(q.at, 1_800_000_000_000);
        assert_eq!(q.drops, 40);
        assert_eq!(q.ring_overrun_samples, 6);
        assert_eq!(q.verdict, Some(SelfTestVerdict::Fail));
        assert!(q.reasons.contains(&QualityReason::LargeGap));
        assert!(q.reasons.contains(&QualityReason::ManyDrops));
        // A legacy row with no report still projects (verdict/reasons empty).
        let legacy = RecordingTelemetry {
            report: None,
            ..t.clone()
        };
        let q2 = quality_report(&legacy, 1);
        assert_eq!(q2.verdict, None);
        assert!(q2.reasons.is_empty());
    }

    #[test]
    fn a_finding_without_a_usable_code_is_dropped() {
        assert!(finding_report(&DiagnosticFinding {
            code: "«»".into(),
            severity: DiagnosticSeverity::Critical,
            title: String::new(),
            detail: String::new(),
            hint: String::new(),
        })
        .is_none());
    }

    // ── The payload itself ───────────────────────────────────────────────────

    #[test]
    fn an_empty_payload_is_recognised_as_not_worth_sending() {
        let mut p = TelemetryPayload::new(NIL_INSTALL_ID, 1, "0.10.0", 0);
        assert!(p.is_empty());
        // Zero-valued counters are still empty — a header is not a report.
        p.counters = vec![CounterReport {
            name: CounterName::EditorOpened,
            value: 0,
        }];
        assert!(p.is_empty());
        p.counters[0].value = 1;
        assert!(!p.is_empty());
    }

    #[test]
    fn a_payload_carrying_only_corrections_is_worth_sending() {
        // The other half of the ratchet below. A payload whose ONLY content is a
        // banded correction has to be both sent AND labelled non-empty: if
        // `is_empty` missed this collection, the preview would print the bands on
        // screen under the caption «ingenting å sende akkurat nå», which is the
        // transparency affordance contradicting itself.
        let mut p = TelemetryPayload::new(NIL_INSTALL_ID, 2, "0.10.0", 0);
        assert!(p.is_empty());
        p.corrections = vec![CorrectionReport::new(
            corrections::CorrectionKey {
                signal: corrections::CorrectionSignal::SermonStart,
                direction: corrections::CorrectionDirection::Earlier,
                band: corrections::CorrectionBand::From30To60s,
            },
            0,
        )];
        assert!(
            p.is_empty(),
            "a zero count is a header, like a zero counter"
        );
        p.corrections[0].count = 1;
        assert!(!p.is_empty());
    }

    #[test]
    fn every_record_collection_counts_towards_is_empty() {
        // A ratchet beside `every_wire_field_is_classified`, guarding a
        // different promise. `is_empty` is what «vis hva som sendes» labels the
        // preview with (`TelemetryPreview::is_empty` → «ingenting å sende
        // akkurat nå»), so a collection this function does not consult would be
        // printed in the JSON on screen underneath a caption telling the user
        // there is nothing there — the preview under-reporting itself, which is
        // precisely what the transparency affordance exists to rule out.
        //
        // E8 added `corrections` (and, until v0.15, `companionOutcomes`), and
        // this test is how each got wired in rather than forgotten: it failed
        // the moment the collection landed on the payload, and the only honest
        // way to make it pass was to teach `is_empty` to consult it. The same
        // ratchet runs the other way: `stale` below is what caught the
        // companion collection leaving the payload.
        const CONSULTED: &[&str] = &[
            "counters",
            "crashes",
            "quality",
            "findings",
            "wakeFailures",
            "corrections",
        ];

        let value = serde_json::to_value(maximal_payload()).expect("serialise");
        let Value::Object(map) = value else {
            panic!("a payload serialises to an object");
        };
        let collections: std::collections::BTreeSet<&str> = map
            .iter()
            .filter(|(_, v)| v.is_array())
            .map(|(k, _)| k.as_str())
            .collect();
        let consulted: std::collections::BTreeSet<&str> = CONSULTED.iter().copied().collect();

        let ignored: Vec<&&str> = collections.difference(&consulted).collect();
        assert!(
            ignored.is_empty(),
            "these payload collections are not consulted by TelemetryPayload::is_empty, \
             so a payload carrying only them would be sent while the preview calls it \
             empty: {ignored:?}"
        );
        let stale: Vec<&&str> = consulted.difference(&collections).collect();
        assert!(
            stale.is_empty(),
            "is_empty consults collections the payload no longer has: {stale:?}"
        );

        assert!(
            !maximal_payload().is_empty(),
            "the fixture must exercise the non-empty branch"
        );
    }

    #[test]
    fn the_caps_keep_the_newest_records() {
        let mut p = maximal_payload();
        p.crashes = (0..MAX_CRASHES + 5)
            .map(|i| CrashReport {
                kind: CrashKind::Panic,
                at: i as i64,
                app_version: "0.10.0".into(),
                os: TelemetryOs::Macos,
                message: format!("p{i}"),
                location: None,
                task: None,
                backtrace_present: false,
            })
            .collect();
        p.truncate_to_caps();
        assert_eq!(p.crashes.len(), MAX_CRASHES);
        assert_eq!(p.crashes.first().unwrap().message, "p5", "oldest dropped");
        assert_eq!(
            p.crashes.last().unwrap().message,
            format!("p{}", MAX_CRASHES + 4),
            "newest kept"
        );
    }

    #[test]
    fn a_payload_round_trips_through_json() {
        let p = maximal_payload();
        let text = serde_json::to_string(&p).expect("serialise");
        let back: TelemetryPayload = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back, p);
        // The schema is on the wire under a stable key the endpoint reads first.
        assert!(text.contains("\"schema\":1"));
    }

    #[test]
    fn the_settings_projection_is_strictly_narrower_than_the_diagnose_summary() {
        // A guard against the tempting shortcut of reusing SettingsSummary: it
        // has fields this projection must never grow.
        let json = serde_json::to_value(WireSettings::default()).expect("serialise");
        let obj = json.as_object().expect("object");
        for banned in ["deviceName", "saveFolder", "videoDeviceName", "language"] {
            assert!(
                !obj.contains_key(banned),
                "WireSettings must not carry {banned}"
            );
        }
        // …while still answering the questions a quality record needs.
        for needed in ["channels", "sampleRateMode", "format", "videoEnabled"] {
            assert!(obj.contains_key(needed), "WireSettings must carry {needed}");
        }
    }
}
