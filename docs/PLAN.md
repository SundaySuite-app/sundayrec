# Plan — hva gjenstår i SundayRec

_Sist gått gjennom: 2026-11-09. Stabil og beta er v0.29.0 (designrunden, rett til stabil etter eiers ordre)._

Én side som svarer på «hva er ikke gjort, og hvem sitt er det?». Hvert punkt
**bor i ett dokument** (kolonnen «Kilde»), med detaljene der; denne sida er
registeret som peker dit. Et punkt som ikke står her, er ikke planlagt.

**Slik holdes sida i live**

- Nytt åpent punkt → skriv det i kildedokumentet, og én linje her.
- Ferdig → marker det i kilden, og fjern linja her i samme PR.
- Ved hver utgivelse leses sida gjennom (`docs/RELEASE-CHECKLIST.md` §5g),
  og datoen øverst oppdateres.

Kartet over alle dokumentene er [`docs/README.md`](README.md).

## Ombyggingen — før appen rulles ut

Eierens beslutning 2026-10-11: store grep, høy risiko, gjennomført lokalt på
Macen. Hovedplanen er [OMBYGGING.md](OMBYGGING.md), med delplanene
[LYDMOTOR.md](LYDMOTOR.md) og [VIDEOMOTOR.md](VIDEOMOTOR.md); øktene (én PR
hver) står i [GJENNOMFORING.md](GJENNOMFORING.md).

**Kodepakkene M1–M4 er levert og merget** (PR-nummer fra `git log`). Det er ikke
det samme som ferdig: ingen milepæl har gått gjennom porten (merget → review →
beta → riggdag → stabil), og 0 av 84 riggpunkter er krysset av.

| Milepæl                      | Levert                                                                                                                                     | Utgitt                          |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------- |
| M1 — Grunnmuren              | A1 #349 #352, A2 #356, A3 #351 #354 #355, A4 #359, A5 #357 #358, C0 #350 #353; M1R #362 #365 #366                                          | v0.26.0-beta.1 (#378)           |
| M2 — Ny lyd i redigeringen   | B0 #367 #374, B1 #376 #382, B2a #375, B2b #388, B3 #391 #393; M2R #395–#397 #399                                                           | v0.27.0-beta.1 (#405), uten B2c |
| M3 — Ny videomotor           | C1 #368 #369 #371, C2 #373 #386, C3 #377 #384 #385, C4 #387 #390, C5 #394 #398; M3R #400–#402 #416                                         | v0.27.0-beta.1 (#405)           |
| M4 — Sanntid, fart og resten | B4 #403 #407, B5 #410 #412 #417, B6 #404, D2 #406 #409, D3 #408; M4R-rettelser #416 #420 #421 #422. **D1 er parkert, ikke levert** (under) | 0.28.0, rett til stabil         |

**Åpent fra ombyggingen** (én linje per punkt; detaljene bor i kilden):

| Punkt                                                                                                                                                                                                                                                                                                                                                                                                                         | Kilde                                                                 |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| **B2c** (sletter filterstrengene og `chain_ab`): venter på at **eieren lytter** på ORENE-B2b, tre gudstjenester × tre profiler, gammel mot ny, og sier ja. Runden har ingen avkrysningsboks i RIG-DAY ennå                                                                                                                                                                                                                    | [GJENNOMFORING.md](GJENNOMFORING.md) §B2c, [LYDMOTOR.md](LYDMOTOR.md) |
| **D1 (earshot) er parkert** til det finnes et ekte korpus (≥ 12 korrigerte opptak). Silero/tract beholdes; grenen `ombygging/d1-earshot-vad` (12a96b37) står umerget. Ikke en 1.0-sperre                                                                                                                                                                                                                                      | [GJENNOMFORING.md](GJENNOMFORING.md) §D1, [VAD.md](VAD.md)            |
| **Riggpunkter:** Mac-boksen, Windows-boksen, videorunden (v1)–(v10) (klappetesten over 90 min), B5 (a)–(c) med 90 min spillehode, (w) varselstatus, (d3-mac)/(d3-rigg), Ørene                                                                                                                                                                                                                                                 | [RIG-DAY.md](RIG-DAY.md)                                              |
| **Designrunden nov. 2026** (luft, fast handlingslinje, seksjonene i Innstillinger, ny tekst på alle sju språk; PR #425–#427, sluppet i v0.29.0 rett til stabil): **riggtest på en ekte Mac og en Windows-PC som etterkontroll av det flåten har fått** — Start/«Neste» synlige i handlingslinja i 1180×760, «Vis i Finder»/«Vis i Utforsker», Opptak-kortene med nye spørsmål, mikseren med hverdagsord, første gang steg 1–6 | [design/atlas/INDEX.md](design/atlas/INDEX.md) §Riggtest              |
| **Milepælsportene:** beta-søndag og så stabil, for M1–M4. Stabil er fortsatt 0.25.3                                                                                                                                                                                                                                                                                                                                           | [GJENNOMFORING.md](GJENNOMFORING.md) §Milepælsportene                 |
| **Eksporten krysstoner ikke som lyttingen** (lyttingen 20 ms, eksporten dukker 15 + 15 ms): rettes i en egen PR som pågår                                                                                                                                                                                                                                                                                                     | [LYDMOTOR.md](LYDMOTOR.md) §B5c                                       |
| **Spillehodet går foran lyden med utgangsforsinkelsen** (Bluetooth 150–250 ms): mistenkt, avgjøres av klappetesten                                                                                                                                                                                                                                                                                                            | [LYDMOTOR.md](LYDMOTOR.md)                                            |
| **M3R-rester:** #7 (cpal-latens på Mac, rigg), #13 (gjenopprettet videoopptak får ikke sidefila med lyd), #16 (konformeren viser bildet i snitt 0,5 bilde for sent), #17 (VIDEOMOTOR sier «summeres», målt er ett dobbelt bilde per grense), #19 (bilder før første lydpar kastes)                                                                                                                                            | [VIDEOMOTOR.md](VIDEOMOTOR.md)                                        |
| **Video-pre-roll:** ikke planlagt. Starten fylles med svart for pre-roll-lengden (M3R #8)                                                                                                                                                                                                                                                                                                                                     | [VIDEOMOTOR.md](VIDEOMOTOR.md) §C3                                    |
| Workeren lagrer ikke `nativeXruns` (flåten melder null hakk): migrasjon, `INSERT`, test, så Worker-deploy                                                                                                                                                                                                                                                                                                                     | `sunday-telemetry`                                                    |
| Plan-PR #344 står fortsatt åpen og bør lukkes                                                                                                                                                                                                                                                                                                                                                                                 | GitHub                                                                |

**Eierbeslutninger 2026-10-19:** D1 parkert (over). B6: videoeksporten kan starte
på nøkkelbildet inntil ca. 2 s tidligere enn lyttingen, og `-aac_coder fast` er
godtatt. D2: bannerstilen «Ingen» vises rødt. D3: bare stillhetsvarselet, og bare
når begge vilkårene gjelder; en påminnelse etter gjenkobling er i orden.

## Kode

Kan gjøres uten eier eller rigg — men flere har en betingelse.

- Nedgradering etter en ny migrasjon ble avgjort 2026-10-04: en nyere database åpnes, og migrasjoner kan bare legge til — se modulhodet i [store.rs](../src-tauri/src/db/store.rs).
- Audit 2026-10-05 (rapport `SundayRec-AUDIT-2026-10-05.md` på suite-rota): 🔴/🟠 rettet i v0.25.2, 🟡 og de fleste 🔵/⚪ i v0.25.3 (#333–#342). Gjenstår: prekenvalget forkaster tale som starter før 5:00 (rapportlinje 93 — **eierbeslutning**, golden-testene låser gulvet), klokkehopp tømmer papirkurven (110, trenger lagret sist-kjørt-tid), planleggerens prune mot samtidig lagring (129, bare innsnevret), sidecar-stamme delt mellom a.mp3/a.wav og døde kapitler/`master_apply` (135/136/139), ASIO-kanalnavn (151; to-prosess-videostien er slettet i C3b, #384), 10 kommandoer uten UI-kaller (85), og Opus-reviewens KAN-liste: `forgetWhatIsNowTrashed` på sti, `_deliver_tmp` synlig i lagringsmappa, preroll-terskel bare for macOS, soak-cache-sti, smalt vindu for patch etter import.

## Spike / maskinvare først

Kode, men bare etter et forsøk på ekte maskin.

| Punkt                                                                                                                          | Kilde                                                            |
| ------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------- |
| macOS: lese om systemet viser SundayRecs varsler (kode levert i D2; **venter på eierens prøve** med varsler av/på, RIG-DAY §w) | [VARSLING.md](VARSLING.md) §Gjenstår fra runde 2                 |
| Testvekkingens resultat (`test_ok`/`test_fail`) — krever et signal for at maskinen våknet                                      | [VARSLING.md](VARSLING.md) §Senere                               |
| Intel/universal Mac-bygg (i dag bare Apple Silicon; trenger ffmpeg-pinner for x86_64)                                          | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #17 |

## Eier (Richard)

| Punkt                                                                                                                | Kilde                                                           |
| -------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| **Notarisering:** godta Apples oppdaterte avtale, sett `NOTARIZE_MAC=true`                                           | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Release blockers, punkt 3 |
| Windows-kodesigneringssertifikat (valgfritt; fjerner SmartScreen-advarselen)                                         | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Signing                   |
| Rive `notify.sundaysuite.app` i `sunday-telemetry` og slette lagrede adresser — når flåten er på v0.23.0 eller nyere | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §PU-1                      |
| Eierbeslutninger fra F2 som står igjen: resten av språkfunnene                                                       | [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §Eierbeslutninger fra F2   |
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
videoopptak på Mac under CPU-last (e2). v0.25.2 (2026-10-05, også rett til stabil)
retter auditens 🔴/🟠-funn i planlegger, levering, innstillinger og opptaksøkt;
første ekte søndag på den er helseporten (ROLLBACK.md). v0.25.3 (2026-10-12, også rett
til stabil etter eiers ordre) retter auditens 🟡/🔵-funn; søndagen etter er helseporten.

## Ikke planlagt

Nevnt, bevisst ikke tatt:

- Varsel til mobil (ntfy/Pushover) — [VARSLING.md](VARSLING.md) §Senere.
- Kapitler i eksporten (ingen kilde siden v0.15) —
  [NEEDS-RICHARD.md](NEEDS-RICHARD.md) §R1. Kjernen kan fortsatt skrive dem;
  en framtidig kapittelkilde trenger bare å fylle lista.
