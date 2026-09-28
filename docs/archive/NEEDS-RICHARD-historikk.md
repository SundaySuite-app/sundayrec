# Trenger Richard — historikk

Seksjoner flyttet ut av `docs/NEEDS-RICHARD.md` 2026-09-28, ordrett. De
gjelder funksjoner som er fjernet fra appen (R1/R2 «Frivilligen først»,
v0.14) eller revisjoner som er ferdig behandlet. Ingen av dem har et åpent
punkt: det som fortsatt gjelder, står i `docs/NEEDS-RICHARD.md` og
`docs/PLAN.md`.

## Statusblokken fra 2026-08-06 (v0.10.0)

> **Status 2026-08-06 (`feat/make-it-real`, v0.10.0).** `email` and `streaming`
> joined `default` in this round, several seams listed below as "remaining glue"
> are now wired, and the IPC surface was audited end to end — see
> `docs/archive/COMMAND_AUDIT_2026-08.md` (arkivert i V1/PR3 — the living
> truth is `scripts/command-reachability-baseline.json`) for which commands
> the UI can and cannot reach, and the morning report `SundayRec-MAKE-IT-REAL-2026-08-06.md` (one
> directory above the repo) for the rig checklist and the owner decisions.

## ~~PU-3 — Podcast RSS publish (`--features publish`)~~ — **FJERNET R1 2026-08-23**

The feed builder, the `publish` seam/feature and the Podcast card left with
the sharing cluster. Git history is the feature flag.

## ~~PU-5 — Whisper transcription (`--features whisper`)~~ — **FJERNET R2 2026-08-23**

Transcription left with the content cluster: the `whisper` feature, whisper-rs
(libwhisper — the build's only C/C++ toolchain dependency, now gone), the
model registry/download, the Transkribering panel, SRT/VTT/TXT export and the
transcript search. No rig item remains. `.transcript.json` sidecars that
already exist still travel with their recording through the trash.

## ~~PU-6 — Episode prep + review queue + Stage import~~ — **FJERNET R1 2026-08-23**

The prep pipeline, the review queue + reminder ladder, the tray callout and
the Stage manifest import left with the sharing cluster. One consequence for
the owner: `learning::record_trim_deltas` / `current_tuning` (the E8/E10
trim-correction loop and the local nudge) were only ever driven from the
review path and had no caller — R2 removed the local nudge, the learning
viewer cards and the `localAdaptivity` setting («dead → delete»); the
trim-adjustment RECORD and its telemetry projection stay, dormant, for the
day the editor writes it (`docs/LEARNING.md` §Status).

## ~~Bridge Integration #2 — Live cue bridge (`--features bridge`)~~ — **FJERNET v0.14**

> Den native WebSocket-halvdelen (`bridge_live::subscribe`, `bridge`-feature-et,
> `live_bridge_*`-kommandoene) ble fjernet med Direkte-siden. Den RENE
> kontrakt-speilingen (`sundayrec-core::integrations::live_bridge`) består,
> testet, med et doknotat om hvorfor — en framtidig konsument starter derfra.

- **A live Supabase project + SundayStage publishing.** The Rec side SUBSCRIBES
  to `church:{churchId}:service:{serviceId}` and folds inbound `LiveEvent`s into
  chapter markers + live/ended state. The channel-name + the `LiveEvent` union +
  the `apply_event` fold (with monotonic-`seq` gap/replay handling) are
  unit-tested in `sundayrec-core::integrations::live_bridge`, and the renderer
  can drive the mapping with `live_bridge_map_event` (no feature). The native
  WebSocket subscribe (`bridge_live::subscribe`, behind `--features bridge`) is
  INFRA-UNVERIFIED — the Phoenix handshake/`phx_join`/broadcast decode need a
  live backend (smoke §10c).
- **Emit + persist glue.** The subscribe loop currently logs each folded
  `BridgeEffect`; wiring `ChapterAdded` into the running recording's metadata +
  emitting a Tauri event for the UI is the remaining glue. The Supabase URL +
  anon key also need to flow from settings (the integration `connection` config).

## ~~R3 — Live streaming (`streaming`)~~ — **FJERNET v0.14**

> Hele strømme-flaten (Direkte-siden, `stream_*`-kommandoene, motoren,
> `sundayrec-core::{streaming,overlay}`) ble fjernet i v0.14 — SundayRec er et
> opptaksprogram. Riggpunktene under er derfor DØDE; historikk beholdt.

- **A real camera + a real RTMP endpoint + a stream key.** The `streaming`
  feature compiles the ffmpeg spawn seam (`src-tauri/src/streaming/mod.rs`)
  in/out — NO new native dep (ffmpeg is a sidecar). **It joined `default` in
  2026-08**: the Direkte page had shipped with an enabled START button in every
  release, and pressing it returned the raw string `feature_disabled`. The RTMP
  push itself is still NETWORK/HARDWARE-UNVERIFIED and the page now says so out
  loud. Only the `sundayrec-core::{streaming,overlay}`
  decisions (the multi-destination `tee` muxer argv with `onfail=ignore`, the
  libx264/aac encode + keyframe-every-2s GOP + bitrate/bufsize math, the
  platform audio-map, the optional local-MP4 branch, the 0.5fps preview, the
  lower-third image/drawtext `filter_complex`, the key/URL validation, the
  key-redacted loggable copy) are unit-tested.
- **✅ Auto-recovery + live stats are ported.** The stderr parse
  (`frame=…fps=…bitrate=…`), the per-destination `connecting/live/failed`
  state, the capped reconnect backoff and the tee-slave-failure step-down all
  live in `sundayrec-core::streaming` and are driven by the supervisor in
  `src-tauri/src/streaming/mod.rs`. `streaming://stats` — the event the Direkte
  page had listened for since the port and **nobody ever sent** — is now emitted
  at 1 Hz plus on every transition, with a tail push after stop. The panel's
  three remaining lies (destination field mismatch, a Live-pill stuck on, raw
  error codes) were fixed at the same time. **Still HARDWARE-UNVERIFIED**: the
  behaviour under a real RTMP disconnect has never been observed.
- **`alsoRecord` history row.** The "Start direktesending + opptak" local MP4 is
  built into the argv (the 3-way split branch), but registering the finished
  file in recording history (the Electron `registerAlsoRecordInHistory` + the
  MP4-duration probe + the 100 KB skeleton guard) is not yet wired.
- **The stream-keys live in the OS keychain** (per-destination, namespaced
  `stream.key.<id>` via `crate::secrets`), never a plaintext file — confirm the
  keychain round-trips on the target machine (the tolerant test skips when no
  keychain is reachable).

## ~~R3 NDI — receiver (`--features ndi`)~~ — **FJERNET v0.14**

> Eierbeslutningen fra kommando-revisjonens §4.5 falt: NDI (mottak OG sending,
> stub + kjerne) er fjernet. Historikk beholdt under.

- **The NDI SDK runtime + a native FFI binding + an NDI source on the LAN.** The
  `ndi` feature compiles a **STUB** seam (`src-tauri/src/ndi/mod.rs`):
  `list_sources` returns empty and `start_receiver` returns
  `ndi_not_bundled: NDI SDK not bundled — see docs/NEEDS-RICHARD.md`. The
  default build returns `feature_disabled`. NO native NDI dep is added (none is
  present in this environment).
- **What's already done (pure + tested).** `sundayrec-core::ndi` has the
  discovered-source model, the delivered-FourCC → ffmpeg-pixfmt selection
  (`UYVY`/`BGRA`/`BGRX` → `uyvy422`/`bgra`, falling back to the alpha request),
  the `-f rawvideo -pix_fmt … -s WxH -framerate … -i tcp://127.0.0.1:<port>`
  input-arg builder, and the saved-source-name matcher. The `streaming` seam
  already knows how to splice an NDI overlay's input args + frame size into the
  pipeline once a receiver hands back an `NdiReceiverInfo`.
- **The real implementation (needs Richard + a rig + the SDK):** vendor the NDI
  SDK (the runtime `.dylib`/`.dll` + headers) and add an FFI crate (the Electron
  app used the `grandiose` Node binding; the Rust equivalent is a thin FFI over
  `NDIlib_find_*` + `NDIlib_recv_*`). Then implement, per the Electron
  `ndi-receiver.ts` architecture: an mDNS-style `find` discovery window
  (~2 s), a receiver that pulls the first frame to resolve `WxH`+FourCC, an
  ephemeral **loopback TCP server** (`127.0.0.1:0`) that serves the raw frame
  bytes (one client = the streamer's ffmpeg, back-pressured by the TCP window,
  late frames dropped), and a clean `stop()` racing a 2 s timeout
  (`RecorderTimeouts::NDI_STOP_TIMEOUT_MS`) so a libndi deadlock can't block
  stream-stop. Bundle the SDK in `tauri.conf.json` (`externalBin`/resources) the
  way the Electron app `asarUnpack`-ed `vendor/grandiose`.

## ~~P6 — Transcript search backend wiring (no feature flag)~~ — **FJERNET R2 2026-08-23**

The transcript index (`transcripts_list`), the hit-snippet rows and the «Med
transkript» chip left with whisper. Historikk's search box still filters by
filename, date and note (e2e-pinned).

## Settings-sync + IPC-seam audit (natt 2026-06-05)

Etter wake-from-sleep-funnet (merget i PR #2) gjorde jeg en systematisk audit av
(a) hvilke `Settings`-felt backend-konsumentene faktisk leser vs. hva
`syncBackendRecordingSettings` (api-shim → `settings_save`) sender, og (b) hele
`call()`/`invoke()`-seamen i api-shim (`legacy/renderer/api-shim.ts` den gang,
`app/lib/api-shim.ts` etter fase B) mot Rust-signaturene.
Bakgrunn: backend-sqlite får KUN det kuraterte opptaks-subsettet; alt utenfor det
re-defaultes av `#[serde(default)]` ved HVER lagring.

**FIKSET (gren `feat/night-settings-sync`, upushet — vent på review):**

- **`filenamePattern` nådde aldri recorderen.** `scheduler::build_opts` bruker
  `settings.filename_pattern` til opptaks-filnavnet, men feltet manglet i det
  kuraterte subsettet → re-defaultet til `date` ved hver `saveSettings`. En
  bruker som valgte `church`/`plain`/`datetime` fikk hvert opptak navngitt med
  `date`-mønster. Lagt til (whitelistet, så en korrupt localStorage-verdi ikke
  feiler HELE `settings_save`). **Rigg-sjekk:** velg et ikke-`date`-mønster, ta
  opp → filnavnet skal følge valget.

**ÅPNE SPØRSMÅL (krever din intensjon — bevisst IKKE rørt):**

- **Sample-rate-valget i UI er frakoblet faktisk oppførsel.** UI-en lar deg velge
  44.1/48/96 kHz og lagrer `sampleRate: number`, men (1) hoved-recorderen bruker
  `sample_rate_mode`-enumet (`resolved_sample_rate`) som UI-en aldri setter →
  alltid `Auto`/native, og (2) pre-roll bruker det gamle `sample_rate`-feltet som
  ikke synkes → alltid 48000. Native/Auto er bevisst valgt for å unngå
  resample-hakking, så å tvinge valget kan forringe lyd. **Spørsmål:** skal
  UI-valget faktisk styre rate (map `sampleRate` → `sample_rate_mode` i synken),
  eller skal vi fjerne velgeren og alltid kjøre native? Jeg gjør ingen av delene
  uten svar.

- ~~**`stream_start` kan aldri lykkes slik den er wiret.**~~ **LØST 2026-08.**
  Kommandoen resolver nå kamera-/mikrofon-tokens selv, fra de lagrede
  enhets-NAVNENE, med samme ffmpeg-enumerering og uklare navnematch som
  opptakeren bruker. Shim-en sender `{destinations, resolution, framerate,
videoBitrateKbps, audioBitrateKbps, alsoRecord, overlays}` og signaturen
  stemmer. `streaming` er dessuten i `default` nå, så knappen er ikke lenger et
  `feature_disabled`-svar. Selve RTMP-pushen er fortsatt uverifisert mot rigg.

**LENGER IKKE SANT (rettet 2026-08, sist 2026-08-09):** notatet under sa at
«e-post/webhook/cloud/integrasjoner … frontend-metodene deres er bevisste
no-op-stubs i `api-shim.ts` → backend drives aldri av dem». Det gjelder nå
**kun cloud**. E-post og webhook er ekte: `email_send_test`,
`email_test_webhook` og nøkkelring-kommandoene er koblet opp, og — viktigere —
det ble funnet at kirke-/e-post-/webhook-innstillingene **aldri nådde sqlite i
det hele tatt** (de lå kun i `localStorage`, så backend leste defaults). Det
kuraterte subsettet i `syncBackendRecordingSettings` er utvidet deretter.
**Integrasjons-stubbene er også borte:** PR #114 (2026-08-09) koblet alle ti
`integrations_*`-kommandoene til ekte kall med ærlige kvitteringer (pinnet i
`e2e/integrations.spec.ts`); se `docs/archive/COMMAND_AUDIT_2026-08.md` §4.2, som nå
er merket løst. HTTP-sidene forblir nettverks-uverifiserte til riggtest.
**(R1 «Frivilligen først» 2026-08-23: hele avsnittet over er historikk —
cloud, webhook og integrasjonene er FJERNET; bare e-post-stien besto — og
den er også fjernet siden, se PU-1.)**
