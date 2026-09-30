# Plan — hva gjenstår i SundayRec

_Sist gått gjennom: 2026-09-29, ved v0.24.0 (stabil og beta)._

Én side som svarer på «hva er ikke gjort, og hvem sitt er det?». Hvert punkt
**bor i ett dokument** (kolonnen «Kilde»), med detaljene der; denne sida er
registeret som peker dit. Et punkt som ikke står her, er ikke planlagt.

**Slik holdes sida i live**

- Nytt åpent punkt → skriv det i kildedokumentet, og én linje her.
- Ferdig → marker det i kilden, og fjern linja her i samme PR.
- Ved hver utgivelse leses sida gjennom (`docs/RELEASE-CHECKLIST.md` §5g),
  og datoen øverst oppdateres.

Kartet over alle dokumentene er [`docs/README.md`](README.md).

## Kode

Kan gjøres uten eier eller rigg — men flere har en betingelse.

| Punkt                                                                                                                                        | Betingelse / når                                       | Kilde                                                            |
| -------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------------------- |
| Snevre inn `opener:allow-open-path` (en kompromittert webview kan åpne vilkårlige stier) til en kommando som bare åpner lagringsmappa/opptak | Fritt                                                  | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #2  |
| Dele `recorder/engine.rs` (≈4 800 linjer) i supervisor / progress / stderr-tolking                                                           | Fritt, men rør ikke opptaksstien uten riggtest etterpå | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #12 |
| `tokio = { features = ["full"] }` → bare det som brukes                                                                                      | Fritt                                                  | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #14 |
| Spesialopptak med eget lydkort: `SpecialRecording.device_id` er en id, opptakeren matcher på navn                                            | Fritt                                                  | `src-tauri/src/scheduler/mod.rs` (modulhodet)                    |
| Gå gjennom R1-seksjonens «utsatte» editor-liste mot dagens editor (skrevet før editor-fanene; trolig delvis gjort)                           | Fritt                                                  | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §R1                         |

## Spike / maskinvare først

Kode, men bare etter et forsøk på ekte maskin.

| Punkt                                                                                     | Kilde                                                            |
| ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| macOS: lese om systemet viser SundayRecs varsler (i dag `unknown` + testvarsel)           | [VARSLING.md](VARSLING.md) §Gjenstår fra runde 2                 |
| Testvekkingens resultat (`test_ok`/`test_fail`) — krever et signal for at maskinen våknet | [VARSLING.md](VARSLING.md) §Senere                               |
| Intel/universal Mac-bygg (i dag bare Apple Silicon; trenger ffmpeg-pinner for x86_64)     | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #17 |

## Eier (Richard)

| Punkt                                                                                                                                                                                                                 | Kilde                                                           |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| **Notarisering:** godta Apples oppdaterte avtale, sett `NOTARIZE_MAC=true`                                                                                                                                            | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Release blockers, punkt 3 |
| Windows-kodesigneringssertifikat (valgfritt; fjerner SmartScreen-advarselen)                                                                                                                                          | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Signing                   |
| Rive `notify.sundaysuite.app` i `sunday-telemetry` og slette lagrede adresser — når flåten er på v0.23.0 eller nyere                                                                                                  | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §PU-1                      |
| Ni eierbeslutninger fra F2 (MSI/UAC, database i Local AppData, webview for frakoblet PC, `-realtime 1`, `_redigert` i biblioteket, «Kirke»-mastringsprofil, automatisk mono, `{when}`-liming, resten av språkfunnene) | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Eierbeslutninger fra F2   |
| Klassisk ffmpeg-pre-roll: fjerne nødluka når en ekte søndag har bevist den native bufferen                                                                                                                            | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Summary                   |
| SoundCloud-API: lagt på is; tellerne `editor.publish.*` avgjør om den tas opp igjen                                                                                                                                   | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §«Legg ut»                 |

## Rigg og ører

Alt her står som avkryssingspunkter i [`docs/RIG-DAY.md`](RIG-DAY.md) — én
økt, i rekkefølge: Mac-boksen, Windows-boksen, Ørene. Hva som mangler
rigg-bevis og hvorfor: [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §A real recording
rig. Nytt fra v0.23.0: at «Legg ut» faktisk åpner nettleseren (SMOKE-TEST
§«Legg ut»). Nytt fra v0.24.0: tapt opptak i varsel, menylinje og
vekkehistorikk (RIG-DAY «(e, varsling — runde 3)»).

## Ikke planlagt

Nevnt, bevisst ikke tatt: varsel til mobil (ntfy/Pushover) —
[VARSLING.md](VARSLING.md) §Senere.
