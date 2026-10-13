# Plan — hva gjenstår i SundayRec

_Sist gått gjennom: 2026-10-12, ved v0.25.3 (stabil og beta)._

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

Eierens beslutning 2026-10-11: store grep, høy risiko, gjennomføres i nye økter
lokalt på Macen. Hovedplanen er [OMBYGGING.md](OMBYGGING.md), med delplanene
[LYDMOTOR.md](LYDMOTOR.md) og [VIDEOMOTOR.md](VIDEOMOTOR.md). Øktene — én PR
hver, med porter, modellvalg og ferdig startmelding — står i
[GJENNOMFORING.md](GJENNOMFORING.md). Én linje per milepæl her; pakkene står i
hovedplanen og fjernes derfra når de er ferdige.

| Milepæl                      | Pakker       | Kort                                                                                       |
| ---------------------------- | ------------ | ------------------------------------------------------------------------------------------ |
| M1 — Grunnmuren              | —            | Alle pakker levert; venter på M1R-gjennomgangen                                            |
| M2 — Ny lyd i redigeringen   | B0–B3        | filmcrafts `audio-dsp` inn, lydstyrke og lydkjede i Rust, «Tale» renser av seg selv        |
| M3 — Ny videomotor           | C1–C5        | Kamera i Rust, lyden alltid gjennom den native motoren, én sti i stedet for fire           |
| M4 — Sanntid, fart og resten | B4–B6, D1–D3 | Én dekoding, sanntidslytting, rask videoeksport, earshot, macOS-varsler, LUFS under opptak |

M2 og M3 kjøres parallelt i egne arbeidstrær (eiers beslutning 2026-10-12).
**Blokkert på eieren:** B2c sletter de gamle filterstrengene først når Ørene har
hørt tre ekte gudstjenester, gammel lyd mot ny, og eieren har sagt ja.

## Kode

Kan gjøres uten eier eller rigg — men flere har en betingelse.

- Nedgradering etter en ny migrasjon ble avgjort 2026-10-04: en nyere database åpnes, og migrasjoner kan bare legge til — se modulhodet i [store.rs](../src-tauri/src/db/store.rs).
- Audit 2026-10-05 (rapport `SundayRec-AUDIT-2026-10-05.md` på suite-rota): 🔴/🟠 rettet i v0.25.2, 🟡 og de fleste 🔵/⚪ i v0.25.3 (#333–#342). Gjenstår: prekenvalget forkaster tale som starter før 5:00 (rapportlinje 93 — **eierbeslutning**, golden-testene låser gulvet), klokkehopp tømmer papirkurven (110, trenger lagret sist-kjørt-tid), planleggerens prune mot samtidig lagring (129, bare innsnevret), sidecar-stamme delt mellom a.mp3/a.wav og døde kapitler/`master_apply` (135/136/139), ASIO-kanalnavn og to-prosess-video uten reconnect/temp-manifest (151, 65), 10 kommandoer uten UI-kaller (85), og Opus-reviewens KAN-liste: `forgetWhatIsNowTrashed` på sti, `_deliver_tmp` synlig i lagringsmappa, preroll-terskel bare for macOS, soak-cache-sti, smalt vindu for patch etter import.

## Spike / maskinvare først

Kode, men bare etter et forsøk på ekte maskin.

| Punkt                                                                                                                                   | Kilde                                                            |
| --------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| macOS: lese om systemet viser SundayRecs varsler (i dag `unknown` + testvarsel) — pakke D2, se [GJENNOMFORING.md](GJENNOMFORING.md) §D2 | [VARSLING.md](VARSLING.md) §Gjenstår fra runde 2                 |
| Testvekkingens resultat (`test_ok`/`test_fail`) — krever et signal for at maskinen våknet                                               | [VARSLING.md](VARSLING.md) §Senere                               |
| Intel/universal Mac-bygg (i dag bare Apple Silicon; trenger ffmpeg-pinner for x86_64)                                                   | [archive/BACKLOG-AUDIT](archive/BACKLOG-AUDIT-2026-07-07.md) #17 |

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
