# Trenger Richard — kontoer, beslutninger og rigg

Det bare eieren kan gjøre: kontoer, nøkler og signering, beslutninger, og
bevis på ekte maskinvare. Koden er ferdig og grønn i `npm run check` for alt
her. Ingen av punktene stopper standardbygget.

- **Oversikten over ALT som gjenstår** (kode, eier, rigg) er
  [`docs/PLAN.md`](PLAN.md). Denne sida har detaljene for eier-punktene.
- **Hvordan riggpunktene bevises** i én økt: [`docs/RIG-DAY.md`](RIG-DAY.md).
- Seksjoner for funksjoner som er fjernet (e-post-reléet unntatt, se PU-1),
  og revisjoner som er ferdige, ligger ordrett i
  [`docs/archive/NEEDS-RICHARD-historikk.md`](archive/NEEDS-RICHARD-historikk.md).

Tre cargo-features er i `default`: `editor`, `tray` og `updater`. Planlegging
og vekking er alltid med.

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
   `APPLE_CERTIFICATE_PASSWORD` at the `[notarization]` marker in
   `release.yml`'s env (`APPLE_SIGNING_IDENTITY` —
   `Developer ID Application: … (784GN847G4)` — is hardcoded there). Published
   releases are **signed** since ~07-31. See DISTRIBUTION.md "macOS code
   signing".

3. **Notarization — the real remaining blocker: the Apple Program License
   Agreement.** Apple's notary service returns **403 "A required agreement is
   missing or has expired"** until the updated PLA is accepted on
   developer.apple.com (team `784GN847G4`). Notarization is therefore
   **deliberately disabled** — since F1-D1 this is a repo **variable**,
   `NOTARIZE_MAC` (Settings → Secrets and variables → Actions → Variables),
   not three commented-out lines someone has to remember to uncomment. Off by
   default, it skips the `[notarize-switch]`-marked step in `release.yml` that
   would otherwise export `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`, so every
   build is Developer ID-signed but not notarized. Re-enable by accepting the
   PLA and setting the variable to `true` — no commit required. See
   RELEASE-CHECKLIST.md §2a for the full mechanism and why it has to be a
   separate step rather than a line in the build step's env.

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

## ~~PU-1 — Email alerts (`email`)~~ — **FJERNET**

E-postvarslene er fjernet — både SMTP-varsleren (`email`-featuren, `lettre`,
passordet i nøkkelringen) og SundaySuite-reléet (`notify.sundaysuite.app`,
kvitteringen). Oppsettet var for tungvint for en frivillig, og appen sender
ikke lenger noe ut av maskinen for å melde en feil: et systemvarsel på
opptaksmaskinen er kanalen, og ingen innstilling slår det av. En oppdatert
installasjon sletter SMTP-passordet én gang og viser en engangsbeskjed til dem
som hadde e-post slått på (smoke §8).

- **👤 Gjenstår for Richard:** rive relé-rutene i `sunday-telemetry`
  (`notify.sundaysuite.app`) og slette de lagrede adressene der. v0.23.0
  (2026-09-28, stabil og beta) er første versjon uten e-post; rives når
  flåten har oppdatert (telemetri-oversikten viser versjonsfordelingen). De
  åpne relé-PR-ene #205/#206 ble lukket samme dag.

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
- **Tray icon assets — ✅ løst uten egne filer.** Ikonet er appens eget, med en
  statusprikk malt på i kjøretid (`sundayrec_core::tray::with_status_badge`):
  rød = tar opp, gul = noe er galt. Egne ikoner per tilstand (inkl. macOS
  `Template`) er valgfritt pynt, ikke et hull.

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
  prod**: the `latest.json` feed serves published releases and real installs
  update from it (release notes and ring promotion: `docs/RELEASE-CHECKLIST.md`). A dev build still short-circuits the check by
  design.

## «Legg ut» — manuell SoundCloud-flyt (2026-09), API-integrasjon lagt på is

Eksporten fikk «Innhold» (tittel/taler/beskrivelse som tagger, tittelen som
filnavn) og kvitteringen et «Legg ut»-panel: kanalen menigheten velger under
Innstillinger → Avansert (SoundCloud som standard, ellers YouTube, Spotify for
Creators, en egen `https://`-lenke eller ingen) åpnes i nettleseren, og tittel
og beskrivelse står klare til å kopieres. **Appen laster ikke opp noe selv.**

- **Eierbeslutning: ingen SoundCloud-API nå.** SoundCloud tar ikke imot åpen
  registrering (søknadsskjema, krever Artist Pro, manuell godkjenning), og en
  integrasjon ville gjeninnført OAuth, tokenlagring og opplastingskø — det R1
  fjernet med vilje. Gevinsten over dra-og-slipp er liten.
- **Grunnlaget for å ta den opp igjen:** tellerne `editor.publish.soundcloud`
  / `.youtube` / `.spotify` / `.custom` (samtykkestyrt, som alle tellere) sier
  hvor ofte knappen brukes og til hvilken kanal. Viser de reell bruk, er neste
  steg å søke om API-tilgang. Den gamle OAuth/PKCE-loopback-koden fra fase 6
  ligger i git-historikken (PR #139, commit `daadeb7`) som referanse.
- **GUI-UNVERIFIED:** at `open_url` faktisk åpner systemnettleseren på macOS og
  Windows, og om SoundCloud fyller inn tittelen fra filnavnet eller fra
  ID3-`title` — se SMOKE-TEST.md, «Legg ut».

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
- **The Electron-parity list, checked against the code (2026-10-01).** This
  used to be a "deferred to a later editor phase" list, written when the R1
  panel could only export the whole file. Of its four items, two are done (the
  cut UI, export progress + cancel), one is half done and half dropped on
  purpose (the atomic swap is in, the in-place replace is gone), and one has no
  source and is parked (chapters). The one small gap that was left, in the
  atomic swap (below), is closed too (#307).
  - ~~**Cut-region timeline UI.**~~ **DONE.** Drag-to-mark on the waveform
    (`app/editor/canvas-input.ts`, drawn by `WaveformHost.tsx`), a cut list with
    a remove button per region (`EditorPage.tsx`), undo/redo and an unsaved-
    draft sidecar that survives a crash (`app/editor/cuts.ts`). Export sends
    `cutRegions` (`app/editor/export.ts`); `editor::export` hands them to the
    core's `build_keeps` (`src-tauri/src/editor/mod.rs`). `e2e/editor.spec.ts`
    covers the list, the remove button and a draft coming back on reopen.
  - ~~**Export progress events + cancel.**~~ **DONE.** `editor_export` streams
    `time=` progress as `editor://export-progress`, `editor_cancel_export` is a
    real cancel handle, and the 2026-08 progress round put a monotone
    percentage + an ETA on the bar (`export_timeout_ms` is still the tested
    kill-timer).
  - ~~**Atomic swap.**~~ **DONE for exports.** Every export renders into
    `<name>.__editor_tmp.<ext>` beside its destination and is renamed onto its
    collision-free name only once ffmpeg has exited zero (`editor_tmp_path` in
    `crates/sundayrec-core/src/editor.rs`, the rename in step 7 of
    `src-tauri/src/editor/mod.rs`). An aborted render never leaves a
    half-written file under the final name: a drop guard (`TempRender`)
    removes the temp, and the startup sweep (`editor::startup_sweep`, started
    from `src-tauri/src/lib.rs`) reaps what a hard crash leaves behind in the
    save folder and the library's folders. ~~**Gap:** it does not look in a
    hand-picked export folder (`pickExportFolder`) or beside a file opened from
    outside the library, so a power cut mid-export can leave a
    `.__editor_tmp.` file there.~~ **DONE (#307).** The export writes the
    temp's exact path into the app database (table `export_temp`, created at
    runtime — not a migration, so a downgrade to an older stable still starts)
    before ffmpeg starts, and drops the row once the temp has been renamed or
    removed. A row that survives to the next launch is a render that never
    finished, and the startup sweep reaps the file it names wherever that is
    (`src-tauri/src/editor/export_journal.rs`). It deletes only a regular file
    whose path is exactly one `editor_tmp_path` could have built — never a
    symlink, a directory or anything else a row might name. Neither the
    journal nor the old folder scan deletes a file with the name of the temp
    an export is rendering into at that moment (before, the folder scan could
    delete a live render in the save folder and fail that export). A row whose
    folder is not there (an unplugged USB stick) waits up to 30 days for a
    launch that can see it. The folder scan stays, for crashes from before
    the journal. One limit: the row is as durable as the database's other
    writes (WAL, `synchronous=NORMAL`), so a power cut seconds after the
    export started can still lose it — and then that one temp is litter, as
    before.
  - **Replace-mode (overwrite the original) — dropped on purpose.** The Tauri
    editor never overwrites a recording: every export is a NEW file (the
    module header of `sundayrec-core::editor` says so). The Rust port of the
    Electron `saveEdited`/`safeReplaceFile` layer (`resolve_save_ext` and
    friends, with its FORCE_WAV refusal and atomic-replace plan) was removed in
    #70 once the audit found no callers outside its own tests, so there is
    nothing left to port. The delivered name is
    `export_stem`: `<source>_redigert` when the export has no title,
    `<YYYY-MM-DD> <title>` when it has one (just `<title>` when the date is
    unknown).
  - **Chapter metadata on export — no source; moved to "Ikke planlagt" in
    [`PLAN.md`](PLAN.md).** The core still builds the `;FFMETADATA1` chapter
    sidecar (`ffmetadata`, `metadata_args`, kept + tested), but since v0.15
    nothing produces chapters — the transcript-driven detector left with the
    content cluster, and `chapters` is gone from `EditorExportRequest` — so
    the export hands the core an empty list (`chapters: Vec::new()` in
    `editor::export`). A future chapter source only has to fill that list.

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

### A real recording rig (HARDWARE-UNVERIFIED)

This list is WHAT is still unverified. [`docs/RIG-DAY.md`](RIG-DAY.md) is HOW
to verify it in one sitting — a checklist that walks the mixer-pull, the
Windows camera/video restart, the `kill -9` mid-slot recovery, the WAL check
against a real database, the wake test, an ASIO subset, and the #111
gain-listening pass in a deliberate order, so the day doesn't turn into
re-discovering these bullets one at a time.

- **Record** (smoke §3–§6): a Mac/Windows box with a real mic + camera; prove
  the 30 s capture → history row → reveal-in-folder path, and the OS mic/camera
  permission prompts. Reconnect/split/preroll/two-process-fallback paths are
  wired but unproven on a device.
- **F2's Windows-only fixes** (`docs/RIG-DAY.md` (w1)/(w2)/(w3)/(w5)/(w6)/(w14)):
  six fixes touched real Windows-only code paths and are unproven beyond a
  cross-compiled `cargo check`/`clippy` and CI's `windows-check` lane (which
  since #231 also runs `cargo test --workspace` on a real Windows runner, not
  just check+clippy) — the auto-updater no longer killing its own installer
  via the ffmpeg job-object (#243), the update button being refused outright
  while a recording is live (#243), no console window opening behind any of
  22 process-spawn sites (#237), a Windows video recording surviving a crash
  via an MKV capture + recovery manifest (#246), the machine staying awake
  between a scheduled wake and the recording actually starting via a
  `SetThreadExecutionState` block (#252), and recording start no longer
  sweeping every installed ASIO driver when a plain WASAPI device is
  selected (#253). A/V sync on the Windows video path remains unproven
  regardless (unchanged by #246). A seventh, narrower gap: four cpal/WASAPI
  unit tests (`audio::vu`, `native_capture::segment` ×2,
  `native_capture::preroll`) are
  `#[cfg_attr(windows, ignore = "F2-W7: … — see PR #231")]` because the CI
  runner's image crashes (`STATUS_ACCESS_VIOLATION`) the moment they open a
  real audio stream — they need a Windows box with a working audio service
  and a microphone to even run, let alone pass.
- **The same keep-awake block, macOS side** (`docs/RIG-DAY.md` tillegg —
  #252): the Electron port carried the _decision_ to keep the machine awake
  (`wake.rs`'s `should_block`) but never the _action_ — no
  `IOPMAssertionCreateWithName`/`SetThreadExecutionState` call existed on
  either platform until #252. Unproven on real hardware: whether a machine
  that actually fell asleep, woken by a timer, stays up the ~10 minutes
  until a scheduled recording starts (macOS via `IOPMAssertionCreateWithName`
  ×2, Windows via `SetThreadExecutionState`), and whether `PreventSystemSleep`
  is honored on battery the same way it is on AC power.
- **The wake test no longer endangers the real schedule, unproven on
  hardware** (`docs/RIG-DAY.md` (e)/(e, fortsettelse)/(tillegg — Windows)):
  F2-W3 (#235) found that "Test wake in 2 min" used to erase Sunday's real
  wake (same `pmset` owner / the same `SetWaitableTimer` clear on Windows) —
  the fix gives the test its own owner (macOS: `SundayRec-test`) and its own
  `TimerSlot` (Windows), but that separation itself has not been exercised on
  a real box yet, and whether `pmset schedule cancelall SundayRec` actually
  filters by owner (rather than deleting everything, or failing silently) is
  still an open question only a real Mac can answer.
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

- ~~**SMTP credentials**~~ — gone with e-mail alerts (PU-1). v0.23.0/v0.24.0
  deleted the stored password once on upgrade; from v0.25.0 the app has no
  keychain access at all (`keyring` removed).
- **Anthropic API key**: NOT consumed by SundayRec — the AI sermon companion
  (the one seam that read it, from the keychain slot `companion.llm_api_key`)
  left in R2. A key stored there by an earlier build is left alone, like the
  other retired keychain slots; delete it by hand if wanted.

### Signing, notarization & auto-update

- **Apple Developer ID signing — ✅ DONE** (macOS release): the Developer ID
  Application cert is set as `MAC_CERTS` / `MAC_CERTS_PASSWORD` (mapped at the
  `[notarization]` marker in `release.yml`); releases are signed since
  ~2026-07-31.
- **Notarization — ⏸ the remaining blocker:** accept the updated Apple Program
  License Agreement on developer.apple.com (notary returns 403 until then),
  then set the repo variable `NOTARIZE_MAC` to `true` (see item 3 above and
  the `[notarize-switch]` marker in `release.yml` — no commit required, unlike
  before F1-D1).
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

## Eierbeslutninger fra F2

Funn fra F2s Fable-granskinger (rigg + lydkjede + Windows) som ikke er kodet
— hver av dem trenger et eiervalg før noen skriver en fiks. Ingen av disse
blokkerer noe i dag; de ligger her så de ikke går tapt mellom rundene.

- **MSI på stable trigger UAC uten admin (F-W7).** `.msi`-installereren
  bruker Windows Installers standard `perMachine`-omfang, som ber om
  administrator-elevering selv når brukeren ikke har administratorrettigheter
  — en frivillig på en låst kirke-PC kan sitte fast på nettopp det spørsmålet.
  Betaer sender allerede kun NSIS (`docs/RELEASE-CHECKLIST.md` §5a — MSI kan
  ikke uttrykke et beta-versjonsnummer), og NSIS' standard er `currentUser`
  (ingen UAC). Anbefaling: gjør stable NSIS-only også, og fjern `.msi` fra
  release-matrisen. **Ingen kode er skrevet** — `src-tauri/tauri.conf.json`s
  `bundle.windows` har ingen `nsis`/`wix`-overstyring i dag, så dette er
  fortsatt bare et funn.
- **Database-mappa bør flytte fra Roaming til Local AppData (F-W10).**
  `sundayrec.sqlite` (og resten av appdataen) ligger i dag under Windows'
  Roaming-profil, som synkroniserer over nettverket på domenepåloggede
  maskiner — en stor, stadig voksende SQLite-fil med WAL-sidefiler er
  nøyaktig den typen data Roaming-profiler håndterer dårlig. tmp-mappa og
  loggeren flyttes allerede til Local AppData i en annen F2-runde (F-W6); DB-
  flyttingen er IKKE del av den og trenger sitt eget owner-OK (dataflytting
  på en allerede installert base er ikke en ren tilleggsendring).
- **`webviewInstallMode: embedBootstrapper` for den frakoblede kirke-PC-en.**
  Standard WebView2-installasjon laster en liten bootstrapper som henter
  resten fra nettet ved førstegangsbehov — en kirke-PC uten internett (eller
  bak en restriktiv brannmur) kan sitte uten en fungerende WebView2-runtime.
  `embedBootstrapper` bygger hele runtimen inn i installereren (større
  installer, ingen nettverksavhengighet ved installasjon). Ikke satt i
  `tauri.conf.json` i dag.
- **`-realtime 1` for VideoToolbox-enkoderen (C, mening) — ✅ AVGJORT
  2026-10-04 og gjennomført.** Eier: robusthet foran kvalitet. **Opptaket**
  skal ha `-realtime 1` på VideoToolbox-enkoderen, slik at den ikke sakker
  akterut under et langt opptak der CPU-en er presset; en **eksport** i
  redigereren er ikke sanntid og skal ikke ha flagget. Ved gjennomføringen
  viste det seg at opptaket allerede hadde flagget (`push_video_encoder_args`
  i `crates/sundayrec-core/src/capture.rs`, rett etter `-b:v`, både H.264 og
  HEVC) — det som manglet i dette punktet var at **eksporten også hadde det**
  (`videotoolbox_codec_args` i `editor.rs`). Flagget er nå fjernet derfra, og
  goldentestene viser `-realtime 1` på opptak, ingenting på eksport,
  ingenting på Windows/Linux eller med maskinvareenkoder av, og en
  lyd-only-argv byte-lik som før. Flagget finnes i den medfølgende ffmpeg
  8.1.2 (`-h encoder=h264_videotoolbox`). Gjenstår: ett langt videoopptak på
  Mac under CPU-last på riggdagen (`docs/RIG-DAY.md`, Mac-boksen).
- **`_redigert`-eksporter finnes ikke i biblioteket — ✅ AVGJORT
  2026-10-04: NEI.** En eksportert `*_redigert.<format>`-fil (Redigering →
  Eksportering) skrives til disk, men får ingen egen rad i
  historikken/biblioteket — den er kun synlig som en fil i Utforsker/Finder.
  Eier: de forblir bare filer på disk, ingen egen biblioteksrad, ingen
  kobling til originalopptaket. Ingen kode.
- **En «Kirke»-mastringsprofil (C, mening) — ⏸ AVGJORT 2026-10-04: VENT.**
  Lydkjede-gjennomgangen foreslo et fjerde mastringspreset (ved siden av
  `speech-clear`/`speech-natural`/`speech-punchy`/`music-speech`) tunet
  spesifikt for kirkerom — mer forsiktig kompresjon, en høyere gate-terskel
  for romklang. Eier: vent til det er gjort en lyttetest i et kirkerom; først
  da finnes tallverdiene å bygge profilen på. Ikke innført.
- **Automatisk monolevering ved høyt korrelerte L/R-kanaler (C, mening) —
  ⏸ AVGJORT 2026-10-04: VENT.** Forslag: når eksportens L/R-kanaler måler
  ≥ 0,98 korrelert (praktisk talt samme signal på begge, typisk en enkelt
  mikrofon matet inn i begge kanaler), lever automatisk som mono i stedet for
  en stereofil med to identiske kanaler — halvert filstørrelse, ingen hørbar
  forskjell. Eier: vent til lyttetest; terskelen og valget mellom automatikk
  og et forslag brukeren bekrefter avgjøres da. Ikke innført.
- **`{when}`-limingen i `app.status.next`/`app.banner.missedTitle` — ✅
  AVGJORT 2026-10-04: LA VÆRE, god nok.** Begge nøklene limer en formatert
  dato/klokke rett inn i en frase (`"Neste opptak {when}"`,
  `"{when} ble ikke tatt opp"`) — et mønster som fungerer på norsk, og som
  eier mener er godt nok på de andre språkene også. Ingen omskriving av
  språkfilene; én delt mal beholdes. Ingen kode.
- **Resten av språkrundens kildefunn.** `scratchpad/i18n/source-text-findings.md`
  punkt 4, 8–12, 14, 15, 17, 19–21 og 24 er merket eiervalg/rest av den
  runden selv — de er IKKE gjengitt her, fordi kildefila ikke var
  tilgjengelig i denne økten (se sluttmeldingen på PR-en som førte inn denne
  seksjonen). Fylles inn punkt for punkt når fila er lesbar igjen.
  **Eier 2026-10-04:** kan ikke avgjøres før kildefila finnes — punktene
  ligger urørt til den er lesbar igjen.
