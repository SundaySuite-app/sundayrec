# Needs Richard — Electron-parity seams (PU-1…R7)

The pure decision logic for these features is ported into `sundayrec-core` and
fully unit-tested; the impure seams sit behind cargo features. **Four of them
are in `default`** — `editor`, `tray`, `updater` and `email` — so the
Rediger-screen, the tray, auto-update and failure e-mail all ship in a normal
build. Scheduler/wake are always compiled. (v0.14: `streaming`, `ndi` and
`bridge` were REMOVED together with the Direkte page; R1 «Frivilligen først»
2026-08-23 removed the sharing cluster — cloud backup, podcast RSS + `publish`,
the chat webhook, the Gmail transport, the Sunday-suite integrations +
`sundayrec://`, cover art and the review queue; R2 the same day removed the
content cluster — `whisper` transcription (the only C/C++ dependency), the AI
companion, chapter detection, the learning cards + local adaptivity, the dead
settings fields and the Video tab's quality knobs. Their sections below are
kept only as struck history.) The items below need a real account / desktop session / device
/ signing identity that the headless gate cannot provide. None block the default
build or the gate. The consolidated "what only Richard can provide" checklist is
at the bottom of this file.

> **Status 2026-08-06 (`feat/make-it-real`, v0.10.0).** `email` and `streaming`
> joined `default` in this round, several seams listed below as "remaining glue"
> are now wired, and the IPC surface was audited end to end — see
> `docs/COMMAND_AUDIT_2026-08.md` for which commands the UI can and cannot
> reach, and the morning report `SundayRec-MAKE-IT-REAL-2026-08-06.md` (one
> directory above the repo) for the rig checklist and the owner decisions.

## ⭐ Release blockers — current checklist (only Richard can do these)

A precise, up-to-date list of the account/key/identity work standing between the
code-complete state and a **signed, auto-updating, public release**. See
`docs/archive/RELEASE-AUDIT-2026-06-01.md` for the pipeline audit and
`docs/DISTRIBUTION.md` for the step-by-step.
Status updated 2026-08: releases are **signed + auto-updating in prod**; the
only remaining release blocker is **notarization** (item 3).

1. **GitHub Actions billing block — LØST (2026-07-08).** Repoet er nå
   offentlig, og Actions-minutter er gratis for offentlige repoer (også
   macOS/Windows-runnerne). CI kjører nå på push til `main` + PR-er i tillegg
   til `v*`-tagger; `release.yml` kjører på tag som før.

2. **Apple Developer ID signing — ✅ RESOLVED (~2026-07-31).** The cert was
   re-exported and the secrets are set: `MAC_CERTS` (base64 of the `.p12`) +
   `MAC_CERTS_PASSWORD`, mapped to tauri-action's `APPLE_CERTIFICATE` /
   `APPLE_CERTIFICATE_PASSWORD` in `release.yml` env (lines 143–145;
   `APPLE_SIGNING_IDENTITY` — `Developer ID Application: … (784GN847G4)` — is
   hardcoded there). Published releases are **signed** since ~07-31. See
   DISTRIBUTION.md "macOS code signing".

3. **Notarization — the real remaining blocker: the Apple Program License
   Agreement.** Apple's notary service returns **403 "A required agreement is
   missing or has expired"** until the updated PLA is accepted on
   developer.apple.com (team `784GN847G4`). Notarization is therefore
   **deliberately disabled**: the `notarytool` env lines (`APPLE_ID`,
   `APPLE_PASSWORD` ← `APPLE_APP_SPECIFIC_PASSWORD`, `APPLE_TEAM_ID`) are
   commented out in `release.yml` (lines 146–155) — the comment block there
   documents how to re-enable them once the agreement is signed. ⚠️ The
   **app-specific password was leaked in chat** — **revoke it** at
   appleid.apple.com → Sign-In and Security → App-Specific Passwords, **generate
   a new one**, and store it only as the `APPLE_APP_SPECIFIC_PASSWORD` GitHub
   secret.

4. **Tauri updater — ✅ DONE, proven in prod.** The `plugins.updater` block
   (pubkey + endpoints) is in `tauri.conf.json`, `uploadUpdaterJson: true` is
   set in `release.yml`, the keypair exists (key-id `4f08a2f48edd9a17`, backup
   `~/.tauri/sundayrec_updater.key`), and the `TAURI_SIGNING_PRIVATE_KEY`
   (+ `…_PASSWORD`) secrets are set. The updater has been **live in published
   releases since v0.4.x** — the `latest.json` feed is verified in prod (v0.8.0
   current). See `docs/RELEASE-CHECKLIST.md`.

5. ~~**Google OAuth console client (Desktop app type).**~~ **GONE in R1
   «Frivilligen først»** — cloud backup and the Gmail transport that needed
   `SUNDAYREC_GOOGLE_CLIENT_ID` were removed; e-mail is SMTP-only and needs no
   Google client. (`docs/archive/GOOGLE-OAUTH-SETUP.md` stays as history.)

The per-feature seam detail follows below; this checklist is the release-gating
subset.

## PU-1 — Email alerts (`email`, now in `default`)

- **✅ The feature ships.** `email` joined `default` (and both release feature
  lists) in 2026-08. Before that it was in no published build: an unattended
  volunteer operator could configure e-mail alerts in the UI and get nothing,
  forever.
- **✅ The keychain write path exists.** `email_set_smtp_password` /
  `email_has_smtp_password` / `email_clear_smtp_password` are wired to the
  **Oppsett → Avansert → «E-postserver (SMTP)»** card. The SMTP password is
  still intentionally NOT
  in the settings bag — it lives in the OS keychain, and a stored password takes
  precedence over anything typed into the field. `email_smtp_from` lets the
  From: address differ from the account.
- **SMTP credentials.** SMTP is the one transport (the Gmail-API path left
  with cloud backup in R1): a host, port, user and app-password.
- **👤 Deliverability check (rig).** Confirm a real "✓ email works" message
  arrives from **Test e-post**, that a killed recording produces a failure
  e-mail, and that the throttle suppresses a 2nd identical alert within 10 min
  (smoke §8).

## PU-2 — Tray (`--features tray`) — deep links REMOVED in R1

- **A desktop session.** The native menubar/tray item needs a real GUI to
  verify. **(R7 update)** the tray is now actually **installed** in `setup()`
  under `--features tray`: `tray::install` builds the `TrayIcon` from the
  unit-tested core menu model and wires `on_menu_event` → `handle_menu_event`
  (Stop calls `RecorderEngine::stop()` directly; start/preflight/diagnostics
  emit `tray://action`; show/quit are in-process). Build proven with
  `cargo build -p sundayrec --features tray` + clippy `-D warnings`. (The
  `sundayrec://` scheme, `tauri-plugin-deep-link` and `dispatch_deep_link`
  left with the Sunday-suite integrations in R1.)
- **Tray icon assets.** The Electron app shipped `tray-idle/recording/error`
  PNGs (+ macOS `Template` + Windows dark variants) under `assets/`. **(R7)** the
  shell currently reuses the app's **default window icon** for the tray; the
  per-state idle/recording/error assets still need bundling + a swap on
  `TrayState` change (`sundayrec_core::tray::icon_for` already picks the base).

## R7 — Auto-update (`--features updater`) — ✅ DONE, proven in prod

- **All of it is in place and live since v0.4.x.** The `updater` feature
  compiles the seam (`src-tauri/src/update/mod.rs`) + registers
  `tauri-plugin-updater`; the status model + dev-check guard + percent math +
  semver "is newer" decision are the unit-tested `sundayrec-core::update`. The
  once-only setup is done:
  1. ✅ Keypair generated (`~/.tauri/sundayrec_updater.key`, key-id
     `4f08a2f48edd9a17` — keep the backup; losing it means users can't
     auto-update and need a manual reinstall with a new key).
  2. ✅ The **public** key is in `tauri.conf.json` under
     `plugins.updater.pubkey`, with the `endpoints` array pointing at the
     `latest.json` the release CI publishes.
  3. ✅ The release CI secrets `TAURI_SIGNING_PRIVATE_KEY` (+ `…_PASSWORD`) are
     set and `uploadUpdaterJson: true` is in `release.yml` — see
     docs/DISTRIBUTION.md "Auto-update signing".
- The feed fetch, signature verify, download and relaunch are **verified in
  prod**: the `latest.json` feed serves published releases (v0.8.0 current) and
  real installs update from it. A dev build still short-circuits the check by
  design.

## ~~PU-3 — Podcast RSS publish (`--features publish`)~~ — **FJERNET R1 2026-08-23**

The feed builder, the `publish` seam/feature and the Podcast card left with
the sharing cluster. Git history is the feature flag.

## PU-4 — OS wake-timers + scheduled launch (no feature flag)

- **A real Mac/Windows box.** The scheduler supervisor's wall-clock timing, the
  `pmset`/`osascript` admin prompt, and whether the machine _truly_ wakes from
  sleep remain HARDWARE-UNVERIFIED. What is NO LONGER unverified: the command
  shaping and its quoting, the macOS escalation ladder and its
  Permission/Cancelled classification, the Windows arm/clear ladder, the
  `wmic`→CIM fallback, and the dedup invariants — all unit-tested over a fake
  shell and fake timers in `src-tauri/src/wake/{plan,shell,win_timer,mod}.rs`.
  The macOS IOKit read runs for real in the gate. Decisions stay in
  `sundayrec_core::{schedule, wake}`; the rig exit is smoke §11.
- **Windows wakes only while SundayRec runs.** The mechanism is an in-process
  `SetWaitableTimer(fResume = TRUE)`, not a scheduled task: no UAC prompt and
  nothing left behind on the machine, but the timer dies with the process. That
  is the owner-approved model (autostart + tray), and it is stated in the app's
  own capability text — please confirm on the rig that quitting the app really
  does stop the wake, so nobody later "fixes" it back into a scheduled task.
- **Windows code is compile-checked only here.** Nothing on a Mac builds the
  `SetWaitableTimer` path; CI's `windows-check` lane is the only thing that
  proves it compiles, and no automated test anywhere proves it wakes.
- **Apple Silicon can lie about its own schedule.** `pmset -g sched` is known to
  omit active schedules, so verification reads IOKit first and falls back to
  `pmset`. A wake we cannot see is reported as a mismatch (prompting a
  re-register) rather than assumed present — expect the occasional
  "click Planlegg again" that turns out to have been unnecessary.
- **Missed-recording persistence** still waits on a `status`/`error` column on
  the `recording` table (see the `scheduler/mod.rs` honest-gaps note).

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

## R1 — Non-destructive editor (`--features editor`)

- **A real recording + a smoke run.** The cut/keep planning, the audio/video
  filter graphs, the codec/output-path/chapter decisions, the EBU R128
  loudnorm measure/apply chains + JSON parse, and the VAD/sermon classifier are
  all unit-tested in `sundayrec-core::{editor, mastering, audio_analysis}`. The
  I/O seam (`src-tauri/src/editor`) spawns the ffmpeg/ffprobe sidecar with that
  argv (load / peaks / segments / mastering-analyze / export). NO new native dep
  (ffmpeg is a sidecar; WAV/PCM parsed by hand). All five runs are
  HARDWARE-UNVERIFIED — they need real media (smoke §12). Build proven to
  compile with `cargo build -p sundayrec --features editor`.
- **Deferred to a later editor phase (parity gaps, not bugs):**
  - **Cut-region timeline UI.** The R1 panel exports the _whole_ file
    (`cutRegions: []`) — it proves the full IPC surface end-to-end. The
    drag-to-mark cut UI + waveform-overlaid timeline (the Electron
    `renderer/pages/editor/*`) is the renderer work for the next phase; the
    backend already accepts `cutRegions` and the core plans the keeps.
  - **Chapter metadata on export.** The core still builds the `;FFMETADATA1`
    chapter sidecar (`ffmetadata`, `metadata_args`, kept + tested), but since
    R2 nothing produces chapters — the transcript-driven detector and the
    chapter list left with the content cluster — so the export hands it an
    empty list. A future chapter source only has to fill that list.
  - **Replace-mode + atomic swap.** R1 exports a new `*_redigert.<fmt>` file
    only. The Electron `saveEdited`/`safeReplaceFile` in-place replace (with the
    `.__editor_tmp`/`.__editor_bak` crash-recovery sweep) + the FORCE_WAV
    replace refusal (`resolve_save_ext` is already tested in core) is the next
    increment.
  - ~~**Export progress events + cancel.**~~ **DONE.** `editor_export` streams
    `time=` progress as `editor://export-progress`, `editor_cancel_export` is a
    real cancel handle, and the 2026-08 progress round put a monotone
    percentage + an ETA on the bar (`export_timeout_ms` is still the tested
    kill-timer).

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

## E2 — Observability: crash ring, log file, capture/video probes (no feature flag)

Etappe 2 added a panic hook + bounded crash ring (E2.1), a supervisor that
restarts long-lived tasks (E2.2), a rotating file log (E2.3), a renderer-side
IPC-failure ring (E2.4), and a real capture/video probe in Diagnose (E2.5) —
see smoke §13 for the full walkthrough. All of it is unit-tested at the
decision level (`sundayrec-core::diagnostics`, `src-tauri/src/crash.rs`,
`src-tauri/src/logfile.rs`); what needs a rig is whether real hardware and a
real long session behave the way the tests assume.

- **Live-exercise `SUNDAYREC_TEST_PANIC`.** Run it end to end on your own
  machine (smoke §13): quit the installed app, set the env var, launch a debug
  build, and confirm a `crash-*.json` lands in `<app-data>/crashes/` and
  **SR-CRASH-01** shows up in Diagnose with the right count/message. Nobody has
  watched this happen outside the unit tests yet.
- **Capture probe against the Qu-5.** The digital-mixer channel-count/
  negotiation incident (2026-07-31) is exactly the kind of device the capture
  probe (`SR-CAPTURE-02`) exists to catch honestly. Run Diagnose against the
  Qu-5 with a channel actually carrying signal and with one that is not, and
  confirm `captureOk` matches reality rather than a stale device handle.
- **Video probe with the camera held by another app.** Start Zoom/Teams (or
  anything else that opens the camera) and then run Diagnose with video
  enabled. `SR-VIDEO-02` should fire with a clear message rather than the probe
  hanging or crashing on the camera-busy failure — the same class of
  contention the two software-side refusal paths (a live recording / the VU
  meter) already guard against, this time from an OS/other-app angle.
- **Log rotation after a 90-minute service.** `MAX_FILE_BYTES` is 2 MB, which
  the module's own header comment estimates as "roughly a very chatty
  three-hour session at `info`" — confirm that estimate against a REAL
  90-minute service's log volume (does it rotate zero times, once, or more?),
  and that the rotated files (`sundayrec.1.log` … `.4.log`) are intact and in
  the right order afterwards.
- **A Windows pass on the rotation rename path.** `logfile.rs`'s `rotate()`
  explicitly closes the file handle before renaming — "Windows will not
  rename an open file" — but that line has never run on a real Windows box.
  Force several rotations there (a local build with a lowered
  `MAX_FILE_BYTES`, or just log enough at `debug`) and confirm no rotation is
  skipped, no file is left open/locked, and no rotated file is lost.

---

## Summary — what only Richard can provide

The code is feature-complete and gate-green; everything below needs an account,
a key, a signing identity, or a physical rig that the headless gate cannot have.
None of it blocks the default build or the gate.

### A real recording/streaming rig (HARDWARE-UNVERIFIED)

- **Record** (smoke §3–§6): a Mac/Windows box with a real mic + camera; prove
  the 30 s capture → history row → reveal-in-folder path, and the OS mic/camera
  permission prompts. Reconnect/split/preroll/two-process-fallback paths are
  wired but unproven on a device.
- **The classic ffmpeg pre-roll hatch** (`classicFfmpegPreroll`, no UI): R2
  kept this field ON PURPOSE — it is the only fallback to the legacy rolling
  ffmpeg pre-roll engine, and the native cpal buffer is still unproven on the
  rig. Once a real Sunday has proven the native buffer, the owner decides
  whether the hatch AND the classic engine (`recorder/preroll.rs`
  `ClassicPrerollEngine`) go — a live-path decision, not a settings sweep.
- **OS wake-timers** (smoke §11): a real box for the `pmset` admin prompt, the
  Windows `SetWaitableTimer` resume, and a true sleep/wake cycle. The argument
  shaping, quoting and escalation ladders are unit-tested; the resume is not and
  cannot be.
- **Observability** (no feature flag, smoke §13): live-exercise
  `SUNDAYREC_TEST_PANIC` end to end, run the capture probe against the Qu-5 and
  the video probe with the camera held by another app, watch log rotation
  survive a real 90-minute service, and confirm the Windows rotation-rename
  path on a real Windows box.

### Keys & secrets

- **SMTP credentials** (`--features email`, SMTP path): host/port/user +
  app-password. The password is stored in the OS keychain, never the settings
  bag; the host/port/user now have a UI (R7).
- **Anthropic API key**: NOT consumed by SundayRec — the AI sermon companion
  (the one seam that read it, from the keychain slot `companion.llm_api_key`)
  left in R2. A key stored there by an earlier build is left alone, like the
  other retired keychain slots; delete it by hand if wanted.

### Signing, notarization & auto-update

- **Apple Developer ID signing — ✅ DONE** (macOS release): the Developer ID
  Application cert is set as `MAC_CERTS` / `MAC_CERTS_PASSWORD` (mapped in
  `release.yml:143-145`); releases are signed since ~2026-07-31.
- **Notarization — ⏸ the remaining blocker:** accept the updated Apple Program
  License Agreement on developer.apple.com (notary returns 403 until then),
  then uncomment the `notarytool` env lines in `release.yml:146-155` (see
  item 3 above).
- **Windows code-signing cert** (Windows release): for a non-SmartScreen-warned
  installer.
- **Updater keypair — ✅ DONE, live since v0.4.x** (`--features updater`, R7):
  `~/.tauri/sundayrec_updater.key` (private, backed up) + the public key in
  `tauri.conf.json` `plugins.updater` + the `TAURI_SIGNING_PRIVATE_KEY` CI
  secret + `uploadUpdaterJson: true` — feed verified in prod. See the R7
  section above and docs/DISTRIBUTION.md "Auto-update signing".
- What remains here is **account work only** (the Apple PLA + optionally a
  Windows cert), NOT code — the release pipeline consumes the credentials the
  moment they're provided.

## Settings-sync + IPC-seam audit (natt 2026-06-05)

Etter wake-from-sleep-funnet (merget i PR #2) gjorde jeg en systematisk audit av
(a) hvilke `Settings`-felt backend-konsumentene faktisk leser vs. hva
`syncBackendRecordingSettings` (api-shim → `settings_save`) sender, og (b) hele
`call()`/`invoke()`-seamen i `legacy/renderer/api-shim.ts` mot Rust-signaturene.
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
`e2e/integrations.spec.ts`); se `docs/COMMAND_AUDIT_2026-08.md` §4.2, som nå
er merket løst. HTTP-sidene forblir nettverks-uverifiserte til riggtest.
**(R1 «Frivilligen først» 2026-08-23: hele avsnittet over er historikk —
cloud, webhook og integrasjonene er FJERNET; bare e-post-stien består.)**
