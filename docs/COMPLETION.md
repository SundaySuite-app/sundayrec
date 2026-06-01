# SundayRec (Tauri 2) — Completion summary

This document records the **finished state** of the Electron → Tauri 2 rebuild of
SundayRec and draws the exact boundary between what is **code-complete + gate-green**
and what **needs Richard** (a rig, an account, a key, a signing identity). It is the
companion to `SMOKE-TEST.md` (the hardware-in-the-loop checklist) and
`NEEDS-RICHARD.md` (the per-feature glue list).

## TL;DR

- **Architecture:** all recorder/editor/streaming/transcribe/publish/update/etc.
  _behaviour_ is distilled into the pure, GUI-free, fs/network-free
  `sundayrec-core` crate and exhaustively unit-tested. `src-tauri` is a thin
  command/event/IO shell on top; every impure path that touches a device, the
  network, a key, or a GUI is annotated `// HARDWARE/NETWORK/GUI/INFRA-UNVERIFIED`.
- **Default build stays lean:** every native or risky dependency lives behind a
  **default-off** cargo feature (`email`, `tray`, `publish`, `editor`, `whisper`,
  `streaming`, `ndi`, `bridge`, `updater`). The shipping build and the CI gate
  carry none of them; a feature-disabled command returns a clear `feature_disabled`
  error and the matching panel shows a calm "not built into this build" hint.
- **Gate-green:** `npm run check` (eslint + tsc + vitest + clippy `-D warnings` +
  `cargo test --workspace`) passes — **918 Rust tests** (687 core + 231 src-tauri)
  - **296 vitest**. Each default-off feature also compiles in isolation
    (`cargo build -p sundayrec --features <flag>`), the `whisper` C++ build being the
    single by-inspection exception.

## Feature inventory (what's built)

| Area                         | Core (pure, tested)                                                        | Shell seam              | Feature                | Tier                              |
| ---------------------------- | -------------------------------------------------------------------------- | ----------------------- | ---------------------- | --------------------------------- |
| Recorder core                | `recorder`,`reconnect`,`silence`,`preroll`,`two_process`,`capture`         | `recorder/*`            | (none)                 | P2b — HARDWARE-UNVERIFIED         |
| Devices / VU / preview       | `device_enum`,`device_match`,`audio`,`mjpeg`                               | `audio/*`,`media/*`     | (none)                 | P2b — HARDWARE-UNVERIFIED         |
| Settings                     | `settings` (full Electron parity incl. R7 church/notify/email/intro-outro) | `settings/*`            | (none)                 | P2a + persist                     |
| Schedule / wake              | `schedule`,`wake`                                                          | `scheduler/*`,`wake/*`  | (none)                 | P2b — HARDWARE-UNVERIFIED         |
| History + dialogs            | (sqlx store)                                                               | `db/*`                  | (none)                 | done                              |
| Diagnostics / preflight      | `diagnostics`,`preflight`                                                  | `diagnostics/*`         | (none)                 | done                              |
| Editor                       | `editor`,`mastering`,`audio_analysis`                                      | `editor/*`              | `editor`               | P2b — HARDWARE-UNVERIFIED         |
| Transcription                | `whisper`                                                                  | `whisper/*`             | `whisper`              | P2b — HARDWARE-UNVERIFIED         |
| Review / prep / Stage import | `prep`,`review_queue`,`integrations::stage`                                | `commands/review`       | (none)                 | P2a + persist                     |
| Cloud backup                 | `cloud`                                                                    | `cloud/*`               | (none, OAuth deferred) | P2b — NETWORK-UNVERIFIED          |
| Email alerts                 | `email`                                                                    | `email/*`               | `email`                | P2b — NETWORK-UNVERIFIED          |
| Live streaming (RTMP)        | `streaming`,`overlay`                                                      | `streaming/*`           | `streaming`            | P2b — NETWORK/HARDWARE-UNVERIFIED |
| NDI receiver                 | `ndi`                                                                      | `ndi/*` (STUB)          | `ndi`                  | P2c — SDK not bundled             |
| Podcast RSS publish          | `feed`                                                                     | `publish/*`             | `publish`              | P2b — NETWORK-UNVERIFIED          |
| Live cue bridge              | `integrations::live_bridge`                                                | `bridge_live/*`         | `bridge`               | P2b — INFRA-UNVERIFIED            |
| Suite hand-offs              | `link`                                                                     | `commands/bridge`       | (none)                 | done                              |
| Tray + deep links            | `tray`,`link`                                                              | `tray/*` (installed R7) | `tray`                 | P2b — GUI-UNVERIFIED              |
| Auto-update                  | `update`                                                                   | `update/*`              | `updater`              | P2b — NETWORK/GUI-UNVERIFIED      |

The renderer surfaces every area behind the `<details>` disclosure pattern in
`src/App.tsx` (until the Phase-8 shell/nav lands), each panel following the same
TanStack-Query + `invoke` + `react-i18next` + ts-rs-bindings idiom, with a vitest
suite that mocks `invoke` and asserts render + IPC calls.

## R7 additions (this phase)

- **Settings completeness:** the Electron `store.ts` fields that were deferred —
  `churchName`/`responsiblePerson`, `notifyStart`/`notifyStop`,
  `emailOnError`/`emailAddress`/`emailSmtp`/`emailSmtpPort`/`emailSmtpUser`,
  `editorIntroPath`/`editorOutroPath` — are now in the typed `sundayrec-core::settings`
  model with defaults + validation (port clamped 1..=65535) and a UI in **Generelt**.
  The SMTP **password** stays in the OS keychain, never the settings bag.
- **Auto-update:** `sundayrec-core::update` (status phases, dev-check guard,
  percent math, semver `is_newer`) + the `update` seam behind the default-off
  `updater` feature + the **Oppdateringer** panel ("Se etter oppdateringer / Last
  ned / Start på nytt og installer"). NETWORK/GUI-UNVERIFIED — needs a signed feed.
- **Tray installed:** the `tray` feature now actually installs the menubar icon +
  menu in `setup()`, wires start/stop/show to commands (Stop → `RecorderEngine::stop()`
  directly), and registers the `sundayrec://` deep-link handler.
- **Editor backend parity (P1):** closed the depth gap vs the Electron editor/
  master backend. New `sundayrec-core` decisions (all tested): the three sidecar
  paths (`.meta`/`.cuts-draft`/`.transcript`) with the `..`-escape guard, the
  400 MB inline-vs-stream guard, the `__editor_tmp`/`__editor_bak` cleanup
  predicate + dir de-dup, the POSIX/Windows atomic safe-replace plan, the
  single-pass mastering-preview argv, and a pure `JobRegistry` state machine
  (register/cancel/complete). Nine new commands wire these: sidecar read/write/
  delete + stream probe + inline read-guard + temp-file sweep all compile and
  run in the **default build** (fs, not ffmpeg — gate-tested via tempdir
  round-trips), and the full mastering flow (`master_preview`/`master_apply`
  with `editor-master-progress` events/`master_cancel`) sits behind the
  default-off `editor` feature (HARDWARE-UNVERIFIED). The panel gained
  cuts-draft reopen-ability (restore banner + autosave + delete-on-export) and a
  mastering A/B preview. (Still deferred to a later pass: the destructive
  in-place `saveEdited`/video-save handlers — the non-destructive export already
  covers the audio + mp4 render path.)
- **i18n:** the `update.*` catalog (Electron-ported) gained the two new R7 keys in
  all 7 locales; every other new R-phase string follows the established
  inline-`t(key, "Norsk fallback")` idiom (the panels work without catalog entries).

## P6 additions (frontend-tests + i18n parity + transcript/history search)

- **i18n PARITY (canonical):** the editor / streaming / transcribe / email /
  integrations / review / publish / update / home / onboarding keys added across
  the R-feature phases existed only in `no.json` (they worked at runtime via the
  inline `t(key, "fallback")` idiom). They are now **translated into
  en/sv/da/de/fr/pl** — English is the canonical fallback — so every shipped
  catalog exposes the **same 979-key leaf set**, plus the new P6 `search.*` keys.
  A build-failing guard (`src/i18n/parity.test.ts`) asserts all 7 catalogs share
  the exact flattened key set (no missing, no extra); drift now fails the gate
  instead of silently degrading some languages to raw key strings.
- **Transcript search (pure):** `src/features/search/searchIndex.ts` mirrors the
  Electron `search-page.ts` contract as a side-effect-free module — build an
  in-memory index over transcript sidecars (newest-first), capped
  case-insensitive substring scan, structured before/match/after highlight
  context, per-recording grouping in recency order, aggregate stats. 13 unit
  tests. The IPC sidecar-load + render is the only GUI-deferred part.
- **History depth (pure + UI):** `src/features/history/historyFilter.ts` mirrors
  the Electron `home.ts` `filterAndRenderHistory` + `updateHistoryStats` +
  audio/video pairing, ported to the Tauri `RecordingRow` shape (full-text over
  filename/date/note, pair-by-`started_at`+video-tag, count/duration/last stats).
  15 unit tests + a live search box & stats line wired into `HistoryPanel`
  (+4 panel tests).
- **Thin-panel coverage deepened:** DevicePicker (+enum-error / multi-sample-rate
  stereo / dshow-index-null video fallback), SchedulePage (+slot-delete /
  late-start edit / special-overlap), VuMeter (+positive-clamp / sub-floor-clamp
  / clip-warn-safe colour thresholds).

## Electron-parity matrix (P6)

How the Tauri rebuild lines up against the Electron renderer pages
(`src/renderer/pages/*`) and main modules (`src/main/*`). **Matches** = behaviour

- depth mirrored and gate-tested; **GUI-unverified** = handlers/data/IPC are
  tested but pixel paint / native shell is not; **rig-deferred** = needs a real
  device/account/key (see NEEDS-RICHARD.md + SMOKE-TEST.md).

| Electron surface                              | Tauri parity                                              | Status                 |
| --------------------------------------------- | --------------------------------------------------------- | ---------------------- |
| `home.ts` countdown/hero/review-card          | `features/home/HomePage` (fmtCountdown/Next/Bytes tested) | Matches                |
| `home.ts` recent-history + stats              | `historyFilter` + `HistoryPanel` stats line               | Matches                |
| `home.ts` `filterAndRenderHistory`            | `filterHistory` (filename/date/note)                      | Matches                |
| `home.ts` audio+video pairing                 | `pairAudioVideo` (started_at + video-tag)                 | Matches                |
| `home.ts` silent preflight banner             | `DiagnosticsPanel` (manual run)                           | GUI-unverified         |
| `home.ts` video preview (`startVideoPreview`) | `DevicePicker` MJPEG preview                              | rig-deferred [HW]      |
| `search-page.ts` transcript index/search      | `features/search/searchIndex` (pure)                      | Matches (load: GUI)    |
| `search-page.ts` thumbnail attach             | (not ported — decorative)                                 | GUI-unverified         |
| `schedule-page.ts` weekly/special edit        | `SchedulePage` (add/delete/toggle/late-start/overlap)     | Matches                |
| `schedule-page.ts` next/upcoming list         | `SchedulePage` reads `scheduler_status`                   | Matches                |
| `calendar-page.ts` wake-before badge          | `home`/`wake` (wakesBefore key + WakePanel)               | rig-deferred [HW]      |
| `home-vu.ts` live VU                          | `VuMeter` (clamp + threshold colour tested)               | rig-deferred [HW]      |
| `audio-page.ts` device enumerate              | `DevicePicker` (cpal mic + ffmpeg cam, dshow fallback)    | rig-deferred [HW]      |
| `files-page.ts` format/preroll/podcast        | `SettingsPage` + `PublishPanel`                           | Matches / NET-def      |
| `editor-page.ts` waveform/cuts/master         | `EditorPanel` + `editing` machine                         | rig-deferred [HW]      |
| `editor-transcript.ts` transcribe             | `TranscribePanel`                                         | rig-deferred [HW]      |
| `integrations-page.ts` peers/bridge           | `IntegrationsPanel` + `SuiteHandoffPanel`                 | Matches / INFRA-def    |
| `publish-page.ts` feed preview/generate       | `PublishPanel` (feed XML tested)                          | Matches / NET-def      |
| `review-queue-home.ts` queue card             | `ReviewPanel` + home review card                          | Matches                |
| `live-page.ts` / `live-overlays.ts` RTMP      | `StreamingPanel` (argv tested)                            | rig-deferred [HW/NET]  |
| `onboarding.ts` first-run wizard              | `OnboardingFlow`                                          | GUI-unverified         |
| email/SMTP test                               | `EmailSettingsPanel`                                      | rig-deferred [NET]     |
| auto-update toast/flow                        | `UpdatePanel`                                             | rig-deferred [NET/GUI] |

i18n now spans **all 7 catalogs identically** — the parity guard is the gate-level
proof, replacing the earlier "panels work without catalog entries" footnote.

## The code-complete vs needs-rig boundary

**Code-complete + verified in the gate (no rig needed):**

- Every `sundayrec-core` decision (the entire 687-test core).
- Every command's IPC surface + the panel data-flow (the 296 vitest), including
  the pure transcript search index, history filter/pairing/stats, and the i18n
  7-catalog parity guard.
- Every default-off feature _compiles_ (build + clippy `-D warnings`), so the
  feature-gated seams are wired correctly even though their effects are unproven.
- The full settings round-trip, history persistence, diagnostics report, schedule
  decisions, prep/review queue, suite hand-off URL building, RSS XML shaping,
  transcript export rendering, overlay/stream/ndi argv building — all pure + tested.

**Needs Richard (a rig / account / key / signing identity — see NEEDS-RICHARD.md):**

- A real Mac/Windows box with mic + camera to prove the recording, preview,
  schedule-launch, wake-timer, editor, streaming and whisper _effects_
  (HARDWARE-UNVERIFIED). The migration's "validated on a real rig" exit is reached
  here, not in the gate.
- Network + a Google OAuth Desktop client for cloud connect/upload + the
  cloud-Gmail email path (NETWORK-UNVERIFIED).
- SMTP credentials for the SMTP email path; the NDI SDK + a LAN source for NDI; a
  live Supabase project + SundayStage for the live bridge.
- Apple Developer ID + notarization, a Windows signing cert, and an updater keypair
  - `plugins.updater` config for a signed, auto-updating release.

None of the needs-rig items block the default build or the gate; the pipeline is
wired to consume each one the moment it's provided.

## Redesign (src/design) + new wiring

The Electron-style `<details>` disclosure shell described above (and its early
flat-nav successor) has been **replaced** by a macOS-native redesign handed off
from Claude Design and rebuilt against the `sr-*` design system
(`src/design/{tokens.css,atoms.tsx,Icon.tsx,hooks.ts}`). `src/App.tsx` now mounts
`MainLayout` and routes every everyday view to a redesigned `src/design` screen;
the remaining feature panels stay reachable via the ⌘K palette / settings hub.

### The new shell + 7 screens

`MainLayout` is the new macOS sidebar shell (nav + status footer + ⌘K palette).
The seven redesigned screens (`src/design/screens/*.tsx`, ~7.8k LOC) are:

| Screen                               | View tag    | Drives                                                                                                                                         |
| ------------------------------------ | ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| **Hjem** (`HomeScreen`)              | `home`      | `scheduler_status`, `settings_get`/`settings_save`, recent history                                                                             |
| **Tidsplan** (`ScheduleScreen`)      | `schedule`  | `settings_get`/`save`, `scheduler_status`/`reschedule`, `wake_capabilities`, `liturgical_month`                                                |
| **Direkte** (`LiveScreen`)           | `streaming` | `stream_status`/`start`/`stop`, `stream_set_key`/`delete_key`                                                                                  |
| **Rediger** (`EditScreen`)           | `editor`    | `recordings_list`, `editor_load_recording`/`peaks`/`segments`, `editor_mastering_analyze`/`master_preview`/`export`, whisper transcribe/export |
| **Søk** (`SearchScreen`)             | `search`    | `recordings_list`, **`transcripts_list`**, `open_in_sundayedit`                                                                                |
| **Innstillinger** (`SettingsScreen`) | `settings`  | `settings_get`/`save`, `plan_recording_opts` (preview save path)                                                                               |
| **Opptaksmodus** (`RecordingScreen`) | overlay     | `plan_recording_opts` → `start_recording`/`stop_recording`, the `recording://{started,progress,silence,levels,error,state}` events             |

These are **wired to real IPC** (not mocks): each screen calls the same Tauri
commands/events the removed legacy panels used, via TanStack-Query + `invoke` +
the shared data hooks in `src/design/hooks.ts` (device enum, VU engine, MJPEG
preview, disk probe). NOTE: the header comment in `App.tsx` still calls the
screens "presentational … live data is rewired in a later pass" — that comment is
**stale**; the wiring landed in commits `8dda14d`/`c91959d`/`63f3387`/`0abf6d7`/
`a59f199`/`43f78de`.

### New backend commands + event

- **`transcripts_list`** (`src-tauri/src/commands/db.rs:64`) — lists every
  recording's parsed `<name>.transcript.json` sidecar as `{ basePath, transcript }`
  for the Søk full-text index; reuses the editor sidecar reader + history listing,
  skips un-transcribed/unparseable rows. Read-only, no new dep.
- **`plan_recording_opts`** (`src-tauri/src/commands/recorder.rs:39`) — plans the
  full `RecordingOpts` for a manual "Start opptak nå" from persisted settings
  (same save-folder + liturgical-filename + audio processing as the scheduler), so
  a manually-started recording lands in the right folder/name; the returned
  `output_path` is the real save path shown in Opptaksmodus + previewed in
  Innstillinger.
- **`liturgical_month`** (`src-tauri/src/commands/calendar.rs:29`) — the
  Norwegian feast days ("Kirkehøytider") for a `(year, month)`, over the pure
  `sundayrec_core::church_calendar` computus, so the Tidsplan calendar renders
  feast markers. Pure/sync; 3 unit tests in-file.
- **`recording://levels`** event (`src-tauri/src/recorder/engine.rs:113`,
  `LEVELS_EVENT`) — live per-channel peak audio levels (dBFS) parsed from the
  recorder's OWN ffmpeg `astats` telemetry (no second mic open), driving the L/R
  meters in Opptaksmodus. The parser is the pure, unit-tested
  `sundayrec_core::levels` (207 LOC) → `RecordingLevels` payload.

### New Settings fields

Added to the typed `sundayrec-core::settings` model (with defaults + validation;
`src/design/screens/SettingsScreen.tsx` surfaces them in Innstillinger):
`video_resolution` (default `"720p"`), `video_framerate` (default `30`, validate
clamps 1–120), `output_mode` (default `"combined"`), `keep_separate_audio`
(default false), `av_sync` (default true), `eq_enabled` (default false),
`webhook_url` (default empty), `webhook_on_warning` (default false).

### Other redesign work

- **Full 7-language i18n of the screens** (`61da534`): every screen's UI chrome
  routes through `t()` under dedicated namespaces (`homeScreen`, `scheduleScreen`,
  `liveScreen`, `editScreen`, `searchScreen`, `settingsScreen`, `recordingScreen`)
  — 381 new keys translated into all 7 catalogs (no/en/de/sv/da/fr/pl), every
  catalog now identical at 1360 keys so the strict parity guard
  (`src/i18n/parity.test.ts`) still passes.
- **Editor trim → cut-regions + interactive waveform**: the "Trim — start & slutt"
  fields convert to `editor_export` `cutRegions` (`buildTrimCuts`, `0abf6d7`) so
  export/master honour the trim; the waveform gained zoom/pan/scrub (`a59f199`).
- **Legacy screen components removed** (`9299398`): now that the design screens
  carry interaction tests and `App` routes every view to them, the superseded
  dead cluster was deleted — HomePage, SchedulePage + ScheduleCalendar, SearchPage,
  SettingsPage, EditorPanel + EditorCanvas, StreamingPanel, and the early
  VuMeter/CameraPreview/RecorderPanel spikes (+ their tests). Shared modules they
  relied on (searchIndex, editorGeometry, waveform, per-feature queryKeys) stay,
  still imported by the design screens.

### Test posture (redesign)

- The 7 `src/design` screens each have a **mocked-IPC interaction test**
  (`*.test.tsx`, `d027257`) asserting render + the IPC calls they fire.
- The new Rust parsers/builders are **unit-tested**: `sundayrec_core::levels`
  (astats parse), `church_calendar` (computus), `settings` validation/clamps, and
  `liturgical_month` (3 in-file tests).
- **Hardware/network paths remain rig-unverified** — the actual recording capture,
  VU/preview, streaming, whisper inference, wake-timers and cloud upload still need
  a real device/account/key (see NEEDS-RICHARD.md + SMOKE-TEST.md). The redesign
  changed the renderer + added thin command/event seams; it did **not** flip any
  of those `HARDWARE/NETWORK-UNVERIFIED` annotations to verified.
