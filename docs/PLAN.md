# Plan — hva gjenstår i SundayRec

_Sist gått gjennom: 2026-10-04, ved v0.25.1 (stabil og beta)._

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

Ingenting åpent. (Siste punkt, nedgradering etter en ny migrasjon, ble avgjort 2026-10-04: en nyere database åpnes, og migrasjoner kan bare legge til — se modulhodet i [store.rs](../src-tauri/src/db/store.rs).)

## Spike / maskinvare først

Kode, men bare etter et forsøk på ekte maskin.

| Punkt                                                                                     | Kilde                                                            |
| ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| macOS: lese om systemet viser SundayRecs varsler (i dag `unknown` + testvarsel)           | [VARSLING.md](VARSLING.md) §Gjenstår fra runde 2                 |
| Testvekkingens resultat (`test_ok`/`test_fail`) — krever et signal for at maskinen våknet | [VARSLING.md](VARSLING.md) §Senere                               |
| Intel/universal Mac-bygg (i dag bare Apple Silicon; trenger ffmpeg-pinner for x86_64)     | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #17 |

## Eier (Richard)

| Punkt                                                                                                                | Kilde                                                           |
| -------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| **Notarisering:** godta Apples oppdaterte avtale, sett `NOTARIZE_MAC=true`                                           | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Release blockers, punkt 3 |
| Windows-kodesigneringssertifikat (valgfritt; fjerner SmartScreen-advarselen)                                         | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Signing                   |
| Rive `notify.sundaysuite.app` i `sunday-telemetry` og slette lagrede adresser — når flåten er på v0.23.0 eller nyere | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §PU-1                      |
| Eierbeslutninger fra F2 som står igjen: resten av språkfunnene                                                       | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Eierbeslutninger fra F2   |
| Klassisk ffmpeg-pre-roll: fjerne nødluka når en ekte søndag har bevist den native bufferen                           | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Summary                   |
| SoundCloud-API: lagt på is; tellerne `editor.publish.*` avgjør om den tas opp igjen                                  | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §«Legg ut»                 |

## Rigg og ører

Alt her står som avkryssingspunkter i [`docs/RIG-DAY.md`](RIG-DAY.md) — én
økt, i rekkefølge: Mac-boksen, Windows-boksen, Ørene. Hva som mangler
rigg-bevis og hvorfor: [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §A real recording
rig. Nytt fra v0.23.0: at «Legg ut» faktisk åpner nettleseren (SMOKE-TEST
§«Legg ut»). Nytt fra v0.24.0: tapt opptak i varsel, menylinje og
vekkehistorikk (RIG-DAY «(e, varsling — runde 3)»). Nytt fra v0.25.0: at
opptaksmotoren oppfører seg som før etter oppdelingen (RIG-DAY a, c, h og
w6), «Åpne opptaksmappen» og «Vis i Finder» på Mac og Windows
(SMOKE-TEST), og spesialopptak med eget lydkort, også reserven når det
mangler (RIG-DAY «(c, fortsettelse) Spesialopptak med eget lydkort», inkludert
Windows-punkt 1–4), og etter sikkerhetsrunden (#309–#315): avspilling og
forhåndslytting i redigeringen (asset-tilgang per fil), krasjgjenoppretting på
Mac og Windows, valg av opptaksmappe ved første oppstart, oppgradering fra
v0.24.0, og «Vis i Finder» på opptaks- og eksportkvitteringen. **v0.25.0 gikk
rett til stabil 2026-10-04 etter eiers ordre** (RELEASE-CHECKLIST §5, slik
v0.12.0) — riggpunktene over er derfor etterkontroll av det flåten alt har.
Det samme gjelder v0.25.1 (2026-10-04): flyttingen av appdata til Local
AppData og NSIS/MSI på Windows (RIG-DAY w-appdata\*, w7) og et langt
videoopptak på Mac under CPU-last (e2).

## Ikke planlagt

Nevnt, bevisst ikke tatt:

- Varsel til mobil (ntfy/Pushover) — [VARSLING.md](VARSLING.md) §Senere.
- Kapitler i eksporten (ingen kilde siden v0.15) —
  [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §R1. Kjernen kan fortsatt skrive dem;
  en framtidig kapittelkilde trenger bare å fylle lista.
