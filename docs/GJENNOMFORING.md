# Gjennomføringen — ombyggingen økt for økt

_Skrevet 2026-10-12, ved v0.25.3. Status: **klar til å startes.**_

[OMBYGGING.md](OMBYGGING.md) sier hva som skal bygges om og hvorfor.
Dette dokumentet sier **hvordan arbeidet deles opp**: 21 pakker blir 45 økter,
der én økt er én PR og cirka én arbeidsøkt. Hver økt har mål, hvilke filer som
endres eller slettes, porter, et «ferdig når» som kan sjekkes, hva Macen kan
bevise selv, hvilken modell den skal kjøres med, og en **ferdig startmelding**
du kan lime inn i en ny økt.

Delplanene har de tekniske detaljene: [LYDMOTOR.md](LYDMOTOR.md) for B-pakkene,
[VIDEOMOTOR.md](VIDEOMOTOR.md) for C-pakkene, [AVHENGIGHETER.md](AVHENGIGHETER.md)
for lisensreglene og bakgrunnen.

## Fellesreglene — gjelder alle 45 økter

Dette gjentas ikke per økt. Startmeldingene peker hit.

1. **Én gren, én PR:** `ombygging/<økt>-<kort-navn>`, fra `main`.
2. **Les modulhodene (`//!`) til filene du endrer, før du endrer dem.** De er
   dokumentasjonen i dette repoet. Er hodet feil etterpå, rettes det i samme PR.
3. **Kjernebeslutninger i `sundayrec-core`, med tester.** Skallet i `src-tauri`
   gjør bare I/O. Ingen klokke og ingen filsystem i kjernen.
4. **Porter før push:** `npm run check`, `cargo test --workspace`, og
   `bash scripts/ci-local.sh`. Den siste starter med `npm run ffmpeg` og slutter
   med et fullt `tauri build --no-bundle`, så den er treg — kjør den likevel.
5. **Node 22, ikke 24.** Kjør alt npm-arbeid med `nvm exec 22`. npm 11
   regenererer låsfila slik at CIs npm 10 dør på `npm ci`.
6. **Oppdater i samme PR:** modulhodene, [SMOKE-TEST.md](SMOKE-TEST.md),
   [RIG-DAY.md](RIG-DAY.md) med nye riggpunkter, `CHANGELOG.md` under «Uutgitt»,
   og stryk pakkens linje i [PLAN.md](PLAN.md) når hele pakken er ferdig.
7. **Commit-meldinger på norsk**, Conventional Commits (`feat(recorder): …`).
8. **Vent til hele CI-kjøringen er `completed`** før merge. Jobber med `needs:`
   vises ikke i `gh pr checks` før de starter, så «ingen pending» kan være løgn.
9. **Nytt eller omdøpt jobbnavn i `ci.yml` må inn i grenbeskyttelsen**, ellers
   henger alle PR-er i repoet.
10. **Spør eieren bare hvis noe i planen viser seg å være feil eller umulig.**
    Da oppdateres planen i samme PR.

**Steg 0, én gang før første økt:**

```bash
git pull
nvm use 22
npm install && npm run ffmpeg      # ffmpeg 9.0.1; Macen hadde 8.1.2 per 2026-10-12
cargo install cargo-about --features cli   # A1b. ⚠️ uten `--features cli`
                                           # bygger den ferdig og installerer
                                           # ingenting — advarselen er lett å
                                           # lese forbi (målt 2026-10-12,
                                           # cargo-about 0.9.2)
bash scripts/ci-local.sh           # se at v0.25.3 er grønn på denne maskinen
```

**Steg 0 var rød på en rask Mac (rettet).** Målt 2026-10-12 på v0.25.3 med ffmpeg
9.0.1: `cargo test --workspace` feiler på
`recorder::longrun::tests::killing_the_delivery_encode_keeps_the_whole_capture_and_retries`,
6 av 6 ganger, mens CI er grønn på samme commit (jobben «macOS check + test» i
run 38204469019 logger testen som `ok`). Alt før Rust-steget i `ci-local.sh` er
grønt.

Det er ikke en regresjon i appen. Testen beviser ikke det den sier:

1. Leveringen kodes til `<stem>_deliver_tmp.<ext>` og flyttes på plass med
   `rename` først når kodingen er validert (`concat::deliver_via_temp`,
   `delivery_tmp_path`). Leveringsfila kan altså ikke finnes mens kodingen går.
2. Testens killer venter på at `sermon.mp3` dukker opp og kjører
   `pkill -9 -f <save-dir>` da. Kodingen er ferdig, så `pkill` treffer ingenting.
3. Killeren melder likevel `killed = true`, fordi den ignorerer `pkill`-ens
   exitkode. SKIP-vakten kan derfor aldri slå inn, og `assert!(result.is_err())`
   feiler.
4. På en treg CI-løper rekker ikke `rename` innenfor killerens budsjett
   (2 000 × 2 ms), killeren returnerer `false`, og testen går grønn via
   SKIP-grenen uten noensinne å ha drept en koding. 1–3 er lest i koden; 4 er
   den eneste forklaringen som stemmer med grønn CI.

**Fikset i #348 (2026-10-12):** killeren venter på
`delivery_tmp_path(&delivery_path)` i stedet for `delivery_path`, og `killed`
følger `pkill`-ens exitkode. Feilen traff også Linux-løperne i CI, ikke bare
Macen. Dette er en
skjøtefeil av typen `docs/LEARNING.md` beskriver: temp+rename var riktig, testen
var riktig da den ble skrevet, de er uenige i skjøten, og CI ble grønn.

## Modellvalget

**Sonnet er standard.** Åtte økter krever Opus, og hver milepæl får i tillegg én
Opus-review (`M1R`–`M4R`) før betaslipp.

| Økt     | Hvorfor Opus                                                                         |
| ------- | ------------------------------------------------------------------------------------ |
| **A3b** | Skjøten mellom cpals nye feiltaksonomi og gjenkoblingsmaskinen, pluss COM-levetid    |
| **A4**  | Håndskrevet COM-grensesnitt via `#[implement]`; vranglås ved avregistrering          |
| **C1a** | `unsafe` FFI mot AVFoundation; levetid og trådeierskap                               |
| **C1b** | `unsafe` COM/Media Foundation, og en lisensgrense (AGPL) som ikke må krysses         |
| **C2**  | Konformeren: klokketilpasning og bildeplasser — en feil her er usynlig til riggdagen |
| **C3a** | Kjernen i opptaket: to klokker, to prosesser, krasjgjenoppretting                    |
| **C3b** | ~12 000 linjer ut uten reserve; det som fortsatt brukes må flyttes, ikke slettes     |
| **B5a** | Sanntids lydtråd med cpal som klokke; hakk og klikk kan ikke testes inn etterpå      |

Begrunnelsen for mønsteret: det er skjøtene og det uomgjørlige som trenger den
dyre modellen. Rene tillegg med gode tester gjør Sonnet like godt og billigere.

## Oversikt

| Milepæl | Økter                                            | Antall |
| ------- | ------------------------------------------------ | ------ |
| **M1**  | A1a A1b A2 A3a A3b A3c A4 A5a A5b C0a C0b + M1R  | 11 + 1 |
| **M2**  | B0a B0b B0c B1a B1b B2a B2b B2c B3a B3b + M2R    | 10 + 1 |
| **M3**  | C1a C1b C1c C2 C3a C3b C3c C4a C4b C5a C5b + M3R | 11 + 1 |
| **M4**  | B4a B4b B5a B5b B5c B6 D1 D2 D3 + M4R            | 9 + 1  |
|         |                                                  | **45** |

**Kolonnen «Hvor det bevises»** i øktene under bruker tre ord:

- **Macen** — kan bevises ferdig i økta, på denne maskinen.
- **CI** — bevises av `windows-check`-jobben; ingen maskin trengs.
- **Riggen** — må vente på Windows-boksen eller riggdagen. Står i
  [RIG-DAY.md](RIG-DAY.md) som et avkryssingspunkt.

---

# M1 — Grunnmuren (0.26)

Lisensvakt før kode utenfra. cpal 0.18 før motorene. Rask gevinst for video på
Windows med en gang.

## A1a — Lisensvakten i CI

**Modell:** Sonnet · **Gren:** `ombygging/a1a-lisensvakt` · **Pakke:** A1

**Mål:** Ingenting skal kunne dra inn et GPL- eller AGPL-crate via `Cargo.lock`
eller `package-lock.json` uten at CI sier fra.

**Endrer:** `deny.toml` (ny), `.github/workflows/ci.yml` (§`audit`),
`package.json` (`license-checker` i devDependencies + et `licenses:npm`-skript).

**Lista er målt, ikke gjettet.** Treet inneholdt per 2026-10-12 også
`CDLA-Permissive-2.0` (`webpki-root-certs`) og `Apache-2.0 WITH LLVM-exception`
(6 crates). Begge må på lista. `0BSD`, `CC0-1.0`, `MIT-0`, `Unlicense` og
`LGPL-2.1-or-later` skal **ikke** på lista: de står i et `OR`-uttrykk sammen med
MIT eller Apache-2.0, og cargo-deny velger det tillatte alternativet.

**Port:** `cargo deny check licenses bans` og `npm run licenses:npm`.

**Ferdig når:** begge er grønne på treet som det er, **og** en midlertidig
GPL-avhengighet får `audit`-jobben til å feile (prøv det, og fjern den igjen i
samme PR). Det nye jobbnavnet, hvis det får et, er lagt inn i grenbeskyttelsen.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A1a, pakke A1 i `docs/OMBYGGING.md` og
> `docs/AVHENGIGHETER.md` §Regelen. Gjennomfør økta som én PR på
> `ombygging/a1a-lisensvakt`. Følg fellesreglene i GJENNOMFORING §Fellesreglene.
> Lisenslista i A1 er målt mot treet — bruk den som den står, og ikke utvid den
> uten å si hvorfor. Bevis vakten ved å legge inn en GPL-avhengighet
> midlertidig, se CI feile, og fjerne den igjen i samme PR.

## A1b — Tredjepartsmerknadene i appen

**Modell:** Sonnet · **Gren:** `ombygging/a1b-lisenssida` · **Pakke:** A1

**Mål:** MIT og Apache krever at merknadene følger med det vi distribuerer.
Installasjonsfilene har ingen slik liste i dag.

**Endrer:** `about.toml` (ny), bunten (`tauri.conf.json` → ressurs),
System-sida (nytt «Lisenser»-kort), i18n-nøklene for alle sju språk,
`docs/DISTRIBUTION.md`.

**Port:** `npm run check` (i18n-vaktene fanger manglende nøkler),
`cargo about generate` kjører i `check`-jobben.

**Ferdig når:** `THIRD-PARTY-NOTICES.html` ligger i bunten, lenka fra System-sida
åpner den i appen, og ffmpeg står nevnt med GPL og hvor kildene finnes — slik
`DISTRIBUTION.md` beskriver.

**Hvor det bevises:** Macen (bygg en bundle og åpne sida).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A1b og pakke A1 i `docs/OMBYGGING.md`.
> Gjennomfør økta som én PR på `ombygging/a1b-lisenssida`. Følg fellesreglene.
> Lista vises på System-sida som et kort kalt «Lisenser» — det er avgjort, ikke
> et åpent valg. Alle sju språk skal ha nøklene. Bygg en bundle og bekreft at
> fila faktisk ligger i den før du sier ferdig.

## A2 — Dokumentene sier hvilken ffmpeg som følger med

**Modell:** Sonnet · **Gren:** `ombygging/a2-ffmpeg-versjon` · **Pakke:** A2

**Mål:** `fetch-ffmpeg.mjs` har hentet 9.0.1 siden v0.21.0 (2026-09-28), men
dokumentene og kommentarene sier fortsatt 8.1.2.

**Endrer** (stedene er verifisert 2026-10-12):

| Fil                                      | Hva                                             |
| ---------------------------------------- | ----------------------------------------------- |
| `docs/DISTRIBUTION.md`                   | 8.1.2 på 6 steder, og «**Not 9.0.**» som er død |
| `docs/SMOKE-TEST.md`                     | kommentaren i `npm run ffmpeg`-blokka           |
| `docs/NEEDS-RICHARD.md`                  | versjonsnevnelsen                               |
| `crates/sundayrec-core/src/mastering.rs` | 4 kommentarer                                   |
| `docs/docs-truth.test.ts`                | ny påstand                                      |

Mål samtidig på nytt de `loudnorm`-påstandene i `mastering.rs` som gjelder til
B1 — de ble målt på 8.1.2.

**Port:** `npm run test` (dokumentvakten ligger i `docs/`-mønsteret i
`vitest.config.ts`, så den kjører av seg selv).

**Ferdig når:** den nye påstanden er at versjonen i `DISTRIBUTION.md` er lik
`FFMPEG_VERSION` i `fetch-ffmpeg.mjs`, og testen **feiler** hvis bare én av dem
endres (prøv det).

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A2 og pakke A2 i `docs/OMBYGGING.md`. Gjennomfør
> økta som én PR på `ombygging/a2-ffmpeg-versjon`. Følg fellesreglene. Kjør
> `npm run ffmpeg` først, så du faktisk har 9.0.1 på maskinen, og mål
> `loudnorm`-påstandene i `mastering.rs` på nytt med den — ikke bare bytt tall.
> Bevis den nye dokumentvakten ved å endre én av de to versjonene og se testen
> feile.

## A3a — cpal 0.18: den mekaniske API-sveipen

**Modell:** Sonnet · **Gren:** `ombygging/a3a-cpal-018` · **Pakke:** A3

**Mål:** Få treet over på cpal 0.18.x uten å endre oppførsel. 0.18 samler
`Error`/`ErrorKind`, krever eksplisitt `start()`, tar konfigurasjonen som verdi i
`build_*_stream`, og har en ny `StreamInstant`.

**Endrer:** `src-tauri/Cargo.toml` (`cpal = "0.18"`) og de ~30 filene i
`src-tauri` som nevner cpal. Feilklassifiseringen holdes **semantisk identisk** —
den flyttes i A3b.

**Port:** `cargo test --workspace`, `cargo clippy -D warnings`,
`cargo clippy --no-default-features`, og `windows-check`-jobben.

**Ferdig når:** alt bygger og alle tester er grønne på macOS lokalt og på
Windows i CI, og PR-beskrivelsen sier eksplisitt at **ingen oppførselsendring er
tilsiktet**.

**Hvor det bevises:** Macen (bygg og test) + CI (Windows). Riggen kommer i A3c.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A3a, pakke A3 i `docs/OMBYGGING.md` og
> `docs/AVHENGIGHETER.md` §cpal. Gjennomfør økta som én PR på
> `ombygging/a3a-cpal-018`. Følg fellesreglene. Dette er en **ren
> API-migrasjon**: ingen oppførselsendring er tilsiktet, og feilklassifiseringen
> skal være semantisk identisk etterpå — den flyttes i A3b. Noter i PR-en hvilke
> steder du var i tvil om, så A3b kan ta dem.

## A3b — De nye feiltypene inn i gjenkoblingen

**Modell:** **Opus** — skjøten mellom cpals feiltaksonomi og
gjenkoblingsmaskinen, pluss COM-levetid. En feil her ser ut som en flaky
gjenkobling på riggdagen, ikke som en bug.

**Gren:** `ombygging/a3b-cpal-feiltyper` · **Pakke:** A3

**Mål:** Koble 0.18s feiltyper til det gjenkoblingen faktisk skal gjøre:

| Feil                 | Handling                                     |
| -------------------- | -------------------------------------------- |
| `DeviceNotAvailable` | gjenkobling                                  |
| `StreamInvalidated`  | gjenkobling (dette er frekvensbyttet på Mac) |
| `DeviceBusy`         | nytt forsøk                                  |
| `PermissionDenied`   | fatal, med riktig melding til brukeren       |

**Endrer:** `recorder/engine/supervisor.rs`,
`recorder/native_capture/stream.rs`, `audio/com_anchor.rs`.

I dag fanges et frekvensbytte på Mac bare over en gjenkobling. Med 0.18 går det
gjennom samme vei. **Les `audio/com_anchor.rs` på nytt mot den nye
WASAPI-koden** — modulhodet begrunner hvorfor COM-ankeret finnes, og
begrunnelsen må fortsatt stemme.

**Port:** `cargo test --workspace` med nye tester per feilklasse,
`windows-check`.

**Ferdig når:** hver av de fire feilklassene har en test som viser hvilken vei
den tar, og `com_anchor`-modulhodet er bekreftet eller rettet.

**Hvor det bevises:** Macen (testene, og frekvensbytte kan tvinges med
Audio MIDI-oppsett) + Riggen for RIG-DAY (a).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A3b, pakke A3 i `docs/OMBYGGING.md` og
> `docs/AVHENGIGHETER.md` §cpal. Gjennomfør økta som én PR på
> `ombygging/a3b-cpal-feiltyper`. Følg fellesreglene. Les modulhodene til
> `recorder/engine/supervisor.rs`, `recorder/native_capture/stream.rs` og
> `audio/com_anchor.rs` før du endrer dem. Hver av de fire feilklassene skal ha
> en test som viser hvilken vei den tar. Si eksplisitt om `com_anchor`-hodets
> begrunnelse fortsatt stemmer mot cpal 0.18s WASAPI-kode, eller rett det.

## A3c — Xruns fra lydsystemet, ikke fra stderr

**Modell:** Sonnet · **Gren:** `ombygging/a3c-xruns` · **Pakke:** A3

**Mål:** 0.18 melder xruns fra CoreAudio og WASAPI-opptak gjennom
`CallbackInfo`. Det gir ekte tall for hakk, i stedet for det `soak/` og
`test_recording/` utleder fra ffmpegs stderr.

**Endrer:** den native motoren (teller), telemetri (ny teller), `soak/`,
`test_recording/`. Stderr-tolkningen fjernes **bare** der den native motoren
brukes — ffmpeg-stiene lever til C3.

**Port:** `cargo test --workspace`; `cargo test native_bench_60s_machine_proof
-- --ignored` på Macen.

**Ferdig når:** et 60-sekunders opptak fra innebygd mikrofon rapporterer et
xrun-tall, soak-rapporten viser det, og telleren er deployet i telemetri-Workeren
**før** versjonen promoteres.

**Hvor det bevises:** Macen (innebygd mikrofon) + Riggen for RIG-DAY (h).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A3c og pakke A3 i `docs/OMBYGGING.md`.
> Gjennomfør økta som én PR på `ombygging/a3c-xruns`. Følg fellesreglene. Fjern
> stderr-tolkningen bare der den native motoren brukes — ffmpeg-stiene lever til
> C3. Kjør `cargo test native_bench_60s_machine_proof -- --ignored` og vis
> tallet i PR-en. Husk at den nye telemetri-telleren må deployes i Workeren før
> versjonen promoteres.

## A4 — Windows lytter på endringer i enhetslista

**Modell:** **Opus** — et håndskrevet COM-grensesnitt. Å avregistrere mens en
tilbakekalling kjører gir vranglås, og håndtaket må ikke lukkes om
avregistreringen feilet. Begge fellene er stille i test og dødelige i drift.

**Gren:** `ombygging/a4-windows-enhetslytter` · **Pakke:** A4

**Mål:** `audio/device_watch.rs` har macOS-lytteren, men ikke Windows. Uten den
venter gjenkoblingen ut back-offen i stedet for å koble til med en gang mikseren
settes inn.

**Endrer:** `audio/device_watch.rs`, `src-tauri/Cargo.toml`.

**`windows` må erklæres direkte.** Den ligger i `Cargo.lock` i dag, men
transitivt via cpal; `src-tauri` har bare `windows-sys` selv. Legg `windows` inn
under `[target.'cfg(windows)'.dependencies]` med `implement`- og
`Win32_Media_Audio`-featurene, og **pin versjonen eksplisitt** — ellers kan en
cpal-bump flytte den under vtabellen vår. Det er også grunnen til at A4 kommer
etter A3.

**Forbildet** er cpals `DefaultDeviceMonitor` i `src/host/wasapi/stream.rs`
(Apache-2.0). Krediter den i modulhodet. Lytteren skal gjøre **én** ting: dytte
samme `Notify` som macOS-halvdelen. Kamera-halvdelen
(`CM_Register_Notification`) tas i C5a, ikke her.

**Rett modulhodet:** begrunnelsen om at `windows`-cratet måtte dras inn, stemmer
ikke lenger.

**Port:** `windows-check`-jobben; `cargo clippy -D warnings` for
Windows-målet.

**Ferdig når:** RIG-DAY (a) på Windows viser at back-offen kuttes når mikseren
settes inn igjen.

**Hvor det bevises:** CI (bygg) + **Riggen** (Windows-boksen). Macen kan ikke
bevise noe her.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A4, pakke A4 i `docs/OMBYGGING.md` og
> `docs/AVHENGIGHETER.md` §«Windows: lytter på endringer i enhetslista».
> Gjennomfør økta som én PR på `ombygging/a4-windows-enhetslytter`. Følg
> fellesreglene. Les modulhodet i `audio/device_watch.rs` først — det forklarer
> hvorfor Windows mangler, og den begrunnelsen skal rettes i denne PR-en.
> `windows` skal legges inn som en direkte, eksplisitt pinnet avhengighet.
> Lytteren gjør bare én ting: dytter samme `Notify` som macOS. Kamera-halvdelen
> hører til C5a. Skriv det nye RIG-DAY-punktet, og si i PR-en at punktet venter
> på Windows-boksen.

## A5a — Nødluka for pre-roll fjernes

**Modell:** Sonnet · **Gren:** `ombygging/a5a-preroll-nodluke` · **Pakke:** A5

**Mål:** `classic_ffmpeg_preroll` og den klassiske ffmpeg-pre-rollen ut. Den
native bufferen har vært standard siden 2026-08-01, og eierpunktet ble avgjort i
v0.25.3.

**Endrer / sletter:** `crates/sundayrec-core/src/settings.rs` (feltet),
`src-tauri/src/settings/profile.rs`, og `src-tauri/src/recorder/preroll.rs`
(1 060 linjer) slettes.

**Flytt før du sletter.** #337 la `sweep_stale_preroll_files()` (68 linjer) inn i
`recorder/preroll.rs`, og sveipen rydder `sundayrec-preroll-*.wav` for **begge**
motorene — doc-kommentaren sier «the engines», flertall. Flytt den til
`native_capture/preroll.rs` med testen sin, og behold kallet fra `lib.rs`.
Slettes den med fila, kommer de lekkede WAV-ene tilbake.

**Kjernens `preroll.rs` røres ikke her.** Den har bare ≈280 linjer ffmpeg
(`build_preroll_capture_args`, `build_preroll_trim_args`, `ms_to_seconds_string`

- tester); resten er matematikk den native pre-rollen bruker. De 280 dør i C3b.

**Port:** `cargo test --workspace`, `npm run bindings:check` (feltet forsvinner
fra `Settings.ts`/`WireSettings.ts`).

**Ferdig når:** en gammel database med feltet leses uten feil (test — serde
dropper ukjente felt, se `settings.rs` og testene
`legacy_blob_with_*_fields_imports_cleanly`), sveipen for
`sundayrec-preroll-*.wav` virker fortsatt (test), og ingen kodevei når
ffmpeg-pre-rollen.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A5a og pakke A5 i `docs/OMBYGGING.md`.
> Gjennomfør økta som én PR på `ombygging/a5a-preroll-nodluke`. Følg
> fellesreglene. **Flytt `sweep_stale_preroll_files()` til
> `native_capture/preroll.rs` FØRST** — den tjener også den native pre-rollen, og
> slettes den med fila kommer de lekkede WAV-ene tilbake. Kjernens `preroll.rs`
> røres ikke i denne økta. Skriv en test som viser at en gammel database med
> `classic_ffmpeg_preroll` leses uten feil.

## A5b — Nødluka for ffmpeg-lyd fjernes

**Modell:** Sonnet · **Gren:** `ombygging/a5b-lyd-nodluke` · **Pakke:** A5

**Mål:** `classic_ffmpeg_audio` ut, med ffmpeg-fangsten av **rene** lydopptak.

**Endrer:** feltet i `settings.rs`, de 46 kallestedene i `src-tauri/src`, og
inngangene til ffmpeg-fangst av lyd uten video.

**`cpal_capture.rs` blir stående.** Modulhodet sier det selv: fila tjener både
denne luka **og Windows' videoøkter**, og er «HARDENED, not deleted» til den
native motoren får video. A5b fjerner bare luka; fila dør i C3b. Det samme
gjelder `engine.rs` og `engine/`, som fortsatt kjører videoøktene.

**Port:** `cargo test --workspace`, `npm run bindings:check`,
`npm run reachability` (ingen kommandoer skal bli uoppnåelige).

**Ferdig når:** innstillingene leses uten feil fra en gammel database (test), og
**ingen kodevei når ffmpeg-fangst av lyd uten video**. Si i PR-en hvilke filer
som ble stående og hvorfor.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §A5b og pakke A5 i `docs/OMBYGGING.md`.
> Gjennomfør økta som én PR på `ombygging/a5b-lyd-nodluke`. Følg fellesreglene.
> `cpal_capture.rs`, `engine.rs` og `engine/` skal **ikke** slettes her — de
> kjører fortsatt Windows' videoøkter, og dør i C3b. Fjern bare det `classic_ffmpeg_audio`
> alene nådde. List i PR-en hvilke filer du lot stå, og hvorfor.

## C0a — Maskinvarekoding på Windows

**Modell:** Sonnet · **Gren:** `ombygging/c0a-hw-koder` · **Pakke:** C0

**Mål:** `push_video_encoder_args` velger maskinvare bare når
`hw_accel && macOS`. På Windows kodes 1080p30 alltid i programvare, og på en
eldre kirke-PC gir det høy CPU-last, tapte bilder og hakking. Windows-bygget som
følger med (gyan.dev essentials 9.0.1) har `h264_nvenc`, `h264_qsv`, `h264_amf`
og `h264_mf`.

**Endrer:** `crates/sundayrec-core/src/capture.rs`, diagnosen, og per-maskin
lagring av valget.

**Prøven:** ved første videoopptak og i diagnosen prøves koderne i rekkefølge
med et kort testopptak
(`-f lavfi -i testsrc2=s=1280x720:d=0.2 -c:v <koder> -f null -`):
`h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf` → `libx264`. Første som
virker, lagres per maskin. Bitrate som på Mac (`default_video_bitrate_kbps`).
Feiler den valgte koderen **under** et opptak, faller økta til `libx264` ved
gjenkoblingen, og valget nullstilles.

**Port:** golden-testene i `capture.rs` dekker argumentene for hver koder
(mønsteret finnes: `audio_only_argv_is_byte_identical_golden`).

**Ferdig når:** golden-testene er på plass, og et 60-minutters opptak på
Windows-boksen bruker under halvparten av CPU-en det bruker i dag
(Oppgavebehandling), uten tapte bilder.

**Hvor det bevises:** Macen (golden-testene) + **Riggen** (CPU-målingen).
Koderne finnes ikke i Mac-sidecaren, så prøven kan ikke kjøres her.

**Rettelser til planen (funnet i C0a):**

- Windows' vanlige videoøkt går ikke gjennom `push_video_encoder_args`, men
  gjennom cpal-røret (`build_cpal_pipe_video_args`), som hadde `libx264`
  hardkodet. Begge stiene bruker nå samme valg
  (`sundayrec_core::video_encoder`), ellers hadde endringen ikke nådd Windows'
  vanlige videoopptak. Koderrunnen er delt i `push_encoder_args`.
- «Faller til `libx264` ved gjenkoblingen» gjelder ffmpeg-/dshow-stien
  (`run_session`). cpal-røret har ikke gjenkobling og avslutter økta når
  ffmpeg dør; der glemmer C0a bare valget, så NESTE opptak får `libx264`.
- Prøven bruker nøyaktig koderargumentene opptaket bruker, ikke bare
  `-c:v <koder>`: `-hw_encoding 1` for `h264_mf` (ellers kan Media Foundation
  velge sin programvare-koder og «bestå»), og bitrate som på Mac.
- Etter en koderfeil midt i et opptak prøves ikke maskinvare på nytt før appen
  startes på nytt, slik at en koder som består prøven men feiler under last
  ikke velges igjen ved neste opptak samme dag.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C0a og `docs/VIDEOMOTOR.md` §C0. Gjennomfør økta
> som én PR på `ombygging/c0a-hw-koder`. Følg fellesreglene. Golden-testene er
> porten — se `audio_only_argv_is_byte_identical_golden` i `capture.rs` for
> mønsteret. Koderne finnes ikke i Mac-sidecaren, så selve prøven kan ikke kjøres
> her: skriv CPU-målingen som et nytt RIG-DAY-punkt for Windows-boksen.

## C0b — Nøkkelbilde hvert 2. sekund

**Modell:** Sonnet · **Gren:** `ombygging/c0b-nokkelbilder` · **Pakke:** C0

**Mål:** Kortere tap ved krasj, raskere søk i redigeringen, og grunnlaget for
rask videoeksport (B6).

**Endrer:** `crates/sundayrec-core/src/capture.rs` — `-g 2×fps` og
`-keyint_min fps` i **alle** videoopptak.

**Port:** golden-testene; `ffprobe` på et ekte Mac-opptak.

**Ferdig når:** golden-testene dekker flaggene, og `ffprobe -select_streams v
-show_frames` på et Mac-opptak viser nøkkelbilde hvert 2. sekund.

**Hvor det bevises:** Macen (innebygd kamera + `ffprobe`).

**Rettelser til planen (funnet i C0b):**

- Flaggene står ikke i `capture.rs` per sti, men i den ene sømmen
  `push_video_encoder_args_with` som C0a la inn. Den deles av den samlede stien,
  tostegs-fallbackens videoprosess og Windows' cpal-rør, så alle tre og alle
  seks koderne er dekket av ÉN golden-test
  (`every_encoder_and_path_gets_a_keyframe_every_two_seconds`).
- Kameratilgang ble ikke gitt til byggeøkta, og tillatelsen ble ikke omgått.
  Intervallet er målt med `ffprobe -skip_frame nokey` på `testsrc2` gjennom
  nøyaktig samme koderargv; det ekte kamerabeviset er RIG-DAY (c0b).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C0b, `docs/VIDEOMOTOR.md` §C0 og
> `docs/LYDMOTOR.md` fase 6 (den er grunnen til at dette gjøres nå). Gjennomfør
> økta som én PR på `ombygging/c0b-nokkelbilder`. Følg fellesreglene. Ta et ekte
> opptak med det innebygde kameraet og bevis intervallet med `ffprobe` — ikke
> bare med golden-testen.

## M1R — Opus-review av M1

**Modell:** **Opus** · **Gren:** ingen (review, funn blir egne PR-er)

**Mål:** Lese M1 som en helhet, ikke PR for PR. Det Sonnet-økter systematisk
ikke ser er kryss-PR-kjeder og skjøtefeil.

**Ser etter:**

- Kryss-PR-kjeder: en endring i A3a som A5b bygger videre på feil premiss.
- [Skjøtefeil](LEARNING.md): to lag korrekte hver for seg, uenige i skjøten,
  begge grønne. 9 av 11 funn i gjennomgangen i august var av denne typen.
- At A5 ikke etterlot døde grener, og at `sweep_stale_preroll_files()` faktisk
  ble flyttet og ikke mistet.
- At `com_anchor`-hodet stemmer med cpal 0.18.
- At alle nye jobbnavn er i grenbeskyttelsen.

**Ferdig når:** funnene er skrevet som nummererte punkter med alvorsgrad, og de
🔴/🟠-merkede er rettet før beta.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §M1R. Gå gjennom alle PR-ene i M1 som én helhet.
> Let særlig etter skjøtefeil (se `docs/LEARNING.md`) og kryss-PR-kjeder, der en
> senere økt bygger på et premiss en tidligere økt endret. Rapporter nummererte
> funn med alvorsgrad 🔴/🟠/🟡/🔵/⚪. Ikke rett noe i denne økta — funnene blir
> egne PR-er.

---

# M2 — Ny lyd i redigeringen (0.27)

Størst hørbar forbedring. Uavhengig av videoarbeidet, og kjøres parallelt med
M3. Detaljene står i [LYDMOTOR.md](LYDMOTOR.md); fase N der er pakke BN her.

## B0a — `filmcraft-audio-dsp` tas inn i repoet

**Modell:** Sonnet · **Gren:** `ombygging/b0a-filmcraft-dsp` · **Pakke:** B0

**Mål:** Craten inn i treet, låst til én commit, med lisensene i orden.

**Endrer:** `crates/filmcraft-audio-dsp/` (ny, ~12 500 linjer),
workspace-`Cargo.toml` (`members`), `crates/sundayrec-core/Cargo.toml`
(avhengighet), `crates/filmcraft-audio-dsp/UPSTREAM.md` (ny), `LICENSE-MIT`,
`LICENSE-APACHE` og filmcrafts `NOTICE`.

**Dette er et bevisst unntak** fra «avhengighet før kopi» i
[AVHENGIGHETER.md](AVHENGIGHETER.md) — eiers beslutning 2026-10-11.

**Slik velges commiten:** gjennomgangen brukte
`0800a8b0aa4a703ba42172f415b45a61f8d8a5e1`. Velg **nyeste** commit når økta
starter, les endringene siden den over, og skriv den valgte i `UPSTREAM.md`
sammen med hver lokal endring. Vi følger ikke oppstrøms fortløpende: en
oppdatering er en egen PR med diff mot forrige commit.

**Ta ikke med:** kodekene, containerne, egui-grensesnittet, tidslinje- og
flersporsmotoren, musikkeffektene.

**Port:** `cargo test --workspace` kjører deres 122 tester;
`cargo deny check licenses` (A1a) må fortsatt være grønn.

**Ferdig når:** deres tester er grønne i vår CI på macOS, Windows og Linux, og
`UPSTREAM.md` navngir commiten.

**Hvor det bevises:** Macen + CI.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B0a, `docs/LYDMOTOR.md` §«Kilden» og fase 0, og
> `docs/AVHENGIGHETER.md` §Regelen. Gjennomfør økta som én PR på
> `ombygging/b0a-filmcraft-dsp`. Følg fellesreglene. Velg nyeste
> oppstrømscommit, les diffen siden `0800a8b0aa4a703ba42172f415b45a61f8d8a5e1`,
> og skriv den valgte commiten i `crates/filmcraft-audio-dsp/UPSTREAM.md`.
> Lisensfilene og filmcrafts `NOTICE` skal med. Ta **ikke** med kodekene,
> containerne, egui-UI-et, tidslinjemotoren eller musikkeffektene. Deres 122
> tester skal kjøre i vår CI.

## B0b — Oracle-testen mot vår egen sidecar

**Modell:** Sonnet · **Gren:** `ombygging/b0b-oracle-test` · **Pakke:** B0

**Mål:** `tests/loudness_oracle.rs` bruker filmcrafts `testkit` til å finne
ffmpeg og skrive WAV. Den skal bruke vår sidecar i stedet.

**Endrer:** `crates/filmcraft-audio-dsp/tests/loudness_oracle.rs` — bytt
`testkit` for `src-tauri/binaries/ffmpeg-*`, som `check`-jobben i `ci.yml`
allerede henter.

**Port:** testen i `cargo test --workspace` og i `check`-jobben.

**Ferdig når:** måleren er innenfor **±0,1 LU** mot ffmpeg `ebur128` med
**9.0.1**-sidecaren, på alle tre plattformene i CI.

**Hvor det bevises:** Macen + CI.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B0b og `docs/LYDMOTOR.md` fase 0. Gjennomfør økta
> som én PR på `ombygging/b0b-oracle-test`. Følg fellesreglene. Testen skal finne
> ffmpeg i `src-tauri/binaries`, ikke via filmcrafts `testkit`. Kravet er ±0,1 LU
> mot `ebur128` på 9.0.1 — kjør `npm run ffmpeg` først hvis sidecaren er eldre.

## B0c — Målingene gjentas med 9.0.1 på Macen

**Modell:** Sonnet · **Gren:** `ombygging/b0c-dsp-maalinger` · **Pakke:** B0

**Mål:** Tallene i [LYDMOTOR.md](LYDMOTOR.md) §Vedlegg er målt i en
sky-container med 4 kjerner og **ffmpeg 6.1** — ikke den medfølgende 9.0.1. De
viser retningen, ikke fasiten. Nå måles de på maskinen som skal gjøre jobben.

**Endrer:** `src-tauri/examples/dsp_bench.rs` (ny), vedlegget i `LYDMOTOR.md`.

**Oppskriften står i vedlegget** og skal følges som den står: signalene (støy
modulert med 4 Hz, 50 Hz brum på −34 dB, sus med topp på −46 dBFS), blokkene
(1024 for effekter, 4800 for måleren), og de to kvalitetsmålingene (pulser med
sus, og støtstøy foldet med en RT60-hale på 1,5 s). Samme filer gjennom ffmpeg
med SundayRecs **egne** filterstrenger, hentet fra
`processing::vocal_chain_presets()` og `mastering::build_measure_pass_filters()`.

**Dette er et beslutningspunkt.** Vedlegget sier selv at `afftdn` **uten**
`tn=1` og med riktig `nf` tok −57 dB, mens dagens innstillinger tar −46,5 dB.
Hvis 9.0.1 bekrefter det, er ffmpeg ikke dårlig på **støy** — det er
innstillingene våre som er feil. Da skrives det i vedlegget, og B3a kan velge
mellom filmcrafts `denoise` og en rettet `afftdn`. Argumentene for filmcraft som
**ikke** rokkes av dette: romklangfjerning og brumfjerning finnes ikke i ffmpeg
i det hele tatt, `loudnorm` treffer ikke målet, fila leses fem–seks ganger, og
forhåndslyttingen kan ikke bli sanntid med en ffmpeg-renderet mp3.

**Port:** `cargo run --example dsp_bench --release`.

**Ferdig når:** vedlegget i `LYDMOTOR.md` har tallene fra denne Macen, med
maskin, ffmpeg-versjon og dato, og en setning som sier hva de endrer eller ikke
endrer for B2 og B3.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B0c og `docs/LYDMOTOR.md` §Vedlegg. Gjennomfør
> økta som én PR på `ombygging/b0c-dsp-maalinger`. Følg fellesreglene. Følg
> oppskriften i vedlegget nøyaktig, med `src-tauri/binaries`-sidecaren (9.0.1).
> Mål også `afftdn` **uten** `tn=1` og med riktig `nf` — vedlegget mistenker at
> ffmpeg er langt bedre på støy enn våre nåværende innstillinger. Skriv tallene
> inn i vedlegget med maskin, ffmpeg-versjon og dato, og én setning om hva de
> endrer for B2 og B3. Ikke endre kode utenfor eksempelet.

## B1a — Lydstyrke i Rust

**Modell:** Sonnet · **Gren:** `ombygging/b1a-loudness` · **Pakke:** B1

**Mål:** Erstatte `loudnorm` med en måling og en plan vi styrer selv. Målet skal
alltid nås, limiteren skal bare ta de sjeldne toppene, og programmet skal ikke
komprimeres.

**Endrer:** `crates/sundayrec-core/src/loudness.rs` (ny).

- BS.1770-måling over PCM (filmcrafts `LoudnessMeter`).
- `plan_gain(målt, preset) → (gain_db, limiter)`.
- Påføring er **lineær** forsterkning til målet, og etter den en
  true-peak-limiter på presettets tak (−1 dBTP).

**Port:** `cargo test --workspace`, med en test som kryssjekker mot ffmpeg
`ebur128`.

**Ferdig når:** hver fil i en testmappe med ekte opptak treffer målet **±0,5 LU**
med true peak **≤ −1,0 dBTP**, kryssjekket med `ebur128` i en test.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B1a og `docs/LYDMOTOR.md` fase 1. Gjennomfør økta
> som én PR på `ombygging/b1a-loudness`. Følg fellesreglene. Dette er ren
> kjernekode i `sundayrec-core`: ingen I/O, ingen klokke. Påføringen er lineær
> forsterkning til målet, med true-peak-limiteren etter — ikke dynamisk
> regulering. Kryssjekk mot ffmpeg `ebur128` i en test, og si i PR-en hvilke
> ekte opptak du målte på.

## B1b — Eksporten bruker den, og `loudnorm` forsvinner

**Modell:** Sonnet · **Gren:** `ombygging/b1b-loudnorm-ut` · **Pakke:** B1

**Mål:** Eksporten måler det **kuttede, behandlede** signalet i Rust og koder i
samme runde. `mastering_analyze` bruker samme måler.

**Endrer / sletter:** `crates/sundayrec-core/src/mastering.rs` —
`loudnorm`-strengene (måling og påføring), JSON-parseren, `plan_pass2` og
«Normalization Type»-sjekken. Eksportveien i `src-tauri/src/editor/`.

**En avgjørelse tas på målt tid, ikke på forhånd:** om behandlet PCM
mellomlagres som f32 i temp, eller om kjeden kjøres to ganger. 90 minutter stereo
f32 er rundt 2 GB, så ingenting holder hele fila i minnet. Mål begge og skriv
tallene i PR-en.

**Port:** `cargo test --workspace`; eksportér en 90-minutters fil og ta tiden
før og etter.

**Ferdig når:** `loudnorm` finnes ikke i treet, målet treffes som i B1a, og en
90-minutters eksport er **raskere** enn i dag (tallene i PR-en).

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B1b og `docs/LYDMOTOR.md` fase 1. Gjennomfør økta
> som én PR på `ombygging/b1b-loudnorm-ut`. Følg fellesreglene. Mål både
> disk-mellomlagring og to kjeder-kjøringer på en ekte 90-minutters fil, velg på
> tallene, og skriv begge i PR-en. `loudnorm`, JSON-parseren, `plan_pass2` og
> «Normalization Type»-sjekken skal være borte når du er ferdig.

## B2a — Lydkjeden i kjernen

**Modell:** Sonnet · **Gren:** `ombygging/b2a-chain` · **Pakke:** B2

**Mål:** Mastring-presettene og `VocalChain` blir en liste av
filmcraft-effekter, bygget fra de **samme** presettabellene.

**Endrer:** `crates/sundayrec-core/src/chain.rs` (ny),
`src-tauri/examples/chain_ab.rs` (ny).

`chain_ab` lager gammel og ny versjon av samme utdrag, til lytting før
godkjenning. Den slettes i B2c.

**Port:** `cargo test --workspace`; `cargo run --example chain_ab`.

**Ferdig når:** hver profil i `vocal_chain_presets()` har en tilsvarende kjede,
og `chain_ab` produserer to filer av samme utdrag.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B2a og `docs/LYDMOTOR.md` fase 2. Gjennomfør økta
> som én PR på `ombygging/b2a-chain`. Følg fellesreglene. Kjeden bygges fra de
> **samme** presettabellene som i dag — ikke en ny tabell. Ingenting slettes i
> denne økta. `chain_ab` skal lage gammel og ny versjon av samme utdrag, så
> Ørene kan høre forskjellen i B2b.

## B2b — Kjeden i bruk, ffmpeg får ingen `-af`

**Modell:** Sonnet · **Gren:** `ombygging/b2b-kjeden-i-bruk` · **Pakke:** B2

**Mål:** Alt mellom dekoding og koding skjer i Rust. ffmpeg dekoder og koder.

**Endrer:** `crates/sundayrec-core/src/processing.rs`,
`crates/sundayrec-core/src/mastering.rs`, `app/editor/MixerPanel.tsx`.

**Forhåndsinnstillingene stilles inn på nytt etter gehør,** fordi filtrene ikke
er identiske. Samme LUFS-mål, ny lytting. Mikseren mappes til samme kjede, slik
at «det du hører er det du får» holder.

**Port:** `npm run check`, `cargo test --workspace`; `chain_ab` på tre ekte
opptak.

**Ferdig når:** ingen `-af` går til ffmpeg i redigeringen, mikseren mapper til
samme kjede, og utdragene til Ørene er lagt klar.

**Hvor det bevises:** Macen (koden) — godkjenningen hører til B2c.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B2b og `docs/LYDMOTOR.md` fase 2. Gjennomfør økta
> som én PR på `ombygging/b2b-kjeden-i-bruk`. Følg fellesreglene. Behold samme
> LUFS-mål, men still presettene inn på nytt etter gehør — filtrene er ikke
> identiske. Mikseren skal mappe til **samme** kjede som eksporten. Slett
> ingenting her: filterstreng-byggerne dør i B2c, etter at Ørene har sagt ja.
> Lag utdragene til lyttingen og legg dem i PR-en.

## B2c — De gamle filterstrengene slettes

**Modell:** Sonnet · **Gren:** `ombygging/b2c-filterstrenger-ut` · **Pakke:** B2

**Blokkert på eieren.** Dette er ett av to steder i hele planen der en PR venter
på ører, ikke på kode.

**Mål:** Siste skritt i beslutning B5 i [OMBYGGING.md](OMBYGGING.md): ingen
reserve etter godkjenning.

**Sletter:** filterstreng-byggerne i `processing.rs` og `mastering.rs`, og
`src-tauri/examples/chain_ab.rs`.

**Port:** `cargo test --workspace`, `npm run reachability`.

**Ferdig når:** Ørene i [RIG-DAY.md](RIG-DAY.md) har lyttet til de **tre**
profilene på **minst tre ekte gudstjenester**, gammel mot ny, med minst ett
stort kirkerom og ett brumopptak — og eieren har sagt ja. Først da slettes det.

**Hvor det bevises:** **Riggen** (Ørene) + Macen (slettingen).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B2c og `docs/LYDMOTOR.md` fase 2 og §Risiko.
> Gjennomfør økta som én PR på `ombygging/b2c-filterstrenger-ut`.
> **Sjekk først at eieren har sagt ja etter lyttingen** — har hen ikke det, stopp
> og si det. Følg fellesreglene. Når du sletter: det skal ikke finnes noen vei
> tilbake til filterstrengene etterpå, og `chain_ab` går med.

## B3a — «Tale» renser av seg selv

**Modell:** Sonnet · **Gren:** `ombygging/b3a-tale-renser` · **Pakke:** B3

**Mål:** Standardprofilen renser ingenting i dag. «Tale» (`speech-clear` i
`app/editor/sound-profiles.ts`) er høypass, EQ, kompressor og `loudnorm`. Støy-
og romklangfjerning nås bare gjennom den avanserte mikseren.

**Endrer:** `crates/sundayrec-core/src/chain.rs`, analysen.

Analysen avgjør hva «Tale» tar med:

| Hva              | Når                                                                                                      |
| ---------------- | -------------------------------------------------------------------------------------------------------- |
| brumfjerning     | energi på 50/100/150 Hz over nabobåndene (Goertzel på 16 kHz-strømmen; 50 eller 60 Hz velges automatisk) |
| støyfjerning     | **alltid**, mildt — minimumsstatistikken trenger ikke å lære støyen                                      |
| romklangfjerning | når analysen anslår en lang etterklang                                                                   |
| klikkfjerning    | når kanaldiagnosen ser knitring                                                                          |

**Ingen nye knapper.** Hvis B0c viste at en rettet `afftdn` er like god på støy,
velg på tallene og skriv valget i modulhodet.

**Port:** `cargo test --workspace` med en test per utløser.

**Ferdig når:** hver utløser har en test med et syntetisk signal som slår den på
og et som ikke gjør det.

**Hvor det bevises:** Macen (testene) — hørbarheten hører til B3b.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B3a og `docs/LYDMOTOR.md` fase 3. Gjennomfør økta
> som én PR på `ombygging/b3a-tale-renser`. Følg fellesreglene. **Ingen nye
> knapper** — analysen bestemmer. Hver av de fire utløserne skal ha en test som
> slår den på og en som ikke gjør det. Les B0c-tallene i `LYDMOTOR.md` §Vedlegg
> før du velger støyfjerner, og skriv valget i modulhodet.

## B3b — Tre ord til den frivillige

**Modell:** Sonnet · **Gren:** `ombygging/b3b-tale-forklaring` · **Pakke:** B3

**Mål:** «Frivillig først»: den som tar opp gudstjenesten skal se hva som ble
gjort, i tre ord — uten å måtte forstå DSP.

**Endrer:** lyd-steget i `app/editor/`, i18n for alle sju språk,
[FRIVILLIG.md](FRIVILLIG.md).

Et forslag sier hva som ble gjort: «Vi fjernet brum fra strømnettet».

**Port:** `npm run check` (i18n-vaktene), `npm run test`.

**Ferdig når:** et ekte brumopptak og et ekte opptak fra et stort kirkerom er
hørbart bedre (Ørene), og **et rent opptak er hørbart uendret**.

**Hvor det bevises:** Macen (UI og tekst) + **Riggen** (Ørene).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B3b, `docs/LYDMOTOR.md` fase 3 og
> `docs/FRIVILLIG.md`. Gjennomfør økta som én PR på
> `ombygging/b3b-tale-forklaring`. Følg fellesreglene. Tre ord, ingen fagspråk,
> alle sju språk. Skriv lyttepunktet inn i `RIG-DAY.md`: brumopptak og stort
> kirkerom skal være hørbart bedre, og et rent opptak hørbart uendret.

## M2R — Opus-review av M2

**Modell:** **Opus** · **Gren:** ingen (review)

**Ser etter:**

- **Måle-mot-påfør-skjøten.** Måles ett signal og påføres et annet, blir
  lydstyrken feil for alle, stille. Dette er M2s farligste skjøte.
- At forhåndslytting og eksport faktisk bruker **samme kode og samme
  parametre** — løftet i [LYDMOTOR.md](LYDMOTOR.md) §Målbildet.
- Minnebruk på 90 minutter (2 GB f32): holder noe kodepath hele fila?
- At mikseren og presettene gir samme kjede.
- [Skjøtefeil](LEARNING.md) mellom kjernen og skallet.

**Ferdig når:** nummererte funn med alvorsgrad; 🔴/🟠 rettet før beta.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §M2R. Gå gjennom alle PR-ene i M2 som én helhet.
> Begynn med måle-mot-påfør-skjøten: måles ett signal og påføres et annet?
> Sjekk at forhåndslytting og eksport bruker samme kode og samme parametre, og
> at ingen kodevei holder 90 minutter f32 i minnet. Rapporter nummererte funn med
> alvorsgrad. Ikke rett noe her.

---

# M3 — Ny videomotor (0.28)

Størst forbedring i robusthet. Bygger på A3 og A4, og kjøres parallelt med M2.
Detaljene står i [VIDEOMOTOR.md](VIDEOMOTOR.md).

**Lydklokka er sjefen.** Bildene følger lyden, aldri omvendt. Et opptak der
bildet hakker er et dårlig opptak. Et opptak der lyden mangler, er et tapt
opptak.

## C1a — Mac: AVFoundation portet til objc2

**Modell:** **Opus** — `unsafe` FFI mot AVFoundation. Levetid, trådeierskap og
autorelease-pooler, i kode som ikke kan testes uten maskinvare.

**Gren:** `ombygging/c1a-kamera-mac` · **Pakke:** C1

**Mål:** Caps kamerafangst for Mac **portes** i stedet for å tas inn, fordi Cap
bygger på en egen gaffel av `cidre` mens `src-tauri` allerede bruker
objc2-familien. Én binding-familie for Apple-API-ene er verdt 600 linjer
porting.

**Endrer:** `src-tauri/src/camera/avfoundation.rs` (ny, ~600 linjer),
`src-tauri/Cargo.toml` (`objc2-av-foundation`).

**Caps syv rettelser må være med.** Hver var en ekte feil i produksjon hos dem:

1. Enhetens konfigurasjon **låses** mens `startRunning` kjører — ellers skriver
   macOS over det valgte formatet.
2. Etter `setActiveFormat` settes `activeVideoMinFrameDuration` til valgt fps.
   Ellers leverer kameraet f.eks. 60 b/s mens opptaket regnes som 30, og
   opptaket blir «strukket». **Maks-varigheten røres ikke**, så kameraet får
   senke farten i dårlig lys.
3. Be om kameraets **egen** pikselformat fra `AVCaptureVideoDataOutput` (ingen
   konvertering). Leverer økten ikke bilder (sett på macOS 26), prøves den på
   nytt i «kompatibilitetsmodus», der OS-et forhandler formatet.
4. Én **global lås** rundt start og stopp av økter.
5. **Autorelease-pool** rundt enhetsoppslag.
6. Buffere som kommer uten bilde telles og hoppes over.
7. Tidsstempelet er presentasjonstiden konvertert fra **vertsklokka**, og
   kameraet identifiseres med `uniqueID` (og USB-leverandør/produkt), ikke med
   nummer.

Tillatelse via den eksisterende `AVCaptureDevice authorizationStatus`-koden i
`media/permissions.rs`.

**Les ikke** Caps `cap-mediafoundation-utils`, kodere, `recording`, `timestamp`
eller `enc-*` — de er AGPL. Bare `crates/camera` og `camera-avfoundation` (MIT)
er spesifikasjonen her.

**Port:** `cargo test --workspace`; `cargo clippy -D warnings`; manuell kjøring
mot de to kameraene på Macen.

**Ferdig når:** alle sju rettelsene har en kommentar i koden som sier hvilken de
er, og fangst virker mot både FaceTime HD og Continuity-kameraet.

**Hvor det bevises:** Macen (innebygd + Continuity). USB-kamera venter på
riggen.

**Utfall (2026-10-13, `ombygging/c1a-kamera-mac`):** portert fra Cap
`8a1480262db2b54629806e936226ff06fbced9f1`; bare de to MIT-cratene ble lest.
Tre ting planen ikke sa: (1) Info.plist trenger
`NSCameraUseContinuityCameraDeviceType`, ellers melder macOS 14+ iPhonen som
et innebygd kamera. (2) Konstantene for de nyere enhetstypene (macOS 13/14)
kan ikke lenkes når minstekravet er 10.15; de sendes som strengverdier, og en
test låser dem til rammeverkets konstanter. (3) Caps «modell-ID» (siste åtte
tegn av `uniqueID`) er oppdiktet for innebygde og Continuity-kameraer; den
leses nå bare for USB-kameraer, og `uniqueID` matches først. De rene
beslutningene ligger i `sundayrec-core::camera_format`, og Caps MIT-merknad
står i THIRD-PARTY-NOTICES. **Kameraprøven gjenstår:** økta kjørte uten
skjerm og uten kameratilgang, så den står som RIG-DAY (c1a).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C1a, `docs/VIDEOMOTOR.md` §Kildene og §C1, og
> `docs/AVHENGIGHETER.md` §Videoopptak. Gjennomfør økta som én PR på
> `ombygging/c1a-kamera-mac`. Følg fellesreglene. Port Caps `camera` og
> `camera-avfoundation` (MIT) til objc2 — ikke ta inn `cidre`. Alle sju
> rettelsene i GJENNOMFORING §C1a skal være med, hver med en kommentar som sier
> hvilken den er. **Les ikke** `cap-mediafoundation-utils`, koderne,
> `recording`, `timestamp` eller `enc-*`: de er AGPL. Prøv mot både FaceTime
> HD-kameraet og Continuity-kameraet.

## C1b — Windows: Caps kamera-crates tas inn

**Modell:** **Opus** — `unsafe` COM og Media Foundation, og en lisensgrense som
ikke må krysses. Å lese feil fil her har juridiske følger, ikke bare tekniske.

**Gren:** `ombygging/c1b-kamera-windows` · **Pakke:** C1

**Mål:** Kamerafangst på Windows med Media Foundation, DirectShow som reserve,
og støtte for opptakskort og virtuelle kameraer.

**Endrer:** `crates/camera-windows/`, `crates/camera-mediafoundation/`,
`crates/camera-directshow/` (nye, fra Cap, **MIT** — uttrykkelig unntatt fra
Caps AGPL), hver med `UPSTREAM.md`, låst til én commit.
Gjennomgangen brukte `a32e4806b3004071effe00243e1dabd52bc7068c`; velg nyeste,
les diffen, skriv den valgte.

**Lisensgrensen, eksplisitt:** `cap-mediafoundation-utils` er **AGPL** (108
linjer: MF-oppstart, MMCSS, bufferlås). Den skal **ikke leses og ikke
kopieres**. De 108 linjene skrives fra Microsofts egen dokumentasjon. Det samme
gjelder koderne, `recording`, `timestamp` og `enc-*`.

**Port:** `windows-check`-jobben; `cargo deny check licenses` (A1a) må være
grønn med de nye cratene.

**Ferdig når:** cratene bygger i CI, `UPSTREAM.md` navngir commiten, og PR-en
sier eksplisitt at AGPL-delene ikke er lest.

**Hvor det bevises:** CI (bygg). **Riggen** for alt som krever et kamera.
Macen kan ikke bevise noe her.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C1b, `docs/VIDEOMOTOR.md` §Kildene, og
> `docs/AVHENGIGHETER.md` §Regelen og §Videoopptak. Gjennomfør økta som én PR på
> `ombygging/c1b-kamera-windows`. Følg fellesreglene. Ta inn **bare** Caps
> `camera-windows`, `camera-mediafoundation` og `camera-directshow` (MIT), hver
> med `UPSTREAM.md` og låst commit. **`cap-mediafoundation-utils` er AGPL: ikke
> les den, ikke kopier den.** Skriv de 108 linjene (MF-oppstart, MMCSS,
> bufferlås) fra Microsofts dokumentasjon, og si i PR-en at du gjorde det slik.

## C1c — Felles kamera-API og en probe

**Modell:** Sonnet · **Gren:** `ombygging/c1c-kamera-api` · **Pakke:** C1

**Mål:** Ett API over de to plattformene, som erstatter `-list_devices` og
modussøket via ffmpegs feilmeldinger. Ingen endring i opptaket ennå.

**Endrer:** `src-tauri/src/camera/mod.rs` (ny),
`src-tauri/examples/camera_probe.rs` (ny), diagnoserapporten.

- `list()` → stabil ID, visningsnavn, modell-ID, og om det er et opptakskort
  eller et virtuelt kamera.
- `formats(id)` → oppløsning, fps, pikselformat.
- `start(id, format, callback)` → bilder (NV12/UYVY/YUY2/MJPEG) med vertstid.
  **Callbacken gjør bare én ting: legger bildet i en begrenset kø.**

**Port:** `cargo test --workspace`;
`cargo run --example camera_probe`.

**Ferdig når:** probe-eksempelet lister kameraene og tar opp 10 s til en rå fil
med statistikk (fps, jitter, tapte bilder), og diagnoserapporten bruker den nye
lista. På Macen betyr det FaceTime HD, Continuity og — på riggdagen —
USB-kameraet. **Opptakskort finnes ikke, så det punktet står åpent, ikke
blokkerende.**

**Hvor det bevises:** Macen (2 kameraer) + **Riggen** (USB-kamera,
Windows-boksen).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C1c og `docs/VIDEOMOTOR.md` §C1. Gjennomfør økta
> som én PR på `ombygging/c1c-kamera-api`. Følg fellesreglene. Opptaket røres
> **ikke** i denne økta. Kamera-callbacken skal gjøre én ting: legge bildet i en
> begrenset kø — den må aldri blokkere. Kjør `camera_probe` mot FaceTime
> HD-kameraet og Continuity-kameraet og vis statistikken i PR-en. Skriv
> USB-kamera og Windows som nye RIG-DAY-punkter; opptakskort finnes ikke og
> føres som åpent.

## C2 — Konformeren

**Modell:** **Opus** — klokketilpasning og bildeplasser. Dette er ren
matematikk, men en feil her er usynlig til riggdagen og viser seg som
leppesynk-drift over 90 minutter.

**Gren:** `ombygging/c2-konformer` · **Pakke:** C2

**Mål:** Lydklokka er sjefen. Konformeren gjør bildene om til en fast
bildefrekvens låst til lydklokka. **Ingen I/O, ingen klokke — bare tall inn og
tall ut, og alt testes.**

**Endrer:** `crates/sundayrec-core/src/av_clock.rs` (ny),
`crates/sundayrec-core/src/video_conform.rs` (ny).

- **Lydklokka:** par av (sample-indeks, vertstid) fra lydmotorens callbacks.
  Vertstid knyttes til sample-indeks med en lineær tilpasning over et glidende
  vindu. Lydkort og vertsklokke driver 20–50 ppm fra hverandre, og 50 ppm er
  0,27 s på 90 minutter. Tilpasningen fanger det.
- **Bildeplassene:** plass `k` ligger på sample `k × rate / fps`. For hver plass
  velges det nyeste bildet med tid ≤ plassen. Kommer ikke noe nytt, gjentas
  forrige (dårlig lys gir 15 b/s fra kameraet). Kommer flere, droppes de ekstra.
- **Hull:** er kameraet borte, gjentas siste bilde i opptil 2 s, deretter svart.
  Starter video etter lyden, fylles starten med svart. Hopper vertstiden
  (maskinen sov), startes tilpasningen på nytt.

**Port:** `cargo test --workspace`, inkludert **proptest**.

**Ferdig når:** proptesten viser at tilfeldig drift, jitter, hull og fps-fall
**aldri** gir mer enn ±1 bilde (33 ms) avvik fra lydklokka over **3 timer**
simulert opptak.

**Hvor det bevises:** Macen (helt og holdent — dette er ren kjernekode).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C2 og `docs/VIDEOMOTOR.md` §C2 og §Målbildet.
> Gjennomfør økta som én PR på `ombygging/c2-konformer`. Følg fellesreglene.
> Dette er ren kjernekode: ingen I/O, ingen klokke, ingen `std::time::now`.
> Proptesten er porten — ±1 bilde over 3 timer simulert opptak, med tilfeldig
> drift, jitter, hull og fps-fall. Dekk også de tre hull-tilfellene: kamera
> borte, video som starter etter lyden, og et vertstidshopp fordi maskinen sov.

## C3a — Videosporet inn i den native økta

**Modell:** **Opus** — kjernen i opptaket. To klokker, to prosesser,
krasjgjenoppretting. Dette er den økta som kan miste en søndag.

**Gren:** `ombygging/c3a-videospor` · **Pakke:** C3

**Mål:** Den native motorens segmentløper får et **valgfritt** videospor. Lyden
går alltid gjennom den native lydmotoren; et kamera som faller ut kan aldri ta
lyden med seg.

**Endrer:** `src-tauri/src/recorder/native_capture/segment.rs`, en ny
video-writer, og leveringen.

- Per fragment startes **én ffmpeg-prosess kun for video**: `-f rawvideo
-pix_fmt nv12 -s WxH -framerate 30 -i pipe:0` (eller `-f mjpeg` for
  MJPEG-kameraer), C0as maskinvarekoder, `-g` fra C0b, `video_flip` som `hflip`,
  og MKV ut.
- Fragmentene får **samme navn og grenser** som lydfragmentene, så oppdeling og
  gjenkobling styres av lydmotoren som i dag.
- Køen til ffmpeg er begrenset til rundt **1 s** med bilder. Er den full, droppes
  det **eldste** bildet og telles. **Kamera-callbacken blokkerer aldri.**
- **Levering:** WAV + MKV blir mp4 (`-c:v copy`, lyd til AAC), og WAV-en beholdes
  som ren lydfil — `keep_separate_audio` blir gratis. Manifestet for
  krasjgjenoppretting lister **begge**, og gjenopprettingen setter dem sammen.
- **Pre-roll:** lyd-pre-rollen virker som i dag. Video starter når opptaket
  starter, og starten fylles med svart for pre-roll-lengden. Video-pre-roll er
  bevisst ikke planlagt.

**To rettelser må arves, ikke mistes:**

- **#341:** stopprekkefølgen fra `cpal_capture.rs` — stopp fangsten **før**
  writeren, og sett tidsgrense på hver venting. Uten den kommer tapt hale og
  hengende stopp tilbake.
- **#337:** melding om en **død lydprosess** under videoopptak, som i dag ligger
  i `two_process.rs`. Den nye motoren må si fra om det samme på egen kjøl.

**Port:** `cargo test --workspace`; et ekte videoopptak på Macen med
`kill -9` midt i.

**Ferdig når:** et opptak med kamera gir WAV + MKV + mp4, manifestet lister
begge, `kill -9` midt i gir en gjenopprettet mp4, og dropp-telleren øker når køen
tvinges full i en test.

**Hvor det bevises:** Macen (mye: opptak, `kill -9`, gjenoppretting) +
**Riggen** for den fulle (v)-runden i C3c.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C3a, `docs/VIDEOMOTOR.md` §C3 og §Målbildet, og
> modulhodene i `recorder/native_capture/{mod,segment,writer}.rs`. Gjennomfør
> økta som én PR på `ombygging/c3a-videospor`. Følg fellesreglene. **Slett
> ingenting her** — de gamle stiene dør i C3b. Lydklokka er sjefen: et kamera som
> faller ut skal aldri påvirke lyden, og kamera-callbacken skal aldri blokkere.
> Arv to rettelser: #341s stopprekkefølge (stopp fangsten før writeren,
> tidsgrense på hver venting) og #337s melding om en død lydprosess under
> videoopptak. Prøv `kill -9` midt i et ekte opptak og vis at mp4-en kan
> gjenopprettes.

## C3b — De fire gamle stiene slettes

**Modell:** **Opus** — rundt 12 000 linjer ut, uten reserve (beslutning B5). Det
som fortsatt brukes må **flyttes**, ikke slettes, og det er ikke åpenbart hva det
er.

**Gren:** `ombygging/c3b-slett-gamle-stier` · **Pakke:** C3

**Mål:** Én opptaksmotor i stedet for fire videostier med ulike evner.

**Sletter** (linjetallene er målt 2026-10-12):

| Fil / ting                                  | Linjer         |
| ------------------------------------------- | -------------- |
| `recorder/cpal_capture.rs`                  | 1 562          |
| `recorder/two_process.rs` (kjerne og skall) | 561            |
| `recorder/engine.rs` — **kirurgisk**        | 1 457          |
| `recorder/engine/` (7 filer)                | 4 051          |
| ffmpeg-byggerne i kjernens `capture.rs`     | ≈700–1 300     |
| `device_enum` (begge filene, i sin helhet)  | 977            |
| `media/camera.rs`                           | 165            |
| `media/video_probe.rs`                      | 473            |
| kjernens ffmpeg-preroll-byggere             | ≈280           |
| innstillingen `classic_directshow`          | 38 kallesteder |

**Tre presiseringer som ikke kan hoppes over:**

1. **`engine.rs` slettes kirurgisk.** `SupervisorPanicGuard` (#341) har to
   kallesteder, og det ene er den **generelle** `run_session` som native-økter
   går gjennom i dag. Vakten flyttes; den dør ikke.
2. **Kjernens `preroll.rs` er bare ≈280 linjer ffmpeg** —
   `build_preroll_capture_args`, `build_preroll_trim_args`,
   `ms_to_seconds_string` og testene deres. De øvrige ~800 linjene er
   matematikk den native pre-rollen bruker (segment-beholdning, harvest-trim,
   uttoning, `preroll_restart_delay`). De overlever.
3. **`push_video_encoder_args`, `audio_encode_args` og kanalkartene i
   `capture.rs` beholdes** — de brukes fortsatt av concat, finalize og ASIO.

`preflight`, `diagnostics` og `test_recording` går over til cpal- og
kameralista.

**Port:** `cargo test --workspace`, `npm run reachability`,
`npm run bindings:check`, `windows-check`, og `cargo clippy -D warnings` for
begge mål (døde `#[allow]` og ubrukte importer er lukta av en halvveis sletting).

**Ferdig når:** ingen døde grener, ingen ubrukte funksjoner, en gammel database
leses uten feil, og `reachability` viser at ingen kommando ble uoppnåelig.

**Hvor det bevises:** Macen + CI. Riggbeviset er C3c.

**Levert (C3b).** ≈13 500 Rust-linjer ut (netto ≈11 500). Planen var feil på ett
punkt, rettet her: `recorder/engine/` (7 filer) kunne ikke slettes i sin helhet.
`supervisor.rs`s `run_session` er den generelle økta native-opptak alltid har
gått gjennom, og `finalize.rs`, `emit.rs` og `payloads.rs` er den native
øktas levering, hendelser og IPC-typer. Slettet ble `args.rs`, `process.rs` og
`reader.rs` (ffmpeg-argv, ffmpeg-prosess, stderr-leser) og ffmpeg-delene av
`supervisor.rs` (`run_segment`, rutingen, to-prosess-reserven, ffmpeg-
gjenoppslaget ved gjenkobling). Flyttet, med begrunnelse i PR-en:
`SupervisorPanicGuard` til `supervisor.rs` ved `spawn_supervisor` (den ene
stedet en økt startes), `error_code_str` til `emit.rs`, `sleep_opt`/`wait_opt`
til `native_capture/segment.rs`, `build_separate_audio_args` til
`finalize.rs`, `current_platform`/`recording_preview_path` til `engine.rs`,
`AudioDiagnostics` til kjernens `diagnostics.rs`. Ut over lista gikk også det
bare den slettede fangsten nådde: kjernens `progress.rs`, `selftest::parse_*`
og stderr-telleren, filterbyggerne i kjernens `ffmpeg.rs`, diagnosefunnet
`SR-AUDIO-10` (reserve til DirectShow) og lavfi-soaken. Detaljene står i
VIDEOMOTOR §C3 «Levert (C3b)».

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C3b og `docs/VIDEOMOTOR.md` §C3. Gjennomfør økta
> som én PR på `ombygging/c3b-slett-gamle-stier`. Følg fellesreglene. **Les de
> tre presiseringene i GJENNOMFORING §C3b før du sletter noe:** `engine.rs`
> slettes kirurgisk (`SupervisorPanicGuard` flyttes, den dør ikke), kjernens
> `preroll.rs` har bare ≈280 ffmpeg-linjer, og `push_video_encoder_args`,
> `audio_encode_args` og kanalkartene i `capture.rs` beholdes. Flytt det som
> fortsatt brukes, og si i PR-en hva du flyttet og hvorfor. Kjør clippy for
> **begge** mål — ubrukte importer og døde `#[allow]` er lukta av en halvveis
> sletting. Denne PR-en skal ikke deles med noen annen gren.

## C3c — Tellere og runbooks for den nye motoren

**Modell:** Sonnet · **Gren:** `ombygging/c3c-video-tellere` · **Pakke:** C3

**Mål:** Gjøre den nye motoren synlig for den som tar opp, og for oss.

**Endrer:** telemetri (`video.frames_dropped`), opptaksoverlegget,
[SMOKE-TEST.md](SMOKE-TEST.md), [RIG-DAY.md](RIG-DAY.md) (ny «(v)
Videomotor»-runde).

**Telleren må deployes i telemetri-Workeren FØR versjonen promoteres** — ellers
mister vi tallene fra den første ringen.

**Port:** `npm run check`, `cargo test --workspace`.

**Ferdig når:** den nye (v)-runden er skrevet og grønn på Mac og Windows:

- 90 minutter med klapp i start og slutt — synk innenfor **±1 bilde**;
- USB-kabelen til kameraet trukket i **10 s** midt i — lyden uten hull, bildet
  fryser og kommer tilbake;
- `kill -9` midt i — gjenopprettet mp4;
- CPU-last som i C0.

**Hvor det bevises:** Macen (skrive runden, og kjøre den med Continuity-kameraet)

- **Riggen** (USB-kabel, Windows-boksen, CPU-last).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C3c og `docs/VIDEOMOTOR.md` §C3. Gjennomfør økta
> som én PR på `ombygging/c3c-video-tellere`. Følg fellesreglene. Skriv den nye
> «(v) Videomotor»-runden i `RIG-DAY.md` med de fire punktene i GJENNOMFORING
> §C3c. Si eksplisitt i PR-en at `video.frames_dropped` må deployes i
> telemetri-Workeren **før** versjonen promoteres. Kjør det du kan på Macen med
> Continuity-kameraet, og marker resten som riggpunkter.

## C4a — Forhåndsvisningen kodes i Rust

**Modell:** Sonnet · **Gren:** `ombygging/c4a-preview-rust` · **Pakke:** C4

**Mål:** Kameraøkta åpnes når Opptak-sida viser kamerakortet, og lever videre inn
i opptaket. Ingen overlevering, ingen ventetid.

**Endrer:** `src-tauri/src/camera/preview.rs` (ny), en Tauri-kanal,
`src-tauri/Cargo.toml` (`fast_image_resize`, `jpeg-encoder` — begge MIT/Apache,
ren Rust).

Annethvert til hvert sjuende bilde nedskaleres til 640 px bredde og kodes som
JPEG i Rust. Bildene sendes på kanalen, cirka **4 bilder i sekundet**.

**Port:** `cargo test --workspace`; `cargo deny check licenses` med de to nye
cratene.

**Ferdig når:** forhåndsvisningen kommer fra kameraøkta på ~4 b/s, og
bildefrekvensen holder seg mens et opptak kjører.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C4a og `docs/VIDEOMOTOR.md` §C4. Gjennomfør økta
> som én PR på `ombygging/c4a-preview-rust`. Følg fellesreglene. Slett
> ingenting her — `getUserMedia`-sida dør i C4b. Nedskalering og JPEG i Rust,
> ~4 b/s på en Tauri-kanal. Sjekk at `cargo deny` er grønn med
> `fast_image_resize` og `jpeg-encoder`.

## C4b — `getUserMedia` ut av webviewet

**Modell:** Sonnet · **Gren:** `ombygging/c4b-getusermedia-ut` · **Pakke:** C4

**Mål:** Kameraet skal ikke bytte eier ved start. I dag må webviewets
`getUserMedia` slippe kameraet før fangsten åpner det, og vente 3 s etter
opptaket før det tas tilbake — og to systemer med hvert sitt navn på kameraet
betyr at det som **vises** og det som **tas opp** kan være forskjellige kameraer.

**Sletter** (linjetallene er målt):

| Fil                                              | Linjer |
| ------------------------------------------------ | ------ |
| `app/ui/CameraPreview/LiveCameraPreview.tsx`     | 283    |
| `app/ui/CameraPreview/PolledCameraPreview.tsx`   | 78     |
| `app/ui/CameraPreview/ownership.ts` + test       | 150    |
| `app/ui/CameraPreview/frame-poll-core.ts` + test | 185    |

Pluss ffmpegs ekstra JPEG-utgang (`preview_jpg`) og `PREVIEW_RESTART_MS`.

**Port:** `npm run check`, `npm run e2e` (med fixture-bilder),
`npm run reachability`.

**Ferdig når:** bildet før og under opptaket kommer fra **samme** økt, uten
avbrudd ved Start og Stopp — bevist i e2e med fixture-bilder og manuelt på
Macen.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C4b og `docs/VIDEOMOTOR.md` §C4. Gjennomfør økta
> som én PR på `ombygging/c4b-getusermedia-ut`. Følg fellesreglene. Alt
> `getUserMedia`-eierskap går ut: `LiveCameraPreview`, `PolledCameraPreview`,
> `ownership`, `frame-poll-core`, `preview_jpg` og `PREVIEW_RESTART_MS`. Bevis i
> e2e med fixture-bilder at bildet ikke blinker ved Start og Stopp, og prøv det
> manuelt på Macen også.

## C5a — Kameraet kobler seg til igjen, uavhengig av lyden

**Modell:** Sonnet · **Gren:** `ombygging/c5a-kamera-gjenkobling` · **Pakke:** C5

**Mål:** Faller kameraet ut, skal lyden ikke merke noe. Når samme stabile ID
dukker opp igjen, startes kameraøkta på nytt, og konformeren fyller hullet.

**Endrer:** `src-tauri/src/camera/`,
`src-tauri/src/audio/device_watch.rs` (kamera-halvdelen).

- **Mac:** `AVCaptureDeviceWasDisconnected` / `…WasConnected`.
- **Windows:** kameradelen av enhetslytteren fra A4 —
  `KSCATEGORY_VIDEO_CAMERA` via `CM_Register_Notification`.

**Port:** `cargo test --workspace`, `windows-check`.

**Ferdig når:** kabeltrekket fra C3c gir et gjenopprettet bilde **uten at
opptaket stopper**, på begge plattformer. På Macen prøves det med
Continuity-kameraet, som kommer og går av seg selv.

**Hvor det bevises:** Macen (Continuity) + **Riggen** (USB-kabel på Mac og
Windows).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C5a og `docs/VIDEOMOTOR.md` §C5. Gjennomfør økta
> som én PR på `ombygging/c5a-kamera-gjenkobling`. Følg fellesreglene.
> Windows-halvdelen bygger på enhetslytteren fra A4 — les den først. Gjenkoblingen
> skal kjenne igjen **samme stabile ID**, ikke et nummer. Prøv det på Macen med
> Continuity-kameraet: slå det av og på, og bekreft at lyden ikke merker noe.

## C5b — UI og varsel når kameraet er borte

**Modell:** Sonnet · **Gren:** `ombygging/c5b-kamera-varsel` · **Pakke:** C5

**Mål:** Den frivillige skal vite at opptaket lever, selv om bildet er borte.

**Endrer:** opptakssida i `app/`, i18n for sju språk,
[VARSLING.md](VARSLING.md).

UI-et sier «Kameraet falt ut — lyden tas fortsatt opp». Er kameraet borte i over
**60 s**, går det et varsel.

**Port:** `npm run check` (i18n-vaktene), `npm run test`.

**Ferdig når:** meldingen vises ved utfall og forsvinner ved gjenkobling, og
varselet kommer etter 60 s — ikke før.

**Hvor det bevises:** Macen (Continuity-kameraet gir både utfall og gjenkobling).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §C5b, `docs/VIDEOMOTOR.md` §C5 og
> `docs/VARSLING.md`. Gjennomfør økta som én PR på
> `ombygging/c5b-kamera-varsel`. Følg fellesreglene. Teksten er «Kameraet falt
> ut — lyden tas fortsatt opp», på alle sju språk. Varselet kommer etter 60 s,
> ikke før. Prøv hele veien på Macen med Continuity-kameraet.

## M3R — Opus-review av M3

**Modell:** **Opus** · **Gren:** ingen (review)

**Ser etter:**

- **Synk-skjøten.** Lydklokka og kameraets vertstid møtes i konformeren. Er
  konverteringen riktig i én retning og feil i den andre, drifter leppesynken
  over 90 minutter og ingen test fanger det.
- **Slettingens rester.** Døde `#[allow]`, ubrukte importer, kodeveier som
  returnerer `unreachable!()`, innstillingsfelt uten lesere.
- **At de to lærdommene faktisk er arvet:** #341s stopprekkefølge og #337s
  melding om død lydprosess. Søk etter dem i den nye motoren, ikke i den gamle.
- **Krasjgjenoppretting med to filer.** Manifestet lister WAV og MKV; hva skjer
  når bare én av dem finnes?
- At `keep_separate_audio` nå er gratis og faktisk virker.

**Ferdig når:** nummererte funn med alvorsgrad; 🔴/🟠 rettet før beta.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §M3R. Gå gjennom alle PR-ene i M3 som én helhet.
> Begynn med synk-skjøten mellom lydklokka og kameraets vertstid, og med
> krasjgjenoppretting når bare én av de to filene finnes. Søk i den **nye**
> motoren etter #341s stopprekkefølge og #337s melding om død lydprosess — er de
> arvet, eller mistet? Let etter rester av slettingen. Rapporter nummererte funn
> med alvorsgrad. Ikke rett noe her.

---

# M4 — Sanntid, fart og resten (1.0)

B6 trenger nøkkelbildene fra C0b og lydkjeden fra B2b, så den kan først tas her.
D1, D2 og D3 er helt uavhengige av alt annet og kan tas når som helst etter M1.

## B4a — Én dekoding når fila åpnes

**Modell:** Sonnet · **Gren:** `ombygging/b4a-en-dekoding` · **Pakke:** B4

**Mål:** I dag leses en fil fem–seks ganger fra den åpnes til den er eksportert:
bølgeformen (8 kHz), prekensøket (16 kHz), kanalanalysen (`astats`),
lydstyrkemålingen og eksportens to runder dekoder hver for seg.

**Endrer:** analyseveien i `src-tauri/src/editor/`.

Ett ffmpeg-rør i opptakets **egen** samplingsfrekvens. I samme runde fordeles
PCM-en til:

- bølgeform i flere oppløsninger,
- resampling til 16 kHz for `audio_analysis`/`detect`,
- kanalanalyse (erstatter `astats`-runden),
- lydstyrke (B1a) og brumsjekk (B3a).

Sidevognene (`.peaks.json`, segmentene) beholdes, og nøkles på samme måte.

**Avvik fra planen, funnet i B4a:** «lydstyrke» kan ikke samles i åpningsrunden.
Programmålingen (`pcm::measure_program`, forhåndslytting og eksport) måler
signalet _etter_ kutt og kjede, og både kuttene og kjeden (mikser, «Tale»s
vedtak) finnes ikke når fila åpnes. Den forblir en egen runde, med egen cache
(`PLAN_BOOK`). Brumsjekken (B3a) er med: den er en del av «Tale»s analyse, som
nå får PCM-en fra åpningsrunden. Det som er samlet er altså fire lesinger til
én (bølgeform, prekensøk, kanalmåling, «Tale»); programmålingen og eksportens
kjede-runde står igjen. B4b bevises derfor på analysene, ikke på «hele
eksporten».

**Port:** `cargo test --workspace`; alle eksisterende analysetester.

**Ferdig når:** alle fem forbrukerne får data fra samme runde, og ingen
analysetest måtte endres for å bli grønn.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B4a og `docs/LYDMOTOR.md` fase 4. Gjennomfør økta
> som én PR på `ombygging/b4a-en-dekoding`. Følg fellesreglene. Røret skal gå i
> opptakets egen samplingsfrekvens, og resamplingen til 16 kHz skje i Rust.
> Sidevognene beholdes og nøkles som før. Måtte du endre en eksisterende
> analysetest for å få den grønn, si hvorfor i PR-en — det er et varsel.

## B4b — `astats`-runden ut, og telleren som beviser det

**Modell:** Sonnet · **Gren:** `ombygging/b4b-astats-ut` · **Pakke:** B4

**Mål:** Bevise at det faktisk bare er én dekoding, ikke bare tro det.

**Endrer / sletter:** `astats`-runden i kanalanalysen.

**Port:** `cargo test --workspace` med en **teller** som viser antall
dekodinger.

**Ferdig når:** å åpne en 90-minutters fil dekoder den **én** gang, sjekket med
telleren i en test, og alle eksisterende analyse-tester er grønne.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B4b og `docs/LYDMOTOR.md` fase 4. Gjennomfør
> økta som én PR på `ombygging/b4b-astats-ut`. Følg fellesreglene. Porten er en
> teller i en test som viser at en 90-minutters fil dekodes én gang — ikke en
> antakelse. Mål åpningstiden før og etter og skriv begge i PR-en.

**Gjort i B4b:** `astats`-resten er slettet (`levels::parse_levels`/`parse_noise_floor_db`,
`decode_analysis_pcm` og `analysis_decode_args`). Sammenligningen mot de gamle rutene er
beholdt som orakel mot ffmpeg selv (`the_round_agrees_with_ffmpeg_itself_or_skips`):
ffmpegs egen resampler, `astats` og 8 kHz-nedmiks er en uavhengig implementasjon, og kjernetestene
mot brute force låser bare målerne til sin egen definisjon. Telleren (`analysis_decodes_of`)
teller nå alle helfilslesinger som måler, per slag (`Decode`): åpningsrunden, eksportens
måle-pass (`Staging`), programmålingen (`Programme`) og `volumedetect`-proben (`PeakProbe`).
Porten er `a_ninety_minute_recording_is_decoded_once_or_skips` (en 90-minutters fil, alle
forbrukere, én lesing). Skygge-VAD (`vad`) tar nå PCM-en fra åpningsrunden
(`take_shadow_pcm`) i stedet for å dekode på nytt; prisen er at 16 kHz-PCM-en (~345 MB for 90
minutter) lever til skyggen har tatt den, bare i `vad`-bygg.

## B5a — Avspillingsmotoren

**Modell:** **Opus** — en sanntids lydtråd med cpal som klokke. Allokering,
låsing eller en treg kjede i callbacken gir hakk og klikk som ikke kan testes
inn etterpå.

**Gren:** `ombygging/b5a-avspillingsmotor` · **Pakke:** B5

**Mål:** Forhåndslyttingen er ikke ekte tid i dag: «Etter» er en 20-sekunders
mp3 som må rendres av ffmpeg før den kan høres, og avspillingen er et
`<audio>`-element der kuttene glattes ved å skru volumet ned og opp.

**Endrer:** `src-tauri/src/playback/` (ny).

Kjeden: ffmpeg-dekoding fra en posisjon → ringbuffer → kutt-plan med
konstant-effekt-overganger → lydkjeden fra B2a → resampler til enhetens
frekvens → cpal. **Lydkortet er klokka**, og spillehodet i UI-et følger
hendelser fra motoren.

**Krav som ikke kan fires på:** ingen allokering i lydtråden, ingen lås som kan
ventes på, og kjeden må holde sanntid med god margin (B0c-tallene sier 18–21×).

**Port:** `cargo test --workspace`; manuell lytting på Macen.

**Ferdig når:** avspilling og hopp er uten klikk og hakk på Macen, og en test
viser at lydtråden ikke allokerer.

**Hvor det bevises:** Macen + **Riggen** (Windows).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B5a og `docs/LYDMOTOR.md` fase 5 og §Målbildet.
> Gjennomfør økta som én PR på `ombygging/b5a-avspillingsmotor`. Følg
> fellesreglene. Lydkortet er klokka; spillehodet i UI-et følger hendelser fra
> motoren, ikke omvendt. **Ingen allokering og ingen ventende lås i lydtråden** —
> skriv en test som viser det. UI-arbeidet hører til B5b og B5c; her bygges bare
> motoren, med en liten kjører som beviser den.

**Gjort i B5a:** motoren i `src-tauri/src/playback/` (kutt-planen i
`sundayrec_core::playback`) og kjøreren `examples/playback.rs`. Arkitektur, bevis og
målinger: `docs/LYDMOTOR.md` §Fase 5 «Gjort i B5a»; lytteprøven: RIG-DAY «(a)
Avspillingsmotoren». To ting B5b/B5c arver: (1) lydkjeden kjører i produsenttråden før
ringen (den allokerer), så et profilbytte midt i avspilling bør gjøres som et hopp til
samme posisjon for å høres med en gang; (2) eksporten bruker fortsatt 15 ms ut/inn-toning
ved kutt, ikke kutt-planens 20 ms konstant-effekt-overganger.

## B5b — Før/Etter blir et øyeblikkelig bytte

**Modell:** Sonnet · **Gren:** `ombygging/b5b-for-etter` · **Pakke:** B5

**Mål:** Lyd-steget først: «Før/Etter» skal være et bytte, ikke en ventetid, og
profilen skal kunne byttes mens det spiller.

**Endrer:** lyd-steget i `app/editor/`.

**Port:** `npm run check`, `npm run test`.

**Ferdig når:** profilbytte midt i avspilling er uten klikk, og «Etter» høres
umiddelbart — ingen mp3 rendres.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B5b og `docs/LYDMOTOR.md` fase 5. Gjennomfør
> økta som én PR på `ombygging/b5b-for-etter`. Følg fellesreglene. Bytte av
> profil midt i avspilling skal være uten klikk — filmcrafts parametre glattes
> per sampel, så det skal gå. Slett ingenting her; `master_preview` dør i B5c.

**Gjort i B5b:** Før/Etter er to lag i ringen som lydtråden krysstoner på 20 ms (hørt innen
20 ms, også mens det spiller), og en profil- eller mikserendring mens det spiller bygges som en
ny kjede ved siden av og krysstones inn i produsenten (40 ms, uten klikk eller pause). Ingen mp3
gjengis for lyd-steget. Arkitektur, bevis og målinger: `docs/LYDMOTOR.md` §Fase 5 «Gjort i B5b»;
lytteprøven: RIG-DAY «(b) Før/Etter og profilbytte». Til B5c: `master_preview`,
`editor_prepare_listen` er fortsatt i bruk (forhåndsmålingen deler `listen_plan`), temp-mp3-ene
rydder ingen sveip (nå opprettes de ikke lenger av lyd-steget); eksporten bruker fortsatt 15 ms
ut/inn-toning ved kutt.

## B5c — `<audio>` ut av Klipp-steget

**Modell:** Sonnet · **Gren:** `ombygging/b5c-audio-element-ut` · **Pakke:** B5

**Mål:** Motoren erstatter `<audio>`, volumrampen og avspillingsproxyen.

**Sletter:** `master_preview` og temp-mp3-ene,
`extract_playback_proxy`, og volum-de-klikket i `app/editor/playback.ts`.

**Port:** `npm run check`, `npm run test`, `npm run reachability`,
`cargo test --workspace`.

**Ferdig når:** avspilling, hopp og bytte av profil er uten klikk og hakk på Mac
**og** Windows, og spillehodet holder seg i synk gjennom en hel gudstjeneste
(nytt RIG-DAY-punkt).

**Hvor det bevises:** Macen + **Riggen** (Windows, og 90-minutters synk).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B5c og `docs/LYDMOTOR.md` fase 5. Gjennomfør
> økta som én PR på `ombygging/b5c-audio-element-ut`. Følg fellesreglene.
> `master_preview`, temp-mp3-ene, `extract_playback_proxy` og volumrampen i
> `playback.ts` skal være borte. Skriv et nytt RIG-DAY-punkt: spillehodet i synk
> gjennom en hel gudstjeneste, på Mac og Windows.

## B6 — Rask videoeksport

**Modell:** Sonnet · **Gren:** `ombygging/b6-rask-videoeksport` · **Pakke:** B6

**Mål:** Når videoen bare **kuttes**, skal videostrømmen kopieres i stedet for å
kodes om.

**Krever:** nøkkelbildene fra **C0b** og lydkjeden fra **B2b**. Begge må være
merget.

**Endrer:** eksportveien i `src-tauri/src/editor/`.

Kuttene snappes til nøkkelbildene (`-c:v copy`), og lyden rendres i Rust over
**nøyaktig samme** intervaller og muxes inn. At lyd og bilde bruker samme
intervaller er hele poenget — bommer de, drifter synken ved hvert kutt.

**Port:** `cargo test --workspace`; en ekte 90-minutters video på Macen.

**Ferdig når:** «Behold bare prekenen» på en 90-minutters video tar **sekunder,
ikke minutter**, og lyd og bilde er i synk ved **hvert** kutt (nytt
RIG-DAY-punkt).

**Hvor det bevises:** Macen (fart og synk) + **Riggen** (bekreftelse).

**Rettelser til planen (funnet i B6):**

- **Én liste av intervaller, ikke to.** Planen (`sundayrec_core::video_copy::plan_copy_cut`)
  snapper brukerens kutt til det en kopi kan, og de snappede intervallene erstatter
  `keeps` i hele eksporten: concat-listen for bildet og den Rust-rendrede lyden
  leser samme liste. Synken kan ikke gli ved et kutt, fordi det ikke finnes to tall som
  kan være uenige. Lyden går alltid via Rust-stien i kopimodus (også uten profil), siden
  det er den som rendrer nøyaktig disse intervallene.
- **Starten snapper tilbake, slutten fram.** Starten til nærmeste _rene_ nøkkelbilde før
  kuttet (aldri etter, så ingen ord forsvinner; opptil 2,5 s tilbake), slutten til neste
  sted der strømmen kan deles uten at B-bilder krysser (som regel 0–3 bilder
  fram). Nøkkelbilder med ledende bilder fra forrige gruppe (åpen GOP) er ikke
  start-steder.
- **Det som ikke kan kopieres nøyaktig, kodes om som før**, med grunn i loggen: nøkkelbilde
  lenger tilbake enn 2,5 s (opptak fra før C0b), et kutt kortere enn snappen
  (det ville forsvunnet), en kilde som ikke er mp4/mov, en kodek som ikke er den
  eksporten ber om (HEVC-opptak, standard H.264), eller pakker uten tidsstempler.
  Svikter selve kopien (ffmpeg-feil, eller bilde og lyd ulikt langt over 70 ms),
  kastes den og eksporten kjøres på nytt som omkoding.
- **Tre ffmpeg-fakta** (målt på 9.0.1, bevist på nytt av testene): concat-demuxerens
  `outpoint` kutter på **dts**, ikke pts (B-bilder gir to bilder for mye med pts);
  uten `duration` plasseres neste del etter dts-avstanden (bildet glir mot lyden ved
  hvert kutt); og CLI-en re-baserer inngangen på containerens starttid, som concat
  melder 80 ms for tidlig på grunn av AAC-forsinkelsen (løst med `-copyts` og
  `-itsoffset` for avrundingen).
- **Målt (syntetisk 90-minutters 720p30-video, M1 Pro, release, prekenen = 30 min):**
  uten profil 82,9 s før, 20,4 s etter; med «Tale» 120,9 s før, 51,3 s etter. Det som
  er igjen er lyden, ikke bildet: AAC-koding (standardkoderen `twoloop` går 31× sanntid,
  `-aac_coder fast` 133×, derfor `fast` i kopimodus) og Rust-kjeden. Bildet tar 0,2 s å
  kopiere. «Sekunder» gjelder derfor bildet; hele eksporten tar ca. 20 s uten og ca.
  50 s med profil. Måles av `b6_bench_the_sermon_cut_on_a_long_video` (`--ignored`).
- **Forutsetningene i B6-teksten holdt:** C0b og B2b er merget (nøkkelbilder hvert 2. s,
  `resolve_audio_params`/`pre_chain_for` delt med forhåndslyttingen).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §B6 og `docs/LYDMOTOR.md` fase 6. Gjennomfør økta
> som én PR på `ombygging/b6-rask-videoeksport`. Følg fellesreglene. Sjekk først
> at C0b (nøkkelbilder) og B2b (lydkjeden) er merget. Lyden må rendres over
> **nøyaktig** de samme intervallene som videokuttene snappes til — det er der
> synken går tapt. Ta tiden på en ekte 90-minutters video før og etter.

## D1 — `earshot` erstatter tract og Silero

**Modell:** Sonnet · **Gren:** `ombygging/d1-earshot-vad` · **Pakke:** D1

**Mål:** Samme VAD, lettere motor. Skyggemodusen og porten i [VAD.md](VAD.md) er
**uendret** — den får bare en annen motor, og bestemmer fortsatt ingenting.

**Endrer / sletter:**

| Hva                                                                         | Status                  |
| --------------------------------------------------------------------------- | ----------------------- |
| ny `VadBackend` med `earshot` (MIT/Apache, 16 kHz, 256-samples rammer)      | legges til              |
| `tract-onnx`                                                                | ut                      |
| `src-tauri/resources/vad/` (2 845 718 byte)                                 | ut                      |
| SHA-sjekken i `src-tauri/build.rs`                                          | ut                      |
| `vad`-jobbens modellkontroll i `ci.yml`                                     | ut                      |
| `vad`-featuren                                                              | beholdes, men blir lett |
| `crates/sundayrec-core/src/ab_eval.rs`, `src-tauri/examples/vad_ab_eval.rs` | ny motor                |
| [VAD.md](VAD.md)                                                            | skrives om              |

**Påstanden om at `earshot` slår Silero er utviklerens egen,** og sang kan bli
tolket som tale. Det er greit: VAD-en er i skyggemodus og bestemmer ingenting
før porten i VAD.md er passert.

**Port:** `cargo test --features vad vad::`, `cargo clippy --features vad`,
`vad`-jobben i CI.

**Ferdig når:** `ab_eval` kjører med den nye motoren og gir tall som kan
sammenlignes med de gamle, og VAD.md er skrevet om.

**Hvor det bevises:** Macen.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §D1, pakke D1 i `docs/OMBYGGING.md` og
> `docs/VAD.md`. Gjennomfør økta som én PR på `ombygging/d1-earshot-vad`. Følg
> fellesreglene. Skyggemodusen og porten i VAD.md er **uendret** — bare motoren
> byttes. `ab_eval` ligger i `crates/sundayrec-core/src/ab_eval.rs` med kjøreren
> i `src-tauri/examples/vad_ab_eval.rs`. Kjør den med begge motorene og vis
> tallene side om side i PR-en før du sletter `tract-onnx`, modellfila og
> SHA-sjekken i `build.rs`.

## D2 — macOS: leser systemet SundayRecs varsler?

**Modell:** Sonnet · **Gren:** `ombygging/d2-varselstatus-mac` · **Pakke:** D2

**Mål:** I dag er svaret `unknown` + et testvarsel. Med
`UNUserNotificationCenter.getNotificationSettings` kan appen si det den vet.

**Endrer:** `src-tauri/src/notify/`, `src-tauri/Cargo.toml`
(`objc2-user-notifications` — allerede i `Cargo.lock`), [VARSLING.md](VARSLING.md),
[PLAN.md](PLAN.md).

Bare når appen kjører som `.app`. **Utenfor en bundle er svaret `unknown`, som i
dag** — det må testes, ikke antas.

**Port:** `cargo test --workspace`; manuell prøve på Macen med varsler av og på
i Systeminnstillinger.

**Ferdig når:** «Hvem får beskjed?»-kortet viser riktig status i begge
tilstander, og `unknown` når appen kjøres utenfor en bundle.

**Hvor det bevises:** Macen — hele punktet kan lukkes her.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §D2, pakke D2 i `docs/OMBYGGING.md` og
> `docs/VARSLING.md` §«Gjenstår fra runde 2». Gjennomfør økta som én PR på
> `ombygging/d2-varselstatus-mac`. Følg fellesreglene. Bruk
> `objc2-user-notifications` (samme familie som resten av `src-tauri`), ikke en
> Swift-pakke. Utenfor en bundle skal svaret fortsatt være `unknown` — test det.
> Bygg en bundle og prøv med varsler både av og på i Systeminnstillinger, og
> stryk punktet i `PLAN.md`.

## D3 — Lydstyrke under opptak

**Modell:** Sonnet · **Gren:** `ombygging/d3-lufs-under-opptak` · **Pakke:** D3

**Mål:** En dempet mikser skal oppdages mens opptaket går, ikke etterpå.

**Endrer:** den native motoren (måleren fra B0a kjører på den rutede
f32-strømmen), opptaksoverlegget, historikken, [VARSLING.md](VARSLING.md).

- Under opptak: kortsiktig LUFS ved siden av toppnivåene.
- Etter hvert opptak: integrert LUFS i historikken.
- Under **−40 LUFS i mer enn 5 minutter** gir et varsel: «Opptaket er svært
  svakt — er mikseren dempet?»

**Port:** `cargo test --workspace` med en test for terskelen og en for at et
normalt nivå ikke utløser den.

**Ferdig når:** et opptak med dempet mikser gir varselet, og et normalt opptak
gir det ikke.

**Hvor det bevises:** Macen delvis (innebygd mikrofon, 1 kanal) + **Riggen**
(dempet mikser på flerkanalskortet).

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §D3 og pakke D3 i `docs/OMBYGGING.md`. Gjennomfør
> økta som én PR på `ombygging/d3-lufs-under-opptak`. Følg fellesreglene.
> Måleren fra filmcraft gikk 200× sanntid, så den skal kunne kjøre på den rutede
> f32-strømmen uten å røre opptaket — bevis at den ikke påvirker xrun-tallet fra
> A3c. Terskelen er under −40 LUFS i over 5 minutter. Skriv riggpunktet for
> dempet mikser på flerkanalskortet.

## M4R — Opus-review før 1.0

**Modell:** **Opus** · **Gren:** ingen (review)

**Ser etter:**

- **Hele kjeden fra opptak til eksport**, lest som én historie: lydkort → native
  motor → WAV → åpning → analyse → kjede → lytting → eksport → levering.
- At [PLAN.md](PLAN.md) ikke lenger har åpne punkter for ombyggingen.
- At alle riggpunktene fra M1–M4 er krysset av i [RIG-DAY.md](RIG-DAY.md), eller
  står som bevisste unntak med grunn (opptakskortet er ett slikt).
- At ingen nødluke, reservesti eller `classic_*`-innstilling er igjen.
- At telemetri-tellerne fra A3c, C3c og D3 er deployet i Workeren.

**Ferdig når:** nummererte funn med alvorsgrad, og en setning som sier om appen
er klar for 1.0 og bred utrulling.

**Startmelding:**

> Les `docs/GJENNOMFORING.md` §M4R. Dette er den siste porten før 1.0. Les hele
> kjeden fra lydkort til levert fil som én historie, og si hvor den er svakest.
> Sjekk at `PLAN.md` er tom for ombyggingen, at alle riggpunktene i `RIG-DAY.md`
> er krysset av eller står som bevisste unntak, at ingen `classic_*`-innstilling
> er igjen, og at telemetri-tellerne fra A3c, C3c og D3 er deployet. Rapporter
> nummererte funn med alvorsgrad, og avslutt med én setning om appen er klar for
> 1.0.

---

# Rekkefølge

## Arbeidstrærne

Hvert spor får sitt eget arbeidstre, så to økter aldri konkurrerer om samme
fil. Trærne lages med `git worktree add`, under
`sunday-suite/wt/`, slik resten av suiten gjør det.

```bash
git worktree add ../wt/omb-grunn -b ombygging/a1a-lisensvakt main
```

### M1 — tre spor samtidig

| Arbeidstre     | Kjede                            | Hvorfor seriell innenfor sporet                                |
| -------------- | -------------------------------- | -------------------------------------------------------------- |
| `wt/omb-grunn` | A1a → A1b → A2                   | Rører bare CI, bunten og dokumenter — konflikter med ingenting |
| `wt/omb-a3`    | A3a → A3b → A3c → A5a → A5b → A4 | A3a berører ~30 filer i `src-tauri` og konflikter med alt der  |
| `wt/omb-c0`    | C0a → C0b                        | Bare kjernens `capture.rs`                                     |

**A4 ligger sist i `wt/omb-a3`,** ikke fordi den hører til A3, men fordi
cpal 0.18 kan flytte `windows`-versjonen A4 bygger vtabellen sin på. Rekkefølgen
er en konsekvens av avhengigheten, ikke av temaet.

### M2 og M3 — parallelt (eiers beslutning 2026-10-12)

| Arbeidstre     | Kjede                                                                                                                             |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `wt/omb-lyd`   | B0a → B0b → B0c → B1a → B1b; **B2a kan starte så snart B0a er merget** → B2b → B3a → B3b. **B2c ligger åpen til Ørene har svart** |
| `wt/omb-video` | C1a ∥ C1b (uavhengige) → C1c → C2 → C3a → C3b → C3c; deretter C4a → C4b ∥ C5a → C5b                                               |

Berøringsflatene er disjunkte: lyden bor i `crates/sundayrec-core`
(`loudness`, `chain`, `mastering`, `processing`) og `app/editor`, videoen i
`src-tauri/src/{camera,recorder}` og `app/ui/CameraPreview`.

**Prisen, så den er sagt:** to betaringer i luften samtidig, `CHANGELOG.md` og
`PLAN.md` som små konfliktpunkter mellom sporene, og at Ørene får både ny lyd og
ny video å vurdere i samme periode. Blir det for mye på én riggdag, er **M2** den
som kan vente — M3 har flest ubekreftede riggpunkter (w6, og leppesynk over en
hel gudstjeneste på Windows).

### M4 — tre spor samtidig

| Arbeidstre       | Kjede                              |
| ---------------- | ---------------------------------- |
| `wt/omb-sanntid` | B4a → B4b → B5a → B5b → B5c        |
| `wt/omb-b6`      | B6 (etter at C0b og B2b er merget) |
| `wt/omb-d`       | D1 ∥ D2 ∥ D3 — helt uavhengige     |

## Aldri parallelt

- **A3a** mot noe annet i `src-tauri`. Den rører ~30 filer.
- **C3b** mot noe som helst. ~12 000 linjer ut konflikter med alt, og en rebase
  midt i en sletting er der rester blir igjen.
- **B2c** før eieren har sagt ja. Det er ikke en teknisk sperre, men den er like
  bindende.

## Milepælsportene

Hver milepæl: alle øktene merget → `MxR`-reviewen lest og 🔴/🟠 rettet →
beta-utgivelse → riggdag → stabil. Ringen er et manuelt steg og **enveis**: et
rent tagg går til stabil og beta samtidig. En GitHub-utgivelse er ikke en
utgivelse.

---

# Det Macen ikke kan bevise

Samlet, så ingen økt blir erklært ferdig på et bevis som ikke finnes.
Maskinvaren er sjekket 2026-10-12.

| Mangler                | Rammer                                         | Når                                                   |
| ---------------------- | ---------------------------------------------- | ----------------------------------------------------- |
| Windows-maskin         | A4, C0a, C1b, og Windows-halvdelen av A3b/C5a  | Windows-boksen. CI bygger, men beviser ikke oppførsel |
| ASIO                   | RIG-DAY (f), A3s ASIO-vranglåser               | Windows-boksen                                        |
| 24-bit lydkort         | A3s WASAPI-24-bit-sjekk                        | Riggdagen (kortet finnes)                             |
| Flerkanals mikser      | A3 kanalvalg, RIG-DAY (a) og (h), D3s terskel  | Riggdagen (mikseren finnes)                           |
| USB-webkamera          | C1c og C3c sitt kabeltrekk                     | Riggdagen (kameraet finnes)                           |
| **HDMI-opptakskort**   | C1s opptakskort-punkt, Caps DirectShow-reserve | **Finnes ikke.** Punktet står åpent, ikke blokkerende |
| Ekte gudstjenester     | B2c, B3b — Ørene                               | Riggdagen. B2c er blokkert på dette                   |
| Ekte kirkerom med brum | B3b                                            | Riggdagen                                             |

**Det Macen kan mer av enn man tror:** Continuity-kameraet kommer og går som en
ekte AVFoundation-enhet, så C5a, C5b og macOS-halvdelen av A4 kan prøves uten en
eneste kabel. Og frekvensbytte midt i et opptak (A3b) kan tvinges fra Audio
MIDI-oppsett.
