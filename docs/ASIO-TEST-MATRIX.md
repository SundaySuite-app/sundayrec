# ASIO — test- og utgivelsessjekkliste

ASIO-veien er **HARDWARE-UVERIFISERT** til den er kjørt på en ekte Windows-maskin
med en ASIO-driver. Alt pure (arg-bygging, kanal-ruting, enhets-merge) er
enhetstestet og grønt i CI; capture-stien (cpal-stream → ffmpeg-pipe) kan KUN
verifiseres på rigg. Bygg-oppsett: [`BUILD_ASIO.md`](./BUILD_ASIO.md).

## Bygg (forutsetning)

- [ ] `cargo run --example asio_spike --features asio` lister minst én ASIO-enhet
      (ASIO4ALL holder) — beviser at `cpal/asio` bygger + linker på toolchainen.
- [ ] `npm run tauri build -- --no-default-features --features editor,tray,asio`
      produserer en Windows-installer uten feil.

## Windows — funksjonell test

ASIO-kort (f.eks. Soundcraft MADI-USB):

- [ ] **Enumerering:** Innstillinger → Lyd viser kortet som ÉN enhet med
      «ASIO»-merke, øverst i lista (ikke oppdelt i stereopar).
- [ ] **Kanalvalg:** kanalvelger (V/H) viser alle kortets inn-kanaler; velg f.eks.
      9 og 10.
- [ ] **Lyd-opptak:** 1-kanals (mono) taleopptak → ren fil, riktig kanal, ingen
      «dropped»-advarsel i loggen, ingen hakking.
- [ ] **Stereo med custom kanaler:** ta opp valgt V/H-par → begge kanaler korrekt.
- [ ] **Lyd + video:** opptak med kamera + ASIO-lyd → sjekk **lepp-synk** over et
      lengre opptak (dual-klokke er den høyeste risikoen — se
      `build_asio_video_args`).
- [ ] **USB-uttrekk midt i opptak:** trekk ut kortet → appen finaliserer pent med
      «device_disconnected»-melding, henger IKKE.
- [ ] **Auto-stopp (manual-max):** sett en kort grense → opptaket stopper selv.

Fallback:

- [ ] **Class-compliant USB-mikrofon UTEN ASIO-driver:** velges via vanlig
      enhetsliste → tas opp via WASAPI som før.
- [ ] **ASIO-enhet feiler ved start** (åpne kortet i et annet program først) →
      SundayRec faller tilbake til WASAPI + viser «asio_fallback»-melding, opptak
      fungerer.
- [ ] **ASIO4ALL** fungerer som generisk ASIO-fallback.

## macOS — regresjon (skal være uendret)

- [ ] Samme lydkort via Core Audio: enumerering + opptak fungerer som før.
- [ ] Ingen ASIO-kort/-merke vises (ASIO-feature er ikke kompilert på macOS).
- [ ] `asio_spike`-eksempelet skriver «no-op»-meldingen og avslutter 0.

## Utgivelse

- [ ] Bump versjon (package.json + `src-tauri/tauri.conf.json` + `Cargo.toml`).
- [ ] Changelog:
      _«Windows: ASIO-støtte for pro-lydkort (flerkanals, lav latens). Faller
      automatisk tilbake til WASAPI.»_
- [ ] Bekreft Steinberg-attribusjonen vises i Windows-bygget (Innstillinger →
      Generelt → «Lyd-teknologi»).
- [ ] ASIO forblir en **gratis kjernefunksjon** (ikke bak Pro-tier).

## Kjent utsatt (faller tilbake til dshow/WASAPI på ASIO-stien)

- Split, reconnect, preroll-over-ASIO, live VU-metere og stopp-på-stillhet er
  IKKE wiret på ASIO-stien i v1 (se `recorder::asio`-modul-doc). Velg vanlig
  enhet for disse, eller bruk dem på lyd uten ASIO.
- Ekte driver-leverte kanalnavn (v1 viser «Input N») krever ASIO SDK
  `ASIOGetChannelInfo` under cpal — TODO.
