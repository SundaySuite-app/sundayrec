# Varsling — plan og status

Hvordan SundayRec sier fra når noe går galt, og hva som gjenstår. Levende
dokument: oppdater det når et punkt er gjort.

## Prinsippet

Den som står ved opptaksmaskinen får beskjed, med et systemvarsel (macOS
Varslingssenter / Windows-varsler). Appen sender ingenting ut av maskinen for å
melde en feil. Feilvarsler kan ikke slås av; bryteren «Varsel på maskinen»
gjelder bare meldingene om at et planlagt opptak startet og stoppet.

## ✅ Runde 1 — e-post ut

E-postvarslene er fjernet: SMTP-varsleren (`email`-featuren, `lettre`,
passordet i nøkkelringen) og SundaySuite-reléet (`notify.sundaysuite.app`,
kvitteringen). Se CHANGELOG «E-postvarsler er fjernet» og `docs/SMOKE-TEST.md`
§8.

- Systemvarselet er den ene kanalen for feil: `notify::dispatch_failure`
  (`src-tauri/src/notify/mod.rs`).
- Varselet om tapte opptak sies én gang per opptak, også på tvers av
  omstarter: `notify_seen`-tabellen (`src-tauri/src/notify/seen.rs`), trimmet
  ved oppstart.
- Oppgraderte installasjoner ryddes én gang: `settings::email_cleanup`
  (e-postfeltene, SMTP-passordet der SMTP var satt opp, engangsbanneret).
  Migrering `0008` dropper `notify_outbox` og relé-abonnementet.
- Kortet «Hvem får beskjed hvis noe går galt?» svarer «På maskinen».

## Runde 2 — varslene på maskinen

### A. OS-tillatelse og testvarsel

- ⚠️ `tauri-plugin-notification` svarer alltid `Granted` på desktop, og kan
  ikke brukes til å sjekke tillatelsen.
- Ny `src-tauri/src/notify/permission.rs`, etter mønsteret i
  `src-tauri/src/media/permissions.rs`:
  - **macOS:** `UNUserNotificationCenter` (`getNotificationSettings…` /
    `requestAuthorization…`), bare fra en `.app`-bundle — ellers `unknown`.
    Spike først på ekte Mac: stemmer statusen for varsler vist via
    notify-rust/NSUserNotification? Hvis ikke: `unknown` + testknappen.
  - **Windows:** registeret (`…\PushNotifications\ToastEnabled` og
    `…\Notifications\Settings\<AUMID>\Enabled`) via `windows-sys`
    (`Win32_System_Registry`).
- Kommandoer: `notification_permission`, `notification_request_permission`,
  `notification_open_settings` (`x-apple.systempreferences:…Notifications…` /
  `ms-settings:notifications`), `notification_send_test`.
- Varsel-siden: statusrad, «Tillat varsler» / «Åpne innstillinger», «Send
  testvarsel».

### B. Kortet «Hvem får beskjed?» følger tillatelsen

| Tillatelse                 | Kort      | Tekst                                    |
| -------------------------- | --------- | ---------------------------------------- |
| `granted`                  | `done`    | «På maskinen»                            |
| `denied` / `notDetermined` | `todo`    | «Varsler er slått av» + «Tillat varsler» |
| `unknown`                  | `unknown` | hint om testvarselet                     |

Første oppstart får samme logikk (`app.first.notifyTodo` er allerede skrevet
for `todo`-tilfellet).

### C. Nye systemvarsler under opptak

- Lite disk, stillhet/mangler lyd, lydkilden falt ut.
- Bare når hovedvinduet ikke er i fokus (synlig + fokusert + ikke minimert),
  og maks én gang per opptak per type (nullstilles på `recording://started`).
- Observerende, som `wire_failure_sources`: `app.listen` på
  `recording://silence`, `recording://quality`, `recording://reconnecting`;
  diskobservatøren (`notify/disk.rs`) får et native-kall ved siden av
  `DISK_LOW`.
- Ren beslutning i kjernen med tester:
  `should_native_during_take(kind, window_focused, already_sent)`.
- Nye `AlertText`-varianter på alle sju språk.

### D. Stille startfeil som skal tettes

1. Forsinket start der opptaksvalgene ikke kan bygges, logges bare
   (`check_missed` i `scheduler/mod.rs`, grenen `could not build opts for
late-start`). Skal gå gjennom `dispatch_scheduler_failure`.
2. Vekking som ikke kunne settes opp logges bare (`core/wake.rs`,
   `scheduler/mod.rs` sin registrering). Nytt systemvarsel, én gang per
   oppstart per feiltype: «SundayRec får ikke satt opp vekking før neste
   opptak. La maskinen stå på.»
3. Tapte opptak eldre enn 24 t sies aldri fra om. Utvid
   `MISSED_LOG_WINDOW_MS` (`core/schedule.rs`) til 7 dager; ledgeren beholder
   rader i 8 dager allerede (`notify::seen::SEEN_RETENTION_MS`).
4. «Planlagt opptak avsluttet» vises selv om ingenting ble tatt opp. Send
   `StoppedScheduled` bare når motoren faktisk var aktiv.

### Verifisering

Enhetstester for beslutningene, og på rigg (Mac med signert bundle + Windows):
varsler slått av i OS-et gir gult kort; «Send testvarsel» viser et varsel;
skjult vindu + utdratt mikrofon gir ÉTT varsel; ugyldig lagringsmappe + sen
start gir systemvarsel; planlagt stopp uten opptak gir ikke «avsluttet».

## Senere

- Oversette rå engelske/ffmpeg-feiltekster som havner i systemvarsler
  (`recorder/engine.rs`, `two_process.rs`, `cpal_capture.rs`).
- Varselet om tapt opptak: slot-navnet er alltid norsk (det inngår i
  ledger-nøkkelen) og tidspunktet er en rå ISO-streng.
- `wake_failure`-tabellen skrives aldri, så «Vekkehistorikk» er alltid tom.
- Menylinje-ikonet viser verken planleggerfeil eller tapte opptak.
- `TrayLang` og `lang::Lang` er to like enumer.
- Fjerne `secrets`-modulen og `keyring` når e-postoppryddingen har vært med i
  to utgivelser.
- `sunday-telemetry`: rive `notify.sundaysuite.app` og slette lagrede adresser
  når flåten har oppdatert.
- Varsel til mobil (ntfy/Pushover) er et eget prosjekt, hvis det noen gang
  blir aktuelt.
