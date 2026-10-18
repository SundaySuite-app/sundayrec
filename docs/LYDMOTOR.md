# Lydmotoren — redigeringens lyd i Rust, ffmpeg bare til filer

_Skrevet 2026-10-11, lest mot koden på nytt 2026-10-12 ved v0.25.3. Status:
**plan, ikke startet.** Del av
[OMBYGGING.md](OMBYGGING.md): fase N her er pakke BN der. Eierens beslutning:
gjennomføres, og vi tar risikoen._

Dette er planen for å bytte ut hvordan **Rediger** behandler lyd. I dag går all
lyd gjennom ffmpeg-filtre, og en fil leses fra start til slutt fem–seks ganger
fra den åpnes til den er eksportert. Etter planen dekodes fila av ffmpeg, mens
alt mellom dekoding og koding skjer i Rust: analyse, lydkjede, lydstyrke og
forhåndslytting. Kilden til DSP-en er `audio-dsp`-craten fra
[filmcraft](https://github.com/storytold/filmcraft) (MIT OR Apache-2.0).

Det åpne står med én linje i [`PLAN.md`](PLAN.md); detaljene står her.

## Hvorfor

Målt og lest 2026-10-11. Tallene er fra en sky-container med 4 kjerner,
ffmpeg 6.1 (ikke den medfølgende 9.0.1) og syntetiske signaler. De viser
retningen, ikke fasiten.

1. **Fila leses mange ganger.** Bølgeformen (`peaks`, 8 kHz-dekoding),
   prekensøket (`segments`, 16 kHz), kanalanalysen (`auto_process`/`astats`),
   og eksportens to runder (måling av
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
   gjør altså lite. **Sjekket med 9.0.1 i B0c: bekreftet**, se vedlegget.

4. **Standardprofilen renser ingenting.** «Tale» (`sound-profiles.ts`) er
   `speech-clear`: høypass, EQ, kompressor og `loudnorm`. Støy- og
   romklangfjerning nås bare gjennom den avanserte mikseren.
5. **Forhåndslyttingen er ikke ekte tid.** «Etter» er en 20-sekunders mp3 som
   rendres av ffmpeg (`master_preview`) før den kan høres. Avspillingen er et
   `<audio>`-element der kuttene glattes ved å skru volumet ned og opp
   (`playback.ts`). Temp-filene ryddes forøvrig allerede: oppstartssveipen
   `cleanup_preview_temp_files()` (`src-tauri/src/editor/mod.rs`) går over
   `is_preview_temp_name`. Det er ventetiden og `<audio>`-elementet som er
   problemet, ikke søl i temp-mappa.

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
  ganger; det avgjøres på målt tid). (M2R #8: den frittstående `mastering_analyze` er slettet; den hadde ingen kaller og målte ukuttet fil.)
- **Det som forsvinner:** `loudnorm` (måle- og påføringsstrengene), JSON-parseren,
  `plan_pass2` og «Normalization Type»-sjekken.
- **Ferdig når:** hver fil i en testmappe med ekte opptak treffer målet
  ±0,5 LU med true peak ≤ −1,0 dBTP, kryssjekket med ffmpeg `ebur128` i en test.
  En 90-minutters eksport er raskere enn i dag.

**Gjort (B1a + B1b).** `loudnorm`, JSON-parseren, `plan_pass2`, «Normalization Type»-sjekken
og alle `loudnorm`-strengene er borte. Eksporten måler det kuttede, behandlede signalet i Rust
(`sundayrec_core::pcm_stream`), planlegger én lineær forsterkning (`loudness::plan_gain`), og
koder fra det jevnede signalet. Forhåndslyttingen bruker samme måler og samme nivåsteg.

**Avgjort på målt tid (B1b): behandlet PCM mellomlagres som f32 i temp.** Ekte 90-minutters
fil (`2026.01.04.mp3`, 5400 s, `speech-clear`, ett kutt, mp3 256k, `--release`, Mac,
ffmpeg 9.0.1), to kjøringer hver, sekunder:

|                                                     | måling + kjede | dry-run (begrenseren) | koding      | sum               |
| --------------------------------------------------- | -------------- | --------------------- | ----------- | ----------------- |
| Før (`loudnorm`, to ffmpeg-kjøringer)               |                |                       |             | **222,6 / 222,0** |
| Etter, disk-mellomlager                             | 21,2 / 21,3    | 26,3 / 26,4           | 47,6 / 47,8 | **95,1 / 95,6**   |
| Etter, kjeden kjørt to ganger (+1 gang per dry-run) | 20,5 / 20,2    | 37,2 / 36,9           | 53,4 / 54,3 | **111,0 / 111,3** |

Disk vinner med ca. 16 s fordi hver ekstra kjørelse av kjeden (dekoding + EQ + kompressor)
koster mer enn å lese 2 GB tilbake fra disk, og fordi dry-runene (som korrigerer for
limiterens lydstyrketap) kan kjøres over fila uten å dekode på nytt. Begge er klart raskere enn
før, og før landet dessuten 6 LU under målet. Prisen: 2 GB ledig i OS-ens temp-mappe per
90 minutter stereo (sjekkes før målingen; en krasjet eksport etterlater fila til neste oppstart,
som sveiper den).

Resultat på samme fil (uavhengig `ebur128` på den leverte mp3-fila): **−16,03 LUFS, −1,74 dBTP**
mot mål −16 / tak −1,0. Målet nås med én dry-run (limiteren tok 5,6 dB av toppene).

### Fase 2 — lydkjeden i Rust

- Mastring-presettene og `VocalChain` blir en liste av filmcraft-effekter
  (`sundayrec-core::chain`), bygget fra de samme presettabellene. ffmpeg får
  ingen `-af` lenger, bare koding. Mikseren (`MixerPanel.tsx`) mappes til
  samme kjede.
- Forhåndsinnstillingene stilles inn på nytt etter gehør, fordi filtrene ikke
  er identiske: samme LUFS-mål, ny lytting.
- **Det som forsvinner:** filterstreng-byggerne i `processing.rs` og
  `mastering.rs` slettes **i samme milepæl** som fase 2 godkjennes (beslutning
  B5 i OMBYGGING.md). Til sammenligningen før godkjenningen lager
  `examples/chain_ab.rs` begge versjonene av samme utdrag til lytting. Den
  slettes også når kjeden er godkjent.
- **Ferdig når:** «Ørene» i [RIG-DAY.md](RIG-DAY.md) har lyttet til de tre
  profilene på minst tre ekte gudstjenester, gammel mot ny, og eieren har sagt
  ja.

**Gjort (B2b).** Eksport, nivåanalyse og forhåndslytting tar lyden fra ffmpeg som rå f32
uten noe `-af` (bare klipp, skjøtefades og konkatenering gjenstår i grafen) og kjører kjeden i
Rust (`chain::ChainReader`, strømmende, ett blokk i minnet). Det gjelder også eksport uten
mastring-preset (vokalkjede eller «Normaliser»-gain): den går samme vei og mellomlagres som
mastret eksport. Mikseren (`processing`) og presetene bygger samme `Chain`. `chain_ab` kan
prøve en endring uten å røre tabellene: `--set <stadium>[:n].<param>=<verdi>`. Filterstreng-
byggerne står igjen til B2c.

Innstillingen er målt på tre ekte opptak (30 s fra `2026-05-17_2`, `31.5.26`, `2026.01.04`, alle
stille rom, gulv rundt −60 dBFS) og tre syntetiske signaler (holdt akkord mellom to talepartier,
tale med romklanghale, tale med stasjonær støy). Begrunnelsene står i `chain.rs`' modulhode.
Kort:

| Stadium      | Endring                          | Måling                                                                  |
| ------------ | -------------------------------- | ----------------------------------------------------------------------- |
| kompressor   | terskel −3 dB                    | samme lydstyrkespenn (LRA) som ffmpeg innen 0,3 LU, alle fire presetene |
| romklang     | `rt60` 0,4 s (før 0,8)           | ren sinus mister 2–3 dB over hele glideren (før 7–14 dB)                |
| støyfjerning | maks 12 dB (preset ba om 20)     | gulvet −11 dB (før −1,7), men holdt akkord mister det samme             |
| de-esser     | uendret (20·i dB, 6 kHz, −12 dB) | sibilanter −3 dB, vokalrammene rundt −0,2 dB                            |

**Funn til B3a:** filmcrafts `denoise` kan ikke skille en holdt tone fra støygulv. Toner som
varer under ca. 1,5 s (minimumsstatistikkens vindu) slipper gjennom urørt; lengre toner dempes
like mye som gulvet. En «Tale» som alltid støyfjerner mildt vil derfor spise orgel og
akkompagnement med mindre B3a gater støyfjerningen på tale.

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

**Gjort (B3a).** «Tale» (`speech-clear`, uten mikser) leser opptaket én gang på 16 kHz
(`sundayrec_core::tale`, ca. 3000× sanntid; ffmpeg dekoder) og tar bare med det analysen finner.
Resultatet bufres per fil, så forhåndslytting, lydstyrketallet og eksporten leser filen én gang.
Stegene settes inn mellom kanalreparasjonen og presettets egen kjede. Ingen nye knapper; Tale-
kjeden er `Chain::speech_clean`, og `SpeechClean::said()` gir kodene B3b skal oversette.

| Trinn                          | Slås på når                                                                                                                                             | Måling                                                                                                                                                                                                                            |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| brum (`dehum`)                 | en linje på 50/100/150 Hz står ≥ 15 dB over båndene ±4 Hz i ≥ 60 % av 2 s-blokkene; 50 eller 60 Hz velges                                               | ekte opptak + 50 Hz på −34 dB og 60 Hz på −40 dB: riktig familie funnet på alle tre; uten brum: aldri                                                                                                                             |
| støy (`denoise`, 9 dB)         | nesten alltid: ikke når gulvet er under −85 dBFS, når gulvet er > 60 dB under talen, eller når det ikke finnes noe gulv ved siden av signalet (< 20 dB) | `2026-05-17_2` (gulv −83,5): av og urørt (±0,0 dB); `31.5.26` og `2026.01.04`: på, gulv −3,7 / −2,1 dB, talen −0,0 / −0,1 dB                                                                                                      |
| romklang (`dereverb`, 30–70 %) | anslått RT60 ≥ 1,0 s (den raskeste tiendedelen av fallene, −5 til −20 dB)                                                                               | de tre ekte opptakene tørre: 0,16 s, av. Med påført hale (RT60 → anslag, tre opptak): 0,5 → 0,4–0,5; 0,9 → 0,7–0,8; 1,8 → 1,5–1,7; 3,0 → 2,6–2,8. Av for 0,5 og 0,9, på fra 1,2 (`2026.01.04`, `2026-05-17_2`) og 1,8 (`31.5.26`) |
| klikk (`click_remover`)        | kanaldiagnosen ser knitring (`processing::channel_crackles`) eller ≥ 20 isolerte pigger i minuttet                                                      | ekte opptak: 1 til 5 i minuttet; 1 pigg/s påført: 56 i minuttet                                                                                                                                                                   |

**Støyfjerneren, og hvordan musikk beskyttes.** Valget står i modulhodene til `chain.rs` og `tale`. Kort:
filmcrafts `denoise` beholdes, mildt (9 dB), og **gates på tale**: `denoise` og `dereverb` kjører
på hele signalet, men utgangen blandes inn bare mens inngangen er talelignende (10 ms-nivået
spenner ≥ 9 dB over de siste 0,6 s, godt over gulvet) eller ligger på gulvet. Gulvet måler porten
selv, på signalet den gater (se «M2R-rettelse» under). `afftdn` ble ikke valgt:
ffmpeg skal ikke ha noe lydfilter (B2b), og B0c målte den bare mot gulv, aldri mot holdte toner.
Målt på en syntetisk gudstjeneste (tale, 12 s holdt firetoners akkord på −26 dBFS, gulv −55 dBFS,
tale), samme 48 kHz-signal:

|                                       | holdt akkord | gulvet i en 1,4 s pause |
| ------------------------------------- | ------------ | ----------------------- |
| `denoise` 9 dB uten gate              | **−9,00 dB** |                         |
| `Chain::speech_clean` (støy, gatet)   | **+0,00 dB** | −8,8 dB                 |
| `dereverb` 70 %, rt60 0,4 s uten gate | −2,71 dB     |                         |
| `dereverb` 70 %, gatet                | **+0,00 dB** |                         |

**M2R-rettelse: porten måler gulvet på signalet den ser (funn 1, 2 og 4).** Første versjon
sammenlignet med gulvet fra analysen, og det er målt på et annet signal enn porten ser: hele den
ukuttede fila, 16 kHz, før kanalreparasjon og før `dehum`. To feil fulgte av det, begge rettet:

- _Brum ble tatt for gulvet._ Brum −52…−45 dBFS la analysens gulv på −54…−47, porten regnet alt
  jevnt under «brum + 6 dB» som gulv, og en holdt akkord ~25 dB under talen ble dempet −8,3…−9,0 dB.
  Ved brum −38 sank avstanden til talen under 20 dB og støyfjerningen ble slått helt av.
- _Støyfjerningen pumpet i pauser over ca. 0,8 s_ når analysens gulv lå under portens (stille
  forspill som klippes bort, død kanal + `DuplicateLeft`, hvit sus over 8 kHz).

Valg: **porten måler gulvet selv** (`FloorTracker`, 5. persentil av portens egne 10 ms-nivåer
over de siste 30 s), i samme samplinger, rate og nivå som støyfjerneren ser, altså etter
kanalreparasjon og `dehum`, og bare av det som faktisk eksporteres. Analysens gulv er bare
startverdi de første 2 s. Alternativene var å kalibrere analysen gjennom samme forkjede (en ekstra
dekoding av det kuttede programmet i full rate, dvs. ett pass til på 90 minutter, og en cache nøklet
på kuttene), eller en minimumsfølger (kan lære en holdt tone som gulv). En persentil over 30 s er
begge deler billig og trygg: en tone må fylle 95 % av 30 s for å bli gulv; en holdt akkord er
10–20 s med pauser rundt. Grensen står i koden: en tone holdt over ca. 28 s uten en eneste pause
læres som gulv (det ville støyfjerneren gjort uansett, B2b). Beslutningen (`decide()`) leser
fortsatt gulvet fra analysen, men med 50/60 Hz-familiene og tre overtoner notchet ut når brum er
funnet (Q 10), så den ser det støyfjerneren ser etter `dehum`.

|                                              | før                  | etter                      |
| -------------------------------------------- | -------------------- | -------------------------- |
| holdt akkord, brum −52 dBFS                  | −8,57 dB             | 0,00 dB                    |
| holdt akkord, brum −45 dBFS                  | −9,0 dB (målt i M2R) | 0,00 dB                    |
| brum −38 dBFS: analysens gulv / støyfjerning | −39,5 / av           | −71,0 / på, akkord 0,00 dB |
| stille forspill: gulv sent i pausen          | 0,0 dB (pumping 8,7) | −8,5 dB (pumping 0)        |
| død kanal + `DuplicateLeft`: sent i pausen   | −0,05 dB             | −8,55 dB                   |

Testene som beviser det (`tale/tests.rs`) feiler på gammel kode og er grønne nå. B3a-testen for
«ved gulvet»-grenen er strammet: pausen er 3,5 s og vinduet 5,5–7,4 s ligger utenfor holdetiden, og
porten får et feil oppgitt gulv (−90 og −30) uten at pausen eller akkorden skifter oppførsel.

**90-minutterseksport etter B2b/B3a (M2R funn 6), målt.** Hele eksportstien (`export`, ikke bare
kjeden): `2026.01.04.mp3`, 5400 s, ett kutt, `speech-clear` («Tale»), mp3 256k, `--release`, M1,
ffmpeg 9.0.1. Analysen valgte bare støyfjerning (ingen brum, ingen lang romklang). Analysen selv:
2,5 s (2128× sanntid). Hele eksporten: **234 s** ved maskinlast ca. 25 (andre økter kjørte), og
**219 s** ved last 9→5. Før M2 var tallet 222 s (B1b-tabellen over), så en «Tale»-eksport uten romklang er
omtrent på nivå med i dag, ikke raskere; B1b-kravet «raskere enn i dag» er ikke oppfylt for «Tale»
(95 s var uten rensing). Med `dereverb` på (stort kirkerom) legger M2R-målingen til ca. 50 s i
kjeden, altså rundt 270 s, trolig tregere enn før. Ikke målt ende til ende med dereverb (ingen ekte
fil med lang etterklang tilgjengelig). Kjeden er enkelttrådet og er det som tar tiden: `denoise`
(minimumsstatistikk) og `dereverb` kan kjøres parallelt per kanal eller på egen tråd; ikke gjort.

**Kjent grense:** lyd der nivået beveger seg som tale (trommer, strummet gitar) åpner porten, og en
holdt tone under den møter støyfjerneren på inntil 9 dB. «Tale og musikk» bruker ikke denne rensingen.
Ikke hørt etter: hørbarheten (brumopptak, stort kirkerom, rent opptak uendret) hører til B3b/Ørene.

**Gjort (B3b).** Lyd-steget forteller den frivillige hva «Tale» gjorde, uten fagord. Kommandoen
`editor_tale_said` (kildelapp inn, `EditorTaleSaid { analysed, said }` ut) leser samme bufrede
beslutning som forhåndslytting og eksport (`tale_clean_for`), så setningen på skjermen er kjeden
som kjører. Bare kodene krysser (`hum`, `clicks`, `noise`, `reverb`); ordene eies av skallet
(`app.editor.said.*`, sju språk). Én linje per ting; ble ingenting gjort på en lest fil: «Opptaket
var rent.»; kunne fila ikke leses, mikseren er på eller profilen ikke er «Tale»: ingenting.
Hørbarheten er ikke prøvd (Ørene, RIG-DAY (B3b)).

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
  merker. Derfor lytter Ørene før fase 2 og 3 slippes. Det finnes ingen
  reserve etterpå (B5), så lyttingen må være grundig: tre ekte gudstjenester,
  minst ett stort kirkerom og ett brumopptak.
- **Ung kode oppstrøms.** Craten er låst og ligger i repoet. En feil i den
  rettes hos oss (`UPSTREAM.md`) og meldes oppstrøms.
- **Redigeringen er fortsatt `HARDWARE-UNVERIFIED`.** Fase 5 (avspilling) er den
  med mest maskinvare i seg, og den har sin egen riggsjekk.
- **Minne og disk.** 90 min stereo f32 er rundt 2 GB. Ingenting holder hele
  fila i minnet. Avgjort i B1b: eksporten mellomlagrer på disk (se Fase 1).

## Vedlegg: målingene, og hvordan de gjentas

Målt 2026-10-11 i en sky-container med 4 kjerner, Linux, ffmpeg 6.1, filmcraft
`0800a8b0`. **Gjentatt på Macen 2026-10-13 med ffmpeg 9.0.1, se «Gjentatt på
Macen (B0c)» under.**

**Fart, én kjerne, 120 s stereo 48 kHz syntetisk tale-lignende signal:**

| Effekt (filmcraft)        | × sanntid | Forsinkelse  |
| ------------------------- | --------- | ------------ |
| `dehum`                   | 354       | 0            |
| `denoise`                 | 60        | 2048 samples |
| `dereverb`                | 77        | 2048 samples |
| `gate`                    | 814       | 0            |
| `parametric_eq`           | 210       | 0            |
| `compressor`              | 538       | 0            |
| `deesser`                 | 382       | 0            |
| `limiter` (true peak)     | 192       | 248 samples  |
| **Hele kjeden**           | **18–21** |              |
| `LoudnessMeter` (BS.1770) | 200       |              |

Dagens ffmpeg-kjede «voice-noisy-room» gikk 32× sanntid, og 10× med
`loudnorm` (`-threads 1 -filter_threads 1`).

### Gjentatt på Macen (B0c)

Målt 2026-10-13 på MacBook Pro med **Apple M1 Pro** (10 kjerner), macOS 27.0.1
(26A434), **ffmpeg 9.0.1** (`src-tauri/binaries`, martin-riedl.de-bygget),
filmcraft `audio-dsp` 0.7.2 slik den ligger i `crates/filmcraft-audio-dsp`.
Hentet med `cargo run --example dsp_bench --release` (`src-tauri/examples/dsp_bench.rs`).
Medianer av fem kjøringer (ffmpeg: tre). Andre økter bygget i bakgrunnen, men
spredningen var under 1 % (maks mot median), så tidene er rene. Samme
oppskrift som under; to tolkninger er mine: «sus med topp på −46 dBFS» er
jevn støy med topp −46 dBFS, og effektene kjører med standardparametre.

**Fart, én kjerne, 120 s stereo 48 kHz:**

| Effekt (filmcraft)        | × sanntid | Forsinkelse  | Før (sky, 4 kjerner) |
| ------------------------- | --------- | ------------ | -------------------- |
| `dehum`                   | 811       | 0            | 354                  |
| `denoise`                 | 114       | 2048 samples | 60                   |
| `dereverb`                | 123       | 2048 samples | 77                   |
| `gate`                    | 2173      | 0            | 814                  |
| `parametric_eq`           | 10067     | 0            | 210                  |
| `compressor`              | 1082      | 0            | 538                  |
| `deesser`                 | 963       | 0            | 382                  |
| `limiter` (true peak)     | 329       | 248 samples  | 192                  |
| **Hele kjeden**           | **42,5**  |              | 18–21                |
| `LoudnessMeter` (BS.1770) | 611       |              | 200                  |

ffmpeg 9.0.1 med SundayRecs egne strenger, samme fil, `-threads 1
-filter_threads 1`, null-muxer (tid inkluderer start av prosess og dekoding):

| Kjede                                                   | × sanntid |
| ------------------------------------------------------- | --------- |
| «voice-noisy-room» (`vocal_chain_presets()`)            | 120       |
| «voice-noisy-room» + `loudnorm` (pass 1)                | 25,6      |
| Master-forvalg, målepass (`build_measure_pass_filters`) | 29–32     |

(Før: 32× og 10×. ffmpeg-kjeden er raskere enn filmcrafts, men den gjør også
mindre: `afftdn` med `tn=1` rører knapt støyen, se under. `loudnorm` er
fortsatt det som drar farten ned.)

**Kvalitet (RMS i dB, kanal 0).** Inn: støygulv −45,0, pulser −23,0, romklang-
hale −36,3, direktelyd −19,3. Pulser og direktelyd skal være urørt; gulv og
hale skal ned.

| Prosessor                                              | Støygulv      | Pulser | Hale          | Direkte       |
| ------------------------------------------------------ | ------------- | ------ | ------------- | ------------- |
| filmcraft `denoise` (standard, 12 dB)                  | −55,8         | −23,0  | −36,4         | −19,5         |
| filmcraft `denoise`, `reduction=20`                    | −60,8         | −23,0  | −36,4         | −19,5         |
| filmcraft `dereverb` (standard)                        | −49,5         | −30,7  | −42,5         | −19,4         |
| filmcraft `dereverb`, `rt60=1.5`, `amount=100`         | −54,0         | −40,6  | −48,7         | −19,5         |
| ffmpeg `afftdn=nr=10:nf=-25:tn=1` («voice-podcast»)    | −46,7         | −23,0  | −36,8         | −19,3         |
| ffmpeg `afftdn=nr=20:nf=-20:tn=1` («voice-noisy-room») | −46,7         | −23,0  | −36,8         | −19,3         |
| ffmpeg `afftdn=nr=20:nf=-45:tn=1`                      | −46,7         | −23,0  | −36,8         | −19,3         |
| ffmpeg `afftdn=nr=12:nf=-45` (uten `tn`)               | −54,9         | −23,0  | −37,1         | −19,4         |
| ffmpeg `afftdn=nr=20:nf=-45` (uten `tn`)               | −58,5         | −23,0  | −37,1         | −19,4         |
| ffmpeg `afftdn=nr=30:nf=-45` (uten `tn`)               | −60,2         | −23,0  | −37,1         | −19,4         |
| ffmpeg `afftdn=nr=20:nf=-50` (uten `tn`)               | −50,2         | −23,0  | −36,6         | −19,3         |
| ffmpeg `afftdn=nr=20:nf=-40` (uten `tn`)               | −64,3         | −23,0  | −38,6         | −19,4         |
| ffmpeg `afftdn=nr=20:nf=-20` (uten `tn`)               | −65,0         | −23,1  | −56,3         | −28,8         |
| ffmpeg `agate`-«dereverb», styrke 0,5 / 1,0            | −48,6 / −57,1 | −23,0  | −36,5 / −37,5 | −19,3 / −19,4 |

**`afftdn`-funnet: bekreftet.** Med `tn=1` (støysporing) tar `afftdn` bare
1,7 dB av et stasjonært støygulv, uansett `nr` og `nf`, så dagens innstillinger
«voice-podcast» og «voice-noisy-room» renser praktisk talt ingenting. Uten `tn=1`
og med `nf` nær det faktiske gulvet (−45) tar `afftdn=nr=20` 13,5 dB og
`nr=30` 15,2 dB, mot filmcrafts `denoise` på 10,8 dB (standard) og 15,8 dB
(`reduction=20`), med pulsene urørt. ffmpeg er altså ikke dårlig på **støy**;
innstillingene våre er feil. Forbehold: (1) støyen her er hvit og stasjonær,
som er `afftdn`s beste tilfelle; ekte kirkestøy (vifte, folk, bevegelse) er
ikke målt. (2) Uten `tn=1` må `nf` treffe: −50 gir bare 5,2 dB, −40 spiser
allerede av halen, og dagens `nf=-20` uten `tn` fjerner −9,5 dB av **direktelyden**.
`nf` må måles per opptak, noe `tn=1` slapp å gjøre. (3) Filmcrafts
`denoise` trenger ingen `nf`.

**Romklang:** `agate`-erstatningen rører ikke halen (−0,2 til −1,2 dB).
Filmcrafts `dereverb` tar −6,2 dB med standardverdier og −12,4 dB med
`rt60=1,5` og `amount=100`, direktelyden urørt. Men den dempet også de
stasjonære 1 kHz-pulsene (−7,7 til −17,6 dB), som en stasjonær tone ikke
skiller fra en hale; på ekte tale er dette ikke målt og må lyttes etter.
Brumfjerning (`dehum`) finnes ikke i ffmpeg og er ikke målt mot noe.

**Hva det endrer for B2 og B3:** farten er ikke lenger noe argument for å
nøle (hele kjeden går 42× sanntid på M1 Pro, lydstyrkemåleren 611×, begge godt
over det forhåndslytting og eksport trenger), og B3a må velge mellom filmcrafts
`denoise` og en rettet `afftdn` uten `tn=1` på **støy** (omtrent likeverdige på
syntetisk støy, `afftdn` må få målt `nf`), mens filmcraft fortsatt er eneste
vei for romklang og brum og for sanntid i forhåndslyttingen.

**Slik gjentas det:** en liten crate utenfor repoet med
`filmcraft-audio-dsp = { path = "<filmcraft>/crates/audio-dsp" }` som:
(1) lager samme signal (støy modulert med 4 Hz, 50 Hz brum på −34 dB, sus med topp på
−46 dBFS), (2) kjører hver effekt med `create_effect(id, 48000.0, 2)` i blokker
på 1024 og tar tiden, (3) kjører `LoudnessMeter::process` i blokker på 4800.
Kvalitetsmålingene: (a) 1 kHz-pulser 0,5 s på / 0,5 s av med sus på −45 dBFS,
RMS målt i pausene og i pulsene etter 5 s innsvinging, justert for effektens
`latency()`, (b) korte støystøt hvert sekund foldet med en eksponentielt
avtagende hale (RT60 1,5 s), RMS målt i halen (0,3–0,95 s) og i direktelyden
(0–0,05 s). Samme filer kjøres gjennom ffmpeg med SundayRecs egne
filterstrenger. De hentes ut med `processing::vocal_chain_presets()` og
`mastering::build_measure_pass_filters()`.

**M2R #11: mikserens støyfjerning er gatet (samme `SpeechGate` som «Tale»).** `Chain::from_vocal_chain`
pakker `denoise` inn i `Stage::SpeechGated`, så «mer støyfjerning» i mikseren ikke lenger mister
musikkbeskyttelsen. Porten leser gulvet selv; mikseren har ingen analyse og gir bare et forhåndsgulv
(−60 dBFS) for de to første sekundene. Målt på samme syntetiske gudstjeneste som B3a ved 12 dB
(skyvens topp): akkorden +0,00 dB (ugatet: tapte hele reduksjonen, B2b −9,6 dB ved 10), gulvet i
pausen −11,2 dB. Test: `the_mixers_denoise_leaves_a_held_chord_alone_but_cleans_the_pause`.
`dereverb` i mikseren er fortsatt ugatet (B2b: −2,2 til −3,3 dB på en holdt tone). Samme kjente
grense som «Tale»: trommer åpner porten.
