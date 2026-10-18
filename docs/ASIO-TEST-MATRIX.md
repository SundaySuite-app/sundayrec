# Windows-lyd (cpal: WASAPI + ASIO) — test- og utgivelsessjekkliste

På Windows tar SundayRec opp lyd via **cpal** (WASAPI som standard, ASIO for pro)
i stedet for ffmpeg/DirectShow. Hele cpal-capture-stien er **HARDWARE-UVERIFISERT**
til den kjøres på en ekte Windows-maskin. Alt pure (arg-bygging, kanal-ruting,
generisk sample-konvertering, enhets-merge) er enhetstestet og grønt i CI + på mac;
capture-stien kan KUN verifiseres på rigg. **Siden C3b** går alle opptak — også
**video**-økter — gjennom `recorder::native_capture` (cpal → ring → WAV-skriver,
og kameraets videospor ved siden av); cpal-PCM-pipen inn i ffmpeg
(`recorder::cpal_capture`), DirectShow-reserven og «Klassisk (DirectShow)» er
borte. ffmpeg brukes bare til å kode bildet og til leveringen. Bygg-oppsett: [`BUILD_ASIO.md`](./BUILD_ASIO.md).

## Bygg (forutsetning)

- [ ] `cargo run --example asio_spike --features asio` lister minst én ASIO-enhet
      MED sample-format (ASIO4ALL holder) — beviser at `cpal/asio` bygger + linker,
      OG at **24-bit-enheter nå enumereres** (på cpal 0.15 ga 24-bit «no configs»).
- [ ] `npm run tauri build -- --no-default-features --features editor,tray,asio`
      produserer en Windows-installer uten feil.

## Windows — WASAPI (standard, vanlige enheter)

- [ ] **Vanlig USB-mikrofon / lydkort:** velg i Oppsett → «Hvilken lyd?» → ta opp →
      ren fil, ingen «dropped»-advarsel, ingen hakking. Loggen viser
      `recorder: native capture starting host=WASAPI` (lyd-only-stien, ikke
      `cpal capture starting` — det navnet gjelder video-økter).
- [ ] **Stabilitet vs gammel dshow:** samme rigg som tidligere ga ustabile opptak
      → bekreft at WASAPI-veien er stabil.
- [ ] **Lyd + video (WASAPI-lyd + dshow-kamera):** sjekk lepp-synk over et lengre
      opptak (dual-klokke — `build_cpal_pipe_video_args`).
- [ ] **24-bit enhet:** en enhet som rapporterer 24-bit tas opp uten feil (bevis at
      cpal 0.17 + generisk `from_sample` virker).
- [ ] **Enhetsmatching (fuzzy):** valgt enhet med ulik Web Audio-etikett vs cpal-navn
      treffer likevel (cpal slår faktisk inn, ikke stille dshow-fallback). Loggen viser
      `host=WASAPI`.

## Windows — funksjons-paritet på cpal-veien (Runde 3)

- [ ] **Live nivåmetere:** «Opptaksmodus»-overlayet viser levende V/H-metere under et
      WASAPI-opptak (H1 — beregnes i cpal-callbacken, ikke ffmpeg-astats).
- [ ] **Separat lyd-sidecar:** videoopptak med «behold separat lyd» på → sidecar-fil
      produseres ved siden av videoen (H2).
- [ ] **Live auto-stopp:** sett manual-max → nedtelling vises; «+30 min» forlenger;
      «avbryt» fjerner (H3 — watch-kanal).
- [ ] **Ruting:** preroll/oppdeling/stopp-på-stillhet virker på alle enheter, også
      ASIO og i videoopptak — den native motoren har alle tre (siden C3b går
      ingen økt via DirectShow).

## Windows — ASIO (pro-lydkort, f.eks. Soundcraft MADI-USB)

- [ ] **Enumerering:** Oppsett → «Hvilken lyd?» viser kortet som ÉN enhet med
      «ASIO»-merke, øverst i lista (ikke oppdelt i stereopar).
- [ ] **Kanalvalg:** kanalvelger (V/H) viser alle kortets inn-kanaler; velg f.eks.
      9 og 10.
- [ ] **Lyd-opptak:** 1-kanals (mono) taleopptak → ren fil, riktig kanal, ingen
      «dropped»-advarsel i loggen, ingen hakking.
- [ ] **Stereo med custom kanaler:** ta opp valgt V/H-par → begge kanaler korrekt.
- [ ] **Lyd + video:** opptak med kamera + ASIO-lyd → sjekk **lepp-synk** over et
      lengre opptak. ASIO-klokka stemples med vertsklokka i callbacken
      (`CaptureTimeSource::HostNowMinusBlock`, VIDEOMOTOR §C3) — riggubevist.
- [ ] **USB-uttrekk midt i opptak:** trekk ut kortet → appen finaliserer pent med
      «device_disconnected»-melding, henger IKKE.
- [ ] **Auto-stopp (manual-max):** sett en kort grense → opptaket stopper selv.

## Fallback + sikkerhetsbryter

- [ ] **cpal feiler ved start** (åpne kortet i et annet program først) → opptaket
      starter IKKE, og appen sier hvorfor («enheten er i bruk»). Det finnes ingen
      reserve å falle til (A5b for lyd, C3b for video).
- [ ] **«Opptaksmotor»-valget er borte** (C3b): Oppsett → Avansert har ingen
      motorrad, heller ikke på Windows.
- [ ] **ASIO4ALL** fungerer som generisk ASIO-driver.

## macOS — regresjon (skal være uendret)

- [ ] Lydkort via Core Audio (ffmpeg avfoundation): enumerering + opptak som før.
- [ ] Ingen ASIO/WASAPI-merke vises (Windows-only).
- [ ] `asio_spike`-eksempelet skriver «no-op»-meldingen og avslutter 0.

## Utgivelse

- [ ] Bump versjon (package.json + `src-tauri/tauri.conf.json` + `Cargo.toml`).
- [ ] Changelog:
      _«Windows: moderne lyd-motor (WASAPI som standard, ASIO for pro-lydkort med
      flerkanals + lav latens) erstatter DirectShow.»_ (Reserven og «Klassisk
      lyd-motor» er borte siden C3b.)
- [ ] Bekreft Steinberg-attribusjonen vises i Windows-bygget (Oppsett →
      Avansert, nederst — «Lyd-teknologi»). Kortet forsvant med legacy-skallet i
      fase B og er bygget opp igjen; `AsioAttribution.test.tsx` pinner ordlyden,
      men at kortet FAKTISK står i et Windows-bygg er fortsatt en rigg-sjekk.
- [ ] Lyd-motoren forblir en **gratis kjernefunksjon** (ikke bak Pro-tier).

## Funksjons-paritet (Runde 3) — status

- ✅ Wiret inn på cpal-stien: live nivåmetere, separat lyd-sidecar, live auto-stopp
  (+30/avbryt/nedtelling), fuzzy enhetsmatching.
- ✅ Rutes til dshow for vanlige enheter: preroll, oppdeling (split), stopp-på-stillhet
  (full funksjon, ingen stille tap). ASIO + disse → tydelig varsel.

## Kjent utsatt

- **Krasj-recovery-manifest** skrives ikke på cpal-stien (H4 ikke implementert): en app-
  krasj MIDT i et cpal-opptak gir ikke auto-gjenoppretting ved neste oppstart (best-effort
  funksjon; filen som ffmpeg allerede skrev finnes fortsatt på disk). dshow-veien har full
  recovery.
- Split/preroll/stillhet PÅ en ASIO-enhet (ikke ruterbart til dshow — dshow kan ikke ASIO);
  gir «feature_unsupported_asio»-varsel.
- Ekte driver-leverte ASIO-kanalnavn (viser «Input N») krever ASIO SDK
  `ASIOGetChannelInfo` under cpal — TODO.
- Kosmetisk «WASAPI»-merke på vanlige enhetskort i UI (ASIO-kort har «ASIO»-merke);
  loggen bekrefter host-valget under rigg-test.
