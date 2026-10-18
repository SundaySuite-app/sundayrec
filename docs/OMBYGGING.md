# Ombyggingen — store grep før SundayRec rulles ut

_Skrevet 2026-10-11, lest mot koden på nytt 2026-10-12 ved v0.25.3. Status:
**plan, ikke startet.** Gjennomføres i nye økter lokalt på Macen, én PR per
økt. Øktregisteret — hvilke PR-er hver pakke blir, hvilken modell som skal
brukes, og en ferdig startmelding per økt — står i
[GJENNOMFORING.md](GJENNOMFORING.md)._

Dette er hovedplanen. Den samler alt fra gjennomgangen 2026-10-11 (biblioteker
utenfra, redigeringsprogrammer, lydmotoren og videomotoren) i én rekkefølge, med
arbeidspakker som kan tas én om gangen. To delplaner har de tekniske detaljene:

- [LYDMOTOR.md](LYDMOTOR.md): redigeringens lyd i Rust (pakkene B0–B6)
- [VIDEOMOTOR.md](VIDEOMOTOR.md): videoopptak med lyden først (pakkene C0–C5)

Bakgrunnen og lisensreglene står i [AVHENGIGHETER.md](AVHENGIGHETER.md).

**Eierens ramme:** det er få faste brukere ennå, og appen skal bli så god som
mulig før den rulles ut. Store og tøffe valg er ønsket, og risiko er akseptert.
Planen tar derfor valg som ellers ville vært for dristige: gamle stier slettes i
stedet for å leve videre som reserve, og innstillinger som ikke trengs lenger,
fjernes.

## De store beslutningene

| #   | Beslutning                                                                                                                                    | Konsekvens                                                                 |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| B1  | **ffmpeg brukes bare til filer:** dekoding, koding og muxing. Ikke til opptak fra enheter, og ikke til lydbehandling                          | All enhetsfangst og all lyd-DSP flyttes til Rust                           |
| B2  | **Én opptaksmotor** for lyd og video: den native (cpal → WAV), med kamera som et valgfritt spor                                               | Fire videostier og to ffmpeg-fangster slettes (11 000–14 000 linjer, målt) |
| B3  | **Lydklokka er sjefen.** Video følger lyden                                                                                                   | Et kamera som faller ut, tar aldri lyden med seg                           |
| B4  | **Kode utenfra tas inn i repoet, låst:** filmcrafts `audio-dsp` (MIT/Apache) og Caps kamera-crates (MIT). Mac-kameraet portes til objc2       | `UPSTREAM.md` per crate. Oppdateringer er egne PR-er                       |
| B5  | **Ingen reserve etter godkjenning.** Når en ny vei er godkjent på riggen, slettes den gamle i samme milepæl, ikke etter en overgangsutgivelse | Mindre kode å vedlikeholde. Feil rettes framover                           |
| B6  | **Nødlukene fjernes:** `classic_ffmpeg_audio`, `classic_ffmpeg_preroll` og `classic_directshow`                                               | Gamle innstillingsverdier ignoreres stille, som andre utgåtte felt         |
| B7  | **«Tale» renser av seg selv:** brum, støy, romklang og klikk, uten nye knapper                                                                | Lyden endrer seg for alle. Ørene må godkjenne                              |
| B8  | **Maskinvarekoding på begge plattformer**, med programvare som reserve                                                                        | Mindre CPU-last på kirke-PC-er                                             |
| B9  | **cpal 0.18 først**, fordi begge motorene bygger på den                                                                                       | Én API-overgang i stedet for to                                            |
| B10 | **VAD bytter motor:** `earshot` (ren Rust, uten modellfil) erstatter tract + Silero, fortsatt i skyggemodus                                   | `tract-onnx`, modellen på 2,8 MB og byggesjekken forsvinner                |

## Rekkefølge og milepæler

Hver milepæl er en beta-utgivelse, en riggdag og så stabil. Pakkene innenfor en
milepæl tas i rekkefølge. Hver pakke er én PR (store pakker kan deles).

| Milepæl                          | Pakker                 | Hvorfor i denne rekkefølgen                                                                        |
| -------------------------------- | ---------------------- | -------------------------------------------------------------------------------------------------- |
| **M1 — Grunnmuren**              | A1, A2, A3, A4, A5, C0 | Lisensvakt før kode utenfra. cpal 0.18 før motorene. Rask gevinst for video på Windows med en gang |
| **M2 — Ny lyd i redigeringen**   | B0, B1, B2, B3         | Størst hørbar forbedring. Uavhengig av videoarbeidet                                               |
| **M3 — Ny videomotor**           | C1, C2, C3, C4, C5     | Størst forbedring i robusthet. Bygger på A3 og A4                                                  |
| **M4 — Sanntid, fart og resten** | B4, B5, B6, D1, D2, D3 | B6 trenger nøkkelbildene fra C0 og lydkjeden fra B2                                                |

**Versjonsnummer:** M1 til M3 er 0.26 til 0.28. Når M4 er stabil og
riggpunktene er grønne, er appen klar for **1.0** og bred utrulling.

**M2 og M3 kjøres parallelt** (eiers beslutning 2026-10-12). Berøringsflatene er
disjunkte: lyden bor i `crates/sundayrec-core` og `app/editor`, videoen i
`src-tauri/src/{camera,recorder}` og `app/ui/CameraPreview`. Den eneste
koblingen er at B6 trenger nøkkelbildene fra C0 og lydkjeden fra B2 — og B6
ligger i M4 uansett. Prisen er to betaringer i luften samtidig, og at
`CHANGELOG.md` og `PLAN.md` blir de små konfliktpunktene mellom sporene.
Arbeidstrærne og rekkefølgen innenfor hvert spor står i
[GJENNOMFORING.md](GJENNOMFORING.md) §Rekkefølge.

## Arbeidspakkene

Hver pakke har mål, hva som endres og slettes, og når den er ferdig.
Størrelse: S (en dag), M (noen dager), L (en uke eller mer).

### A — Grunnmuren

**A1 — Lisensvakt og tredjepartsmerknader (S)**

- `deny.toml` med tillatte lisenser og ingen GPL/AGPL i `Cargo.lock`.
  Lista er målt mot treet 2026-10-12, ikke gjettet: MIT, Apache-2.0,
  **Apache-2.0 WITH LLVM-exception** (6 crates, blant dem `rustix`),
  BSD-2/3, ISC, Zlib, Unicode-3.0, **CDLA-Permissive-2.0**
  (`webpki-root-certs`), og MPL-2.0 bare som avhengighet (`cssparser`,
  `selectors`, `option-ext` m.fl.). `0BSD`, `CC0-1.0`, `MIT-0`, `Unlicense`
  og `LGPL-2.1-or-later` trengs **ikke** på lista: de står alle i et
  `OR`-uttrykk sammen med MIT eller Apache-2.0 (`adler2`, `dunce`,
  `aho-corasick`, `memchr`, `walkdir`, `r-efi` …), og cargo-deny velger det
  tillatte alternativet. `cargo deny check licenses bans` kjøres i
  `audit`-jobben i `ci.yml`. Tilsvarende sjekk for npm-avhengighetene
  (`license-checker --onlyAllow`).
- `cargo about generate` lager `THIRD-PARTY-NOTICES.html`. Fila bundles og
  lenkes fra System-siden («Lisenser»). ffmpeg nevnes med GPL og hvor kildene
  finnes, slik `DISTRIBUTION.md` beskriver.
- **Ferdig når:** CI feiler på en GPL-crate (test med en midlertidig
  avhengighet), og lisenssiden vises i appen.

**A2 — Dokumentasjonen sier hvilken ffmpeg som følger med (S)**

- `scripts/fetch-ffmpeg.mjs` henter **9.0.1** (siden v0.21.0, 2026-09-28), men
  `DISTRIBUTION.md`, `SMOKE-TEST.md`, `NEEDS-RICHARD.md` og kommentarene i
  `mastering.rs` sier 8.1.2, og `DISTRIBUTION.md` sier «Not 9.0». Rett
  tekstene. Mål på nytt de `loudnorm`-påstandene i `mastering.rs` som fortsatt
  gjelder til B1.
- Ny påstand i `docs/docs-truth.test.ts`: versjonen i `DISTRIBUTION.md` er lik
  `FFMPEG_VERSION` i `fetch-ffmpeg.mjs`.
- **Ferdig når:** testen er grønn og fanger en endring av den ene uten den andre.

**A3 — cpal 0.17.3 → 0.18.x (M–L, rigg)**

- Detaljene står i [AVHENGIGHETER.md](AVHENGIGHETER.md) §cpal. Felles
  `Error`/`ErrorKind`, eksplisitt `start()`, konfigurasjon som verdi, ny
  `StreamInstant`.
- Knytt de nye feiltypene til gjenkoblingen: `DeviceNotAvailable` og
  `StreamInvalidated` (frekvensbytte på Mac) gir gjenkobling, `DeviceBusy` gir
  nytt forsøk, og `PermissionDenied` er fatal med riktig melding.
- Xruns fra `CallbackInfo` telles i den native motoren og vises i telemetri og
  soak. Det erstatter tolkningen av ffmpeg-stderr der den native motoren
  brukes.
- Les `audio::com_anchor` på nytt mot den nye WASAPI-koden.
- **Ferdig når:** `cargo test --workspace` er grønt på tre plattformer, og
  RIG-DAY (a), (f) og (h) er grønne på Mac og Windows, pluss et 24-bit-lydkort
  på Windows.

**A4 — Windows: lytter på endringer i enhetslista (S–M, rigg)**

- `IMMNotificationClient` i `audio/device_watch.rs`, med `#[implement]` fra
  `windows` 0.62.2. Den er i `Cargo.lock` i dag, men **transitivt via cpal** —
  `src-tauri` har bare `windows-sys` direkte. A4 erklærer derfor `windows` som
  en direkte, **eksplisitt pinnet** avhengighet under
  `[target.'cfg(windows)'.dependencies]` med `implement`- og
  `Win32_Media_Audio`-featurene. Pinningen er poenget: uten den kan en cpal-bump
  flytte `windows` under føttene på vtabellen vår. Det er også grunnen til at
  A4 kommer **etter** A3. Forbildet er cpals `DefaultDeviceMonitor`
  (Apache-2.0, krediteres i modulhodet). Lytteren gjør bare én ting: dytter
  samme `Notify` som macOS-halvdelen.
- Samme lytter melder kameraendringer (`KSCATEGORY_VIDEO_CAMERA` via
  `CM_Register_Notification`) for C5.
- Rett modulhodet: begrunnelsen om at `windows`-cratet måtte dras inn, stemmer
  ikke lenger.
- **Ferdig når:** RIG-DAY (a) på Windows viser at back-offen kuttes når mikseren
  settes inn igjen.
- **Rettet i A4:** `windows` 0.62 har ingen `implement`-feature — makroen er
  alltid med, men ekspanderer til `::windows_core::…`, så `windows-core` må også
  erklæres direkte. Begge er pinnet med `=0.62.2` (samme som cpal 0.18.2), med
  featurene `Win32_Foundation`, `Win32_Media_Audio` og `Win32_System_Com`, som
  cpal alt slår på — ingen ny kode kompileres, `Cargo.lock` får bare to nye
  kanter. Kamerameldingene (`CM_Register_Notification`) ligger fortsatt i C5a,
  ikke i denne lytteren. Lytteren dytter bare ved _ankomst_ (ny enhet, tilstand
  → `ACTIVE`, ny standard opptaksenhet); fjerning dytter ikke. RIG-DAY-punktet
  er (a4) under Windows-boksen.

**A5 — Lydnødlukene fjernes (S–M)**

- `classic_ffmpeg_audio` og `classic_ffmpeg_preroll` slettes, med all kode som
  bare de når: ffmpeg-fangst av rene lydopptak og ffmpeg-pre-rollen. Den native
  motoren har vært standard siden 2026-08-01. `classic_directshow` følger i C3.
- **Flytt før du sletter.** `recorder/preroll.rs` er den klassiske
  ffmpeg-pre-rollen og kan dø her — men #337 la `sweep_stale_preroll_files()`
  inn i den fila, og sveipen rydder `sundayrec-preroll-*.wav` for **begge**
  motorene. Den flyttes til `native_capture/preroll.rs` i samme PR, med testen
  sin. Slettes den med fila, kommer de lekkede WAV-ene tilbake.
- **`cpal_capture.rs` blir stående.** Modulhodet sier det selv: fila tjener både
  `classic_ffmpeg_audio` og Windows' videoøkter, og er «HARDENED, not deleted»
  til den native motoren får video. A5 fjerner bare luka; fila dør i C3.
- Eierpunktet om å fjerne nødluka for pre-roll er avgjort: det ble strøket fra
  både `PLAN.md` og `NEEDS-RICHARD.md` i v0.25.3, mens koden står igjen. A5
  rydder opp etter beslutningen.
- **Ferdig når:** innstillingene leses fortsatt uten feil fra en gammel database
  (test), og ingen kodevei når ffmpeg-fangst av lyd uten video.
- **Rettet i A5b:** `classic_directshow` nådde også lyd uten video (på Windows
  tvang den ffmpeg), så den slutter å styre lydøkter allerede her — den styrer
  bare videoøkter til C3 sletter den. Det samme gjelder reserveveien: den
  automatiske fallbacken fra native til ffmpeg ved startfeil er fjernet, ikke bare
  bryteren. Soak-harnessen (`soak/`, `soak.yml`) og fault-injection-testene i
  `recorder/longrun.rs` kjører fortsatt ffmpeg-lydargv mot en lavfi-kilde; det er
  testkode, ikke en vei appen kan ta, og den må skrives om til den native
  motoren (egen økt) før `build_unified_capture_args` kan miste lydgrenen.
- **Gjort i C3b:** `classic_directshow`, `cpal_capture.rs` og hele
  `build_unified_capture_args` er slettet (videoøktene går gjennom den native
  motoren). Lavfi-soaken (`soak.yml`) er pensjonert, og `longrun.rs` bruker en
  ren lavfi-WAV-skriver som stedfortreder for fangstbytes — se VIDEOMOTOR §C3
  «Levert (C3b)».

### B — Lydmotoren (detaljer i [LYDMOTOR.md](LYDMOTOR.md))

| Pakke | Fase i LYDMOTOR.md                     | Størrelse |
| ----- | -------------------------------------- | --------- |
| B0    | Fase 0 — ta inn filmcrafts `audio-dsp` | S         |
| B1    | Fase 1 — lydstyrke i Rust              | M         |
| B2    | Fase 2 — lydkjeden i Rust              | L         |
| B3    | Fase 3 — «Tale» renser av seg selv     | M         |
| B4    | Fase 4 — én dekoding når fila åpnes    | M         |
| B5    | Fase 5 — forhåndslytting i sanntid     | L         |
| B6    | Fase 6 — rask videoeksport             | M         |

### C — Videomotoren (detaljer i [VIDEOMOTOR.md](VIDEOMOTOR.md))

| Pakke | Hva                                                       | Størrelse |
| ----- | --------------------------------------------------------- | --------- |
| C0    | Maskinvarekoding på Windows, nøkkelbilde hvert 2. s       | S         |
| C1    | Kamerafangst i Rust (Caps crates på Windows, port på Mac) | L         |
| C2    | Konformeren i `sundayrec-core`                            | M         |
| C3    | Den nye videoøkta, og de gamle stiene slettes             | L         |
| C4    | Forhåndsvisning fra samme kameraøkt                       | M         |
| C5    | Kameraet kobles til på nytt uavhengig av lyden            | M         |

### D — Resten

**D1 — VAD: `earshot` erstatter tract + Silero (M)**

- Ny `VadBackend` med `earshot` (MIT/Apache, 16 kHz, 256-samples rammer).
  Skyggemodusen og porten i [VAD.md](VAD.md) er uendret. Den får bare en
  annen motor.
- `tract-onnx`, `resources/vad/`, SHA-sjekken i `build.rs` og VAD-jobbens
  modellkontroll slettes. `vad`-featuren beholdes, men blir lett.
- **Ferdig når:** `ab_eval` kjører med den nye motoren, og VAD.md er skrevet
  om. Den ligger i `crates/sundayrec-core/src/ab_eval.rs`, med kjøreren i
  `src-tauri/examples/vad_ab_eval.rs` — ikke under `src-tauri/src/vad/`.

**D2 — macOS: leser systemet SundayRecs varsler? (S–M, Mac)**

- `UNUserNotificationCenter.getNotificationSettings` via
  `objc2-user-notifications`, bare når appen kjører som `.app`. Utenfor en
  bundle er svaret `unknown`, som i dag. Detaljene står i VARSLING.md
  §Gjenstår fra runde 2.
- Macen der planen gjennomføres, kan teste dette direkte.
- **Ferdig når:** «Hvem får beskjed?»-kortet viser riktig status med varsler av
  og på i Systeminnstillinger.

**D3 — Lydstyrke under opptak (M)**

- Måleren fra B0 kjører på den rutede f32-strømmen i den native motoren.
  Opptaksoverlegget viser kortsiktig LUFS ved siden av toppnivåene.
- Etter hvert opptak: integrert LUFS i historikken. Under −40 LUFS i mer enn
  5 minutter gir et varsel: «Opptaket er svært svakt — er mikseren dempet?»
- **Ferdig når:** et opptak med dempet mikser gir varselet, og et normalt
  opptak gir det ikke.

## Slik gjennomføres planen i en ny økt på Macen

Hver pakke er delt i økter — én PR per økt — i
[GJENNOMFORING.md](GJENNOMFORING.md). Der står også hvilken modell hver økt
skal kjøres med, og en ferdig startmelding.

**Før første økt:**

```bash
git clone https://github.com/SundaySuite-app/sundayrec && cd sundayrec
nvm use 22                         # CI kjører node 22; npm 11 ødelegger låsfila
npm install && npm run ffmpeg      # ffmpeg 9.0.1-sidecars, sjekksumverifisert
cargo install cargo-about --features cli   # A1b. UTEN --features cli
                                           # bygger den, men installerer
                                           # ingen binær, og sier det bare i
                                           # en advarsel
cargo test --workspace             # grønt før noe endres
```

**Maskinvaren som finnes (sjekket 2026-10-12):** Macen har innebygd FaceTime
HD-kamera og Continuity-kamera (nyttig: det kommer og går som en ekte
AVFoundation-enhet, så C5 og A4s macOS-halvdel kan prøves uten å dra en kabel),
men bare 1-kanals innebygd mikrofon. Til riggdagen finnes USB-webkamera,
flerkanals mikser og et 24-bit-lydkort på Windows. **Det finnes ikke noe
HDMI-opptakskort** — C1s opptakskort-punkt står derfor åpent, ikke
blokkerende, og Caps DirectShow-reserve blir kodegransket og kompilert, aldri
kjørt, til et kort finnes.

**Per pakke:**

1. Ny gren fra `main`: `ombygging/<pakke>-<kort-navn>`, for eksempel
   `ombygging/a3-cpal-018`.
2. Les pakken her, delplanen den peker til, og modulhodene til filene som
   endres. Modulhodene i dette repoet er dokumentasjonen.
3. Kjernebeslutninger går i `sundayrec-core` med tester. Skallet i `src-tauri`
   gjør bare I/O, som i resten av kodebasen.
4. Porter: `npm run check`, `cargo test --workspace`, og
   `bash scripts/ci-local.sh` før push.
5. Oppdater i samme PR: modulhodene, `SMOKE-TEST.md` og `RIG-DAY.md` med nye
   riggpunkter, `CHANGELOG.md` under «Uutgitt», og fjern pakkens linje i
   `PLAN.md` når den er ferdig.
6. Commit-meldinger på norsk, med Conventional Commits (`feat(recorder): …`).
7. Det Macen kan teste selv (mikrofon, kamera, eksport, lytting), testes i
   økta. Windows testes av `windows-check`-jobben i CI og på Windows-boksen
   ved milepælens riggdag.

**Kode utenfra (B0, C1):**

| Kilde                                | Commit ved gjennomgangen                   | Hva                                                                  |
| ------------------------------------ | ------------------------------------------ | -------------------------------------------------------------------- |
| `storytold/filmcraft`                | `0800a8b0aa4a703ba42172f415b45a61f8d8a5e1` | `crates/audio-dsp` (+ `tests/loudness_oracle.rs`)                    |
| `CapSoftware/Cap`                    | `a32e4806b3004071effe00243e1dabd52bc7068c` | `crates/camera*` (MIT). **Ikke** `mediafoundation-utils` eller annet |
| `pykeio/earshot` (crates.io `1.2.2`) | `ffd32d426f556c55a8c84490c12745743ee4bd79` | Som vanlig avhengighet, ikke kopi                                    |

Velg nyeste commit når pakken starter. Les endringene siden commitene over, og
skriv den valgte commiten i `UPSTREAM.md`.

**Startmelding til en ny økt** (lim inn og bytt pakke):

> Les `docs/OMBYGGING.md` og gjennomfør pakke **A3**. Følg «Slik gjennomføres
> planen», og lag én PR. Spør meg bare hvis noe i planen viser seg å være
> feil eller umulig. Da oppdaterer du planen i samme PR.

## Det som ikke er med

- **Helt native videokoding** (VideoToolbox/NVENC uten ffmpeg). Se VIDEOMOTOR.md
  §Notert.
- **Klippliste til DaVinci/Premiere** og **Intel/universal Mac-bygg**. Står
  i PLAN.md og AVHENGIGHETER.md.
- **Varsel til mobil**, **kapitler** og **video-pre-roll**. Bevisst ikke tatt.
