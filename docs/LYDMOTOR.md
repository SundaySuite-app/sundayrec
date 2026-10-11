# Lydmotoren — redigeringens lyd i Rust, ffmpeg bare til filer

_Skrevet 2026-10-11, ved v0.25.2. Status: **plan, ikke startet.** Eierens
beslutning: gjennomføres, og vi tar risikoen._

Dette er planen for å bytte ut hvordan **Rediger** behandler lyd. I dag går all
lyd gjennom ffmpeg-filtre, og en fil leses fra start til slutt fem–seks ganger
fra den åpnes til den er eksportert. Etter planen dekodes fila av ffmpeg, mens
alt mellom dekoding og koding skjer i Rust: analyse, lydkjede, lydstyrke og
forhåndslytting. Kilden til DSP-en er `audio-dsp`-craten fra
[filmcraft](https://github.com/storytold/filmcraft) (MIT OR Apache-2.0).

Det åpne står med én linje i [`PLAN.md`](PLAN.md); detaljene står her.

## Hvorfor

Målt og lest 2026-10-11. Tallene er fra en sky-container med 4 kjerner,
ffmpeg 6.1 (ikke den medfølgende 8.1.2) og syntetiske signaler. De viser
retningen, ikke fasiten.

1. **Fila leses mange ganger.** Bølgeformen (`peaks`, 8 kHz-dekoding),
   prekensøket (`segments`, 16 kHz), kanalanalysen (`auto_process`/`astats`),
   lydstyrkemålingen (`mastering_analyze`) og eksportens to runder (måling av
   det kuttede signalet, så påføring) dekoder hver for seg.
2. **`loudnorm` er treg og treffer ikke alltid.** Kjeden «voice-noisy-room» gikk
   32× sanntid uten `loudnorm` og 10× med. `mastering.rs` dokumenterer at
   `linear=true` bare er et ønske: ffmpeg faller tilbake til dynamisk
   regulering, og `plan_pass2` må senke målet når toppene er høye. En preken
   med kraftige plosiver blir dermed svakere enn målet profilen lover.
3. **ffmpeg mangler verktøyene en kirke trenger mest.** Det finnes ingen ekte
   romklangfjerning (`DereverbStage` er en `agate`-ekspander) og ingen
   brumfjerning (bare høypass på grunntonen).

   | Syntetisk test              | ffmpeg slik SundayRec bruker det | filmcraft `audio-dsp`    |
   | --------------------------- | -------------------------------- | ------------------------ |
   | Romklang, RT60 1,5 s: halen | −36,3 → −36,4 dB                 | −36,3 → **−46,9 dB**     |
   | Romklang: direktelyden      | urørt                            | urørt (−21,1 → −21,2 dB) |
   | Sus på −45 dBFS: støygulvet | −44,8 → −46,5 dB (`tn=1`)        | −44,8 → **−58,8 dB**     |
   | Sus: «talen»                | urørt                            | urørt                    |

   `afftdn` uten `tn=1` og med riktig `nf` tok −57 dB. Dagens innstillinger
   gjør altså lite. Dette bør sjekkes med 8.1.2 uansett om planen gjennomføres.

4. **Standardprofilen renser ingenting.** «Tale» (`sound-profiles.ts`) er
   `speech-clear`: høypass, EQ, kompressor og `loudnorm`. Støy- og
   romklangfjerning nås bare gjennom den avanserte mikseren.
5. **Forhåndslyttingen er ikke ekte tid.** «Etter» er en 20-sekunders mp3 som
   rendres av ffmpeg (`master_preview`) og blir liggende i temp-mappa
   (`is_preview_temp_name` har ingen sveip). Avspillingen er et `<audio>`-element
   der kuttene glattes ved å skru volumet ned og opp (`playback.ts`).

## Beslutningen: ffmpeg blir, men bare til filer

ffmpeg er ikke en svak kodek. Det er verktøykassen de fleste bruker, og
kodekene i den (libx264, LAME, libopus, FLAC) er de beste eller
standarden. Det svake er **filtrene**. Derfor:

- **ffmpeg beholder:** opptak (AVFoundation/DirectShow og videokoding),
  dekoding til PCM på et rør, koding fra PCM på et rør, muxing og remuxing.
  Fortsatt som egen prosess, av de samme grunnene som før (GPL-binæren lenkes
  ikke inn, og en krasj i en dekoder tar ikke ned appen).
- **Rust overtar:** alt mellom dekoding og koding.
- **filmcraft erstatter ikke ffmpeg.** Den har ingen MP3- eller FLAC-koder (og
  MP3 er SundayRecs standard opptaksformat), ingen opptak, og kodekene er
  mindre testet på ekte filer fra kameraer og telefoner. Kodekene og
  containerne deres tas ikke inn.

## Kilden: `filmcraft-audio-dsp`

- **Hva:** ITU-R BS.1770 / EBU R128-måling (momentary, short-term, integrert,
  LRA, true peak) og effektene vi trenger: biquad-filtre, DeNoise
  (spektral gating med minimumsstatistikk, uten innlæringssteg), DeReverb
  (statistisk sen-etterklang, Lebart/Habets), DeHum (notchkjede på 50/60 Hz og
  overtonene), gate, EQ, kompressor, de-esser, true-peak-limiter med
  forsinket lytting, klikkfjerner og «Enhance Speech». Den har også en
  strømmende resampler og konstant-effekt-overganger.
- **Hvorfor den passer:** cirka 12 500 linjer, ingen avhengigheter, ingen
  `unsafe`, planar `f32` inn og ut, ingen allokering i `process`, og
  parametre som glattes per sampel (kan endres mens lyden spiller). Hver
  effekt oppgir sin forsinkelse. 122 tester, og måleren er testet mot ffmpegs
  `ebur128` til ±0,1 LU. Den bryter ikke kjernens renhetsregel: ingen I/O,
  ingen klokke.
- **Fart, én kjerne:** hele kjeden (brum, støy, romklang, gate, EQ, kompressor,
  de-esser, limiter) gikk 18–21× sanntid, så en 90-minutters gudstjeneste tar
  rundt 4–5 min. To kanaler kan kjøres parallelt. Måleren gikk 200× sanntid.
- **Vurdering:** prosjektet var 11 dager gammelt, med 1 382 commits og flere
  hundre av dem skrevet med Claude/Codex. Eieren har testet produktet. Lyddelen
  er lærebokkode med tester mot en ekstern fasit.

**Slik tas den inn.** Craten legges i `crates/filmcraft-audio-dsp/`, låst til
én commit, med `LICENSE-MIT`, `LICENSE-APACHE` og filmcrafts `NOTICE`. En
`UPSTREAM.md` i mappa sier hvilken commit den er fra, og lister hver lokal
endring. Testene deres kjøres i vår CI. Vi følger **ikke** oppstrøms
fortløpende: en oppdatering er en egen PR med diff mot forrige commit. Dette
er et bevisst unntak fra «avhengighet før kopi» i
[AVHENGIGHETER.md](AVHENGIGHETER.md).

## Målbildet

```text
            ┌──────────── ffmpeg (egen prosess) ────────────┐
 opptaket ─▶│ dekod til f32 PCM på et rør (-f f32le pipe:1)  │
            └──────────────────────┬─────────────────────────┘
                                   ▼
 ÅPNE:      én runde ─▶ bølgeform (flere oppløsninger)
                     ─▶ 16 kHz (resampler) ─▶ preken/VAD-analyse
                     ─▶ kanalanalyse · lydstyrke · brumsjekk
 LYTTE:     PCM ─▶ kutt-plan (overganger) ─▶ lydkjede ─▶ resampler ─▶ cpal ─▶ høyttaler
 EKSPORT:   PCM ─▶ kutt-plan ─▶ lydkjede ─▶ måler ─▶ forsterkning ─▶ TP-limiter ─┐
            ┌──────────── ffmpeg (egen prosess) ────────────┐                   │
            │ kod fra PCM på et rør · mux (video: -c:v copy) │◀──────────────────┘
            └────────────────────────────────────────────────┘
```

**Det du hører er det du får:** forhåndslytting og eksport bruker samme kjede,
med samme parametre og i samme kode.

## Fasene

Hver fase er én eller flere PR-er, kan slippes for seg, og har et «ferdig når»
som kan sjekkes. Rekkefølgen er valgt slik at det som gir mest og koster minst
kommer først.

### Fase 0 — ta inn craten

- `crates/filmcraft-audio-dsp/` som beskrevet over, inn i workspace-et.
  `sundayrec-core` får den som avhengighet.
- Enhetstestene deres er selvstendige. Oracle-testen mot ffmpeg
  (`tests/loudness_oracle.rs`) bruker filmcrafts `testkit` til å finne ffmpeg
  og skrive WAV. Den skrives om til å bruke vår sidecar i `src-tauri/binaries`,
  som `check`-jobben i `ci.yml` allerede henter.
- **Ferdig når:** `cargo test --workspace` kjører filmcrafts tester grønt i CI,
  på macOS, Windows og Linux.

### Fase 1 — lydstyrke i Rust

- Ny `sundayrec-core::loudness`: BS.1770-måling over PCM, og
  `plan_gain(målt, preset) → (gain_db, limiter)`. Påføring er lineær
  forsterkning til målet, og etter den en true-peak-limiter på presettets tak
  (−1 dBTP). Målet nås alltid. Limiteren tar bare de sjeldne toppene, og
  programmet komprimeres ikke.
- Eksporten måler det kuttede, behandlede signalet i Rust og koder i samme
  runde (behandlet PCM mellomlagres som f32 i temp, eller kjeden kjøres to
  ganger; det avgjøres på målt tid). `mastering_analyze` bruker samme måler.
- **Det som forsvinner:** `loudnorm` (måle- og påføringsstrengene), JSON-parseren,
  `plan_pass2` og «Normalization Type»-sjekken.
- **Ferdig når:** hver fil i en testmappe med ekte opptak treffer målet
  ±0,5 LU med true peak ≤ −1,0 dBTP, kryssjekket med ffmpeg `ebur128` i en test.
  En 90-minutters eksport er raskere enn i dag.

### Fase 2 — lydkjeden i Rust

- Mastring-presettene og `VocalChain` blir en liste av filmcraft-effekter
  (`sundayrec-core::chain`), bygget fra de samme presettabellene. ffmpeg får
  ingen `-af` lenger, bare koding. Mikseren (`MixerPanel.tsx`) mappes til
  samme kjede.
- Forhåndsinnstillingene stilles inn på nytt etter gehør, fordi filtrene ikke
  er identiske: samme LUFS-mål, ny lytting.
- **Det som forsvinner:** filterstreng-byggerne i `processing.rs` og
  `mastering.rs` når fase 2 er godkjent. ffmpeg-kjeden lever **én utgivelse**
  ved siden av, bak en skjult innstilling, og fjernes i den neste.
- **Ferdig når:** «Ørene» i [RIG-DAY.md](RIG-DAY.md) har lyttet til de tre
  profilene på minst tre ekte gudstjenester, gammel mot ny, og eieren har sagt
  ja.

### Fase 3 — «Tale» renser av seg selv

- Analysen fra fase 4 (eller eksportens egen) avgjør hva «Tale» tar med:
  - **brumfjerning** når det er energi på 50/100/150 Hz over nabobåndene
    (Goertzel på 16 kHz-strømmen, 50 eller 60 Hz velges automatisk);
  - **støyfjerning** alltid, mildt (minimumsstatistikken trenger ikke å
    lære støyen);
  - **romklangfjerning** når analysen anslår en lang etterklang;
  - **klikkfjerning** når kanaldiagnosen ser knitring.
- Ingen nye knapper. Frivillig først: tre ord, og et forslag i lyd-steget som
  sier hva som ble gjort («Vi fjernet brum fra strømnettet»).
- **Ferdig når:** et ekte brumopptak og et ekte opptak fra et stort kirkerom
  er hørbart bedre (Ørene), og et rent opptak er hørbart uendret.

### Fase 4 — én dekoding når fila åpnes

- Ett ffmpeg-rør i opptakets egen samplingsfrekvens. I samme runde fordeles
  PCM-en til bølgeform (flere oppløsninger), resampling til 16 kHz for
  `audio_analysis`/`detect`, kanalanalyse (erstatter `astats`-runden),
  lydstyrke og brumsjekk.
- Sidevognene (`.peaks.json`, segmentene) beholdes. De nøkles på samme måte.
- **Ferdig når:** å åpne en 90-minutters fil dekoder den én gang (sjekkes med
  en teller i en test), og alle eksisterende analyse-tester er grønne.

### Fase 5 — forhåndslytting i sanntid

- En avspillingsmotor i `src-tauri`: ffmpeg-dekoding fra en posisjon →
  ringbuffer → kutt-plan med konstant-effekt-overganger → lydkjeden →
  resampler til enhetens frekvens → cpal. Lydkortet er klokka, og
  spillehodet i UI-et følger hendelser fra motoren.
- Lyd-steget først: «Før/Etter» blir et øyeblikkelig bytte, og profilen kan
  byttes mens det spiller. Deretter Klipp-steget, der motoren erstatter
  `<audio>`, volumrampen og avspillingsproxyen.
- **Det som forsvinner:** `master_preview`, temp-mp3-ene, `extract_playback_proxy`
  og volum-de-klikket i `playback.ts`.
- **Ferdig når:** avspilling, hopp og bytte av profil er uten klikk og hakk
  på Mac og Windows, og spillehodet holder seg i synk gjennom en hel
  gudstjeneste (RIG-DAY, ny sjekk).

### Fase 6 — rask videoeksport

- Opptaket setter et nøkkelbilde hvert 2. sekund (`-g`, eller
  `-force_key_frames`) i `capture.rs`.
- Når videoen bare kuttes, kopieres videostrømmen (`-c:v copy`). Kuttene
  snappes til nøkkelbildene, og lyden rendres i Rust over nøyaktig samme
  intervaller og muxes inn.
- **Ferdig når:** «Behold bare prekenen» på en 90-minutters video tar sekunder,
  ikke minutter, og lyd og bilde er i synk ved hvert kutt (RIG-DAY).

## Hva vi ikke tar fra filmcraft

Kodekene og containerne (ffmpeg dekker dem, og de har patentspørsmål),
egui-grensesnittet, tidslinje- og flersporsmotoren, og musikkeffektene. De
løser et annet problem enn én frivillig med ett spørsmål.

## Risiko, og hvordan den tas

- **Lyden blir annerledes.** Det er hele poenget, men en endring brukerne
  merker. Derfor lytter Ørene før fase 2 og 3 slippes, og ffmpeg-kjeden lever
  én utgivelse til.
- **Ung kode oppstrøms.** Craten er låst og ligger i repoet. En feil i den
  rettes hos oss (`UPSTREAM.md`) og meldes oppstrøms.
- **Redigeringen er fortsatt `HARDWARE-UNVERIFIED`.** Fase 5 (avspilling) er den
  med mest maskinvare i seg, og den har sin egen riggsjekk.
- **Minne og disk.** 90 min stereo f32 er rundt 2 GB. Ingenting holder hele
  fila i minnet. Fase 1 avgjør om eksporten mellomlagrer på disk eller
  kjører kjeden to ganger.
