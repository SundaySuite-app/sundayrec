# Kode og biblioteker utenfra — hva vi kan ta inn, og hva som står på lista

_Gjennomgang: 2026-10-11, lest mot koden på nytt 2026-10-12 ved v0.25.3._

Dette dokumentet svarer på to spørsmål: **hva har vi lov til å ta inn** fra
andres prosjekter, og **hva er verdt det**. Det åpne står med én linje i
[`PLAN.md`](PLAN.md); detaljene står her.

## Regelen

Offentlig på GitHub er ikke det samme som fritt å bruke. Det er **lisensen**
som bestemmer, ikke at koden er synlig, og heller ikke at den er skrevet av en
KI-agent.

| Lisens                                       | I SundayRec (MIT)?                                                                  |
| -------------------------------------------- | ----------------------------------------------------------------------------------- |
| MIT, Apache-2.0, BSD, ISC, Zlib, IJG         | ✅ Ja. Opphavsmerknaden og lisensteksten skal følge med.                            |
| MPL-2.0 (f.eks. Symphonia)                   | ⚠️ Som avhengighet, ja. Kopiert inn i våre filer blir de filene MPL.                |
| GPL, AGPL                                    | ❌ Nei. Hele appen måtte blitt GPL/AGPL. (ffmpeg går fordi det er en egen prosess.) |
| Ingen lisens, «all rights reserved»          | ❌ Nei. Alle rettigheter er forbeholdt, uansett hvor synlig koden er.               |
| Blandet (f.eks. Cap: AGPL + noen MIT-crates) | Bare de delene som faktisk er MIT, og bare etter at det er sjekket fil for fil.     |

I tillegg:

- **Avhengighet før kopi.** Et vedlikeholdt crate fra crates.io er bedre enn
  kode limt inn fra en app: vi får rettelsene deres, og lisensen står i
  `Cargo.toml`.
- **Mønster før kopi.** Det vi lærer av andres kode skrives på nytt, med tester,
  slik Electron-appen ble portert — «rebuilt cleanly, not copied» (README).
- **Kopierer vi likevel,** får fila en linje øverst om hvor koden kommer fra,
  hvilken lisens den har og hvem som har opphavsretten.
- **Unntaket: filmcrafts `audio-dsp`** (eiers beslutning 2026-10-11). Hele
  craten tas inn i repoet, låst til én commit, med lisensfilene og `NOTICE`,
  en `UPSTREAM.md` over lokale endringer, og testene deres i vår CI. Den
  følges ikke fortløpende oppstrøms. Se [LYDMOTOR.md](LYDMOTOR.md).
- **KI-skrevne prosjekter er ekstra usikre.** Vi vet ikke hvor koden kommer fra
  (den kan inneholde GPL-kode under en MIT-fil), og de er ofte svakt testet.
  En fork kan dessuten bære en annens opphavsrett (ClawScribe er Meetily med
  Zackriya Solutions som opphavsrettshaver).
- **Lisenser endres.** screenpipe var MIT og er nå kommersiell. Det er
  lisensen i den versjonen vi faktisk henter, som gjelder.

## Gjennomgangen 2026-10-11

Sett på: Rust/Tauri-appene Handy, Meetily, anarlog (tidligere Hyprnote, bare
`crates/`), Mimi, Vibe, et knippe små opptaksapper fra 2026, og bibliotekene
cpal, ebur128, earshot, ffmpeg-sidecar og keepawake.

**Konklusjonen:** appene har lite å gi. SundayRec er lenger fremme på
krasjsikkerhet (Meetily holder 30 s i minnet før noe skrives), gjenkobling
(Meetily spør hvert 2. s, anarlog har ingen Windows-lytter) og tester. Det som
er verdt noe, ligger i **bibliotekene** — særlig i cpal, som vi allerede
bruker.

## Åpne punkter

Alle punktene her er nå arbeidspakker i [OMBYGGING.md](OMBYGGING.md), som
bestemmer rekkefølgen. Detaljene står fortsatt her.

### cpal 0.17.3 → 0.18.x (pakke A3)

Vi er låst på 0.17.3 (`src-tauri/Cargo.toml`, for `I24` og ASIO-rettelsene).
0.18.0 (2026-06) og 0.18.2 (2026-08) har det vi ellers måtte bygge selv:

- **Xruns meldes fra lydsystemet** (CoreAudio, WASAPI-opptak): ekte tall for
  hakk i opptaket, i stedet for det `soak/` og `test_recording/` utleder fra
  stderr.
- **macOS: endret samplingsfrekvens midt i et opptak** gir
  `StreamInvalidated`. I dag fanges et frekvensbytte bare over en
  gjenkobling (`recorder/engine/supervisor.rs`). Med 0.18 ville det gå
  gjennom samme vei.
- **ASIO:** to vranglåser når en strøm lukkes eller bygges, og lasting av en
  driver mens den forrige fortsatt lastes ut.
- **WASAPI:** `drop`/`play`/`pause` får ikke panikk lenger når enheten
  forsvinner. Det er akkurat situasjonen gjenkoblingen står i.
- **WASAPI 24-bit** var feil i 0.17.3 (feil formatbeskrivelse og ingen
  MSB-forskyvning; cpal-PR #1226 og #1309). Vi er **trolig ikke rammet**:
  forhandlingen i `native_capture/stream.rs` velger F32 foran I24, og WASAPI
  i delt modus tilbyr F32. Det skal likevel sjekkes på riggen med et
  24-bit-lydkort.

Pris: 0.18 endrer API-et (én felles `Error`/`ErrorKind`, strømmer starter
ikke av seg selv, `build_*_stream` tar konfigurasjonen som verdi,
`StreamInstant` er nytt). cpal er nevnt i over 30 filer i `src-tauri`, og
`audio::com_anchor` må leses på nytt mot den nye WASAPI-koden.

**Målt i A3b mot 0.18.2-kilden (2026-10-12):**

- «Standardkonfig foretrekker 48 kHz» (0.18-changeloggen) gjelder **ikke** våre
  tre verter: CoreAudio leser fortsatt enhetens strømformat, WASAPI
  `GetMixFormat` og ASIO driverens rate. «Auto = enhetens egen rate» står seg;
  ingenting er pinnet (begrunnelsen står ved `negotiate()`).
- **Nytt på macOS:** når en strøm bygges, setter cpal enhetens _fysiske_ format
  (bitdybde + rate) hvis det finnes et som passer, ellers bare raten. Raten satte
  den også i 0.17; bitdybden er ny. Riggpunkt i [RIG-DAY](RIG-DAY.md) (a).
- Feilklassene og hva gjenkoblingen gjør med dem: `sundayrec_core::stream_fault`.

_Når:_ egen PR, fulgt av en riggrunde på begge maskiner: gjenkobling
([RIG-DAY](RIG-DAY.md) (a)), ASIO (f) og et helt 90-minutters opptak (h).

### Windows: lytter på endringer i enhetslista (pakke A4) — levert

`audio/device_watch.rs` har nå en Windows-halvdel (`IMMNotificationClient`) ved
siden av macOS-lytteren. Den gamle begrunnelsen for at Windows manglet — at det
krevde enten en håndskrevet COM-vtabell eller det mye større `windows`-cratet —
holdt ikke lenger: `windows` 0.62.2 lå allerede i `Cargo.lock` fordi cpal 0.18.2
bruker det. A4 erklærer `windows` og `windows-core` **direkte**, pinnet med
`=0.62.2` (samme versjon som cpal), slik at en cpal-oppgradering ikke kan flytte
versjonen under vtabellen vår uten at noen ser det. Featurene
(`Win32_Foundation`, `Win32_Media_Audio`, `Win32_System_Com`) er de cpal alt slår
på, så ingenting nytt kompileres. `windows` 0.62 har ingen `implement`-feature;
makroen trenger i stedet `windows-core` som direkte avhengighet.

**Ved neste cpal-oppgradering:** sjekk `cargo tree -i windows --target
x86_64-pc-windows-msvc`. Flytter cpal seg til en annen `windows`, flyttes begge
pinnene i samme PR — ellers får treet to kopier.

Forbildet er cpals `DefaultDeviceMonitor` i `src/host/wasapi/stream.rs`
(Apache-2.0), kreditert i modulhodet. Begge fellene derfra er håndtert:
lytteren avregistreres aldri i drift (ingen vranglås mot en pågående
tilbakekalling), og tilbakekallingen eier ikke noe rått håndtak som kan lukkes —
feiler en avregistrering (bare i tester), lekkes klienten heller enn å slippes.

_Gjenstår:_ bevises på Windows-boksen med [RIG-DAY](RIG-DAY.md) (a4): trekk ut og
sett inn mikseren, og se at back-offen kuttes.

### Lisensvakt i CI og tredjepartsmerknader (pakke A1)

**Lisensvakten er på plass (A1a).** `deny.toml` har lista over tillatte
lisenser, og `audit`-jobben i `ci.yml` kjører `cargo deny check licenses bans`
og `npm run licenses:npm` (`license-checker --onlyAllow`). Et GPL- eller
AGPL-crate i `Cargo.lock`, eller en GPL-pakke i `package-lock.json`, gjør
jobben rød. Lokalt: `cargo deny check licenses bans` og `npm run licenses:npm`.
npm-lista er litt videre enn Rust-lista fordi verktøytreet (kun dev) har
`Python-2.0` (argparse), `BlueOak-1.0.0` (minimatch), `CC0-1.0` og `CC-BY-3.0`
(SPDX-datafiler under `license-checker` selv); ingen av dem er kopileft.

MIT og Apache krever også at merknadene følger med det vi distribuerer.
Installasjonsfilene har ingen samlet liste over tredjepartslisenser i dag.
En generert fil (f.eks. med `cargo about`) i bunten, og en lenke til den fra
appen, ville dekke det.

_Når:_ kan gjøres uten rigg. Hvor lista skal vises i appen, er eiers valg.

### macOS: leser systemet SundayRecs varsler? (pakke D2)

Punktet eies av [VARSLING.md](VARSLING.md) §Gjenstår fra runde 2. Fra
gjennomgangen: ingen av appene over løser det. Veien er
`objc2-user-notifications` (samme objc2-familie som `src-tauri` alt bruker),
ikke Swift-pakken anarlog bruker til mikrofontillatelsen.

### Redigeringsprogrammer (andre runde)

Sett på: filmcraft og soundcraft (ArtCraft), Clypra, katana, Gausian, OpenCut,
auto-editor og et knippe Tauri-redigerere. **filmcrafts `audio-dsp`** er
grunnlaget for [LYDMOTOR.md](LYDMOTOR.md). katanas raske videoeksport
(`-c:v copy`) er ideen bak fase 6 der, men koden er for enkel til å bruke.

### Videoopptak (tredje runde)

Sett på: Caps kamera-crates, nokhwa, crabcamera, filmcrafts `platform` og
ArtCraft-appene. ArtCraft har ingen kamerafangst. **Caps `cap-camera*`** (MIT,
uttrykkelig unntatt fra resten av Caps AGPL) er grunnlaget for
[VIDEOMOTOR.md](VIDEOMOTOR.md). Cap byttet selv bort fra nokhwa i 2025.
`cap-mediafoundation-utils` og alt annet i Cap er AGPL og skal ikke leses.

## Notert, ikke planlagt

- **`earshot` som VAD-motor** og **lydstyrke under opptak** er flyttet inn i
  planen som pakke D1 og D3 i [OMBYGGING.md](OMBYGGING.md). `earshot` erstatter
  tract + Silero. Påstanden om at den slår Silero er utviklerens egen, og sang
  kan bli tolket som tale, men VAD-en er i skyggemodus og bestemmer ingenting
  før porten i [VAD.md](VAD.md) er passert.
- **Klippliste til DaVinci/Premiere** (FCPXML/EDL, som auto-editor og
  filmcrafts `interchange` lager). _Når:_ et medieteam vil gjøre ferdig
  redigeringen et annet sted.

## Vurdert og lagt bort

| Hva                                                  | Hvorfor ikke                                                               |
| ---------------------------------------------------- | -------------------------------------------------------------------------- |
| `ffmpeg-sidecar`                                     | `progress.rs`/`levels.rs` og `fetch-ffmpeg.mjs` gjør mer, og er testet     |
| `keepawake`                                          | `power.rs` gjør jobben, med begrunnelse per plattform                      |
| Meetilys `incremental_saver`                         | Opptil 30 s i minnet før disk; vi skriver rett til fil                     |
| Meetilys og anarlogs enhetsovervåking                | Henholdsvis spørring hvert 2. s og ingen Windows-støtte                    |
| Små opptaksapper fra 2026 (Tauri + cpal)             | Svakere enn det vi har, få eller ingen tester                              |
| Cap, Whispering (Epicenter), autoshorts, AIMeeting   | AGPL, ingen lisens eller «all rights reserved»                             |
| filmcrafts kodeker, egui-UI og tidslinjemotor        | ffmpeg dekker kodekene (og har MP3/FLAC-koding); et annet problem enn vårt |
| Clypra                                               | Lenker ffmpeg statisk inn (`ffmpeg-next`); handler om videosammensetting   |
| Gausian, OpenCut                                     | KI-video med GStreamer; OpenCut skrives om fra bunnen                      |
| kerf, autocut                                        | PolyForm Noncommercial; ingen lisens                                       |
| OpenTake, chukcut, Video_For_Lazies, lumen-cut, tern | GPL-3.0, AGPL-3.0, «all rights reserved»                                   |
