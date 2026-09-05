//! The sentences the app says to a person who is not looking at the screen —
//! in the seven languages SundayRec ships (F1 finding A8).
//!
//! ## The hole this closes
//!
//! Every other user-facing surface in this app is localized: the renderer has
//! `legacy/locales`, the tray has [`crate::tray`], the window notices have
//! [`crate::window`], and the mails have [`crate::email`]. The two channels that
//! reach a volunteer who is NOT at the machine — the native OS notification and
//! the plaintext line an alert mail carries — were the exception. They were
//! written in Norwegian, as literals, at the point of failure:
//!
//! ```text
//! dispatch_scheduler_failure(app, "scheduled_start_timeout",
//!     "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon.")
//! ```
//!
//! A Polish volunteer who set the app to Polish, whose church runs an
//! unattended 11:00 service, got a Polish interface, a Polish mail *subject*
//! from [`crate::email`] — and this sentence, in Norwegian, as the one line that
//! told them what had actually happened.
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
//! and the same replace-loop [`crate::email`] uses, so there is one placeholder
//! syntax in the codebase rather than two.
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

use crate::email::MailLang;

/// One user-facing sentence the shell says outside the renderer: a native OS
/// notification body or title, or the line an alert mail prints.
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
    /// The e-mail relay's outbox pump restarted.
    EmailTaskRestarted,

    // ── Scheduler ───────────────────────────────────────────────────────────
    /// Title of the pre-service preflight notification.
    PreflightTitle,
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

    // ── Recorder (terminal failures that reach native + mail) ───────────────
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
        AlertText::EmailTaskRestarted,
        AlertText::PreflightTitle,
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
        AlertText::RecordingNotRecovered,
        AlertText::RecordingStartTimeout,
        AlertText::RecordingStartTimeoutMic,
        AlertText::RecordingDiskFull,
        AlertText::RecordingEmptyOutput,
    ];

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
            AlertText::MissedOne => &["label", "at"],
            AlertText::MissedMany => &["count", "label", "at"],
            _ => &[],
        }
    }

    /// The raw template for `lang`, placeholders unfilled.
    ///
    /// Public because the tests and the `--list` side of a future tooling pass
    /// want to see the template itself; call sites want [`Self::text`] or
    /// [`Self::fill`].
    pub fn template(self, lang: MailLang) -> &'static str {
        use AlertText as A;
        use MailLang as L;
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

            // ── EmailTaskRestarted ──────────────────────────────────────────
            (A::EmailTaskRestarted, L::No) => {
                "Bakgrunnsoppgaven for e-postvarsler startet på nytt."
            }
            (A::EmailTaskRestarted, L::En) => "The background task for e-mail alerts restarted.",
            (A::EmailTaskRestarted, L::De) => {
                "Die Hintergrundaufgabe für E-Mail-Benachrichtigungen wurde neu gestartet."
            }
            (A::EmailTaskRestarted, L::Sv) => {
                "Bakgrundsuppgiften för e-postaviseringar startade om."
            }
            (A::EmailTaskRestarted, L::Da) => {
                "Baggrundsopgaven for e-mailbeskeder startede forfra."
            }
            (A::EmailTaskRestarted, L::Pl) => {
                "Zadanie w tle dla powiadomień e-mail zostało uruchomione ponownie."
            }
            (A::EmailTaskRestarted, L::Fr) => {
                "La tâche d'arrière-plan des alertes e-mail a redémarré."
            }

            // ── PreflightTitle ──────────────────────────────────────────────
            (A::PreflightTitle, L::No) => "SundayRec — sjekk før opptak",
            (A::PreflightTitle, L::En) => "SundayRec — check before recording",
            (A::PreflightTitle, L::De) => "SundayRec — Prüfung vor der Aufnahme",
            (A::PreflightTitle, L::Sv) => "SundayRec — kontroll före inspelning",
            (A::PreflightTitle, L::Da) => "SundayRec — tjek før optagelse",
            (A::PreflightTitle, L::Pl) => "SundayRec — sprawdź przed nagraniem",
            (A::PreflightTitle, L::Fr) => "SundayRec — vérification avant l'enregistrement",

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
        }
    }

    /// The sentence for `lang`, for a variant that takes no parameter.
    ///
    /// A parameterised variant reaches this only by mistake, so it trips a
    /// `debug_assert` in tests and dev builds and still returns something
    /// truthful (the template) in release — an alert with a visible `{detail}`
    /// is bad, an alert that panicked the recorder mid-service is worse.
    pub fn text(self, lang: MailLang) -> String {
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
    /// standing, exactly like [`crate::email`]'s `fill` — the tests below are
    /// what make "left standing" impossible in practice, by checking each
    /// variant against its own [`Self::params`].
    pub fn fill(self, lang: MailLang, vars: &[(&str, &str)]) -> String {
        let mut out = self.template(lang).to_string();
        for (k, v) in vars {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
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
            22,
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
            for &lang in MailLang::ALL {
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
            for (i, &l1) in MailLang::ALL.iter().enumerate() {
                for &l2 in &MailLang::ALL[i + 1..] {
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
            for &lang in MailLang::ALL {
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
            for &lang in MailLang::ALL {
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
            AlertText::Reminder.fill(MailLang::No, &[("min", "10")]),
            "Opptak starter om 10 minutter"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::En, &[("min", "15")]),
            "Recording starts in 15 minutes"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::De, &[("min", "5")]),
            "Aufnahme beginnt in 5 Minuten"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::Sv, &[("min", "5")]),
            "Inspelning börjar om 5 minuter"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::Da, &[("min", "5")]),
            "Optagelse starter om 5 minutter"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::Pl, &[("min", "5")]),
            "Nagranie rozpocznie się za 5 minut"
        );
        assert_eq!(
            AlertText::Reminder.fill(MailLang::Fr, &[("min", "5")]),
            "Enregistrement dans 5 minutes"
        );
    }

    #[test]
    fn an_unknown_language_code_still_gets_a_sentence() {
        // The whole point of the fallback: a settings blob carrying `"xx"` (or
        // nothing at all) must still produce a real alert, in Norwegian.
        let lang = MailLang::from_code(Some("xx"));
        assert_eq!(
            AlertText::ScheduledSkippedBusy.text(lang),
            AlertText::ScheduledSkippedBusy.text(MailLang::No)
        );
        assert_eq!(
            AlertText::ScheduledSkippedBusy.text(MailLang::from_code(None)),
            AlertText::ScheduledSkippedBusy.text(MailLang::No)
        );
    }

    #[test]
    fn the_polish_volunteers_missed_sunday_is_polish() {
        // The scenario finding A8 is about, end to end: a church whose app is
        // set to Polish, whose machine slept through the 11:00 service. Before
        // this module the sentence below was Norwegian — the ONE line telling
        // them the service was not recorded.
        let s = AlertText::MissedOne.fill(
            MailLang::from_code(Some("pl")),
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
    fn fill_ignores_a_key_the_template_does_not_have() {
        let s = AlertText::ScheduledStarted.fill(MailLang::En, &[("nope", "x")]);
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
