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

### Gjenstår fra runde 2

- **macOS-tillatelsen** leses ikke. `UNUserNotificationCenter` krever en
  Objective-C-blokk, krasjer utenfor en `.app`-bundle, og det er ikke bevist at
  den speiler NSUserNotification-veien pluginen viser varsler gjennom. Spike på
  ekte Mac først; til da er svaret `unknown` + testvarselet.

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

## Senere

Hvert punkt har en betingelse for når det kan tas.

- **Testvekkingens resultat** (`test_ok`/`test_fail`) måles ikke. Det krever et
  signal for at maskinen våknet, og Tauri har ikke det. Dommen
  (`classify_test_wake_delta`) er klar. _Når:_ en strømovervåker finnes (egen
  spike, maskinvare).
- **Fjerne `secrets`-modulen og `keyring`.** _Når:_ e-postoppryddingen har vært
  med i to utgivelser. v0.23.0 var den første, så tidligst i utgivelsen etter
  neste.
- **`sunday-telemetry`: rive `notify.sundaysuite.app`** og slette lagrede
  adresser. _Når:_ flåten har oppdatert til v0.23.0 eller nyere (se
  telemetri-oversikten). Eierens oppgave (`docs/NEEDS-RICHARD.md`).
- **Varsel til mobil** (ntfy/Pushover) er et eget prosjekt, hvis det noen gang
  blir aktuelt.
