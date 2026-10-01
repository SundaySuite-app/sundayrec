//! The sentences the app says to a person who is not looking at the screen —
//! in the seven languages SundayRec ships (F1 finding A8).
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
//! A Polish volunteer who set the app to Polish, whose church runs an
//! unattended 11:00 service, got a Polish interface — and this sentence, in
//! Norwegian, as the one line that told them what had actually happened.
//!
//! ## The shape
//!
//! One [`AlertText`] variant per SENTENCE, and a `(variant, language)` match
//! that the compiler checks for exhaustiveness. That check is the whole point:
//! a new alert cannot be added in one language, because the code will not build
//! until all seven arms exist. No runtime "did somebody forget?" test can make
//! that promise, and the tests below therefore spend themselves on what the
//! compiler cannot see — that the seven strings are actually seven *different*
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
//!   a seven-language log is a seven-language grep.
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
//! - **Holiday names** ([`crate::church_calendar`]) and the SR-code reserve in
//!   [`crate::diagnostics`]. Data and a documented backlog item respectively;
//!   both are allowlisted in the `check-rust-norwegian.mjs` ratchet with the
//!   reason spelled out there.

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
    /// A special recording's own audio device was not available at its start,
    /// so it is recording on the usual (global) device instead. `{device}`
    ScheduledSpecialDeviceFallback,
    /// The pre-service reminder. `{min}`
    Reminder,
    /// Exactly one scheduled occurrence was never recorded. `{label}` `{at}`
    MissedOne,
    /// Several were. `{count}` `{label}` `{at}`
    MissedMany,
    /// What a missed weekly slot is called in the sentence. `{start}` `{stop}`
    MissedWeeklyLabel,
    /// What a missed special recording without a name is called.
    MissedSpecialLabel,

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
        AlertText::ScheduledSpecialDeviceFallback,
        AlertText::Reminder,
        AlertText::MissedOne,
        AlertText::MissedMany,
        AlertText::MissedWeeklyLabel,
        AlertText::MissedSpecialLabel,
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
            AlertText::ScheduledSpecialDeviceFallback => &["device"],
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
            (A::SchedulerTaskTitle, L::De) => "SundayRec — Planerfehler",
            (A::SchedulerTaskTitle, L::Sv) => "SundayRec — schemaläggarfel",
            (A::SchedulerTaskTitle, L::Da) => "SundayRec — planlæggerfejl",
            (A::SchedulerTaskTitle, L::Pl) => "SundayRec — błąd harmonogramu",
            (A::SchedulerTaskTitle, L::Fr) => "SundayRec — erreur du planificateur",

            // ── SchedulerTaskBody ───────────────────────────────────────────
            (A::SchedulerTaskBody, L::No) => {
                "Planleggeren har en vedvarende feil og kan gå glipp av planlagte opptak. \
                 Start appen på nytt; vedvarer det, kjør Diagnose under Innstillinger → Lyd."
            }
            (A::SchedulerTaskBody, L::En) => {
                "The scheduler has a persistent fault and may miss scheduled recordings. \
                 Restart the app; if it persists, run Diagnose under Settings → Audio."
            }
            (A::SchedulerTaskBody, L::De) => {
                "Der Planer hat einen anhaltenden Fehler und verpasst möglicherweise geplante \
                 Aufnahmen. Starten Sie die App neu; hält es an, führen Sie die Diagnose unter \
                 Einstellungen → Audio aus."
            }
            (A::SchedulerTaskBody, L::Sv) => {
                "Schemaläggaren har ett ihållande fel och kan missa schemalagda inspelningar. \
                 Starta om appen; kvarstår det, kör Diagnos under Inställningar → Ljud."
            }
            (A::SchedulerTaskBody, L::Da) => {
                "Planlæggeren har en vedvarende fejl og kan gå glip af planlagte optagelser. \
                 Genstart appen; fortsætter det, kør Diagnose under Indstillinger → Lyd."
            }
            (A::SchedulerTaskBody, L::Pl) => {
                "Harmonogram ma trwały błąd i może pominąć zaplanowane nagrania. Uruchom \
                 aplikację ponownie; jeśli to nie pomoże, uruchom Diagnostykę w Ustawienia → \
                 Dźwięk."
            }
            (A::SchedulerTaskBody, L::Fr) => {
                "Le planificateur a une erreur persistante et risque de manquer des \
                 enregistrements programmés. Redémarrez l'application ; si cela persiste, \
                 lancez Diagnostic dans Réglages → Audio."
            }

            // ── TrashSweepTaskTitle ─────────────────────────────────────────
            (A::TrashSweepTaskTitle, L::No) => "SundayRec — opprydding stoppet",
            (A::TrashSweepTaskTitle, L::En) => "SundayRec — cleanup stopped",
            (A::TrashSweepTaskTitle, L::De) => "SundayRec — Aufräumen gestoppt",
            (A::TrashSweepTaskTitle, L::Sv) => "SundayRec — rensningen stoppade",
            (A::TrashSweepTaskTitle, L::Da) => "SundayRec — oprydningen er stoppet",
            (A::TrashSweepTaskTitle, L::Pl) => "SundayRec — czyszczenie zatrzymane",
            (A::TrashSweepTaskTitle, L::Fr) => "SundayRec — nettoyage arrêté",

            // ── TrashSweepTaskBody ──────────────────────────────────────────
            (A::TrashSweepTaskBody, L::No) => {
                "Den automatiske tømmingen av papirkurven har en vedvarende feil, så slettede \
                 opptak blir liggende og bruke plass. Start appen på nytt; vedvarer det, kjør \
                 Diagnose under Innstillinger → Lyd."
            }
            (A::TrashSweepTaskBody, L::En) => {
                "Automatic emptying of the trash has a persistent fault, so deleted recordings \
                 stay on disk and take up space. Restart the app; if it persists, run Diagnose \
                 under Settings → Audio."
            }
            (A::TrashSweepTaskBody, L::De) => {
                "Das automatische Leeren des Papierkorbs hat einen anhaltenden Fehler, sodass \
                 gelöschte Aufnahmen liegen bleiben und Platz belegen. Starten Sie die App neu; \
                 hält es an, führen Sie die Diagnose unter Einstellungen → Audio aus."
            }
            (A::TrashSweepTaskBody, L::Sv) => {
                "Den automatiska tömningen av papperskorgen har ett ihållande fel, så raderade \
                 inspelningar blir kvar och tar plats. Starta om appen; kvarstår det, kör \
                 Diagnos under Inställningar → Ljud."
            }
            (A::TrashSweepTaskBody, L::Da) => {
                "Den automatiske tømning af papirkurven har en vedvarende fejl, så slettede \
                 optagelser bliver liggende og optager plads. Genstart appen; fortsætter det, \
                 kør Diagnose under Indstillinger → Lyd."
            }
            (A::TrashSweepTaskBody, L::Pl) => {
                "Automatyczne opróżnianie kosza ma trwały błąd, więc usunięte nagrania \
                 pozostają na dysku i zajmują miejsce. Uruchom aplikację ponownie; jeśli to nie \
                 pomoże, uruchom Diagnostykę w Ustawienia → Dźwięk."
            }
            (A::TrashSweepTaskBody, L::Fr) => {
                "Le vidage automatique de la corbeille a une erreur persistante : les \
                 enregistrements supprimés restent sur le disque et occupent de l'espace. \
                 Redémarrez l'application ; si cela persiste, lancez Diagnostic dans Réglages → \
                 Audio."
            }

            // ── QualityTaskRestarted ────────────────────────────────────────
            (A::QualityTaskRestarted, L::No) => {
                "Bakgrunnsoppgaven for kvalitetsrapporter startet på nytt."
            }
            (A::QualityTaskRestarted, L::En) => {
                "The background task for quality reports restarted."
            }
            (A::QualityTaskRestarted, L::De) => {
                "Die Hintergrundaufgabe für Qualitätsberichte wurde neu gestartet."
            }
            (A::QualityTaskRestarted, L::Sv) => {
                "Bakgrundsuppgiften för kvalitetsrapporter startade om."
            }
            (A::QualityTaskRestarted, L::Da) => {
                "Baggrundsopgaven for kvalitetsrapporter startede forfra."
            }
            (A::QualityTaskRestarted, L::Pl) => {
                "Zadanie w tle dla raportów jakości zostało uruchomione ponownie."
            }
            (A::QualityTaskRestarted, L::Fr) => {
                "La tâche d'arrière-plan des rapports de qualité a redémarré."
            }

            // ── PreflightTitle ──────────────────────────────────────────────
            (A::PreflightTitle, L::No) => "SundayRec — sjekk før opptak",
            (A::PreflightTitle, L::En) => "SundayRec — check before recording",
            (A::PreflightTitle, L::De) => "SundayRec — Prüfung vor der Aufnahme",
            (A::PreflightTitle, L::Sv) => "SundayRec — kontroll före inspelning",
            (A::PreflightTitle, L::Da) => "SundayRec — tjek før optagelse",
            (A::PreflightTitle, L::Pl) => "SundayRec — sprawdź przed nagraniem",
            (A::PreflightTitle, L::Fr) => "SundayRec — vérification avant l'enregistrement",

            // ── PreflightFfmpegMissing ──────────────────────────────────────
            (A::PreflightFfmpegMissing, L::No) => {
                "ffmpeg-binær mangler. SundayRec må installeres på nytt."
            }
            (A::PreflightFfmpegMissing, L::En) => {
                "The ffmpeg binary is missing. SundayRec must be installed again."
            }
            (A::PreflightFfmpegMissing, L::De) => {
                "Die ffmpeg-Datei fehlt. SundayRec muss neu installiert werden."
            }
            (A::PreflightFfmpegMissing, L::Sv) => {
                "ffmpeg-filen saknas. SundayRec måste installeras om."
            }
            (A::PreflightFfmpegMissing, L::Da) => {
                "ffmpeg-filen mangler. SundayRec skal installeres igen."
            }
            (A::PreflightFfmpegMissing, L::Pl) => {
                "Brakuje pliku ffmpeg. Trzeba ponownie zainstalować SundayRec."
            }
            (A::PreflightFfmpegMissing, L::Fr) => {
                "Le binaire ffmpeg est absent. SundayRec doit être réinstallé."
            }

            // ── PreflightDeviceMissing ──────────────────────────────────────
            (A::PreflightDeviceMissing, L::No) => {
                "Lydenheten som er valgt i innstillingene er ikke tilkoblet."
            }
            (A::PreflightDeviceMissing, L::En) => {
                "The audio device selected in settings is not connected."
            }
            (A::PreflightDeviceMissing, L::De) => {
                "Das in den Einstellungen gewählte Audiogerät ist nicht angeschlossen."
            }
            (A::PreflightDeviceMissing, L::Sv) => {
                "Ljudenheten som är vald i inställningarna är inte ansluten."
            }
            (A::PreflightDeviceMissing, L::Da) => {
                "Lydenheden, der er valgt i indstillingerne, er ikke tilsluttet."
            }
            (A::PreflightDeviceMissing, L::Pl) => {
                "Urządzenie audio wybrane w ustawieniach nie jest podłączone."
            }
            (A::PreflightDeviceMissing, L::Fr) => {
                "Le périphérique audio choisi dans les réglages n'est pas connecté."
            }

            // ── PreflightFolderNotWritable ──────────────────────────────────
            (A::PreflightFolderNotWritable, L::No) => "Lagringsmappen kan ikke skrives.",
            (A::PreflightFolderNotWritable, L::En) => "The save folder cannot be written to.",
            (A::PreflightFolderNotWritable, L::De) => {
                "In den Speicherordner kann nicht geschrieben werden."
            }
            (A::PreflightFolderNotWritable, L::Sv) => {
                "Det går inte att skriva till lagringsmappen."
            }
            (A::PreflightFolderNotWritable, L::Da) => "Der kan ikke skrives til lagringsmappen.",
            (A::PreflightFolderNotWritable, L::Pl) => "Nie można zapisywać w folderze zapisu.",
            (A::PreflightFolderNotWritable, L::Fr) => {
                "Impossible d'écrire dans le dossier d'enregistrement."
            }

            // ── PreflightDiskLow ────────────────────────────────────────────
            (A::PreflightDiskLow, L::No) => {
                "Bare {gb} GB ledig på lagringsdisken — kanskje ikke nok for et helt opptak."
            }
            (A::PreflightDiskLow, L::En) => {
                "Only {gb} GB free on the save disk — perhaps not enough for a whole recording."
            }
            (A::PreflightDiskLow, L::De) => {
                "Nur {gb} GB frei auf dem Speicherlaufwerk — vielleicht nicht genug für eine ganze Aufnahme."
            }
            (A::PreflightDiskLow, L::Sv) => {
                "Bara {gb} GB ledigt på lagringsdisken — kanske inte nog för en hel inspelning."
            }
            (A::PreflightDiskLow, L::Da) => {
                "Kun {gb} GB ledig på lagringsdisken — måske ikke nok til en hel optagelse."
            }
            (A::PreflightDiskLow, L::Pl) => {
                "Tylko {gb} GB wolnego miejsca na dysku zapisu — być może za mało na całe nagranie."
            }
            (A::PreflightDiskLow, L::Fr) => {
                "Seulement {gb} Go libres sur le disque d'enregistrement — peut-être pas assez pour un enregistrement entier."
            }

            // ── PreflightMicDenied ──────────────────────────────────────────
            (A::PreflightMicDenied, L::No) => {
                "Mikrofontilgang er ikke gitt. Åpne Systeminnstillinger → Personvern → Mikrofon."
            }
            (A::PreflightMicDenied, L::En) => {
                "Microphone access has not been granted. Open System Settings → Privacy → Microphone."
            }
            (A::PreflightMicDenied, L::De) => {
                "Der Mikrofonzugriff ist nicht erteilt. Öffnen Sie Systemeinstellungen → Datenschutz → Mikrofon."
            }
            (A::PreflightMicDenied, L::Sv) => {
                "Mikrofonåtkomst har inte getts. Öppna Systeminställningar → Integritet → Mikrofon."
            }
            (A::PreflightMicDenied, L::Da) => {
                "Der er ikke givet adgang til mikrofonen. Åbn Systemindstillinger → Anonymitet → Mikrofon."
            }
            (A::PreflightMicDenied, L::Pl) => {
                "Nie przyznano dostępu do mikrofonu. Otwórz Ustawienia systemowe → Prywatność → Mikrofon."
            }
            (A::PreflightMicDenied, L::Fr) => {
                "L'accès au microphone n'est pas accordé. Ouvrez Réglages Système → Confidentialité → Microphone."
            }

            // ── PreflightCameraDenied ───────────────────────────────────────
            (A::PreflightCameraDenied, L::No) => "Kameratilgang er ikke gitt.",
            (A::PreflightCameraDenied, L::En) => "Camera access has not been granted.",
            (A::PreflightCameraDenied, L::De) => "Der Kamerazugriff ist nicht erteilt.",
            (A::PreflightCameraDenied, L::Sv) => "Kameraåtkomst har inte getts.",
            (A::PreflightCameraDenied, L::Da) => "Der er ikke givet adgang til kameraet.",
            (A::PreflightCameraDenied, L::Pl) => "Nie przyznano dostępu do kamery.",
            (A::PreflightCameraDenied, L::Fr) => "L'accès à la caméra n'est pas accordé.",

            // ── PreflightSaveFolderSynced ──────────────────────────────────────────────
            (A::PreflightSaveFolderSynced, L::No) => {
                "Lagringsmappen synkroniseres av OneDrive, som kan forstyrre et opptak som pågår."
            }
            (A::PreflightSaveFolderSynced, L::En) => {
                "The save folder is synced by OneDrive, which can interfere with a recording in progress."
            }
            (A::PreflightSaveFolderSynced, L::De) => {
                "Der Speicherordner wird von OneDrive synchronisiert, was eine laufende Aufnahme stören kann."
            }
            (A::PreflightSaveFolderSynced, L::Sv) => {
                "Lagringsmappen synkroniseras av OneDrive, vilket kan störa en pågående inspelning."
            }
            (A::PreflightSaveFolderSynced, L::Da) => {
                "Lagringsmappen synkroniseres af OneDrive, hvilket kan forstyrre en igangværende optagelse."
            }
            (A::PreflightSaveFolderSynced, L::Pl) => {
                "Folder zapisu jest synchronizowany przez OneDrive, co może zakłócić trwające nagrywanie."
            }
            (A::PreflightSaveFolderSynced, L::Fr) => {
                "Le dossier d’enregistrement est synchronisé par OneDrive, ce qui peut perturber un enregistrement en cours."
            }

            // ── ScheduledStarted ────────────────────────────────────────────
            (A::ScheduledStarted, L::No) => "Planlagt opptak startet.",
            (A::ScheduledStarted, L::En) => "Scheduled recording started.",
            (A::ScheduledStarted, L::De) => "Geplante Aufnahme gestartet.",
            (A::ScheduledStarted, L::Sv) => "Schemalagd inspelning startade.",
            (A::ScheduledStarted, L::Da) => "Planlagt optagelse startet.",
            (A::ScheduledStarted, L::Pl) => "Zaplanowane nagranie rozpoczęte.",
            (A::ScheduledStarted, L::Fr) => "Enregistrement programmé démarré.",

            // ── ScheduledStopped ────────────────────────────────────────────
            (A::ScheduledStopped, L::No) => "Planlagt opptak avsluttet.",
            (A::ScheduledStopped, L::En) => "Scheduled recording finished.",
            (A::ScheduledStopped, L::De) => "Geplante Aufnahme beendet.",
            (A::ScheduledStopped, L::Sv) => "Schemalagd inspelning avslutad.",
            (A::ScheduledStopped, L::Da) => "Planlagt optagelse afsluttet.",
            (A::ScheduledStopped, L::Pl) => "Zaplanowane nagranie zakończone.",
            (A::ScheduledStopped, L::Fr) => "Enregistrement programmé terminé.",

            // ── ScheduledSkippedBusy ────────────────────────────────────────
            (A::ScheduledSkippedBusy, L::No) => {
                "Planlagt opptak hoppet over — et opptak pågår allerede."
            }
            (A::ScheduledSkippedBusy, L::En) => {
                "Scheduled recording skipped — a recording is already running."
            }
            (A::ScheduledSkippedBusy, L::De) => {
                "Geplante Aufnahme übersprungen — eine Aufnahme läuft bereits."
            }
            (A::ScheduledSkippedBusy, L::Sv) => {
                "Schemalagd inspelning hoppades över — en inspelning pågår redan."
            }
            (A::ScheduledSkippedBusy, L::Da) => {
                "Planlagt optagelse sprunget over — en optagelse er allerede i gang."
            }
            (A::ScheduledSkippedBusy, L::Pl) => {
                "Pominięto zaplanowane nagranie — nagrywanie już trwa."
            }
            (A::ScheduledSkippedBusy, L::Fr) => {
                "Enregistrement programmé ignoré — un enregistrement est déjà en cours."
            }

            // ── ScheduledStartFailed ────────────────────────────────────────
            (A::ScheduledStartFailed, L::No) => "Planlagt opptak startet ikke: {detail}",
            (A::ScheduledStartFailed, L::En) => "The scheduled recording did not start: {detail}",
            (A::ScheduledStartFailed, L::De) => "Die geplante Aufnahme startete nicht: {detail}",
            (A::ScheduledStartFailed, L::Sv) => {
                "Den schemalagda inspelningen startade inte: {detail}"
            }
            (A::ScheduledStartFailed, L::Da) => "Den planlagte optagelse startede ikke: {detail}",
            (A::ScheduledStartFailed, L::Pl) => "Zaplanowane nagranie nie rozpoczęło się: {detail}",
            (A::ScheduledStartFailed, L::Fr) => {
                "L'enregistrement programmé n'a pas démarré : {detail}"
            }

            // ── ScheduledStartTimeout ───────────────────────────────────────
            (A::ScheduledStartTimeout, L::No) => {
                "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon."
            }
            (A::ScheduledStartTimeout, L::En) => {
                "The scheduled recording did not start (timed out) — check the camera/microphone."
            }
            (A::ScheduledStartTimeout, L::De) => {
                "Die geplante Aufnahme startete nicht (Zeitüberschreitung) — prüfen Sie \
                 Kamera/Mikrofon."
            }
            (A::ScheduledStartTimeout, L::Sv) => {
                "Den schemalagda inspelningen startade inte (tidsgräns) — kontrollera \
                 kamera/mikrofon."
            }
            (A::ScheduledStartTimeout, L::Da) => {
                "Den planlagte optagelse startede ikke (tidsudløb) — tjek kamera/mikrofon."
            }
            (A::ScheduledStartTimeout, L::Pl) => {
                "Zaplanowane nagranie nie rozpoczęło się (przekroczono czas) — sprawdź \
                 kamerę/mikrofon."
            }
            (A::ScheduledStartTimeout, L::Fr) => {
                "L'enregistrement programmé n'a pas démarré (délai dépassé) — vérifiez la \
                 caméra/le microphone."
            }

            // ── ScheduledPrepareFailed ──────────────────────────────────────
            (A::ScheduledPrepareFailed, L::No) => "Planlagt opptak kunne ikke forberedes: {detail}",
            (A::ScheduledPrepareFailed, L::En) => {
                "The scheduled recording could not be prepared: {detail}"
            }
            (A::ScheduledPrepareFailed, L::De) => {
                "Die geplante Aufnahme konnte nicht vorbereitet werden: {detail}"
            }
            (A::ScheduledPrepareFailed, L::Sv) => {
                "Den schemalagda inspelningen kunde inte förberedas: {detail}"
            }
            (A::ScheduledPrepareFailed, L::Da) => {
                "Den planlagte optagelse kunne ikke forberedes: {detail}"
            }
            (A::ScheduledPrepareFailed, L::Pl) => {
                "Nie udało się przygotować zaplanowanego nagrania: {detail}"
            }
            (A::ScheduledPrepareFailed, L::Fr) => {
                "L'enregistrement programmé n'a pas pu être préparé : {detail}"
            }

            // ── ScheduledLateStartFailed ────────────────────────────────────
            (A::ScheduledLateStartFailed, L::No) => {
                "Forsinket oppstart av planlagt opptak feilet: {detail}"
            }
            (A::ScheduledLateStartFailed, L::En) => {
                "The late start of the scheduled recording failed: {detail}"
            }
            (A::ScheduledLateStartFailed, L::De) => {
                "Der verspätete Start der geplanten Aufnahme schlug fehl: {detail}"
            }
            (A::ScheduledLateStartFailed, L::Sv) => {
                "Den försenade starten av den schemalagda inspelningen misslyckades: {detail}"
            }
            (A::ScheduledLateStartFailed, L::Da) => {
                "Den forsinkede start af den planlagte optagelse mislykkedes: {detail}"
            }
            (A::ScheduledLateStartFailed, L::Pl) => {
                "Opóźnione uruchomienie zaplanowanego nagrania nie powiodło się: {detail}"
            }
            (A::ScheduledLateStartFailed, L::Fr) => {
                "Le démarrage tardif de l'enregistrement programmé a échoué : {detail}"
            }

            // ── ScheduledSpecialDeviceFallback ──────────────────────────────
            // The recording DID start — on the usual device. The sentence says
            // both halves, so nobody goes looking for a lost recording, and
            // names the device that was not there, so somebody can go and find
            // it before the next one. The one-off terms match the renderer's
            // `app.setup.advanced.specialsTitle` in each language.
            (A::ScheduledSpecialDeviceFallback, L::No) => {
                "Lydenheten «{device}» for spesialopptaket var ikke tilgjengelig — opptaket \
                 bruker den vanlige lydenheten i stedet."
            }
            (A::ScheduledSpecialDeviceFallback, L::En) => {
                "The audio device \"{device}\" for the one-off recording was not available — \
                 recording from the usual audio device instead."
            }
            (A::ScheduledSpecialDeviceFallback, L::De) => {
                "Das Audiogerät „{device}“ für die einmalige Aufnahme war nicht verfügbar — \
                 die Aufnahme verwendet stattdessen das übliche Audiogerät."
            }
            (A::ScheduledSpecialDeviceFallback, L::Sv) => {
                "Ljudenheten ”{device}” för den enstaka inspelningen var inte tillgänglig — \
                 inspelningen använder den vanliga ljudenheten i stället."
            }
            (A::ScheduledSpecialDeviceFallback, L::Da) => {
                "Lydenheden »{device}« til enkeltoptagelsen var ikke tilgængelig — \
                 optagelsen bruger den sædvanlige lydenhed i stedet."
            }
            (A::ScheduledSpecialDeviceFallback, L::Pl) => {
                "Urządzenie audio „{device}” dla nagrania jednorazowego było niedostępne — \
                 nagranie korzysta zamiast tego ze zwykłego urządzenia audio."
            }
            (A::ScheduledSpecialDeviceFallback, L::Fr) => {
                "Le périphérique audio « {device} » de l'enregistrement ponctuel n'était pas \
                 disponible — l'enregistrement utilise le périphérique audio habituel."
            }

            // ── Reminder ────────────────────────────────────────────────────
            // Byte-for-byte the Electron `REMINDER_LABELS` map these were ported
            // from (via `scheduler::reminder_body`, which this replaced): a
            // volunteer who has read the same reminder every Sunday for a year
            // gains nothing from a fresh translation of it.
            (A::Reminder, L::No) => "Opptak starter om {min} minutter",
            (A::Reminder, L::En) => "Recording starts in {min} minutes",
            (A::Reminder, L::De) => "Aufnahme beginnt in {min} Minuten",
            (A::Reminder, L::Sv) => "Inspelning börjar om {min} minuter",
            (A::Reminder, L::Da) => "Optagelse starter om {min} minutter",
            (A::Reminder, L::Pl) => "Nagranie rozpocznie się za {min} minut",
            (A::Reminder, L::Fr) => "Enregistrement dans {min} minutes",

            // ── MissedOne ───────────────────────────────────────────────────
            (A::MissedOne, L::No) => "Planlagt opptak ble ikke gjort: {label} ({at}).",
            (A::MissedOne, L::En) => "A scheduled recording was not made: {label} ({at}).",
            (A::MissedOne, L::De) => "Eine geplante Aufnahme wurde nicht gemacht: {label} ({at}).",
            (A::MissedOne, L::Sv) => "En schemalagd inspelning blev inte gjord: {label} ({at}).",
            (A::MissedOne, L::Da) => "En planlagt optagelse blev ikke lavet: {label} ({at}).",
            (A::MissedOne, L::Pl) => "Zaplanowane nagranie nie zostało wykonane: {label} ({at}).",
            (A::MissedOne, L::Fr) => {
                "Un enregistrement programmé n'a pas eu lieu : {label} ({at})."
            }

            // ── MissedMany ──────────────────────────────────────────────────
            (A::MissedMany, L::No) => {
                "{count} planlagte opptak ble ikke gjort. Det eldste: {label} ({at})."
            }
            (A::MissedMany, L::En) => {
                "{count} scheduled recordings were not made. The oldest: {label} ({at})."
            }
            (A::MissedMany, L::De) => {
                "{count} geplante Aufnahmen wurden nicht gemacht. Die älteste: {label} ({at})."
            }
            (A::MissedMany, L::Sv) => {
                "{count} schemalagda inspelningar blev inte gjorda. Den äldsta: {label} ({at})."
            }
            (A::MissedMany, L::Da) => {
                "{count} planlagte optagelser blev ikke lavet. Den ældste: {label} ({at})."
            }
            (A::MissedMany, L::Pl) => {
                "Nie wykonano {count} zaplanowanych nagrań. Najstarsze: {label} ({at})."
            }
            (A::MissedMany, L::Fr) => {
                "{count} enregistrements programmés n'ont pas eu lieu. Le plus ancien : {label} \
                 ({at})."
            }

            // ── MissedWeeklyLabel ───────────────────────────────────────────
            (A::MissedWeeklyLabel, L::No) => "Ukentlig opptak ({start}–{stop})",
            (A::MissedWeeklyLabel, L::En) => "Weekly recording ({start}–{stop})",
            (A::MissedWeeklyLabel, L::De) => "Wöchentliche Aufnahme ({start}–{stop})",
            (A::MissedWeeklyLabel, L::Sv) => "Veckoinspelning ({start}–{stop})",
            (A::MissedWeeklyLabel, L::Da) => "Ugentlig optagelse ({start}–{stop})",
            (A::MissedWeeklyLabel, L::Pl) => "Cotygodniowe nagranie ({start}–{stop})",
            (A::MissedWeeklyLabel, L::Fr) => "Enregistrement hebdomadaire ({start}–{stop})",
            // ── MissedSpecialLabel ──────────────────────────────────────────
            // The renderer's own word for these (`specialsTitle`), singular.
            (A::MissedSpecialLabel, L::No) => "Spesialopptak",
            (A::MissedSpecialLabel, L::En) => "One-off recording",
            (A::MissedSpecialLabel, L::De) => "Einmalige Aufnahme",
            (A::MissedSpecialLabel, L::Sv) => "Engångsinspelning",
            (A::MissedSpecialLabel, L::Da) => "Enkeltoptagelse",
            (A::MissedSpecialLabel, L::Pl) => "Nagranie jednorazowe",
            (A::MissedSpecialLabel, L::Fr) => "Enregistrement ponctuel",
            // ── RecordingNotRecovered ───────────────────────────────────────
            (A::RecordingNotRecovered, L::No) => "Opptaket kunne ikke gjenopprettes",
            (A::RecordingNotRecovered, L::En) => "The recording could not be recovered",
            (A::RecordingNotRecovered, L::De) => {
                "Die Aufnahme konnte nicht wiederhergestellt werden"
            }
            (A::RecordingNotRecovered, L::Sv) => "Inspelningen kunde inte återupptas",
            (A::RecordingNotRecovered, L::Da) => "Optagelsen kunne ikke genoprettes",
            (A::RecordingNotRecovered, L::Pl) => "Nie udało się przywrócić nagrania",
            (A::RecordingNotRecovered, L::Fr) => "L'enregistrement n'a pas pu être rétabli",

            // ── RecordingStartTimeout (camera + microphone) ─────────────────
            (A::RecordingStartTimeout, L::No) => {
                "Opptaket startet ikke i tide — sjekk at kamera/mikrofon er tilkoblet og at \
                 appen har tilgang (Systeminnstillinger → Personvern)."
            }
            (A::RecordingStartTimeout, L::En) => {
                "The recording did not start in time — check that the camera/microphone are \
                 connected and that the app has access (System Settings → Privacy)."
            }
            (A::RecordingStartTimeout, L::De) => {
                "Die Aufnahme startete nicht rechtzeitig — prüfen Sie, ob Kamera/Mikrofon \
                 angeschlossen sind und die App Zugriff hat (Systemeinstellungen → Datenschutz)."
            }
            (A::RecordingStartTimeout, L::Sv) => {
                "Inspelningen startade inte i tid — kontrollera att kamera/mikrofon är anslutna \
                 och att appen har åtkomst (Systeminställningar → Integritet)."
            }
            (A::RecordingStartTimeout, L::Da) => {
                "Optagelsen startede ikke i tide — tjek at kamera/mikrofon er tilsluttet, og at \
                 appen har adgang (Systemindstillinger → Anonymitet)."
            }
            (A::RecordingStartTimeout, L::Pl) => {
                "Nagranie nie rozpoczęło się na czas — sprawdź, czy kamera/mikrofon są \
                 podłączone i czy aplikacja ma dostęp (Ustawienia systemowe → Prywatność)."
            }
            (A::RecordingStartTimeout, L::Fr) => {
                "L'enregistrement n'a pas démarré à temps — vérifiez que la caméra/le microphone \
                 sont connectés et que l'application a l'autorisation (Réglages Système → \
                 Confidentialité)."
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
            (A::RecordingStartTimeoutMic, L::De) => {
                "Die Aufnahme startete nicht rechtzeitig — prüfen Sie, ob das Mikrofon \
                 angeschlossen ist und die App Zugriff hat (Systemeinstellungen → Datenschutz)."
            }
            (A::RecordingStartTimeoutMic, L::Sv) => {
                "Inspelningen startade inte i tid — kontrollera att mikrofonen är ansluten och \
                 att appen har åtkomst (Systeminställningar → Integritet)."
            }
            (A::RecordingStartTimeoutMic, L::Da) => {
                "Optagelsen startede ikke i tide — tjek at mikrofonen er tilsluttet, og at appen \
                 har adgang (Systemindstillinger → Anonymitet)."
            }
            (A::RecordingStartTimeoutMic, L::Pl) => {
                "Nagranie nie rozpoczęło się na czas — sprawdź, czy mikrofon jest podłączony i \
                 czy aplikacja ma dostęp (Ustawienia systemowe → Prywatność)."
            }
            (A::RecordingStartTimeoutMic, L::Fr) => {
                "L'enregistrement n'a pas démarré à temps — vérifiez que le microphone est \
                 connecté et que l'application a l'autorisation (Réglages Système → \
                 Confidentialité)."
            }

            // ── RecordingDiskFull ───────────────────────────────────────────
            (A::RecordingDiskFull, L::No) => {
                "Lite ledig diskplass — stopper opptaket trygt før disken blir full."
            }
            (A::RecordingDiskFull, L::En) => {
                "Little free disk space — stopping the recording safely before the disk fills up."
            }
            (A::RecordingDiskFull, L::De) => {
                "Wenig freier Speicherplatz — die Aufnahme wird sicher gestoppt, bevor die \
                 Festplatte voll ist."
            }
            (A::RecordingDiskFull, L::Sv) => {
                "Lite ledigt diskutrymme — stoppar inspelningen säkert innan disken blir full."
            }
            (A::RecordingDiskFull, L::Da) => {
                "Lidt ledig diskplads — stopper optagelsen sikkert, før disken bliver fuld."
            }
            (A::RecordingDiskFull, L::Pl) => {
                "Mało wolnego miejsca na dysku — bezpiecznie zatrzymuję nagranie, zanim dysk się \
                 zapełni."
            }
            (A::RecordingDiskFull, L::Fr) => {
                "Peu d'espace disque libre — arrêt sécurisé de l'enregistrement avant saturation \
                 du disque."
            }

            // ── RecordingEmptyOutput ────────────────────────────────────────
            (A::RecordingEmptyOutput, L::No) => {
                "Opptaket ble tomt eller skadet — ingen fil ble lagret."
            }
            (A::RecordingEmptyOutput, L::En) => {
                "The recording came out empty or damaged — no file was saved."
            }
            (A::RecordingEmptyOutput, L::De) => {
                "Die Aufnahme war leer oder beschädigt — es wurde keine Datei gespeichert."
            }
            (A::RecordingEmptyOutput, L::Sv) => {
                "Inspelningen blev tom eller skadad — ingen fil sparades."
            }
            (A::RecordingEmptyOutput, L::Da) => {
                "Optagelsen blev tom eller beskadiget — ingen fil blev gemt."
            }
            (A::RecordingEmptyOutput, L::Pl) => {
                "Nagranie było puste lub uszkodzone — nie zapisano pliku."
            }
            (A::RecordingEmptyOutput, L::Fr) => {
                "L'enregistrement était vide ou endommagé — aucun fichier n'a été enregistré."
            }

            // ── RecordingDeviceNotFound ──────────────────────────────────────
            (A::RecordingDeviceNotFound, L::No) => "Lydenheten ble ikke funnet — sjekk USB-tilkoblingen",
            (A::RecordingDeviceNotFound, L::En) => "Audio device not found — check the USB connection",
            (A::RecordingDeviceNotFound, L::De) => "Audiogerät nicht gefunden — USB-Verbindung prüfen",
            (A::RecordingDeviceNotFound, L::Sv) => "Ljudenheten hittades inte — kontrollera USB-anslutningen",
            (A::RecordingDeviceNotFound, L::Da) => "Lydenheden blev ikke fundet — tjek USB-forbindelsen",
            (A::RecordingDeviceNotFound, L::Pl) => "Nie znaleziono urządzenia audio — sprawdź połączenie USB",
            (A::RecordingDeviceNotFound, L::Fr) => "Périphérique audio introuvable — vérifiez la connexion USB",
            // ── RecordingPermissionDenied ────────────────────────────────────
            (A::RecordingPermissionDenied, L::No) => "Mikrofontilgang nektet — åpne Systeminnstillinger og gi tilgang",
            (A::RecordingPermissionDenied, L::En) => "Microphone access denied — open System Settings and grant access",
            (A::RecordingPermissionDenied, L::De) => "Mikrofonzugriff verweigert — öffnen Sie die Systemeinstellungen und erteilen Sie den Zugriff",
            (A::RecordingPermissionDenied, L::Sv) => "Mikrofonåtkomst nekad — öppna Systeminställningar och ge åtkomst",
            (A::RecordingPermissionDenied, L::Da) => "Mikrofonadgang nægtet — åbn Systemindstillinger og giv adgang",
            (A::RecordingPermissionDenied, L::Pl) => "Odmowa dostępu do mikrofonu — otwórz Ustawienia systemu i udziel dostępu",
            (A::RecordingPermissionDenied, L::Fr) => "Accès au microphone refusé — ouvrez les Réglages système et accordez l’accès",
            // ── RecordingDeviceBusy ──────────────────────────────────────────
            (A::RecordingDeviceBusy, L::No) => "Lydenheten er i bruk av et annet program",
            (A::RecordingDeviceBusy, L::En) => "The audio device is in use by another application",
            (A::RecordingDeviceBusy, L::De) => "Das Audiogerät wird von einem anderen Programm verwendet",
            (A::RecordingDeviceBusy, L::Sv) => "Ljudenheten används av ett annat program",
            (A::RecordingDeviceBusy, L::Da) => "Lydenheden bruges af et andet program",
            (A::RecordingDeviceBusy, L::Pl) => "Urządzenie audio jest używane przez inną aplikację",
            (A::RecordingDeviceBusy, L::Fr) => "Le périphérique audio est déjà utilisé par une autre application",
            // ── RecordingDeviceError ─────────────────────────────────────────
            (A::RecordingDeviceError, L::No) => "Feil ved åpning av lydenhet — prøv å koble til på nytt",
            (A::RecordingDeviceError, L::En) => "Error opening the audio device — try reconnecting it",
            (A::RecordingDeviceError, L::De) => "Fehler beim Öffnen des Audiogeräts — versuchen Sie, es erneut anzuschließen",
            (A::RecordingDeviceError, L::Sv) => "Fel vid öppning av ljudenheten — försök att ansluta den på nytt",
            (A::RecordingDeviceError, L::Da) => "Fejl ved åbning af lydenheden — prøv at tilslutte den igen",
            (A::RecordingDeviceError, L::Pl) => "Błąd otwarcia urządzenia audio — spróbuj podłączyć je ponownie",
            (A::RecordingDeviceError, L::Fr) => "Erreur à l’ouverture du périphérique audio — essayez de le rebrancher",
            // ── RecordingDeviceDisconnected ──────────────────────────────────
            (A::RecordingDeviceDisconnected, L::No) => "Lydenheten ble koblet fra under opptak — sjekk tilkoblingen",
            (A::RecordingDeviceDisconnected, L::En) => "The audio device was disconnected during recording — check the connection",
            (A::RecordingDeviceDisconnected, L::De) => "Das Audiogerät wurde während der Aufnahme getrennt — Verbindung prüfen",
            (A::RecordingDeviceDisconnected, L::Sv) => "Ljudenheten kopplades från under inspelningen — kontrollera anslutningen",
            (A::RecordingDeviceDisconnected, L::Da) => "Lydenheden blev afbrudt under optagelsen — tjek forbindelsen",
            (A::RecordingDeviceDisconnected, L::Pl) => "Urządzenie audio zostało odłączone w trakcie nagrywania — sprawdź połączenie",
            (A::RecordingDeviceDisconnected, L::Fr) => "Le périphérique audio a été déconnecté pendant l’enregistrement — vérifiez la connexion",
            // ── RecordingEngineExited ────────────────────────────────────────
            (A::RecordingEngineExited, L::No) => "Opptaksmotoren stoppet uventet — lyden fram til da er lagret. Start et nytt opptak, og kjør Diagnose hvis det gjentar seg.",
            (A::RecordingEngineExited, L::En) => "The recording engine stopped unexpectedly — the audio up to that point is saved. Start a new recording, and run Diagnose if it happens again.",
            (A::RecordingEngineExited, L::De) => "Die Aufnahme-Engine hat unerwartet gestoppt — der Ton bis dahin ist gespeichert. Starten Sie eine neue Aufnahme, und führen Sie die Diagnose aus, wenn es sich wiederholt.",
            (A::RecordingEngineExited, L::Sv) => "Inspelningsmotorn stoppade oväntat — ljudet fram till dess är sparat. Starta en ny inspelning, och kör Diagnos om det upprepas.",
            (A::RecordingEngineExited, L::Da) => "Optagelsesmotoren stoppede uventet — lyden frem til da er gemt. Start en ny optagelse, og kør Diagnose, hvis det gentager sig.",
            (A::RecordingEngineExited, L::Pl) => "Silnik nagrywania zatrzymał się nieoczekiwanie — dźwięk do tego momentu jest zapisany. Rozpocznij nowe nagranie i uruchom Diagnostykę, jeśli to się powtórzy.",
            (A::RecordingEngineExited, L::Fr) => "Le moteur d’enregistrement s’est arrêté de façon inattendue — le son jusqu’à ce moment est sauvegardé. Démarrez un nouvel enregistrement, et lancez le Diagnostic si cela se répète.",
            // ── RecordingVideoCapture ────────────────────────────────────────
            (A::RecordingVideoCapture, L::No) => "Kameraet leverte ikke bilde — lyden ble lagret og ligger i biblioteket. Sjekk kameratilkoblingen før du tar opp med video igjen.",
            (A::RecordingVideoCapture, L::En) => "The camera delivered no picture — the audio was saved and is in the library. Check the camera connection before recording with video again.",
            (A::RecordingVideoCapture, L::De) => "Die Kamera lieferte kein Bild — der Ton wurde gespeichert und liegt in der Bibliothek. Prüfen Sie die Kameraverbindung, bevor Sie wieder mit Video aufnehmen.",
            (A::RecordingVideoCapture, L::Sv) => "Kameran levererade ingen bild — ljudet sparades och ligger i biblioteket. Kontrollera kameraanslutningen innan du spelar in med video igen.",
            (A::RecordingVideoCapture, L::Da) => "Kameraet leverede ikke noget billede — lyden blev gemt og ligger i biblioteket. Tjek kameraforbindelsen, før du optager med video igen.",
            (A::RecordingVideoCapture, L::Pl) => "Kamera nie dała obrazu — dźwięk został zapisany i jest w bibliotece. Sprawdź połączenie kamery, zanim znów nagrasz z wideo.",
            (A::RecordingVideoCapture, L::Fr) => "La caméra n’a pas fourni d’image — le son a été sauvegardé et se trouve dans la bibliothèque. Vérifiez la connexion de la caméra avant d’enregistrer à nouveau avec la vidéo.",
            // ── RecordingCameraFormat ────────────────────────────────────────
            (A::RecordingCameraFormat, L::No) => "Kameraet støtter ikke valgt bilderate eller oppløsning — velg en annen videoinnstilling og prøv igjen.",
            (A::RecordingCameraFormat, L::En) => "The camera does not support the chosen frame rate or resolution — pick another video setting and try again.",
            (A::RecordingCameraFormat, L::De) => "Die Kamera unterstützt die gewählte Bildrate oder Auflösung nicht — wählen Sie eine andere Videoeinstellung und versuchen Sie es erneut.",
            (A::RecordingCameraFormat, L::Sv) => "Kameran stöder inte vald bildfrekvens eller upplösning — välj en annan videoinställning och försök igen.",
            (A::RecordingCameraFormat, L::Da) => "Kameraet understøtter ikke den valgte billedhastighed eller opløsning — vælg en anden videoindstilling, og prøv igen.",
            (A::RecordingCameraFormat, L::Pl) => "Kamera nie obsługuje wybranej liczby klatek lub rozdzielczości — wybierz inne ustawienie wideo i spróbuj ponownie.",
            (A::RecordingCameraFormat, L::Fr) => "La caméra ne prend pas en charge la fréquence d’images ou la résolution choisie — choisissez un autre réglage vidéo et réessayez.",
            // ── RecordingCameraPermission ────────────────────────────────────
            (A::RecordingCameraPermission, L::No) => "Kameratilgang nektet — åpne Systeminnstillinger og gi appen tilgang til kameraet",
            (A::RecordingCameraPermission, L::En) => "Camera access denied — open System Settings and give the app access to the camera",
            (A::RecordingCameraPermission, L::De) => "Kamerazugriff verweigert — öffnen Sie die Systemeinstellungen und geben Sie der App Zugriff auf die Kamera",
            (A::RecordingCameraPermission, L::Sv) => "Kameraåtkomst nekad — öppna Systeminställningar och ge appen åtkomst till kameran",
            (A::RecordingCameraPermission, L::Da) => "Adgang til kameraet nægtet — åbn Systemindstillinger, og giv appen adgang til kameraet",
            (A::RecordingCameraPermission, L::Pl) => "Odmowa dostępu do kamery — otwórz Ustawienia systemowe i przyznaj aplikacji dostęp do kamery",
            (A::RecordingCameraPermission, L::Fr) => "Accès à la caméra refusé — ouvrez Réglages Système et donnez à l’app l’accès à la caméra",
            // ── RecordingCameraBusy ──────────────────────────────────────────
            (A::RecordingCameraBusy, L::No) => "Kameraet er i bruk av et annet program — lukk det (Teams, Zoom) og prøv igjen",
            (A::RecordingCameraBusy, L::En) => "The camera is in use by another program — close it (Teams, Zoom) and try again",
            (A::RecordingCameraBusy, L::De) => "Die Kamera wird von einem anderen Programm verwendet — schließen Sie es (Teams, Zoom) und versuchen Sie es erneut",
            (A::RecordingCameraBusy, L::Sv) => "Kameran används av ett annat program — stäng det (Teams, Zoom) och försök igen",
            (A::RecordingCameraBusy, L::Da) => "Kameraet bruges af et andet program — luk det (Teams, Zoom), og prøv igen",
            (A::RecordingCameraBusy, L::Pl) => "Kamera jest używana przez inny program — zamknij go (Teams, Zoom) i spróbuj ponownie",
            (A::RecordingCameraBusy, L::Fr) => "La caméra est utilisée par un autre programme — fermez-le (Teams, Zoom) et réessayez",
            // ── RecordingMux ─────────────────────────────────────────────────
            (A::RecordingMux, L::No) => "Lyd og bilde kunne ikke settes sammen — begge råfilene er beholdt i lagringsmappen, så ingenting er tapt.",
            (A::RecordingMux, L::En) => "Audio and video could not be combined — both raw files were kept in the save folder, so nothing is lost.",
            (A::RecordingMux, L::De) => "Ton und Bild konnten nicht zusammengesetzt werden — beide Rohdateien sind im Speicherordner erhalten, es ist also nichts verloren.",
            (A::RecordingMux, L::Sv) => "Ljud och bild kunde inte sättas ihop — båda råfilerna har behållits i lagringsmappen, så ingenting är förlorat.",
            (A::RecordingMux, L::Da) => "Lyd og billede kunne ikke sættes sammen — begge råfiler er beholdt i lagringsmappen, så ingenting er tabt.",
            (A::RecordingMux, L::Pl) => "Nie udało się złożyć dźwięku i obrazu — oba pliki źródłowe zostały zachowane w folderze zapisu, więc nic nie zginęło.",
            (A::RecordingMux, L::Fr) => "Le son et l’image n’ont pas pu être assemblés — les deux fichiers bruts sont conservés dans le dossier d’enregistrement, donc rien n’est perdu.",
            // ── RecordingFailedUnknown ───────────────────────────────────────
            (A::RecordingFailedUnknown, L::No) => "Noe gikk galt under opptak — sjekk at lydenhet og lagringsmappe er klare",
            (A::RecordingFailedUnknown, L::En) => "Something went wrong during recording — check that the audio device and save folder are ready",
            (A::RecordingFailedUnknown, L::De) => "Bei der Aufnahme ist etwas schiefgelaufen — prüfen Sie, ob Audiogerät und Speicherordner bereit sind",
            (A::RecordingFailedUnknown, L::Sv) => "Något gick fel under inspelningen — kontrollera att ljudenheten och lagringsmappen är redo",
            (A::RecordingFailedUnknown, L::Da) => "Noget gik galt under optagelsen — tjek at lydenheden og lagringsmappen er klar",
            (A::RecordingFailedUnknown, L::Pl) => "Coś poszło nie tak podczas nagrywania — sprawdź, czy urządzenie audio i folder zapisu są gotowe",
            (A::RecordingFailedUnknown, L::Fr) => "Un problème est survenu pendant l’enregistrement — vérifiez que le périphérique audio et le dossier d’enregistrement sont prêts",
            // ── TakeSilence ───────────────────────────────────────────────────
            (A::TakeSilence, L::No) => {
                "Opptaket er stille — sjekk at lyden kommer fram til SundayRec."
            }
            (A::TakeSilence, L::En) => {
                "The recording is silent — check that sound is reaching SundayRec."
            }
            (A::TakeSilence, L::De) => {
                "Die Aufnahme ist still — prüfe, ob der Ton bei SundayRec ankommt."
            }
            (A::TakeSilence, L::Sv) => {
                "Inspelningen är tyst — kontrollera att ljudet når SundayRec."
            }
            (A::TakeSilence, L::Da) => {
                "Optagelsen er stille — tjek at lyden når frem til SundayRec."
            }
            (A::TakeSilence, L::Pl) => {
                "Nagranie jest ciche — sprawdź, czy dźwięk dociera do SundayRec."
            }
            (A::TakeSilence, L::Fr) => {
                "L'enregistrement est silencieux — vérifiez que le son arrive bien à SundayRec."
            }

            // ── TakeQuality ───────────────────────────────────────────────────
            (A::TakeQuality, L::No) => {
                "Opptaket mangler lyd — åpne SundayRec og se hva som skjer."
            }
            (A::TakeQuality, L::En) => {
                "The recording is missing sound — open SundayRec and check what is happening."
            }
            (A::TakeQuality, L::De) => {
                "Der Aufnahme fehlt Ton — öffne SundayRec und prüfe, was passiert."
            }
            (A::TakeQuality, L::Sv) => {
                "Inspelningen saknar ljud — öppna SundayRec och se vad som händer."
            }
            (A::TakeQuality, L::Da) => {
                "Optagelsen mangler lyd — åbn SundayRec og se, hvad der sker."
            }
            (A::TakeQuality, L::Pl) => {
                "W nagraniu brakuje dźwięku — otwórz SundayRec i sprawdź, co się dzieje."
            }
            (A::TakeQuality, L::Fr) => {
                "Il manque du son dans l'enregistrement — ouvrez SundayRec pour voir ce qui se passe."
            }

            // ── TakeReconnecting ──────────────────────────────────────────────
            (A::TakeReconnecting, L::No) => {
                "Lydkilden forsvant — SundayRec prøver å koble til igjen."
            }
            (A::TakeReconnecting, L::En) => {
                "The audio source disappeared — SundayRec is trying to reconnect."
            }
            (A::TakeReconnecting, L::De) => {
                "Die Tonquelle ist verschwunden — SundayRec versucht, sich neu zu verbinden."
            }
            (A::TakeReconnecting, L::Sv) => {
                "Ljudkällan försvann — SundayRec försöker ansluta igen."
            }
            (A::TakeReconnecting, L::Da) => {
                "Lydkilden forsvandt — SundayRec prøver at forbinde igen."
            }
            (A::TakeReconnecting, L::Pl) => {
                "Źródło dźwięku zniknęło — SundayRec próbuje połączyć się ponownie."
            }
            (A::TakeReconnecting, L::Fr) => {
                "La source audio a disparu — SundayRec tente de se reconnecter."
            }

            // ── TakeDiskLow ───────────────────────────────────────────────────
            (A::TakeDiskLow, L::No) => {
                "Lite plass på disken — {gb} GB igjen. Opptaket stopper når disken er full."
            }
            (A::TakeDiskLow, L::En) => {
                "Low disk space — {gb} GB left. The recording stops when the disk is full."
            }
            (A::TakeDiskLow, L::De) => {
                "Wenig Speicherplatz — noch {gb} GB. Die Aufnahme stoppt, wenn die Festplatte voll ist."
            }
            (A::TakeDiskLow, L::Sv) => {
                "Lite diskutrymme — {gb} GB kvar. Inspelningen stoppar när disken är full."
            }
            (A::TakeDiskLow, L::Da) => {
                "Lidt diskplads — {gb} GB tilbage. Optagelsen stopper, når disken er fuld."
            }
            (A::TakeDiskLow, L::Pl) => {
                "Mało miejsca na dysku — zostało {gb} GB. Nagranie zatrzyma się, gdy dysk się zapełni."
            }
            (A::TakeDiskLow, L::Fr) => {
                "Espace disque faible — il reste {gb} Go. L'enregistrement s'arrête quand le disque est plein."
            }

            // ── WakeNotArmed ──────────────────────────────────────────────────
            (A::WakeNotArmed, L::No) => {
                "SundayRec får ikke satt opp vekking før neste opptak. La maskinen stå på."
            }
            (A::WakeNotArmed, L::En) => {
                "SundayRec can't set up a wake-up before the next recording. Leave the computer on."
            }
            (A::WakeNotArmed, L::De) => {
                "SundayRec kann vor der nächsten Aufnahme kein Aufwecken einrichten. Lass den Computer eingeschaltet."
            }
            (A::WakeNotArmed, L::Sv) => {
                "SundayRec kan inte ställa in väckning före nästa inspelning. Låt datorn vara på."
            }
            (A::WakeNotArmed, L::Da) => {
                "SundayRec kan ikke sætte vækning op før næste optagelse. Lad computeren være tændt."
            }
            (A::WakeNotArmed, L::Pl) => {
                "SundayRec nie może ustawić wybudzenia przed następnym nagraniem. Zostaw komputer włączony."
            }
            (A::WakeNotArmed, L::Fr) => {
                "SundayRec ne peut pas programmer le réveil avant le prochain enregistrement. Laissez l'ordinateur allumé."
            }

            // ── TestNotification ──────────────────────────────────────────────
            (A::TestNotification, L::No) => {
                "Dette er et testvarsel. Slik sier SundayRec fra hvis noe går galt."
            }
            (A::TestNotification, L::En) => {
                "This is a test notification. This is how SundayRec tells you if something goes wrong."
            }
            (A::TestNotification, L::De) => {
                "Dies ist eine Testbenachrichtigung. So meldet sich SundayRec, wenn etwas schiefgeht."
            }
            (A::TestNotification, L::Sv) => {
                "Det här är en testavisering. Så säger SundayRec till om något går fel."
            }
            (A::TestNotification, L::Da) => {
                "Dette er en testnotifikation. Sådan siger SundayRec til, hvis noget går galt."
            }
            (A::TestNotification, L::Pl) => {
                "To jest powiadomienie testowe. Tak SundayRec daje znać, gdy coś pójdzie nie tak."
            }
            (A::TestNotification, L::Fr) => {
                "Ceci est une notification de test. C'est ainsi que SundayRec vous prévient en cas de problème."
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
/// Norwegian because it is a key; this is the words. A named special recording
/// keeps the name a person typed — it is theirs, not ours to translate.
pub fn missed_label(kind: &crate::schedule::MissedKind, lang: Lang) -> String {
    use crate::schedule::MissedKind;
    match kind {
        MissedKind::Weekly { start, stop } => {
            AlertText::MissedWeeklyLabel.fill(lang, &[("start", start), ("stop", stop)])
        }
        MissedKind::Special { name: Some(name) } => name.clone(),
        MissedKind::Special { name: None } => AlertText::MissedSpecialLabel.text(lang),
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
    fn all_seven_languages_have_a_sentence_for_every_alert() {
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
        // pass that copies the Norwegian into the six other arms. Seven
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
        // Norwegian is the source, `params()` is the declared contract, and all
        // seven must carry exactly the declared set — no more (a translator
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
        // These seven shipped in Electron and then in `scheduler::reminder_body`
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
        assert_eq!(
            AlertText::Reminder.fill(Lang::De, &[("min", "5")]),
            "Aufnahme beginnt in 5 Minuten"
        );
        assert_eq!(
            AlertText::Reminder.fill(Lang::Sv, &[("min", "5")]),
            "Inspelning börjar om 5 minuter"
        );
        assert_eq!(
            AlertText::Reminder.fill(Lang::Da, &[("min", "5")]),
            "Optagelse starter om 5 minutter"
        );
        assert_eq!(
            AlertText::Reminder.fill(Lang::Pl, &[("min", "5")]),
            "Nagranie rozpocznie się za 5 minut"
        );
        assert_eq!(
            AlertText::Reminder.fill(Lang::Fr, &[("min", "5")]),
            "Enregistrement dans 5 minutes"
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
            (Lang::De, include_str!("../../../legacy/locales/de.json")),
            (Lang::Sv, include_str!("../../../legacy/locales/sv.json")),
            (Lang::Da, include_str!("../../../legacy/locales/da.json")),
            (Lang::Pl, include_str!("../../../legacy/locales/pl.json")),
            (Lang::Fr, include_str!("../../../legacy/locales/fr.json")),
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
        // message (engine.rs, cpal_capture.rs, two_process.rs, the native
        // capture's writer). Falling through to the generic sentence would be
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
        use crate::schedule::{missed_recordings, MissedKind, ScheduleSlot};
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
            &[],
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
        assert_eq!(
            missed_label(&MissedKind::Special { name: None }, Lang::No),
            "Spesialopptak",
            "the unnamed special's Norwegian word is its canonical label too"
        );
        assert_eq!(
            missed_label(
                &MissedKind::Special {
                    name: Some("Bryllup Kari og Ola".into())
                },
                Lang::Fr
            ),
            "Bryllup Kari og Ola",
            "a name somebody typed is not ours to translate"
        );
    }

    #[test]
    fn the_polish_volunteers_missed_sunday_is_polish() {
        // The scenario finding A8 is about, end to end: a church whose app is
        // set to Polish, whose machine slept through the 11:00 service. Before
        // this module the sentence below was Norwegian — the ONE line telling
        // them the service was not recorded.
        let s = AlertText::MissedOne.fill(
            Lang::from_code(Some("pl")),
            &[
                ("label", "Ukentlig opptak (11:00–13:00)"),
                ("at", "2026-09-06T11:00:00"),
            ],
        );
        assert!(
            s.starts_with("Zaplanowane nagranie nie zostało wykonane:"),
            "{s}"
        );
        assert!(s.contains("2026-09-06T11:00:00"));
        // …and the slot LABEL is deliberately not translated — see the module
        // header: it is hashed into the durable `notify_seen` key.
        assert!(s.contains("Ukentlig opptak (11:00–13:00)"));
    }

    #[test]
    fn the_special_device_fallback_names_the_missing_device_in_every_language() {
        // The operator needs WHICH device to go and find — a sentence that lost
        // the name in translation is "something was not available", which
        // nobody can act on before the next one-off recording.
        for &lang in Lang::ALL {
            let s =
                AlertText::ScheduledSpecialDeviceFallback.fill(lang, &[("device", "Rode NT-USB")]);
            assert!(s.contains("Rode NT-USB"), "{lang:?}: {s}");
        }
        assert_eq!(
            AlertText::ScheduledSpecialDeviceFallback.fill(Lang::No, &[("device", "Rode NT-USB")]),
            "Lydenheten «Rode NT-USB» for spesialopptaket var ikke tilgjengelig — opptaket \
             bruker den vanlige lydenheten i stedet."
        );
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
