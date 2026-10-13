# Opphav: filmcraft `audio-dsp`

Denne craten er kopiert inn fra [storytold/filmcraft](https://github.com/storytold/filmcraft)
(`crates/audio-dsp`), under **MIT OR Apache-2.0**. Lisensene ligger i
`LICENSE-MIT` og `LICENSE-APACHE`; filmcrafts `NOTICE` følger med uendret.
Opphavsrett: ArtCraft Team and the FilmCraft contributors.

Det er et bevisst unntak fra «avhengighet før kopi» i
[`docs/AVHENGIGHETER.md`](../../docs/AVHENGIGHETER.md) (eiers beslutning
2026-10-11). Bakgrunn: [`docs/LYDMOTOR.md`](../../docs/LYDMOTOR.md).

## Låst commit

```
cee1feb185e675a108f78067f71ad86a640ace3d   (2026-10-13)
Merge pull request #1433 from vangel76/fix-mpegts-hardening
```

Valgt som nyeste commit på `main` da B0a ble gjort (2026-10-13). Craten er
workspace-versjon **0.7.2** oppstrøms.

Gjennomgangen (2026-10-11) brukte `0800a8b0aa4a703ba42172f415b45a61f8d8a5e1`.
Diffen `0800a8b0..cee1feb1` for `crates/audio-dsp` er lest. Den er liten: én
commit, `dcabbda0` («M7: optimize settled mixer DSP and isolate throughput
gate»), som rører tre filer (+338/−10):

- `src/biquad.rs`: ny `Biquad::is_clear()` (nullstilt tilstand, inkl. fortegn på null).
- `src/effects/eq.rs`: hurtigsti i `BandBank::process` når alle parametre er
  innsvingt; flat EQ hoppes over bare når tilstanden er null og inngangen er
  endelig. Referanseimplementasjon og to tester som sammenligner bit for bit.
- `src/effects/time.rs`: ringbuffer-lesing uten heltallsdivisjon, og
  `Reverb::update_gains` hopper over når lengde og RT60 er uendret. Også
  referanseimplementasjon og bit-for-bit-tester.

Ingen endring i API, målemetodikk eller `loudness.rs`. Oppdateringen er ren
ytelse, med tester som beviser at utdataene er identiske.

## Tatt med

`src/**` (alle filer), `tests/loudness_oracle.rs`, `README.md`, `LICENSE-MIT`,
`LICENSE-APACHE`, `NOTICE`, og oppstrøms `rustfmt.toml` / `clippy.toml`.

Bare `audio-dsp`-craten. Den har ingen avhengigheter. Ikke tatt med:
kodekene, containerne, egui-grensesnittet (`ui-egui`), tidslinje- og
flersporsmotoren (`edit`, `engine`, `render`, `export`) og musikkeffektene
utenfor denne craten. Ingenting annet fra oppstrøms ligger i treet.

## Lokale endringer

Hver endring mot oppstrøms står her. Kildefilene under `src/` er **uendret**
bortsett fra at `cargo fmt` (med oppstrøms `rustfmt.toml`) ble kjørt; det ga
ingen endring i `src/`.

1. `Cargo.toml` skrevet om: `version`, `edition` (2024), `license` og
   `rust-version` (1.95) er fastsatt i stedet for å arve fra oppstrøms
   workspace; `[lints]` er skrevet ut (`unsafe_code = "forbid"` og de samme
   clippy-lintene som oppstrøms workspace); `publish = false`; dev-avhengigheten
   `filmcraft-testkit` er fjernet.
2. `rustfmt.toml` og `clippy.toml` kopiert fra oppstrøms rot (bredde 160 og
   «tillat unwrap/indeksering i tester»), slik at vår `cargo fmt --check` og
   `clippy -D warnings` tolker koden som oppstrøms gjør.
3. `tests/loudness_oracle.rs`: kallene til `filmcraft_testkit`
   (`require_ffmpeg!`, `fixtures_dir`, `temp_path`, `wav::write_f32`) er byttet
   mot en liten lokal `mod support` i samme fil: finner `ffmpeg-*` i
   `src-tauri/binaries/` (hopper over tomme stubber som ikke kan kjøres), skriver
   WAV selv og bruker OS-ets temp-katalog. Testen hopper over uten kjørbar
   sidecar, men **feiler** hvis `SUNDAYREC_REQUIRE_SIDECAR` er satt (`check`-jobben
   i `ci.yml` setter den). Selve målingene og toleransene er uendret. B0b
   fullfører dette (±0,1 LU mot 9.0.1-sidecaren på alle tre plattformer).
4. Rot-`Cargo.toml` fikk `[profile.dev.package.filmcraft-audio-dsp] opt-level = 3`
   (oppstrøms har samme linje) fordi sanntidsfaktor- og spektraltestene er
   minutter tregere uten.

## Oppdatere

Vi følger ikke oppstrøms fortløpende. En oppdatering er en egen PR: hent ny
commit, `git diff <låst commit>..<ny> -- crates/audio-dsp`, les diffen, kopier
inn, kjør testene, og endre «Låst commit» og listen over lokale endringer her.
