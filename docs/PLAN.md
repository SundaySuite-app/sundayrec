# Plan — SundayRec etter «rent bord»

_Sist gått gjennom: 2026-11-26. «Rent bord» er merget (#464) og sluppet som **v0.31.0-beta.1 på beta-ringen** (#465). Stabil er fortsatt v0.30.1 til beta-søndagen er ren._

Én side som svarer på «hva er SundayRec nå, hva er bevisst fjernet, og hva er ikke
gjort?». Et punkt som ikke står her, er ikke planlagt. Kartet over alle
dokumentene er [`docs/README.md`](README.md).

## Hva SundayRec er nå

Kjernen i fem linjer:

1. **Ta opp** gudstjenesten — én stor knapp, krasjtrygg lydfangst rett til WAV, valgfritt kamera.
2. **Planlegg** faste opptak i en ukevisning; maskinen vekkes, tar opp og stopper uten at noen trykker.
3. **Rediger** opptaket: bølgeform med to håndtak, klipp bort resten, «Podkastklar lyd» (−16 LUFS) som én bryter.
4. **Eksporter** fila, med intro og outro om du vil — appen laster ikke opp noe sted; fila på disk er overleveringen.
5. **Si fra** på maskinen når et opptak feiler eller blir stille — ingen e-post, ingen sky.

## De fire sidene

| Side              | Hva                                                                                                                                                |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Ta opp**        | Kilde og nivå, den store knappen, kameraet, linja om neste planlagte opptak. Ingenting annet.                                                      |
| **Opptak**        | Lista over opptakene; et klikk åpner redigeringen på samme side. «Eksporter» er et ark i redigeringen («Podkastklar lyd», intro/outro). Papirkurv. |
| **Plan**          | Ukevisning med dra-blokker, «Ta opp automatisk», «Start automatisk med maskinen», vekking av maskinen.                                             |
| **Innstillinger** | Lyd og kanaler, kamera, lagringsmappe og format, varsler, språk (norsk/engelsk), opptaksvalg, oppdatering, personvern, om og logg.                 |

Første gang er tre steg: lydkort og kanaler (med lydport), lagringsmappe, første faste opptak.

## Bevisst fjernet i «rent bord» — legg det ikke inn igjen uten en eierbeslutning

- **Læring:** ingen læring av rettelser, ingen prekensøk (D1/earshot og den nevrale VAD-en er avlyst), ingen forslag om hvor prekenen er.
- **Tillegg:** Diagnose med testopptak og målebenk, «Legg ut», kirkeprofil og kirkekalender, spesialopptak, eksport/import av innstillingsprofil, overføring fra Electron-appen, haptikk, bannerboksen om e-postvarsler.
- **Mastering:** bare én lydoppskrift — «Podkastklar lyd». Borte: «Tale og musikk», den avanserte mikseren, kanalreparasjon som valg.
- **Språk:** bare norsk og engelsk. Svensk, dansk, tysk, fransk og polsk er fjernet (lagrede verdier leses og ignoreres).
- **Sider:** kontrollrommet på Ta opp, Eksportsiden, «Sist redigert»-kortet, Avansert som egen flate, Sjekk maskinen i menylinja, sjekklisten «Klar til søndag», språk-/kvalitets-/varselsteg i første gang.
- **Kommandoer og kode:** seks kommandoer uten kaller (`editor_master_cancel`, `recording_update_note`, `scheduler_check_missed`, `telemetry_count`, `telemetry_queue_status`, `wake_get_sleep_config`), pausemekanismen for språk, `DecisionCard`.
- **Før dette igjen:** direkte strømming, sky/podkast-RSS/kanalhåndtrykk, whisper og AI-følgesvenn, e-postvarsler (fjernet i v0.14–R2).

Historikken ligger i [`archive/`](archive/): [OMBYGGING](archive/OMBYGGING.md),
[GJENNOMFORING](archive/GJENNOMFORING.md), [LYDMOTOR](archive/LYDMOTOR.md),
[VIDEOMOTOR](archive/VIDEOMOTOR.md), [APP-SHELL](archive/APP-SHELL.md),
[NEEDS-RICHARD](archive/NEEDS-RICHARD.md) og de andre.

## Åpent — rigg trengs

Ingenting av dette kan bevises i den headless gaten. Gå gjennom det på
[`RIG-DAY.md`](RIG-DAY.md) / [`SMOKE-TEST.md`](SMOKE-TEST.md) og før resultatet inn her.

- [ ] **Advarselen om død kanal** i editoren på ekte maskinvare (miksebord med en kanal som er av).
- [ ] **Eksportarket:** fokus går inn i arket og tilbake når det lukkes, i WKWebView (Mac) og WebView2 (Windows).
- [ ] **Intro/outro:** den native fildialogen for jingle (velg, fjern, borte fil ved eksport).
- [ ] **Ukevisningen i ekte Tauri** (ikke bare nettleserharnessen), også med berøring og penn.
- [ ] **Qu-5 / Windows ASIO:** et langt opptak på ekte kort, kanalrekkefølge og at dødkanalvarselet stemmer.
- [ ] **Planlagt start/stopp og vekking:** en tid noen minutter fram starter og stopper av seg selv, også fra hvilemodus (Mac: admin-ledetekst én gang; Windows: vekketimere).
- [ ] **Første gang på en fersk maskin:** alle tre stegene, inkludert lydporten med ekte lyd og mappedialogen.
- [ ] Øvrige riggpunkter som alt sto åpne fra v0.30.x (Mac-boksen, Windows-boksen, videorunden, klappetesten over 90 min, Bluetooth-forsinkelse på spillehodet) — se [RIG-DAY.md](RIG-DAY.md).

## Åpent — eier (Richard)

- [ ] Legg gjerne «macOS check + test» til de påkrevde sjekkene på `main` (den døde VAD-sjekken ble fjernet 2026-11-26; påkrevd nå: «Lint, typecheck & test», «Build Tauri app (no bundle)», «Playwright e2e (renderer harness)», «Windows check + test», «Dependency audit»).
- [ ] **Regenerer atlas-skjermbildene** (`npm run atlas`) — sidene, første gang (tre steg) og ukevisningen er endret siden forrige serie, og `docs/design/atlas/INDEX.md` peker på de gamle sceneid-ene.
- [ ] Notarisering: godta Apples oppdaterte avtale, sett `NOTARIZE_MAC=true` ([NEEDS-RICHARD](archive/NEEDS-RICHARD.md) §Release blockers).
- [ ] Windows-kodesigneringssertifikat (valgfritt; fjerner SmartScreen-advarselen).
- [ ] Sletting av lagrede adresser og rivingen av `notify.sundaysuite.app` i `sunday-telemetry`, når flåten er på v0.23.0 eller nyere.
- [ ] **Beta-søndag på v0.31.0-beta.1:** en ekte gudstjeneste på beta-bygget, gjennom `SMOKE-TEST.md` §beta-søndag og helseporten i [RELEASE-CHECKLIST](RELEASE-CHECKLIST.md) §6a (dette slippet endret opptak, editor og oppstart).
- [ ] Er den ren: slipp **v0.31.0** til stabil og beta (RELEASE-CHECKLIST §5g), med `.msi` på Windows-utkastet.

## Kjent og akseptert

- Intel/universal Mac-bygg finnes ikke (bare Apple Silicon).
- Videoopptak har en kort pause ved filbytte; rene lydopptak mister ingenting.
- Varsel til mobil (ntfy/Pushover) er ikke planlagt — se [VARSLING.md](VARSLING.md) §Senere.
- Testen `recorder::native_capture::segment::tests::split_gap` har en gang hengt mer enn 15 minutter i `cargo test --workspace`, men går på ca. 45 s alene. Alle `split_gap`-testene og rundt førti editor-/ffmpeg-tester deler ÉN prosess-global `ENV_LOCK` (en `std::sync::Mutex` som holdes over `.await`), så de kjører etter hverandre; en treg eller sultet kjøring ser ut som et hengt. Ikke gjenskapt i F7 (full gate på 282 s). Skjer det igjen: kjør `cargo test -p sundayrec --lib recorder::native_capture -- --test-threads=1` og se hvilken test som står.
