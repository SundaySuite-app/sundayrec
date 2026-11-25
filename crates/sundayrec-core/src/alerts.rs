//! The sentences the app says to a person who is not looking at the screen —
//! in the two languages SundayRec ships, Norwegian and English (F1 finding A8; F4 cut the other five).
//!
//! ## The hole this closes
//!
//! Every other user-facing surface in this app is localized: the renderer has
//! `legacy/locales`, the tray has [`crate::tray`], the window notices have
//! [`crate::window`]. The native OS notification — the channel that reaches a
//! volunteer who is not looking at the app — was the exception. Its sentences
//! were written in Norwegian, as literals, at the point of failure:
//!
//! ```text
//! dispatch_scheduler_failure(app, "scheduled_start_timeout",
//!     "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon.")
//! ```
//!
//! An English-reading volunteer who set the app to English, whose church runs an
//! unattended 11:00 service, got an English interface — and this sentence, in
//! Norwegian, as the one line that told them what had actually happened.
//!
//! ## The shape
//!
//! One [`AlertText`] variant per SENTENCE, and a `(variant, language)` match
//! that the compiler checks for exhaustiveness. That check is the whole point:
//! a new alert cannot be added in one language, because the code will not build
//! until both arms exist. No runtime "did somebody forget?" test can make
//! that promise, and the tests below therefore spend themselves on what the
//! compiler cannot see — that the two strings are actually two *different*
//! strings, and that a template's `{placeholder}` survived translation.
//!
//! Templates are filled with [`AlertText::fill`], the same `{name}` convention
//! the renderer's catalogue uses, so there is one placeholder syntax in the
//! codebase rather than two.
//!
//! ## What is deliberately NOT here
//!
//! - **Log lines and diagnostics.** `tracing::warn!("scheduler: …")` stays
//!   English. A log is read by whoever is debugging, not by the volunteer, and
//!   a two-language log is a two-language grep.
//! - **Toast warnings** (`notify::warn` → `BackendWarning`). Those carry a
//!   stable CODE and parameters, and the renderer localizes them from
//!   `legacy/locales` — the message string is a fallback detail, not the text a
//!   user reads. Translating them here would put the same sentence in two
//!   catalogs.
//! - **Schedule labels** ("Ukentlig opptak (11:00–13:00)",
//!   [`crate::schedule::missed_recordings`]). They look translatable and are
//!   not: the label is hashed into the durable `notify_seen` key that makes a
//!   missed-Sunday alert fire ONCE. Localize the label and the key changes with
//!   the language, so a volunteer who switches from Norwegian to English is
//!   told about the same missed Sunday a second time — the exact bug
//!   `notify::MissedSlot`'s two time fields exist to prevent.

use crate::lang::Lang;

/// One user-facing sentence the shell says outside the renderer: a native OS
/// notification body or title.
///
/// Fieldless and [`Copy`] on purpose — `supervise::TaskAlert` is `Copy` and is
/// stored beside a task for its whole life, and a variant carrying a `String`
/// would have forced that (and every call site holding one) to allocate. The
/// parameters live in [`AlertText::fill`]'s argument list instead, which also
/// keeps the catalog below readable as a catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertText {
    // ── Supervisor alerts (a background task keeps dying) ───────────────────
    /// Title of the scheduler's supervisor alert.
    SchedulerTaskTitle,
    /// Body of the scheduler's supervisor alert.
    SchedulerTaskBody,
    /// Title of the trash sweep's supervisor alert.
    TrashSweepTaskTitle,
    /// Body of the trash sweep's supervisor alert.
    TrashSweepTaskBody,
    /// The telemetry drain / sender restarted (one body, two tasks — they are
    /// the same news to the operator: quality reporting hiccuped).
    QualityTaskRestarted,

    // ── Scheduler ───────────────────────────────────────────────────────────
    /// Title of the pre-service preflight notification.
    PreflightTitle,
    /// Preflight body: the ffmpeg sidecar is missing.
    ///
    /// The six `Preflight*` bodies below are the NATIVE NOTIFICATION's half of
    /// `sundayrec_core::preflight::PreflightCode` (F2-I18N-R2). The app's card
    /// renders the same codes from `legacy/locales/*.json`; a notification is
    /// fired from Rust, half an hour before a service, and cannot reach that
    /// catalogue — so the code has two catalogues because it has two surfaces.
    PreflightFfmpegMissing,
    /// Preflight body: the configured audio device is not connected.
    PreflightDeviceMissing,
    /// Preflight body: the save folder is not writable.
    PreflightFolderNotWritable,
    /// Preflight body: free space is below the threshold. `{gb}`
    PreflightDiskLow,
    /// Preflight body: the OS is blocking the microphone.
    PreflightMicDenied,
    /// Preflight body: the OS is blocking the camera.
    PreflightCameraDenied,
    /// Preflight body: the save folder is inside a OneDrive-synced tree (F2-W9).
    PreflightSaveFolderSynced,
    /// A scheduled recording started (governed by `notify_start`).
    ScheduledStarted,
    /// A scheduled recording was stopped by the schedule (governed by
    /// `notify_stop`).
    ScheduledStopped,
    /// A scheduled start was skipped because a recording was already running.
    ScheduledSkippedBusy,
    /// The engine refused a scheduled start. `{detail}`
    ScheduledStartFailed,
    /// A scheduled start did not answer within the 30 s bound.
    ScheduledStartTimeout,
    /// The recording options for a scheduled start could not be built.
    /// `{detail}`
    ScheduledPrepareFailed,
    /// The late-start net's recovery attempt failed too. `{detail}`
    ScheduledLateStartFailed,
    /// The pre-service reminder. `{min}`
    Reminder,
    /// Exactly one scheduled occurrence was never recorded. `{label}` `{at}`
    MissedOne,
    /// Several were. `{count}` `{label}` `{at}`
    MissedMany,
    /// What a missed weekly slot is called in the sentence. `{start}` `{stop}`
    MissedWeeklyLabel,

    // ── Recorder (terminal failures that reach the native notification) ────
    /// The reconnect policy gave up.
    RecordingNotRecovered,
    /// The ffmpeg capture produced no first progress in time (audio + video).
    RecordingStartTimeout,
    /// The native capture wrote no first block in time (audio only — no camera
    /// is involved, so the sentence must not send anyone to look for one).
    RecordingStartTimeoutMic,
    /// The disk guard stopped the take before the volume filled.
    RecordingDiskFull,
    /// The finished file was missing, empty or undecodable.
    RecordingEmptyOutput,
    // The raw-text codes (F2-VARSLING «Senere»): the engine emits these with
    // an ffmpeg line or a Rust error as the message, which is diagnostics, not
    // a sentence. Their wording is the renderer's own (`recording.error*` in
    // `legacy/locales`) — see [`AlertText::for_recording_code`].
    /// `device_not_found` — the input device is not there.
    RecordingDeviceNotFound,
    /// `device_permission_denied` — the OS refused the microphone.
    RecordingPermissionDenied,
    /// `device_busy` — another program holds the input device.
    RecordingDeviceBusy,
    /// `device_error` — the input device could not be opened or reopened.
    RecordingDeviceError,
    /// `device_disconnected` — the input device went away mid-take.
    RecordingDeviceDisconnected,
    /// `ffmpeg_exited` — the capture process died on its own.
    RecordingEngineExited,
    /// `video_capture_failed` — the camera delivered no picture.
    RecordingVideoCapture,
    /// `camera_format_unsupported`.
    RecordingCameraFormat,
    /// `camera_permission_denied`.
    RecordingCameraPermission,
    /// `camera_busy`.
    RecordingCameraBusy,
    /// `mux_failed` — audio and video could not be combined.
    RecordingMux,
    /// A code this catalog has no sentence for.
    RecordingFailedUnknown,

    // ── During a take (only while the window is not in focus) ───────────────
    /// The silence watcher tripped: nothing is reaching the recording.
    TakeSilence,
    /// The quality alarm: the take has far less audio than it should.
    TakeQuality,
    /// The input device dropped out and the engine is reconnecting.
    TakeReconnecting,
    /// The camera has been gone for over a minute; the audio is still being
    /// recorded (C5b).
    TakeCameraLost,
    /// The level has been far too low for over five minutes (D3): the mixer is
    /// probably turned down.
    TakeLevelWeak,
    /// Free space fell below the graduated warning threshold. `{gb}`
    TakeDiskLow,

    // ── Wake timers and the notification test ───────────────────────────────
    /// The background wake reschedule failed: the machine may sleep through
    /// the next recording.
    WakeNotArmed,
    /// «Send testvarsel» on the notify page.
    TestNotification,
}

impl AlertText {
    /// Every variant, in declaration order. The completeness tests iterate it;
    /// a variant added without a line here is caught by
    /// `every_variant_is_in_all` below.
    pub const ALL: &'static [AlertText] = &[
        AlertText::SchedulerTaskTitle,
        AlertText::SchedulerTaskBody,
        AlertText::TrashSweepTaskTitle,
        AlertText::TrashSweepTaskBody,
        AlertText::QualityTaskRestarted,
        AlertText::PreflightTitle,
        AlertText::PreflightFfmpegMissing,
        AlertText::PreflightDeviceMissing,
        AlertText::PreflightFolderNotWritable,
        AlertText::PreflightDiskLow,
        AlertText::PreflightMicDenied,
        AlertText::PreflightCameraDenied,
        AlertText::PreflightSaveFolderSynced,
        AlertText::ScheduledStarted,
        AlertText::ScheduledStopped,
        AlertText::ScheduledSkippedBusy,
        AlertText::ScheduledStartFailed,
        AlertText::ScheduledStartTimeout,
        AlertText::ScheduledPrepareFailed,
        AlertText::ScheduledLateStartFailed,
        AlertText::Reminder,
        AlertText::MissedOne,
        AlertText::MissedMany,
        AlertText::MissedWeeklyLabel,
        AlertText::RecordingNotRecovered,
        AlertText::RecordingStartTimeout,
        AlertText::RecordingStartTimeoutMic,
        AlertText::RecordingDiskFull,
        AlertText::RecordingEmptyOutput,
        AlertText::RecordingDeviceNotFound,
        AlertText::RecordingPermissionDenied,
        AlertText::RecordingDeviceBusy,
        AlertText::RecordingDeviceError,
        AlertText::RecordingDeviceDisconnected,
        AlertText::RecordingEngineExited,
        AlertText::RecordingVideoCapture,
        AlertText::RecordingCameraFormat,
        AlertText::RecordingCameraPermission,
        AlertText::RecordingCameraBusy,
        AlertText::RecordingMux,
        AlertText::RecordingFailedUnknown,
        AlertText::TakeSilence,
        AlertText::TakeQuality,
        AlertText::TakeReconnecting,
        AlertText::TakeCameraLost,
        AlertText::TakeLevelWeak,
        AlertText::TakeDiskLow,
        AlertText::WakeNotArmed,
        AlertText::TestNotification,
    ];

    /// The sentence a terminal recorder failure is told with, by its wire code.
    ///
    /// The engine emits `recording://error` with a code and a message, and for
    /// most codes the message is diagnostics — the last ffmpeg stderr line, a
    /// Rust `io::Error`, a camera classifier's English tag. The renderer never
    /// shows that text for a code it knows (`nativeErrorDetail`); the native
    /// notification showed it verbatim. This is the notification's half of the
    /// renderer's `NATIVE_ERRORS` table, worded identically (pinned by
    /// `recording_failures_say_what_the_window_says`), so the volunteer reads
    /// the same sentence on the desktop and in the app.
    ///
    /// An unknown code gets the generic sentence rather than the raw message:
    /// a new code without a line here is a vaguer notification, never an
    /// untranslated one.
    pub fn for_recording_code(code: &str) -> AlertText {
        match code {
            "device_not_found" | "no_device" => AlertText::RecordingDeviceNotFound,
            "device_permission_denied" => AlertText::RecordingPermissionDenied,
            "device_busy" => AlertText::RecordingDeviceBusy,
            "device_error" => AlertText::RecordingDeviceError,
            "device_disconnected" => AlertText::RecordingDeviceDisconnected,
            "disk_full" => AlertText::RecordingDiskFull,
            "start_timeout" => AlertText::RecordingStartTimeout,
            "empty_output" => AlertText::RecordingEmptyOutput,
            "ffmpeg_exited" => AlertText::RecordingEngineExited,
            "video_capture_failed" => AlertText::RecordingVideoCapture,
            "camera_format_unsupported" => AlertText::RecordingCameraFormat,
            "camera_permission_denied" => AlertText::RecordingCameraPermission,
            "camera_busy" => AlertText::RecordingCameraBusy,
            "mux_failed" => AlertText::RecordingMux,
            _ => AlertText::RecordingFailedUnknown,
        }
    }

    /// The placeholder names this variant's templates carry, without braces.
    /// Empty for the sentences that take no parameter.
    ///
    /// Stated here rather than derived from the Norwegian template so the tests
    /// can compare the two: a translator who dropped `{min}` is then a failing
    /// test rather than a notification reading "Recording starts in  minutes".
    pub fn params(self) -> &'static [&'static str] {
        match self {
            AlertText::ScheduledStartFailed
            | AlertText::ScheduledPrepareFailed
            | AlertText::ScheduledLateStartFailed => &["detail"],
            AlertText::Reminder => &["min"],
            AlertText::PreflightDiskLow | AlertText::TakeDiskLow => &["gb"],
            AlertText::MissedOne => &["label", "at"],
            AlertText::MissedMany => &["count", "label", "at"],
            AlertText::MissedWeeklyLabel => &["start", "stop"],
            _ => &[],
        }
    }

    /// The raw template for `lang`, placeholders unfilled.
    ///
    /// Public because the tests and the `--list` side of a future tooling pass
    /// want to see the template itself; call sites want [`Self::text`] or
    /// [`Self::fill`].
    pub fn template(self, lang: Lang) -> &'static str {
        use AlertText as A;
        use Lang as L;
        match (self, lang) {
            // ── SchedulerTaskTitle ──────────────────────────────────────────
            (A::SchedulerTaskTitle, L::No) => "SundayRec — planlegger-feil",
            (A::SchedulerTaskTitle, L::En) => "SundayRec — scheduler fault",

            // ── SchedulerTaskBody ───────────────────────────────────────────
            (A::SchedulerTaskBody, L::No) => {
                "Planleggeren har en vedvarende feil og kan gå glipp av planlagte opptak. Start appen på nytt; vedvarer det, bruk «Sjekk maskinen» under Innstillinger → Hjelp og feilsøking."
            }
            (A::SchedulerTaskBody, L::En) => {
                "The scheduler has a persistent fault and may miss scheduled recordings. Restart the app; if it persists, use “Check this computer” under Settings → Help and troubleshooting."
            }

            // ── TrashSweepTaskTitle ─────────────────────────────────────────
            (A::TrashSweepTaskTitle, L::No) => "SundayRec — opprydding stoppet",
            (A::TrashSweepTaskTitle, L::En) => "SundayRec — cleanup stopped",

            // ── TrashSweepTaskBody ──────────────────────────────────────────
            (A::TrashSweepTaskBody, L::No) => {
                "Den automatiske tømmingen av papirkurven har en vedvarende feil, så slettede opptak blir liggende og bruke plass. Start appen på nytt; vedvarer det, bruk «Sjekk maskinen» under Innstillinger → Hjelp og feilsøking."
            }
            (A::TrashSweepTaskBody, L::En) => {
                "Automatic emptying of the trash has a persistent fault, so deleted recordings stay on disk and take up space. Restart the app; if it persists, use “Check this computer” under Settings → Help and troubleshooting."
            }

            // ── QualityTaskRestarted ────────────────────────────────────────
            (A::QualityTaskRestarted, L::No) => {
                "Bakgrunnsoppgaven for kvalitetsrapporter startet på nytt."
            }
            (A::QualityTaskRestarted, L::En) => {
                "The background task for quality reports restarted."
            }

            // ── PreflightTitle ──────────────────────────────────────────────
            (A::PreflightTitle, L::No) => "SundayRec — sjekk før opptak",
            (A::PreflightTitle, L::En) => "SundayRec — check before recording",

            // ── PreflightFfmpegMissing ──────────────────────────────────────
            (A::PreflightFfmpegMissing, L::No) => {
                "ffmpeg-binær mangler. SundayRec må installeres på nytt."
            }
            (A::PreflightFfmpegMissing, L::En) => {
                "The ffmpeg binary is missing. SundayRec must be installed again."
            }

            // ── PreflightDeviceMissing ──────────────────────────────────────
            (A::PreflightDeviceMissing, L::No) => {
                "Lydkilden som er valgt i innstillingene er ikke tilkoblet."
            }
            (A::PreflightDeviceMissing, L::En) => {
                "The sound source selected in settings is not connected."
            }

            // ── PreflightFolderNotWritable ──────────────────────────────────
            (A::PreflightFolderNotWritable, L::No) => "Lagringsmappen kan ikke skrives.",
            (A::PreflightFolderNotWritable, L::En) => "The save folder cannot be written to.",

            // ── PreflightDiskLow ────────────────────────────────────────────
            (A::PreflightDiskLow, L::No) => {
                "Bare {gb} GB ledig på lagringsdisken — kanskje ikke nok for et helt opptak."
            }
            (A::PreflightDiskLow, L::En) => {
                "Only {gb} GB free on the save disk — perhaps not enough for a whole recording."
            }

            // ── PreflightMicDenied ──────────────────────────────────────────
            (A::PreflightMicDenied, L::No) => {
                "Mikrofontilgang er ikke gitt. Åpne Systeminnstillinger → Personvern → Mikrofon."
            }
            (A::PreflightMicDenied, L::En) => {
                "Microphone access has not been granted. Open System Settings → Privacy → Microphone."
            }

            // ── PreflightCameraDenied ───────────────────────────────────────
            (A::PreflightCameraDenied, L::No) => "Kameratilgang er ikke gitt.",
            (A::PreflightCameraDenied, L::En) => "Camera access has not been granted.",

            // ── PreflightSaveFolderSynced ──────────────────────────────────────────────
            (A::PreflightSaveFolderSynced, L::No) => {
                "Lagringsmappen synkroniseres av OneDrive, som kan forstyrre et opptak som pågår."
            }
            (A::PreflightSaveFolderSynced, L::En) => {
                "The save folder is synced by OneDrive, which can interfere with a recording in progress."
            }

            // ── ScheduledStarted ────────────────────────────────────────────
            (A::ScheduledStarted, L::No) => "Planlagt opptak startet.",
            (A::ScheduledStarted, L::En) => "Scheduled recording started.",

            // ── ScheduledStopped ────────────────────────────────────────────
            (A::ScheduledStopped, L::No) => "Planlagt opptak avsluttet.",
            (A::ScheduledStopped, L::En) => "Scheduled recording finished.",

            // ── ScheduledSkippedBusy ────────────────────────────────────────
            (A::ScheduledSkippedBusy, L::No) => {
                "Planlagt opptak hoppet over — et opptak pågår allerede."
            }
            (A::ScheduledSkippedBusy, L::En) => {
                "Scheduled recording skipped — a recording is already running."
            }

            // ── ScheduledStartFailed ────────────────────────────────────────
            (A::ScheduledStartFailed, L::No) => "Planlagt opptak startet ikke: {detail}",
            (A::ScheduledStartFailed, L::En) => "The scheduled recording did not start: {detail}",

            // ── ScheduledStartTimeout ───────────────────────────────────────
            (A::ScheduledStartTimeout, L::No) => {
                "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon."
            }
            (A::ScheduledStartTimeout, L::En) => {
                "The scheduled recording did not start (timed out) — check the camera/microphone."
            }

            // ── ScheduledPrepareFailed ──────────────────────────────────────
            (A::ScheduledPrepareFailed, L::No) => "Planlagt opptak kunne ikke forberedes: {detail}",
            (A::ScheduledPrepareFailed, L::En) => {
                "The scheduled recording could not be prepared: {detail}"
            }

            // ── ScheduledLateStartFailed ────────────────────────────────────
            (A::ScheduledLateStartFailed, L::No) => {
                "Forsinket oppstart av planlagt opptak feilet: {detail}"
            }
            (A::ScheduledLateStartFailed, L::En) => {
                "The late start of the scheduled recording failed: {detail}"
            }

            // ── Reminder ────────────────────────────────────────────────────
            // Byte-for-byte the Electron `REMINDER_LABELS` map these were ported
            // from (via `scheduler::reminder_body`, which this replaced): a
            // volunteer who has read the same reminder every Sunday for a year
            // gains nothing from a fresh translation of it.
            (A::Reminder, L::No) => "Opptak starter om {min} minutter",
            (A::Reminder, L::En) => "Recording starts in {min} minutes",

            // ── MissedOne ───────────────────────────────────────────────────
            (A::MissedOne, L::No) => "Planlagt opptak ble ikke gjort: {label} ({at}).",
            (A::MissedOne, L::En) => "A scheduled recording was not made: {label} ({at}).",

            // ── MissedMany ──────────────────────────────────────────────────
            (A::MissedMany, L::No) => {
                "{count} planlagte opptak ble ikke gjort. Det eldste: {label} ({at})."
            }
            (A::MissedMany, L::En) => {
                "{count} scheduled recordings were not made. The oldest: {label} ({at})."
            }

            // ── MissedWeeklyLabel ───────────────────────────────────────────
            (A::MissedWeeklyLabel, L::No) => "Ukentlig opptak ({start}–{stop})",
            (A::MissedWeeklyLabel, L::En) => "Weekly recording ({start}–{stop})",
            // ── RecordingNotRecovered ───────────────────────────────────────
            (A::RecordingNotRecovered, L::No) => "Opptaket kunne ikke gjenopprettes",
            (A::RecordingNotRecovered, L::En) => "The recording could not be recovered",

            // ── RecordingStartTimeout (camera + microphone) ─────────────────
            (A::RecordingStartTimeout, L::No) => {
                "Opptaket startet ikke i tide — sjekk at kamera/mikrofon er tilkoblet og at \
                 appen har tilgang (Systeminnstillinger → Personvern)."
            }
            (A::RecordingStartTimeout, L::En) => {
                "The recording did not start in time — check that the camera/microphone are \
                 connected and that the app has access (System Settings → Privacy)."
            }

            // ── RecordingStartTimeoutMic (audio only) ───────────────────────
            (A::RecordingStartTimeoutMic, L::No) => {
                "Opptaket startet ikke i tide — sjekk at mikrofonen er tilkoblet og at appen har \
                 tilgang (Systeminnstillinger → Personvern)."
            }
            (A::RecordingStartTimeoutMic, L::En) => {
                "The recording did not start in time — check that the microphone is connected \
                 and that the app has access (System Settings → Privacy)."
            }

            // ── RecordingDiskFull ───────────────────────────────────────────
            (A::RecordingDiskFull, L::No) => {
                "Lite ledig diskplass — stopper opptaket trygt før disken blir full."
            }
            (A::RecordingDiskFull, L::En) => {
                "Little free disk space — stopping the recording safely before the disk fills up."
            }

            // ── RecordingEmptyOutput ────────────────────────────────────────
            (A::RecordingEmptyOutput, L::No) => {
                "Opptaket ble tomt eller skadet — ingen fil ble lagret."
            }
            (A::RecordingEmptyOutput, L::En) => {
                "The recording came out empty or damaged — no file was saved."
            }

            // ── RecordingDeviceNotFound ──────────────────────────────────────
            (A::RecordingDeviceNotFound, L::No) => {
                "Lydkilden ble ikke funnet — sjekk USB-tilkoblingen"
            }
            (A::RecordingDeviceNotFound, L::En) => {
                "Sound source not found — check the USB connection"
            }
            // ── RecordingPermissionDenied ────────────────────────────────────
            (A::RecordingPermissionDenied, L::No) => {
                "Mikrofontilgang nektet — åpne Systeminnstillinger og gi tilgang"
            }
            (A::RecordingPermissionDenied, L::En) => {
                "Microphone access denied — open System Settings and grant access"
            }
            // ── RecordingDeviceBusy ──────────────────────────────────────────
            (A::RecordingDeviceBusy, L::No) => "Lydkilden er i bruk av et annet program",
            (A::RecordingDeviceBusy, L::En) => "The sound source is in use by another application",
            // ── RecordingDeviceError ─────────────────────────────────────────
            (A::RecordingDeviceError, L::No) => {
                "Feil ved åpning av lydkilden — prøv å koble til på nytt"
            }
            (A::RecordingDeviceError, L::En) => {
                "Error opening the sound source — try reconnecting it"
            }
            // ── RecordingDeviceDisconnected ──────────────────────────────────
            (A::RecordingDeviceDisconnected, L::No) => {
                "Lydkilden ble koblet fra under opptak — sjekk tilkoblingen"
            }
            (A::RecordingDeviceDisconnected, L::En) => {
                "The sound source was disconnected during recording — check the connection"
            }
            // ── RecordingEngineExited ────────────────────────────────────────
            (A::RecordingEngineExited, L::No) => {
                "Opptaksmotoren stoppet uventet — lyden fram til da er lagret. Start et nytt opptak, og hent feilloggen (Innstillinger → Hjelp og feilsøking) hvis det gjentar seg."
            }
            (A::RecordingEngineExited, L::En) => {
                "The recording engine stopped unexpectedly — the audio up to that point is saved. Start a new recording, and get the error log (Settings → Help and troubleshooting) if it happens again."
            }
            // ── RecordingVideoCapture ────────────────────────────────────────
            (A::RecordingVideoCapture, L::No) => {
                "Kameraet leverte ikke bilde — lyden ble lagret og ligger i biblioteket. Sjekk kameratilkoblingen før du tar opp med video igjen."
            }
            (A::RecordingVideoCapture, L::En) => {
                "The camera delivered no picture — the audio was saved and is in the library. Check the camera connection before recording with video again."
            }
            // ── RecordingCameraFormat ────────────────────────────────────────
            (A::RecordingCameraFormat, L::No) => {
                "Kameraet støtter ikke valgt bilderate eller oppløsning — velg en annen videoinnstilling og prøv igjen."
            }
            (A::RecordingCameraFormat, L::En) => {
                "The camera does not support the chosen frame rate or resolution — pick another video setting and try again."
            }
            // ── RecordingCameraPermission ────────────────────────────────────
            (A::RecordingCameraPermission, L::No) => {
                "Kameratilgang nektet — åpne Systeminnstillinger og gi appen tilgang til kameraet"
            }
            (A::RecordingCameraPermission, L::En) => {
                "Camera access denied — open System Settings and give the app access to the camera"
            }
            // ── RecordingCameraBusy ──────────────────────────────────────────
            (A::RecordingCameraBusy, L::No) => {
                "Kameraet er i bruk av et annet program — lukk det (Teams, Zoom) og prøv igjen"
            }
            (A::RecordingCameraBusy, L::En) => {
                "The camera is in use by another program — close it (Teams, Zoom) and try again"
            }
            // ── RecordingMux ─────────────────────────────────────────────────
            (A::RecordingMux, L::No) => {
                "Lyd og bilde kunne ikke settes sammen — begge råfilene er beholdt i lagringsmappen, så ingenting er tapt."
            }
            (A::RecordingMux, L::En) => {
                "Audio and video could not be combined — both raw files were kept in the save folder, so nothing is lost."
            }
            // ── RecordingFailedUnknown ───────────────────────────────────────
            (A::RecordingFailedUnknown, L::No) => {
                "Noe gikk galt under opptak — sjekk at lydkilde og lagringsmappe er klare"
            }
            (A::RecordingFailedUnknown, L::En) => {
                "Something went wrong during recording — check that the sound source and save folder are ready"
            }
            // ── TakeSilence ───────────────────────────────────────────────────
            (A::TakeSilence, L::No) => {
                "Opptaket er stille — sjekk at lyden kommer fram til SundayRec."
            }
            (A::TakeSilence, L::En) => {
                "The recording is silent — check that sound is reaching SundayRec."
            }

            // ── TakeQuality ───────────────────────────────────────────────────
            (A::TakeQuality, L::No) => "Opptaket mangler lyd — åpne SundayRec og se hva som skjer.",
            (A::TakeQuality, L::En) => {
                "The recording is missing sound — open SundayRec and check what is happening."
            }

            // ── TakeReconnecting ──────────────────────────────────────────────
            (A::TakeReconnecting, L::No) => {
                "Lydkilden forsvant — SundayRec prøver å koble til igjen."
            }
            (A::TakeReconnecting, L::En) => {
                "The audio source disappeared — SundayRec is trying to reconnect."
            }

            // ── TakeCameraLost ────────────────────────────────────────────────
            (A::TakeCameraLost, L::No) => "Kameraet falt ut — lyden tas fortsatt opp",
            (A::TakeCameraLost, L::En) => {
                "The camera dropped out — the audio is still being recorded"
            }

            // ── TakeLevelWeak ─────────────────────────────────────────────────
            (A::TakeLevelWeak, L::No) => "Opptaket er svært svakt — er mikseren dempet?",
            (A::TakeLevelWeak, L::En) => "The recording is very quiet — is the mixer turned down?",

            // ── TakeDiskLow ───────────────────────────────────────────────────
            (A::TakeDiskLow, L::No) => {
                "Lite plass på disken — {gb} GB igjen. Opptaket stopper når disken er full."
            }
            (A::TakeDiskLow, L::En) => {
                "Low disk space — {gb} GB left. The recording stops when the disk is full."
            }

            // ── WakeNotArmed ──────────────────────────────────────────────────
            (A::WakeNotArmed, L::No) => {
                "SundayRec får ikke satt opp vekking før neste opptak. La maskinen stå på."
            }
            (A::WakeNotArmed, L::En) => {
                "SundayRec can't set up a wake-up before the next recording. Leave the computer on."
            }

            // ── TestNotification ──────────────────────────────────────────────
            (A::TestNotification, L::No) => {
                "Dette er et testvarsel. Slik sier SundayRec fra hvis noe går galt."
            }
            (A::TestNotification, L::En) => {
                "This is a test notification. This is how SundayRec tells you if something goes wrong."
            }
        }
    }

    /// The sentence for `lang`, for a variant that takes no parameter.
    ///
    /// A parameterised variant reaches this only by mistake, so it trips a
    /// `debug_assert` in tests and dev builds and still returns something
    /// truthful (the template) in release — an alert with a visible `{detail}`
    /// is bad, an alert that panicked the recorder mid-service is worse.
    pub fn text(self, lang: Lang) -> String {
        debug_assert!(
            self.params().is_empty(),
            "{self:?} takes {:?} — use AlertText::fill",
            self.params()
        );
        self.fill(lang, &[])
    }

    /// The sentence for `lang` with its `{placeholder}`s replaced.
    ///
    /// Unknown keys in `vars` are ignored and unfilled placeholders are left
    /// standing — the tests below are
    /// what make "left standing" impossible in practice, by checking each
    /// variant against its own [`Self::params`].
    pub fn fill(self, lang: Lang, vars: &[(&str, &str)]) -> String {
        let mut out = self.template(lang).to_string();
        for (k, v) in vars {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
    }
}

/// What a missed occurrence is called in the notification, in `lang`.
///
/// The CANONICAL label ([`crate::schedule::MissedRecording::label`]) stays
/// Norwegian because it is a key; this is the words.
pub fn missed_label(kind: &crate::schedule::MissedKind, lang: Lang) -> String {
    use crate::schedule::MissedKind;
    match kind {
        MissedKind::Weekly { start, stop } => {
            AlertText::MissedWeeklyLabel.fill(lang, &[("start", start), ("stop", stop)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `{placeholder}` in `s`, without braces.
    fn placeholders(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            match after.find('}') {
                Some(close) => {
                    out.push(after[..close].to_string());
                    rest = &after[close + 1..];
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn every_variant_is_in_all() {
        // `ALL` is hand-written, so it can fall behind the enum. The catalog
        // itself cannot (the match is exhaustive), but the tests below iterate
        // `ALL` — a variant missing from it would be a silently untested
        // sentence. Counting is the cheapest honest check: bump the literal
        // when you add a variant, and read the two lists beside each other.
        assert_eq!(
            AlertText::ALL.len(),
            49,
            "AlertText::ALL is out of step with the enum"
        );
        let mut seen = std::collections::HashSet::new();
        for a in AlertText::ALL {
            assert!(seen.insert(*a), "{a:?} appears twice in ALL");
        }
    }

    #[test]
    fn both_languages_have_a_sentence_for_every_alert() {
        for &a in AlertText::ALL {
            for &lang in Lang::ALL {
                let t = a.template(lang);
                assert!(!t.trim().is_empty(), "{a:?}/{lang:?} is empty");
                // No stray whitespace from a line continuation gone wrong.
                assert_eq!(t, t.trim(), "{a:?}/{lang:?} has edge whitespace");
                assert!(!t.contains("  "), "{a:?}/{lang:?} has a double space");
            }
        }
    }

    #[test]
    fn every_language_says_it_differently() {
        // THE test that catches the actual failure mode: a hurried translation
        // pass that copies the Norwegian into the other arm. Two
        // pairwise-distinct strings per sentence is a property this catalog can
        // hold — no two of these languages spell any of these sentences the
        // same way — so the check is exact rather than "at least a few differ".
        for &a in AlertText::ALL {
            for (i, &l1) in Lang::ALL.iter().enumerate() {
                for &l2 in &Lang::ALL[i + 1..] {
                    assert_ne!(
                        a.template(l1),
                        a.template(l2),
                        "{a:?}: {l1:?} and {l2:?} are the same sentence"
                    );
                }
            }
        }
    }

    #[test]
    fn a_translation_never_drops_a_placeholder() {
        // The failure this prevents: «Recording starts in  minutes». The
        // Norwegian is the source, `params()` is the declared contract, and both
        // must carry exactly the declared set — no more (a translator
        // inventing `{church}`), no fewer.
        for &a in AlertText::ALL {
            let declared: std::collections::BTreeSet<String> =
                a.params().iter().map(|p| p.to_string()).collect();
            for &lang in Lang::ALL {
                let found: std::collections::BTreeSet<String> =
                    placeholders(a.template(lang)).into_iter().collect();
                assert_eq!(
                    found, declared,
                    "{a:?}/{lang:?} placeholders {found:?} ≠ declared {declared:?}"
                );
            }
        }
    }

    #[test]
    fn filling_a_variant_leaves_no_braces_behind() {
        // Every variant, every language, filled with its own declared params:
        // nothing that looks like a placeholder may survive. This is the check
        // that would have caught a `{min}`/`{minutes}` rename on one side only.
        for &a in AlertText::ALL {
            let vars: Vec<(&str, &str)> = a.params().iter().map(|p| (*p, "X")).collect();
            for &lang in Lang::ALL {
                let s = a.fill(lang, &vars);
                assert!(
                    !s.contains('{') && !s.contains('}'),
                    "{a:?}/{lang:?} still has a placeholder: {s}"
                );
                assert!(!s.is_empty());
            }
        }
    }

    #[test]
    fn the_reminder_is_byte_identical_to_the_electron_port() {
        // These shipped in Electron and then in `scheduler::reminder_body`
        // (which this replaced). Re-translating them would change what a
        // volunteer has read every Sunday for a year, for no gain — so the
        // wording is pinned here rather than left to the next tidy-up.
        assert_eq!(
            AlertText::Reminder.fill(Lang::No, &[("min", "10")]),
            "Opptak starter om 10 minutter"
        );
        assert_eq!(
            AlertText::Reminder.fill(Lang::En, &[("min", "15")]),
            "Recording starts in 15 minutes"
        );
    }

    #[test]
    fn an_unknown_language_code_still_gets_a_sentence() {
        // The whole point of the fallback: a settings blob carrying `"xx"` (or
        // nothing at all) must still produce a real alert, in Norwegian.
        let lang = Lang::from_code(Some("xx"));
        assert_eq!(
            AlertText::ScheduledSkippedBusy.text(lang),
            AlertText::ScheduledSkippedBusy.text(Lang::No)
        );
        assert_eq!(
            AlertText::ScheduledSkippedBusy.text(Lang::from_code(None)),
            AlertText::ScheduledSkippedBusy.text(Lang::No)
        );
    }

    #[test]
    fn recording_failures_say_what_the_window_says() {
        // The seam this catalog shares with the renderer: the same failure, the
        // same words, on the desktop notification and in the error banner. Two
        // hand-kept copies drift; this reads the renderer's catalogue and makes
        // a drift on either side a failing test.
        let catalogues = [
            (Lang::No, include_str!("../../../legacy/locales/no.json")),
            (Lang::En, include_str!("../../../legacy/locales/en.json")),
        ];
        let pairs = [
            (AlertText::RecordingDeviceNotFound, "errorDeviceNotFound"),
            (AlertText::RecordingPermissionDenied, "errorPermission"),
            (AlertText::RecordingDeviceBusy, "errorNotReadable"),
            (AlertText::RecordingDeviceError, "errorDeviceError"),
            (
                AlertText::RecordingDeviceDisconnected,
                "errorDeviceDisconnected",
            ),
            (AlertText::RecordingEngineExited, "errorEngineExited"),
            (AlertText::RecordingVideoCapture, "errorVideoCapture"),
            (AlertText::RecordingCameraFormat, "errorCameraFormat"),
            (
                AlertText::RecordingCameraPermission,
                "errorCameraPermission",
            ),
            (AlertText::RecordingCameraBusy, "errorCameraBusy"),
            (AlertText::RecordingMux, "errorMux"),
            (AlertText::RecordingFailedUnknown, "errorUnknown"),
        ];
        for (lang, raw) in catalogues {
            let json: serde_json::Value = serde_json::from_str(raw).expect("locale parses");
            for (alert, key) in pairs {
                let want = json["recording"][key]
                    .as_str()
                    .unwrap_or_else(|| panic!("{lang:?}: recording.{key} is missing"));
                assert_eq!(
                    alert.template(lang),
                    want,
                    "{alert:?}/{lang:?} ≠ recording.{key}"
                );
            }
        }
    }

    #[test]
    fn every_raw_text_code_the_engine_emits_has_its_own_sentence() {
        // The codes that reach `recording://error` with diagnostics as their
        // message (the supervisor, the native capture's stream and writer). Falling through to the generic sentence would be
        // honest but vague; each of these has a specific one.
        for code in [
            "device_not_found",
            "device_permission_denied",
            "device_busy",
            "device_error",
            "device_disconnected",
            "disk_full",
            "ffmpeg_exited",
            "video_capture_failed",
            "camera_format_unsupported",
            "camera_permission_denied",
            "camera_busy",
            "mux_failed",
        ] {
            assert_ne!(
                AlertText::for_recording_code(code),
                AlertText::RecordingFailedUnknown,
                "{code} has no sentence of its own"
            );
        }
        assert_eq!(
            AlertText::for_recording_code("something_new"),
            AlertText::RecordingFailedUnknown
        );
    }

    #[test]
    fn a_missed_label_reads_in_norwegian_exactly_as_the_key_does() {
        // Norwegian notifications are byte-identical to before: the words for a
        // Norwegian volunteer ARE the canonical label. Another language gets
        // its own words, and the key underneath does not move.
        use crate::schedule::{ScheduleSlot, missed_recordings};
        use chrono::NaiveDateTime;
        let slot = ScheduleSlot {
            days: vec![6],
            start: "11:00".into(),
            stop: "13:00".into(),
            max: None,
        };
        let now = NaiveDateTime::parse_from_str("2026-06-07 15:00", "%Y-%m-%d %H:%M").unwrap();
        let missed = missed_recordings(
            std::slice::from_ref(&slot),
            now,
            &[],
            &[],
            &Default::default(),
        );
        assert_eq!(missed.len(), 1, "precondition: one missed Sunday");
        assert_eq!(missed_label(&missed[0].kind, Lang::No), missed[0].label);
        assert_eq!(
            missed_label(&missed[0].kind, Lang::En),
            "Weekly recording (11:00–13:00)"
        );
    }

    #[test]
    fn the_english_volunteers_missed_sunday_is_english() {
        // The scenario finding A8 is about, end to end: a church whose app is
        // set to English, whose machine slept through the 11:00 service. Before
        // this module the sentence below was Norwegian — the ONE line telling
        // them the service was not recorded.
        let s = AlertText::MissedOne.fill(
            Lang::from_code(Some("en")),
            &[
                ("label", "Ukentlig opptak (11:00–13:00)"),
                ("at", "2026-09-06T11:00:00"),
            ],
        );
        assert!(s.starts_with("A scheduled recording was not made:"), "{s}");
        assert!(s.contains("2026-09-06T11:00:00"));
        // …and the slot LABEL is deliberately not translated — see the module
        // header: it is hashed into the durable `notify_seen` key.
        assert!(s.contains("Ukentlig opptak (11:00–13:00)"));
    }

    #[test]
    fn fill_ignores_a_key_the_template_does_not_have() {
        let s = AlertText::ScheduledStarted.fill(Lang::En, &[("nope", "x")]);
        assert_eq!(s, "Scheduled recording started.");
    }

    #[test]
    fn placeholders_reads_what_it_should() {
        // The test helper is itself a small parser; a broken one would make
        // `a_translation_never_drops_a_placeholder` green by finding nothing.
        assert_eq!(placeholders("a {x} b {yy} c"), vec!["x", "yy"]);
        assert_eq!(placeholders("none here"), Vec::<String>::new());
        assert_eq!(placeholders("{unclosed"), Vec::<String>::new());
    }
}
