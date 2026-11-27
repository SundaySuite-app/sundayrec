# Plan — SundayRec etter «rent bord»

_Sist gått gjennom: 2026-11-27. «Rent bord» (v0.31.0-beta.1, #464/#465) er på beta-ringen; **«én flate» (v0.32.0-beta.1)** bygger skjermene på nytt rundt den kjernen og er klar for beta-ringen. Stabil er fortsatt v0.30.1 til beta-søndagen er ren._

Én side som svarer på «hva er SundayRec nå, hva er bevisst fjernet, og hva er ikke
gjort?». Et punkt som ikke står her, er ikke planlagt. Kartet over alle
dokumentene er [`docs/README.md`](README.md).

## Hva SundayRec er nå

Kjernen i fem linjer:

1. **Ta opp** gudstjenesten — én stor knapp, krasjtrygg lydfangst rett til WAV, valgfritt kamera (bildet fyller flaten). Mens det tas opp viser skjermen bare opptaket, og det stoppes ved å holde den røde knappen i ett sekund.
2. **Planlegg** faste opptak i ukestripa nederst på Ta opp; maskinen vekkes, tar opp og stopper uten at noen trykker (med «+30» og «∞» på et planlagt stopp).
3. **Rediger** opptaket: bølgeform med klipp (dra, «Klipp», angre), forslag om stillhet som du godtar eller avviser, bildet for videoopptak, og «Podkastklar lyd» (−16 LUFS) som én bryter.
4. **Eksporter** fila, med intro og outro om du vil — appen laster ikke opp noe sted; fila på disk er overleveringen.
5. **Si fra** på maskinen når et opptak feiler eller blir stille — ingen e-post, ingen sky.

## To flater og et tannhjul («én flate», v0.32.0-beta.1)

| Flate            | Hva                                                                                                                                                                                                                                                                |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Ta opp**       | Kilde-brikka med L/R-måler (åpner **lydarket**: enhet, kanalpar, og under «Mer» mappe, kvalitet og flisene Før, Stille, Del, Rydd, Maks), den store knappen, kameraet, og **ukestripa** med Auto, Oppstart, Vekk og Varsel.                                        |
| **Opptaksmodus** | Bare opptaket: klokke, måler, «Hold» for å stoppe, stopplinja («Stopper 12:30» + «+30» + «∞», eller «Til du stopper»). Skjermleser får en bekreftelsesdialog i stedet for holdet.                                                                                  |
| **Opptak**       | Lista med mini-bølgeform; et klikk åpner redigeringen på samme side (bilde, klipp, stillhetsforslag). **Eksportarket** er et sideark: formatkort (MP3 «Podkast», WAV «Arkiv», MP4 «Video»), Podkast-, Intro- og Outro-flis. Papirkurven er et ikon oppe til høyre. |
| **Tannhjulet**   | Et kort over siden du kom fra: Språk (norsk/engelsk), Oppdatering (kanal, automatisk), Anonym statistikk, Om (logg, lisenser).                                                                                                                                     |

Første gang er tre hintbobler oppå Ta opp: lydkilde (med lydport), mappe (i «Mer»), faste opptak. Gamle lenker (`?goto=plan`, `settings:sound` …) lander på riktig flate; oversettelsen står øverst i [SMOKE-TEST.md](SMOKE-TEST.md).

## Bevisst fjernet i «én flate» — legg det ikke inn igjen uten en eierbeslutning

- **Plan-siden:** ukevisningen bor i stripa på Ta opp; en egen side for den er borte.
- **Innstillinger som side:** Lyd og kanaler, Kamera, Lagringsmappe og format, Varsler og Opptaksvalg er flyttet dit de brukes (lydarket, kameraknappen, stripa). Tannhjulet har bare fire rader.
- **FLAC som valg i eksportarket:** formatkortene er MP3 «Podkast», WAV «Arkiv» og MP4 «Video». Motoren kan fortsatt FLAC; ingen av eierens formater er det.
- **Stopp-dialogen «Stoppe opptaket?»** som standard: erstattet av «Hold» (ett sekund). Dialogen finnes bare igjen som veien for VoiceOver/Narrator, som ikke kan holde en knapp.
- **«+ 15 min» / «Avbryt» på planlagt stopp:** erstattet av «+30» og «∞».
- **Første-gang-veiviseren som sider:** erstattet av tre hint. Språk følger maskinen og velges i tannhjulet.
- **Eksportsiden og eksportdialogen:** eksporten er et sideark i redigeringen (var allerede borte som side siden «rent bord»).

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
- [ ] **Første gang på en fersk maskin:** alle tre hintene, inkludert lydporten med ekte lyd og mappedialogen, og at Tab når hintboblen mens lydarket står åpent.
- [ ] **Kamerabildet på svak Windows-PC:** forhåndsvisningen er 1280 px / 12 bilder i sekundet i hvile og 640 px / 4 under opptak. Mål CPU med kamera på, før og under et opptak, og se at ingen bilder hoppes over i opptaket (`PREVIEW_WIDTH`/`TARGET_FPS` i `src-tauri/src/camera/preview.rs` er knappene hvis tallene er for høye).
- [ ] **H.264-avspilling i redigeringen:** et videoopptak spilles i `<video>` i WKWebView (Mac) og WebView2 (Windows); spill, søk og pause følger spillehodet, og bildet står i takt med lyden etter klipp.
- [ ] **Hold-for-stopp med VoiceOver og Narrator:** knappen leses opp som «Hold inne for å stoppe», et klikk fra skjermleseren åpner bekreftelsen, Escape lar opptaket gå; og med tastatur holder Mellomrom/Enter i ett sekund.
- [ ] **Planlagt opptak med «+30» og «∞» på ekte utstyr:** en tid noen minutter fram; «+30» flytter slutten, «∞» gjør at det aldri stopper av seg selv, og Tab i opptaksmodus går bare mellom Hold, +30 og ∞.
- [ ] Øvrige riggpunkter som alt sto åpne fra v0.30.x (Mac-boksen, Windows-boksen, videorunden, klappetesten over 90 min, Bluetooth-forsinkelse på spillehodet) — se [RIG-DAY.md](RIG-DAY.md).

## Åpent — eier (Richard)

- [ ] Legg gjerne «macOS check + test» til de påkrevde sjekkene på `main` (den døde VAD-sjekken ble fjernet 2026-11-26; påkrevd nå: «Lint, typecheck & test», «Build Tauri app (no bundle)», «Playwright e2e (renderer harness)», «Windows check + test», «Dependency audit»).
- [ ] Notarisering: godta Apples oppdaterte avtale, sett `NOTARIZE_MAC=true` ([NEEDS-RICHARD](archive/NEEDS-RICHARD.md) §Release blockers).
- [ ] Windows-kodesigneringssertifikat (valgfritt; fjerner SmartScreen-advarselen).
- [ ] Sletting av lagrede adresser og rivingen av `notify.sundaysuite.app` i `sunday-telemetry`, når flåten er på v0.23.0 eller nyere.
- [ ] **Beta-søndag på v0.32.0-beta.1:** en ekte gudstjeneste på beta-bygget, gjennom `SMOKE-TEST.md` og helseporten i [RELEASE-CHECKLIST](RELEASE-CHECKLIST.md) §6a (dette slippet bygde om alle skjermene, og rører opptaksmodus, kamerabildet og redigeringen).
- [ ] Er den ren: slipp **v0.32.0** til stabil og beta (RELEASE-CHECKLIST §5g), med `.msi` på Windows-utkastet.

## Kjent og akseptert

- Intel/universal Mac-bygg finnes ikke (bare Apple Silicon).
- Videoopptak har en kort pause ved filbytte; rene lydopptak mister ingenting.
- Varsel til mobil (ntfy/Pushover) er ikke planlagt — se [VARSLING.md](VARSLING.md) §Senere.
- Testen `recorder::native_capture::segment::tests::split_gap` har en gang hengt mer enn 15 minutter i `cargo test --workspace`, men går på ca. 45 s alene. Alle `split_gap`-testene og rundt førti editor-/ffmpeg-tester deler ÉN prosess-global `ENV_LOCK` (en `std::sync::Mutex` som holdes over `.await`), så de kjører etter hverandre; en treg eller sultet kjøring ser ut som et hengt. Ikke gjenskapt i F7 (full gate på 282 s). Skjer det igjen: kjør `cargo test -p sundayrec --lib recorder::native_capture -- --test-threads=1` og se hvilken test som står.
