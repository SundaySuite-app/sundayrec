# Natt-økt 2026-06-14: lydkvalitet, VU-respons, preken-auto-kutt

Richard rapporterte fire ting på nyeste versjon: (1) hakkete opptakslyd, (2)
editor-lyd virker overkomprimert, (3) ønsket «analyser → foreslå preken →
godkjenn → kutt bort resten (all musikk)», (4) VU/«uv-signal» henger langt bak
når man trykker REC. Han ba om aggressiv, selvstendig natt-jobbing (ingen
brukere ennå → høy risiko OK, native-by-default OK).

Alt under er committet på gren `claude/sundayrec-htg52k`. **Gate grønn:**
`cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`,
`npm run typecheck/lint/format:check`, `npm run build`. **Lyd/maskinvare-stiene
er HARDWARE-UVERIFISERT** — dette miljøet har ingen mikrofon/kamera/skjerm, så
jeg kan ikke høre lyd eller kjenne på VU-følelsen. Se RIGG-VERIFISER nederst.

> Miljø-notat: src-tauri (Tauri-skallet) krever GTK/WebKit + ALSA system-libs +
> ffmpeg-sidecar for å kompilere. Jeg installerte dev-libs og la inn
> **stub-sidecars** (`src-tauri/binaries/*` — gitignorert, IKKE committet) så
> enhetstestene kompilerer headless. Ekte ffmpeg hentes av `npm run fetch-ffmpeg`.

## Rotårsak som bandt #1 og #4 sammen (viktigst)

Stderr-leseren i `src-tauri/src/recorder/engine.rs` sendte de høyfrekvente
nivå-meldingene med `msg_tx.send(Levels).await` på en _bounded_ kanal. Når
konsumenten henger et øyeblikk (Tauri `app.emit`), fylles kanalen, den awaitede
send-en **blokkerer leseren**, ffmpeg sin stderr-pipe fylles, ffmpeg stopper på
skriving, og avfoundation dropper capture-samples → **hakkete lyd** + nivåene
kommer i sene byger → **voksende VU-etterslep**. Fikset ved å gjøre nivå-
leveransen ikke-blokkerende (`try_send`, dropp ved full — nivåer er latest-wins).
Dette er trolig den enkeltstående største fiksen for begge symptomene.

## Endringer

### A — Opptak: mot hakking

- **A1** `engine.rs`: nivå-send → `try_send` (dropp ved full); kanal 256→512.
  Ny test `levels_never_block_the_reader_when_consumer_stalls`.
- **A2** `capture.rs`: `-rtbufsize 256M` på avfoundation-input (dypere demux-
  buffer mot dropp ved CPU-spike). Tester for tilstedeværelse + posisjon før `-i`
  - at Windows-stien er uendret.
- **A3** Pre-roll tvang `-ar 48000` uansett enhet (NEEDS-RICHARD §settings-sync).
  `preroll.rs` (core) `build_preroll_{capture,trim}_args` tar nå `Option<u32>`
  (None = native, ingen `-ar`); `preroll.rs` (tauri) + `commands/recorder.rs`
  sender `resolved_sample_rate()`. Fikser også rate-mismatch ved `-c copy`-
  prepend (preroll 48k mot native opptak ga ødelagt/hakkete skjøt).
- **A4** Regresjonstester som låser native default (None → ingen `-ar`).

### B — VU like responsiv som hjem-siden (uten å re-introdusere hakking)

- **B1 = A1** fjerner det _voksende_ etterslepet.
- **B2** `ffmpeg.rs`: astats `reset=10 → reset=5` (~0.1 s peak-vindu, raskere nål).
- **B3** `engine.rs`: emit-kadens 33 ms → **16 ms (~60 Hz)** for å matche hjem-VU
  (trygt med A1). `recording.ts`: lettere fall-glatting (0.8/0.2 → 0.6/0.4).
- **Bevisst IKKE gjort:** åpne en andre device-strøm (getUserMedia / cpal `vu.rs`)
  under opptak — det var nettopp dette astats-tappen ble laget for å unngå (én
  enhets-eier på macOS). `vu.rs`/`vu://levels` forblir hjem-side-only.

### C — Editor/eksport: høyere opplevd kvalitet

- **C1** `editor.rs::codec_args` defaults: aac/m4a/ogg 192→**256k**, opus 128→
  **160k**, mp3/ukjent 192→**256k**. Lossless urørt.
- **C2** Video-eksport audio `192k → 256k` (`video_codec_args`,
  `videotoolbox_codec_args`).
- **C3** Mastering-preview `192k → 320k` (`mastering.rs::preview_args`) — A/B-en
  var mot en lossy render → fikk masteringen til å høres mer komprimert ut enn
  selve eksporten.
- **C4** Mildere default-kompresjon: `speech-clear` (anbefalt) `ratio 3→2.5`,
  `makeup 2→1.5`, `threshold -20→-18 dB`; `speech-punchy` første trinn
  `makeup 3→2` (beholder ratio 4). `master_codec_args` defaults også løftet til
  256k/160k for konsistens.
- **C5** Ny tested kjerne-funksjon `editor.rs::playback_proxy_args` (stereo 48k
  AAC m4a, `+faststart`) for hørbar avspilling av store/eksotiske filer. **IKKE
  wiret i frontend ennå** — dagens 8 kHz mono-WAV brukes til BÅDE waveform og
  avspilling bevisst for å unngå OOM (Web Audio dekoder til f32; en fler-GB-fil
  ville sprenge minnet). Riktig fiks er å spille proxyen via et `<audio>`-element
  (strømmer fra disk, lavt minne) og beholde 8 kHz kun til waveform — en
  playback-transport-endring som ikke kan verifiseres headless. Se RIGG.
- **Frontend-defaults løftet til 256k** (kilde + editor-eksport): opptaks-radio
  «Anbefalt» 192→256, editor-eksport-select mp3+aac default → 256, og alle
  `?? '192'`-fallbacks (`api-shim`, `files-page`, `export`, `home`). Backend
  `settings.rs` default-bitrate 192→256 (= editor-default, så et opptak ikke
  re-komprimeres hardere på vei ut).

### D — Preken: foreslå + godkjenn + kutt all musikk

- **D1** Ny tested `editor.rs::sermon_cut_regions(segments, duration)`: kutter
  hode, hale OG all `music` _inne i_ preken-spennet (beholder indre stillhet =
  naturlige pauser). Frontend `detection.ts::applySermonTrim` speiler logikken
  (kjører i appen i dag; Rust-funksjonen er kanonisk + seam-klar).
- **D2** Rettet en reell bug i `audio_analysis.rs::detect_segments`: prekenen ble
  bare markert når ETT segment matchet bounds _eksakt_ → Case-0-spennet (hele
  tale-området, kan strekke seg over en sang/pause) ble ALDRI markert → «marker
  preken»-knappen gjorde ingenting for de opptakene. Nå promoteres tale-segmentet
  som _starter_ spennet og strekkes til hele bounds. Ny test for fler-segment-
  spenn. Heuristikken forøvrig (lengste tale = preken; «kun preken» ≥80 %
  tale/<5 % musikk; etter-5-min-preferanse) var allerede på plass og beholdt.

### Løse tråder

- **LT (chrono)** `crates/sundayrec-core/Cargo.toml`: la til chrono-feature
  `alloc` (IKKE `clock` — beholder renheten) så `cargo test -p sundayrec-core`
  kompilerer standalone igjen (en test-helper bruker `to_rfc3339_opts`; uten
  `alloc` resolverte den bare under workspace-feature-unifisering).
- **LT (utdaterte kommentarer)** ryddet «~20Hz/50ms» → faktisk ~60 Hz i engine.rs.
- **LT2 (sample-rate-velger) — viser seg ALLEREDE løst.** NEEDS-RICHARD §settings-
  sync er **utdatert**: `index.html` har radioene auto/r44100/r48000/r96000 med
  **auto = default**, `audio-page.ts` skriver `sampleRateMode`, og `api-shim.ts`
  whitelister + mapper den til Rust `SampleRate` (auto = native). Ingen endring
  nødvendig; bekreftet ende-til-ende. (A3 fikset den gjenværende preroll-delen.)

## Åpne valg (overstyrbare)

- **Default-bitrate 256k** (ikke 320) som balanse størrelse/kvalitet. Tak 320
  beholdt; si ifra om du vil ha 320 som default.
- **Mildere `speech-clear`-mastering** som ny default — A/B på ekte opptak; ruller
  enkelt tilbake (kun filterstreng).
- **C5 frontend-rewire** ikke gjort (se over) — vil du ha `<audio>`-strømmet proxy
  for store/eksotiske filer? Da blir avspilling full-fidelitet + lavere minne.

## RIGG-VERIFISER (kan ikke gjøres headless — kun du, på ekte Mac/Win-rigg)

1. **Hakking borte?** Ta opp 30 s + en lang økt (built-in mic + Behringer USB).
   Bekreft jevn lyd. (A1+A2+A3 er de relevante endringene.)
2. **VU-respons** under REC vs. hjem-siden — skal nå føles ~likt. (B1/B2/B3.)
3. **Pre-roll** på native enhet: skru på pre-roll, ta opp, bekreft at den prependede
   delen er synk + ren i skjøten (A3). USB-frakobling/sleep-wake som før.
4. **Editor-eksport** høres mindre komprimert ut (256k default) + mastering-preview
   matcher eksporten (C3) + mildere mastering (C4) — A/B mot original.
5. **Preken-auto-kutt**: dropp inn et opptak (lovsang→preken→avslutning, og et
   «kun preken»-opptak, og ett med en sang midt i talen), trykk Analyser →
   bekreft at prekenen markeres og at «Bruk forslag» kutter hode/hale + all musikk.
   **Trenger 2–3 ekte preken-opptak for å finkalibrere terskler (D2).**
6. **C5** (hvis wiret senere): proxy-avspilling dekoder i editoren på Mac+Win.

## Bevisst utsatt (trenger verifisering / egen økt)

- **C5 frontend-rewire** (`<audio>`-strømmet playback-proxy).
- **Windows opptaks-VU**: astats-sink er `/dev/stderr` (macOS-only) → Windows
  opptaks-VU er reelt ikke-funksjonell (hjem-VU via Web Audio funker overalt).
- **npm audit**: `tmp`-saken (issue #2) er borte. Gjenstår 2 high i **esbuild/vite**
  som KUN er dev-verktøy (ikke i den pakkede appen) og krever en _breaking_
  vite 8-migrering å fikse — gjort `npm audit fix` (trygg vite 7-patch + rettet
  stale lockfile-versjon 0.2.0→0.4.1), men ikke `--force` blindt. Egen oppgave.
- **A5** (kutte astats-print-rate via `asetnsamples`) — kun hvis A1+A2 ikke holder
  på rigg; rører det opptatte `-af`, så høyest forsiktighet.
