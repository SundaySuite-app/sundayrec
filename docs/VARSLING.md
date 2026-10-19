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

## ✅ Runde 2 — varslene på maskinen

- **Viser OS-et varslene?** `notify::permission` (Windows: to DWORD-er i
  registeret; macOS: `unknown`, se under). Varsel-siden har raden «Varsler i
  systemet» med «Send testvarsel» og «Åpne innstillinger», og leser svaret på
  nytt når vinduet får fokus igjen.
- **Kortet «Hvem får beskjed?»** følger svaret: `granted` → grønt «På
  maskinen»; `denied` → gult «Varsler er slått av» + «Sett opp»; `unknown`
  (macOS) → besvart, med testvarselet som bevis; ikke spurt ennå → nøytralt.
- **Under opptak** (`notify::take`): stillhet, manglende lyd, lydkilde som
  faller ut og lite disk gir også et systemvarsel når vinduet ikke er i fokus,
  én gang per opptak per type.
- **Stille startfeil tettet:** forsinket start som ikke kunne forberedes,
  vekking som ikke kan settes opp (én gang per oppstart per type), tapte
  opptak inntil 7 dager tilbake, og «avsluttet» sies bare når noe faktisk tok
  opp.

Verifisering på rigg: `docs/SMOKE-TEST.md` §8b.

### Macs tillatelse (D2, venter på eierens prøve)

- **macOS leser nå systemets svar** via `UNUserNotificationCenter
.getNotificationSettings` (`objc2-user-notifications`, samme objc2-familie
  som resten av appen). `notify::permission` avgjør slik:

  | Systemet sier                                              | Kortet viser |
  | ---------------------------------------------------------- | ------------ |
  | Utenfor en `.app`-bundle (dev, `cargo run`, tester)        | `unknown`    |
  | Kallet svarer ikke innen 1,5 s (`MAC_SETTINGS_TIMEOUT`)    | `unknown`    |
  | `notDetermined` (ikke spurt ennå — «Send testvarsel» spør) | `unknown`    |
  | `denied`                                                   | `denied`     |
  | Tillatt, men bannerstil «Ingen» (`alertSetting` av)        | `denied`     |
  | Tillatt (authorized/provisional/ephemeral), banner på      | `granted`    |

- **Vakter:** bundlen sjekkes FØR noe UserNotifications-kall (kallet kan avbryte
  prosessen utenfor en bundle; `NSBundle.mainBundle`-stien må ende på `.app` og ha
  identifikator). Kallet har en Objective-C-blokk og er avgrenset i tid, så en
  hengende blokk gir `unknown`, aldri en frossen side. Kommandoen går på
  blokkeringspoolen, ikke hovedtråden. Et ObjC-unntak fanges.
- **Testet uten Mac-innstillinger:** utenfor bundle kalles ikke systemet
  (testen panikker hvis det gjøres) og svaret er `unknown`; en stand-in som
  aldri svarer ender som `unknown` innen tidsgrensen; en ekte testprosess gir
  `unknown` uten krasj.
- **Appen ber om tillatelse (D2b):** første gang brukeren trykker «Send
  testvarsel» — og bare da, aldri ved oppstart. Rekkefølgen er:
  `notification_request_permission` → kortet oppdateres med svaret →
  `notification_send_test`.

  | Status når «Send testvarsel» trykkes            | Hva skjer                                                     |
  | ----------------------------------------------- | ------------------------------------------------------------- |
  | Utenfor en `.app`-bundle                        | Ingenting kalles; `unknown`                                   |
  | Statusen kan ikke leses (1,5 s)                 | Ingen forespørsel; `unknown`                                  |
  | `notDetermined`                                 | macOS-dialogen vises (alert + lyd), så leses statusen på nytt |
  | `granted` / `denied` (inkl. bannerstil «Ingen») | Ingen forespørsel (macOS viser aldri dialogen to ganger)      |
  | Windows / Linux                                 | Uendret: bare status                                          |

  Dialogen blir stående til brukeren svarer, så ventetiden har egen, romslig
  grense (`MAC_REQUEST_TIMEOUT`, 120 s) og går på blokkeringspoolen, aldri
  hovedtråden. Svares det ikke i tide, leses statusen som den da er (fortsatt
  `unknown`), og testvarselet sendes likevel. Forespørselen bruker
  `requestAuthorizationWithOptions` (alert + sound) med samme vakter som
  lesingen (bundle først, tidsgrense, ObjC-unntak fanges). Svaret hentes fra
  systemets status, ikke fra completion-flagget, slik at bannerstil teller med.
  Bannerstil «Ingen» = `denied` er bekreftet riktig av eier.

- **Ikke bevist:** at denne tillatelsen speiler veien pluginen faktisk viser
  varsler gjennom (eldre `NSUserNotification`), og at `notDetermined` er riktig
  for en app som aldri ber om tillatelse. Eierens prøve med varsler av og på:
  `docs/RIG-DAY.md` §«(w) Varselstatus på Mac». Først da stryker vi punktet i
  `PLAN.md`.

## ✅ Runde 3 — «Senere»-lista (2026-09-28)

- **Feil under opptak varsles med setningen, ikke ffmpeg-linja.**
  `AlertText::for_recording_code` gir hver feilkode vinduets egen setning
  (`recording.error*` i `legacy/locales`) på sju språk; testen
  `recording_failures_say_what_the_window_says` feiler hvis de to katalogene
  sprikker. `engine::emit_failure` sender setningen og legger detaljen i loggen
  og i Lydhjelps `last-error.json`.
- **Tapt opptak på brukerens språk.** Navnet bygges fra `MissedKind`
  (`alerts::missed_label`), tiden som menyens ukedag + klokkeslett. Nøkkelen i
  `notify_seen` er uendret (tid + det kanoniske norske navnet), så ingen får
  høre om samme søndag to ganger når språket byttes.
- **Vekkehistorikken skrives.** Et tapt opptak logges én gang, og bare når
  «Vekk maskinen fra dvale» er på. Lista viser dag og dato.
- **Menylinja sier fra.** Planleggerfeil (`scheduler://failure`) og tapte
  opptak gir gult ikon og statusraden «Planlagt opptak ble ikke tatt» til neste
  opptak går live.
- **`TrayLang` er borte**; menyen og vinduet bruker `lang::Lang`.

Verifisering på rigg: `docs/RIG-DAY.md` (tapt opptak med vekking på, og
menylinja etterpå).

## ✅ Runde 4 — `secrets` og `keyring` ut (v0.25.0)

E-postoppryddingen var med i v0.23.0 og v0.24.0, som slettet SMTP-passordet fra
nøkkelringen én gang. Nå er `secrets`-modulen og `keyring`-avhengigheten borte,
og appen rører ikke nøkkelringen i det hele tatt. `settings::email_cleanup`
rydder fortsatt innstillingene og viser beskjeden én gang. En installasjon som
hopper rett fra v0.22.0 eller eldre, beholder passordet ulest i nøkkelringen,
som de andre pensjonerte oppføringene; lista over dem står i modulhodet til
`settings::email_cleanup`.

## ✅ Runde 5 — kameraet er borte (C5b)

- Faller kameraet ut under et opptak, sier overlegget «Kameraet falt ut — lyden
  tas fortsatt opp» så lenge det er borte (`recording://progress.video_lost_ms`).
- Er det borte i **60 s**, går det i tillegg et systemvarsel med samme setning
  (`AlertText::TakeCameraLost`, sju språk), når vinduet ikke er i fokus. **Én
  gang per utfall**, ikke per opptak: gjenkobling re-armerer, så et nytt utfall
  varsles på nytt (`notify::CameraOutageWatch`, `notify::take`). Beslutningen
  er ren og testet på varigheter, uten klokke.
- Feilvarsler kan fortsatt ikke slås av.

Verifisering på rigg: `docs/RIG-DAY.md` (v9).

## Senere

Hvert punkt har en betingelse for når det kan tas.

- **Testvekkingens resultat** (`test_ok`/`test_fail`) måles ikke. Det krever et
  signal for at maskinen våknet, og Tauri har ikke det. Dommen
  (`classify_test_wake_delta`) er klar. _Når:_ en strømovervåker finnes (egen
  spike, maskinvare).
- **`sunday-telemetry`: rive `notify.sundaysuite.app`** og slette lagrede
  adresser. _Når:_ flåten har oppdatert til v0.23.0 eller nyere (se
  telemetri-oversikten). Eierens oppgave (`docs/NEEDS-RICHARD.md`).
- **Varsel til mobil** (ntfy/Pushover) er et eget prosjekt, hvis det noen gang
  blir aktuelt.
