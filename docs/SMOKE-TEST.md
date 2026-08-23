# SundayRec — Smoke Test Runbook

A hands-on, hardware-in-the-loop checklist for proving that the Tauri rebuild
actually records. Everything below the line **cannot** be exercised in the
headless CI gate — it needs a real display, microphone, and camera. This doc is
the bridge from "compiles + unit-tests pass" to "validated on a real rig".

> Legend: **[HW]** = HARDWARE-UNVERIFIED in code — never run against a device in
> the gate, only here. **[NET]** = needs network + a Google OAuth client.

---

## 0. Prerequisites

| Tool      | Version            | Check             |
| --------- | ------------------ | ----------------- |
| Node.js   | 20 LTS or newer    | `node --version`  |
| Rust      | stable (1.77+)     | `rustc --version` |
| Xcode CLT | (macOS, for build) | `xcode-select -p` |

ffmpeg/ffprobe are **not** installed system-wide — they are fetched as bundled
sidecars by `scripts/fetch-ffmpeg.mjs` (the `predev`/`pretauri` npm hooks run it
automatically). To fetch them manually:

```bash
npm run ffmpeg         # downloads ffmpeg 8.1.2 → src-tauri/binaries/<name>-<host-triple>
ls src-tauri/binaries  # expect ffmpeg-… and ffprobe-… for your host triple
```

The download is version-pinned and SHA-256-verified twice (archive, then the
unpacked binary — see the script header). A sidecar that is already correct is
left alone, so repeat runs cost a fraction of a second;
`node scripts/fetch-ffmpeg.mjs --force` re-fetches.

The binaries are git-ignored (`.gitignore` → `src-tauri/binaries`) and re-fetched
per machine/platform; the recorder + MJPEG preview resolve them by host triple at
runtime (`SUNDAYREC_TARGET_TRIPLE`).

### macOS privacy permissions (REQUIRED — first-capture blocker)

`src-tauri/Info.plist` ships `NSMicrophoneUsageDescription` +
`NSCameraUsageDescription`. Tauri 2 merges this into the dev app, so the first
mic/camera access triggers the normal macOS consent prompt. **Click Allow.** If
you ever denied it, re-enable under _System Settings → Privacy & Security →
Microphone / Camera → SundayRec_ and relaunch. Without these strings macOS kills
the app at capture time with no error — that is the symptom to watch for.

---

## 1. Pre-gate (headless, do this first)

```bash
npm run check          # prettier + eslint + tsc + rustfmt + clippy + cargo test
cargo build            # debug build of the Tauri binary
npm run build          # tsc + vite frontend build
```

All four must be green before a smoke test is meaningful. As of this runbook the
gate is green: the full Rust test suite (`cargo test --workspace`) + a **vitest**
frontend suite (pure logic like the editor cut-history state machine; grows as
more pure logic is extracted) + clippy `-D warnings`. Every feature also compiles
in isolation — `cargo build -p sundayrec --features <flag>` for
`email`/`tray`/`editor`/`updater` (the
`whisper` C++ build is the one exception, verified by inspection).

⚠️ **Which of these are actually in the shipping build.** `src-tauri/Cargo.toml`
sets `default = ["editor", "whisper", "tray", "updater", "email"]`,
so **all five are ON in a plain `npm run tauri dev` / `cargo build` and in every
release**. The `--features <flag>` lines below them are redundant, not
prerequisites, and a `feature_disabled` response from any of those five is a BUG
to report — not the expected result. Only `asio`/`vad` are genuinely
default-off and need an explicit `--features`. To exercise a disabled path
deliberately, build with `--no-default-features`. (v0.14: `streaming`/`ndi`/
`bridge` were removed with the Direkte page — their old §R3/§R3b sections are
gone from this runbook. R1 «Frivilligen først» 2026-08-23: cloud backup (§7),
podcast RSS + `publish` (§10), the live cue bridge (§10c), the review queue
(§PU-6) and the Sunday-suite integrations (§P2b) followed; §8 e-post is
SMTP-only and §9's deep links are gone.)

---

## 2. Launch [HW]

```bash
npm run tauri dev
```

`predev` fetches ffmpeg if needed; vite serves on the fixed port **1420**
(`strictPort`); Tauri opens the window titled "SundayRec". The header should read
"backend OK" with the version/platform — that proves the Rust ↔ React bridge and
that `setup()` opened the database without panicking.

**Where logs go:** the backend uses `tracing` to **stderr** of the terminal
running `tauri dev`. Bump verbosity with the env filter:

```bash
RUST_LOG=debug npm run tauri dev          # everything
RUST_LOG=sundayrec=debug npm run tauri dev # just our crates
```

Expect at boot: `SundayRec backend ready (db at …/sundayrec.sqlite)` and no
repeated background-task log spam.

**First run:** a fresh install (no `onboardingDone`) boots into the wizard; a
settled install goes straight to Hjem. The wizard's consent step (E3.6) asks
the telemetry question with the «Aldri»-list on display, records the answer —
yes _or_ no — through `telemetry_consent_set`, treats a decline as fully equal
(«Alt er klart!» either way), and cannot trap the operator if the backend
rejects the answer. The renderer half of all of that is pinned in the browser
tier; only the native window/DB boot itself stays a rig observation:

- VERIFIED-BY: e2e/onboarding.spec.ts::first run shows the wizard; a settled install does not
- VERIFIED-BY: e2e/onboarding.spec.ts::the consent step exists, and says what is and is not collected
- VERIFIED-BY: e2e/onboarding.spec.ts::«Ja, del anonymt» grants consent and finishes the wizard
- VERIFIED-BY: e2e/onboarding.spec.ts::«Nei takk» declines — and the app is otherwise identical
- VERIFIED-BY: e2e/onboarding.spec.ts::a backend that cannot record the answer does not trap the operator

---

## 3. Channel grid → live meters move → two-tap L/R [HW]

Since **v0.7.0** the dropdown device picker + "Test lyd" button are gone — the
audio settings surface is a **channel grid** with a live meter per channel.

1. Open **Innstillinger → Lyd**.
   - **Expected:** the channel grid appears — one tile per input channel of the
     device, each with its own **live meter** (driven by the `vu://levels`
     events).
2. Speak / tap the mic (or send signal on a mixer channel).
   - **Expected:** the channels **with signal light up** — their meters move in
     real time. Dead-flat meters on every channel while you speak = the OS
     denied mic access (see §0) or the wrong device is active.
3. Assign channels with **two taps**: first tap sets **L**, second tap sets
   **R**.
   - **Expected:** the choice is saved immediately with a «Lagret ✓»
     confirmation (there is no separate save footer), and the channel status
     shows on the home card.

---

## 4. Camera preview [HW]

The shipped renderer polls `recording_preview_frame` **during recording only** —
there is no idle device-select preview surface. Verify the preview as part of a
video recording:

1. Start a recording with a camera selected (§5 with video).
   - **Expected:** the preview area shows live video (ffmpeg MJPEG → base64
     frames, polled via `recording_preview_frame`) within a second or two of
     the recording starting.
2. No preview + the app still alive = check camera permission (§0). App vanishes
   = permission string missing/denied and the OS killed it.

---

## 5. Record 30 s → stop → history row [HW]

Since **v0.6.0** audio capture is the **native Rust engine** (cpal → ring
buffer → own WAV writer) — ffmpeg is not in the audio capture path. ffmpeg
still captures **video** sessions and serves the `classic_ffmpeg_audio` escape
hatch.

1. Start a recording with mic (+ camera if testing A/V).
   - **Expected:** status flips to recording; with `RUST_LOG=debug` the
     progress you see comes from the **native writer's byte counter** (not
     ffmpeg `size=` lines — those only appear in video /
     `classic_ffmpeg_audio` sessions).
   - The renderer half of the start seam (modal → `plan_recording_opts` +
     `start_recording` → overlay), and the refusal path where the engine says
     no and the operator gets the localized reason with the modal still open:
   - VERIFIED-BY: e2e/recorder.spec.ts::manual start flips the app into the recording overlay
   - VERIFIED-BY: e2e/recorder.spec.ts::a start the engine refuses keeps the modal open and says why
2. Let it run ~30 seconds, talking so the silence-watcher does **not** fire.
3. Stop the recording.
   - **Expected:** a graceful stop — the engine raises the stop flag and does a
     **bounded join** of the capture/writer threads (no process kill). ffmpeg
     only appears at stop if a **delivery encode** (e.g. WAV → FLAC/AAC) runs.
     A **new history row** appears with a plausible **duration (~30 s)** and
     **file size (> 0)**.
   - The renderer half of the stop seam (confirm guard → `stop_recording` once
     → an explicit finalizing overlay that waits for a terminal engine event):
   - VERIFIED-BY: e2e/recorder.spec.ts::stop is guarded by a confirm and then holds a finalizing overlay
4. Confirm the file exists on disk at the path shown.

> [HW] Reconnect/split/preroll fusion paths are wired but unproven on a device
> (preroll still runs via ffmpeg). A basic single-segment 30 s capture is the
> smoke-test target here.

---

## 5b. Recording-health telemetry — prove the stutter/lag fixes [HW]

This is the **verification loop** for the 2026-06 perf/stability work. The app now
records its own health automatically during every recording (Pillar 0).
**Scope note (v0.6.0+):** the ffmpeg-stderr metrics — `drop=`/`dup=` and
xrun/discontinuity lines — apply to **video / ffmpeg sessions only** (video
capture and the `classic_ffmpeg_audio` escape hatch). **Native audio sessions**
instead report `ring_overrun_samples` plus an **exact frame-count cross-check**
(frames written vs ffprobe/wall clock, with a verdict + REC-LOSS alarm). The
key "recording mode lags" signal — how often the live-levels IPC channel was
**full** (`levels_dropped`) — applies to both. Everything persists to
`last-recording.json` + a rolling `recording-telemetry-history.json` and
surfaces in the diagnose report.

**Read the numbers after a normal recording:**

1. Record a service (or ~15 s of speech) normally, then stop.
2. Open **Innstillinger → Lyd → Diagnose** (the audio diagnose), copy the report.
   - **Expected:** a **"Siste opptak (teknisk)"** section with `Dropp`, `xruns`,
     `IPC-overbelastning (tapte nivå-oppdateringer)` and `Avsluttet rent`, plus a
     newest-first **Trend** across recent recordings.
   - The report content (section + SR-CAPTURE-01 rule) and the modal actually
     showing the backend's markdown + audio rows:
   - VERIFIED-BY: crates/sundayrec-core/src/diagnostics.rs::degraded_last_recording_warns_and_renders_section
   - VERIFIED-BY: e2e/system-support.spec.ts::the Diagnose modal shows the audio rows and the full backend report
   - **Healthy target:** `IPC-overbelastning 0` + clean exit, and per session
     type: **native audio** → `ring_overrun_samples 0` + the frame-count
     cross-check verdict exact; **video / ffmpeg** → `Dropp 0`, `xruns 0`.
   - A degraded recording also raises finding **SR-CAPTURE-01** with the counts.

**Prove the telemetry has teeth (it must DETECT a bad recording):**

3. Start a CPU hog (e.g. `yes > /dev/null &` ×4, or a heavy export), record ~15 s,
   stop, re-open Diagnose.
   - **Expected:** the numbers rise — `IPC-overbelastning` and/or `Dropp`/`xruns`
     go up, and SR-CAPTURE-01 appears. If they DON'T move under deliberate stress,
     the instrumentation is wrong — report that.
4. Kill the hog, record again → numbers return toward 0.

**Per-lever check (paste the before/after numbers into the PR / release notes):**

| Lever                                    | What changed                          | Look at                                                | Expected                                 |
| ---------------------------------------- | ------------------------------------- | ------------------------------------------------------ | ---------------------------------------- |
| **B** scrolling-waveform redraw → 30 fps | less main-thread work while recording | `IPC-overbelastning` (`levels_dropped`)                | lower than before, esp. under load       |
| **A** forced sample-rate surfacing       | resampling = a stutter cause          | finding **SR-RATE-01** + the report's `sampleRateMode` | `auto` = no finding; a forced rate flags |
| **C/D/E** (data-gated)                   | buffer/back-pressure/disk             | only pursue if the above numbers stay high             | —                                        |

> [HW] Drop/xrun phrase matching (`selftest::XRUN_PHRASES`) and the Pass/Warn/Fail
> thresholds (`selftest::FAIL_GAP_SEC` …) are conservative defaults — calibrate
> them from the FIRST known-good capture on the Behringer rig (a clean 15 s take's
> numbers are the reference). The parsers/verdict are pure + unit-tested; only the
> real ffmpeg stderr wording + the thresholds need rig confirmation. (The
> stderr-phrase matching only fires in video / `classic_ffmpeg_audio` sessions —
> native sessions are judged by overruns + the frame-count cross-check.)

---

## 6. Add a note → reveal in folder [HW]

1. On the new history row, add a note and save.
   - **Expected:** the note persists (it round-trips through `recording_update_note`
     into SQLite; relaunching the app shows it again).
   - The renderer half (note modal → `recording_update_note` with the text →
     the row wears the note); the SQLite relaunch round-trip stays a rig check:
   - VERIFIED-BY: e2e/history.spec.ts::a note reaches the backend and shows on the row
2. Use "reveal in folder" / open.
   - **Expected:** the OS file manager opens at the recording (via the `opener`
     plugin — capability `opener:allow-open-path` is granted).

---

## 6b. History search + transcript search [HW]

The filter/grouping/stats math and the substring search are pure and
gate-tested (`historyFilter` / `searchIndex`); what a rig confirms is that the
search box wiring and the IPC sidecar-load behave on real data.

1. Record two or three sessions (repeat §5) so History has several rows.
2. In **History**, type into the search box.
   - **Expected:** the list filters live by filename, date, or note text
     (case-insensitive); the stats line ("N opptak · Xt Ym totalt · sist …")
     describes the **filtered view** — the same rows as the table (a deliberate
     departure from the Electron `home.ts` behaviour: `runSearch` renders and
     counts one set, so the two can never disagree). A query that matches
     nothing shows a no-hits message (`search.noHits`), distinct from the
     genuinely-empty state.
   - VERIFIED-BY: e2e/history.spec.ts::the search box filters live, and a miss says so in its own words
3. If you also ran §10b (Whisper), open the transcript search surface and search
   for a word you spoke.
   - **Expected:** hits group by recording, newest-first, each with a ~60-char
     before/match/after context window; clicking a hit seeks the editor to that
     segment's start time. (The index build + sidecar load is the only
     GUI-deferred part — `searchIndex` itself is gate-tested.)

---

## 7. ~~Cloud connect + upload~~ — REMOVED (R1 «Frivilligen først»)

Cloud backup (Drive/Dropbox/OneDrive) left the app 2026-08-23. The file on
disk is the hand-off; the section number stays so cross-references still
resolve.

---

## 8. Email alerts [NET] — `email` (IN DEFAULT)

The error/test mailer is in the **`default` feature set**, so the shipping build
and a plain `npm run tauri dev` both have it and pull the SMTP dep (`lettre`).
The localized templates (7 langs) and the throttle/dedup gate are unit-tested
in `sundayrec-core::email`; the **send** is NETWORK-UNVERIFIED. SMTP is the ONE
transport (the Gmail-API path left with cloud backup in R1 «Frivilligen
først»). Nothing extra to build — just run the app:

```bash
npm run tauri dev   # drive the "E-postvarsler" disclosure
# SMTP needs a host/port/credentials.
```

The **E-postvarsler** panel (R5) drives this. It reads `email_status` up-front
(works in every build) to show whether this binary has the `email` feature,
takes the SMTP host·port·user·pass·from, and fires `email_send_test` with the
chosen language.
In the **default build** `email_status` reports the feature present and
`email_send_test` really sends — a `feature_disabled` here means something is
wrong, not that the build is normal. (The "ikke bygd inn" hint only appears in a
`--no-default-features` build.) The SMTP password is never persisted — it travels
with the request and is dropped.

The card's gate + block-reason logic and the send dispatch (recipient +
language on the request):

- VERIFIED-BY: e2e/system-support.spec.ts::a build without the email feature gates the card and says so
- VERIFIED-BY: e2e/system-support.spec.ts::with the feature built but no transport, the block reason is stated
- VERIFIED-BY: e2e/system-support.spec.ts::«Test e-post» sends through the configured SMTP transport

1. **SMTP test message.** Configure an SMTP host (587 STARTTLS or 465 implicit
   TLS), save the password to the keychain, and send a test.
   - **Expected:** `lettre` connects + delivers a "✓ SundayRec — e-post
     fungerer" message; HTML + plaintext parts both present in the received
     mail.
2. **Error alert throttle.** Trigger two identical recording errors within
   10 minutes.
   - **Expected:** only the first mails; the second is suppressed by the core
     `AlertGate` (10-min window per `(recipient, message)`).

> [NET] The SMTP handshake is compiled into every default build but never run
> against a real server in the gate — se markøren i §8-innledningen («the
> **send** is …»).

---

## 9. Menubar tray [GUI] — `tray` (IN DEFAULT)

The tray menu-model (localized items, actions, tooltip, icon precedence) is
unit-tested in `sundayrec-core::tray`; the native menubar item is
**GUI-UNVERIFIED**. (The `sundayrec://` deep-link scheme left with the
Sunday-suite integrations in R1 «Frivilligen først».) `tray` is in the **`default` feature set**, so it is
present in every release build and in a plain `npm run tauri dev`.

As of **R7** the tray is actually **installed** in `setup()` whenever the
feature is on (`tray::install` builds the `TrayIcon` from the core menu
model and wires `on_menu_event` → `handle_menu_event`). The menu
**start/stop/show** actions are wired to the backend: **Stop** calls
`RecorderEngine::stop()` directly; **start** / preflight / diagnostics emit
`tray://action` for the renderer to turn into the matching `invoke(...)`;
**show**/**quit** are handled in-process.

```bash
npm run tauri dev   # tray is on by default — nothing to add
```

1. Launch; confirm a SundayRec item appears in the macOS menubar / Windows tray.
   - **Expected:** the menu shows status → open → start/stop → folder → check
     system → diagnostics → quit, in the UI language.
2. Click **Stopp opptak** while recording.
   - **Expected:** the recording stops (the `RecorderEngine::stop()` path) and a
     new history row appears, even with the window unfocused.
3. While recording, the menu swaps "Start" → "Stop" and the icon turns red.

> [GUI] The `tauri::tray` item install and the menu paint need a real desktop
> session — se markøren i §9-innledningen. The dedicated tray icon assets aren't bundled yet
> (the app's default window icon is reused) — see docs/NEEDS-RICHARD.md PU-2.

---

## 10. ~~Podcast RSS publish~~ — REMOVED (R1 «Frivilligen først»)

The RSS feed, the `publish` feature and the Podcast card left the app
2026-08-23.

---

## 10b. Whisper transcription [HW] — `whisper` (IN DEFAULT)

The model registry (id/url/size/SHA/quality), the whisper-cli/whisper-rs argv +
thread heuristic, the ffmpeg 16 kHz-mono convert argv, the progress/exit parse,
the JSON-sidecar → `TranscriptData` normalise, and the long-recording
chunk-plan + segment-merge are all unit-tested in `sundayrec-core::whisper`. The
model download (SHA-verified), the ffmpeg conversion, and the actual inference
are **HARDWARE-UNVERIFIED**. `whisper` is in the **`default` feature set**, so
every ordinary build pulls `whisper-rs` and compiles libwhisper from C/C++
source — that needs CMake + a C/C++ toolchain, and it is why a first build is
slow (~16 s on Apple silicon, cached after).

```bash
cargo build -p sundayrec   # default already includes whisper; CMake builds libwhisper
```

1. `whisper_list_models` / `whisper_model_status` / `whisper_delete_model` /
   `whisper_cancel_download` work in **any** build (the registry, on-disk size
   check, fs delete, and the cancel signal are pure/fs). Download a model into
   the app-data `whisper-models/` dir with `whisper_download_model`.
   - **Expected:** `whisper://model-progress` events stream `{ id,
bytesDownloaded, bytesTotal, fraction }` (the shaping is the unit-tested
     `download_progress`); a second download for the same id while one is in
     flight returns `already_downloading`; `whisper_cancel_download` aborts the
     stream and removes the `.partial`; on completion the SHA-256 is verified
     against the registry (`verify_model_hash`) before the `.bin` is promoted.
     // NETWORK-UNVERIFIED (the HTTPS stream + write are wired but unproven).
2. Run `whisper_transcribe` on a short recording (the default build can).
   - **Expected:** ffmpeg converts to 16 kHz mono, whisper-rs runs, and a
     `TranscriptData` (seconds-based segments) comes back. A `feature_disabled`
     validation error here is a BUG in a default build — it should only ever
     appear under `--no-default-features`.

The **Transkribering** panel (R5) drives this: pick a recording (from history) +
a model + a language, **Transkriber**, then the segments render and **SRT** /
**VTT** / **TXT** buttons save the transcript via `whisper_export_transcript`
(native save dialog). The model registry + the export render work in **every**
build; only `whisper_transcribe` needs the feature — which the default build
has. There is **no calm gate hint** on this panel: in a
`--no-default-features` build a transcribe attempt surfaces the
`feature_disabled` rejection through the ordinary error dialog
(«Transkribering feilet» with the real reason) — this runbook used to promise
a calm «ikke bygd inn»-hint that has never existed here. The renderer half is
browser-tier pinned:

- VERIFIED-BY: e2e/editor.spec.ts::a feature_disabled transcribe surfaces its reason in the error dialog

```bash
npm run tauri dev   # drive the Transkribering disclosure
```

3. (any build) **Export.** After a transcript exists, click SRT/VTT/TXT.
   - **Expected:** a file appears at the chosen path. SRT uses `HH:MM:SS,mmm`
     numbered cues; VTT has a `WEBVTT` header + `.` ms separator; TXT is one
     segment per line. The rendering is the pure
     `sundayrec-core::whisper::export_transcript` (unit-tested).

> [HW] The C/C++ build, the model download, and inference are unproven in the
> gate — only the `sundayrec-core::whisper` decisions are unit-tested. The
> export render + the file write are pure/GUI-UNVERIFIED (no feature needed).

---

## 10c. ~~Live cue bridge~~ — REMOVED (v0.14 feature, core consumer gone in R1)

`core/integrations/live_bridge.rs` left with the Sunday-suite integrations
2026-08-23.

---

## 11. OS wake-timers + scheduled launch [HW] (already wired, no feature)

The scheduler→recorder launch and the OS wake plumbing are wired in
`src-tauri/src/{scheduler,wake}` (no feature flag — they were part of Fase 5).
The next-fire / catch-up / skip _decisions_ are unit-tested in
`sundayrec-core::{schedule, wake}`.

**The two mechanisms, so you know what you are testing:**

- **macOS** — scheduling is `pmset schedule wake`, run unelevated first and
  escalated to ONE `osascript … with administrator privileges` prompt when that
  fails (writing a power event needs root; there is no unprivileged path).
  _Reading_ what is scheduled goes through IOKit
  (`IOPMCopyScheduledPowerEvents`, unprivileged), with `pmset -g sched` kept as a
  fallback because Apple Silicon is known to hold schedules `-g sched` does not
  list.
- **Windows** — scheduling is an in-process `SetWaitableTimer(fResume = TRUE)`.
  No UAC prompt, no scheduled task left on the machine — **and no wake at all if
  SundayRec is not running.** That is the model: the app autostarts and lives in
  the tray. Neither platform can start a machine from S5 (full shutdown).

The command shaping, the AppleScript/shell quoting, the escalation ladders, the
`wmic`→CIM fallback and the dedup invariants now have unit tests over a fake
shell and fake timers:

- VERIFIED-BY: src-tauri/src/wake/plan.rs::mac_elevated_plan_is_byte_exact_for_a_normal_schedule
- VERIFIED-BY: src-tauri/src/wake/plan.rs::mac_elevated_plan_keeps_a_hostile_owner_label_inside_the_literal
- VERIFIED-BY: src-tauri/src/wake/mod.rs::mac_partial_failure_escalates_to_one_admin_prompt_for_the_whole_set
- VERIFIED-BY: src-tauri/src/wake/mod.rs::mac_admin_prompt_dismissal_is_cancelled_not_permission
- VERIFIED-BY: src-tauri/src/wake/mod.rs::windows_clears_then_arms_one_waitable_timer_per_point
- VERIFIED-BY: src-tauri/src/wake/mod.rs::engine_records_the_empty_key_so_a_re_add_re_registers
- VERIFIED-BY: src-tauri/src/wake/mac_read.rs::live_iokit_read_is_callable_unprivileged

What is left for the rig:

1. Add a slot a couple of minutes ahead; leave the app running.
   - **Expected:** at the slot time the recorder starts unattended; the tray /
     UI "next recording" updates; a reminder notification fires `reminder_minutes`
     before.
2. **macOS:** enable wake-from-sleep, reschedule (accept the admin prompt), sleep
   the Mac just before a slot.
   - **Expected:** the Status panel lists the wake (read via IOKit); the machine
     wakes and records. Cross-check with `pmset -g sched` — and note that on
     Apple Silicon the two can legitimately disagree; the panel is deliberately
     pessimistic, so "missing" there means "click Planlegg again", not "broken".
3. **Windows:** enable wake-from-sleep, reschedule (no prompt should appear),
   confirm `powercfg -waketimers` lists a timer set by `[PROCESS] …SundayRec.exe`,
   then sleep the machine just before a slot.
   - **Expected:** the machine resumes and records. If it does not, check
     "Tillat vekketimere" in the power options first — an armed timer with that
     setting off fires without waking anything, and nothing in the arming call
     reports it.
4. **Windows, the honest limit:** quit SundayRec entirely, then sleep the machine.
   - **Expected:** it does **not** wake. This is by design, not a bug — verify it
     so nobody later "fixes" it back into a scheduled task.

> [HW] Wall-clock timing, the admin prompt, and the real OS resume can only be
> confirmed on a Mac/Windows box. The `SetWaitableTimer` call itself does not even
> compile on macOS — CI's `windows-check` lane is what proves it builds.

---

## 12. Non-destructive editor [HW] — `editor` (IN DEFAULT)

The editor I/O seam (`src-tauri/src/editor`) drives the bundled ffmpeg/ffprobe
sidecar over the unit-tested `sundayrec-core::{editor, mastering,
audio_analysis}` decisions: load (ffprobe duration/channels/format/rate/streams),
peaks (8 kHz mono decode **streamed on a pipe** → core down-sample, cached in a
`<stem>.peaks.json` sidecar), segments (16 kHz s16le decode → VAD/sermon
classifier, cached the same way), mastering analyze (pass-1 loudnorm measure),
and export (core cut-plan + processing + mastering → mp3/aac/wav/flac/mp4/mov).
NO new native dep — ffmpeg is a sidecar and the PCM is folded into peaks by hand.

Three things the editor overhaul settled, and what you are checking here:

- **Playback is the ORIGINAL**, streamed by a media element over `asset://`.
  The 8 kHz decode feeds the WAVEFORM only; nothing decodes the recording into
  renderer memory. A transcoded AAC proxy is the fallback, taken only when the
  webview has no decoder for the container or the original won't open — and it
  announces itself with a notice.
- **Caches**: peaks and segments land in sidecars beside the recording, keyed on
  size+mtime. A second open of the same file must not re-decode it.
- **Export is honest**: it always runs on the untouched original; a mastering
  target measures the CUT signal (not the raw file) before it normalises;
  "Normaliser" is skipped under a mastering preset and the modal says so; the
  destination defaults to "Samme mappe"; progress is real and the render is
  cancellable and kill-timed.

The ffmpeg runs are **HARDWARE-UNVERIFIED** (they need real media). `editor` is
in the **`default` feature set**, so the Rediger screen is live in every release
and in a plain `npm run tauri dev`; the `feature_disabled` response and the calm
"not built into this build" hint only exist under `--no-default-features`.

```bash
npm run tauri dev   # drive the Redigering disclosure — editor is on by default
```

1. Record (or import) a short service so it shows in History, open the
   **Redigering** disclosure, and pick the recording (or use **Åpne lydfil…**
   to pick any audio/video file via the native dialog).
   - **Expected:** the duration paints almost immediately (ffprobe reads
     container headers only), then the waveform. Press play: it must sound like
     the file, not like a telephone — that is the `asset://` transport on the
     original. No quality notice for a normal wav/flac/mp3/m4a.
     1b. **Reopen the same file.** — **Expected:** the waveform is back in a blink
     and no "Analyserer bølgeform…" line appears (the peaks sidecar answered).
     `ls` next to the recording shows `<stem>.peaks.json`. Delete it and reopen to
     watch the first-open path again.
     1c. **Open an `.ogg`/`.opus`/`.webm`** (a container WKWebView can't decode).
   - **Expected:** a "Klargjør avspilling…" line, then playback works and a
     notice says it is going through a temporary file at full quality, and that
     export still uses the original.
2. Click **Finn segmenter** and **Mål lydstyrke**.
   - **Expected:** segments list with one **Preken** (sermon) block highlighted
     gold; a loudness reading like `-23.4 LUFS → -16`.
3. Mark a cut or two — **click-and-drag on the waveform** («Klikk og dra for å
   markere et kutt»), or let **Marker preken automatisk** place them. (The old
   «Legg til kutt» button and its start/end seconds inputs were removed in the
   v0.9 editor overhaul — drag is the cut gesture now.) Remove one with **✕**.
   - **Expected:** red cut bands overlay the waveform at the marked spots
     (the canvas paint itself is still // GUI-UNVERIFIED); region rows show
     `m:ss–m:ss`; removed rows disappear.
   - VERIFIED-BY: e2e/editor.spec.ts::a cut row shows its range and the ✕ really removes it
4. Open **Eksporter** WITHOUT picking a destination, choose a format + a
   mastering target (**Ingen / Tale — naturlig −19 / Tale — tydelig −16 /
   Tale — kraftig −14 / Musikk + tale −16** — the preset list was renamed from
   the old Podkast/Strømming set), and export.
   - **Expected:** the destination pill reads "Samme mappe" and a
     `*_redigert.<fmt>` file lands next to the source (no "path must be
     absolute"). The progress bar moves for real — with a mastering target it
     reads `Måler lydstyrke` up to ~50 % and then `Koder`. On playback the
     marked regions are gone and the loudness is on target.
   - With a mastering target the level row must say **"Volum styres av
     mastring"** rather than promising a normalize gain the export skips.
   - The modal's two honesty claims (default destination pill «Samme mappe» +
     the mastering-owns-the-volume level row):
   - VERIFIED-BY: e2e/editor.spec.ts::the export modal is honest about destination and level
     4b. **Cancel a long export** mid-render. — **Expected:** it stops within a
     second or two and the result row says "Eksport avbrutt" — not a frozen bar.
     4c. **Video file:** open an mp4, keep "Behold video", export.
   - **Expected:** the mp4 out has both streams and honours the cuts. The
     intro/outro rows are greyed with "Jingler støttes ikke for video ennå" —
     jingles are audio-only, and the export no longer pretends otherwise.
   - Settings → Video → **Maskinvare-koding (VideoToolbox)** is off by default.
     Turn it on (macOS) and re-export: same file, faster. If VideoToolbox
     refuses, the log shows a warning and the export completes in software
     anyway — a failed hardware render must never cost the user the export.
5. **P1 reopen-ability (cuts-draft sidecar):** with cuts marked, close the
   editor (or reselect another recording) then reselect the same recording.
   - **Expected:** the cut rows are back — restored **silently** from the
     cuts-draft sidecar the autosave writes every 2 s (with a 7-day freshness
     guard; the old «Fant lagrede kutt fra forrige økt»-banner + Gjenopprett
     button were dropped — the restore is automatic now, see
     editor/loader.ts). After a successful **Eksporter** the draft is deleted,
     so a later reopen restores nothing.
   - VERIFIED-BY: e2e/editor.spec.ts::unsaved cuts from a previous session come back on reopen
6. **P1 mastering A/B preview:** with a mastering target chosen, click
   **Forhåndsvis mastering (15 s)**.
   - **Expected:** an `<audio>` control appears playing a temp
     `sundayrec-master-preview-*.mp3` of the first 15 s through the preset chain
     — A/B it against the original before committing to the full export.
7. **P1 mastering apply + cancel:** start a master, watch the
   `editor-master-progress` ticks, and abort mid-render with **Avbryt**.
   - **Expected:** progress advances, and a cancel kills the ffmpeg child and
     returns `true` only while the job is live (`false` afterwards — the pure
     `JobRegistry` bookkeeping). A duplicate job id is rejected up front.
   - The panel measures loudness first and then hands that measurement to the
     apply, so an Apply on a long service starts encoding straight away instead
     of reading the whole file a second time. Time it: the gap between the
     "Original: −23.4 LUFS → −16 LUFS" line and the first progress tick should
     be short even on a 90-minute recording.
8. **E8 sermon-pick correction survives a reopen:** on a recording where the
   auto-pick is wrong, choose the right block in the sermon dropdown, close the
   editor, and reopen the same recording.
   - **Expected:** once the analysis card finishes, **your** block is the starred
     one in the dropdown and the highlighted one on the timeline — not the
     detector's. A `<base>.feedback.json` sits next to the recording; open it
     and check what is in it: offsets, durations, confidences and reason CODES,
     and no path, filename or clock time anywhere.
   - Picking the detector's own block again DELETES the file (agreement is not a
     correction), and cycling through three options before settling leaves ONE
     record — the block you settled on.
   - The restore rides on detection, so it happens when detection does: video
     files and a restored cuts-draft do not auto-analyse, and there the
     correction comes back when you press **Analyser opptak**.
9. **E8 the companion signal reaches the same file:** build the AI companion on
   a transcribed recording, press **→ Bruk i metadata**, type one character
   into the title field, and switch to another recording.
   - **Expected:** the same `<base>.feedback.json` now also holds
     `companionSuggestions` entries (`title` accepted with
     `editedAfterAccept: true`, the ones you never touched `left_alone`). Still
     no path, filename, suggestion text or clock time anywhere in the file.
   - (The third signal, `trimAdjustments`, was only ever written by the review
     queue's publish step — with the queue gone in R1 «Frivilligen først»
     nothing writes it any more; existing sidecars keep theirs. R2 decides the
     learning loop's fate.)
   - The companion events belong to the recording the panel was showing, not the
     one you switched to — check that the second recording's sidecar did not
     appear when you switched away from the first.
   - With diagnostics ON, **Innstillinger → System → vis hva som sendes** should
     now list both `corrections` (a signal, a direction and a coarse band) and
     `companionOutcomes` (`title` / `accepted_edited`, `chapters` /
     `left_alone`) with counts — and the caption must NOT say «ingenting å sende
     akkurat nå» while they are on screen. With diagnostics OFF, do the same
     edits and confirm both collections stay empty: nothing is accumulated for
     someone who has not opted in, not even in memory.
   - The preview surface's half of this (both collections rendered, the caption
     honest while they are on screen) — the accumulation gating stays
     backend-verified:
   - VERIFIED-BY: e2e/telemetry-preview.spec.ts::a payload carrying corrections + companion outcomes shows them, not «ingenting å sende»
   - The consent question itself (the one-time card, and a decline recorded as
     a real answer so nothing is ever collected without an explicit yes):
   - VERIFIED-BY: e2e/telemetry-preview.spec.ts::the one-time consent card asks, and a decline is recorded as a real answer

> The sidecar read/write/delete + the 400 MB inline-vs-stream guard + the
> `__editor_tmp`/`__editor_bak` startup sweep are **fs, not ffmpeg** — they
> compile and run in the default build and ARE exercised in the gate (real
> tempdir round-trips). Only the ffmpeg-driven probe/preview/apply need real
> media — se [HW]-markøren i §12-innledningen.

> [HW] The ffprobe/decode/measure/render runs only execute against real media —
> never in the gate (samme markør). The core argv-building, filter-graph,
> loudnorm parse, and VAD/sermon decisions are unit-tested in Rust core, and
> the renderer's load→peaks→regions→export data flow is covered by the browser
> tier (e2e/editor.spec.ts — the "no JS unit-test harness on this branch" note
> that used to sit here predates E5.2). The waveform/cut-band canvas paint
> itself is still unproven — se markøren i steg 3 over.
> A `--no-default-features` build returns `feature_disabled` for every editor
> command and the panel shows a calm hint; the default build does not.

### 12b. Editor STABILITY loop [HW] — prove the 2026-06 hardening

Targets the "editor is unstable" reports. Run this stress loop after the fixes:

1. **Large file:** open a 100 MB–4 h recording.
   - **Expected:** the timeline appears in well under a second (ffprobe headers)
     and the waveform follows; no multi-second freeze, no OOM/renderer crash.
     Renderer memory must stay flat — a 4 h FLAC costs the same as a 4 min one,
     because neither the recording nor an extract of it is held in memory.
2. **Rapid play/stop/seek:** play, stop, seek, undo, redo ~20× quickly.
   - **Expected:** no stuck play-icon, no doubled/looping audio, no hang.
3. **Switch files mid-play:** start playback, then open a different recording
   before it finishes; repeat fast a few times.
   - **Expected:** the new file loads cleanly — no wrong audio/video-layout from
     the previous file, no extra AudioContext piling up (the loader seq-guards),
     no stale clip-warning badge.
4. **Undo mid-drag:** drag a cut handle and press Cmd/Ctrl+Z mid-drag.
   - **Expected:** the undo is ignored until the drag ends (no cut-history
     corruption); cuts stay consistent.
5. **Full-fidelity playback, no flag.** Every file plays at full quality: the
   original over `asset://`, or an AAC proxy for containers the webview can't
   decode. There is no `sundayrec.editor.playbackProxy` opt-in any more — the
   8 kHz preview transport (and the flag that gated its replacement) is gone.
   - **Expected:** no file sounds low-fi. If one does, it is a bug, not a
     setting.

> [HW] These are interactive webview behaviours (AudioContext lifecycle, the
> `<audio>` proxy transport, canvas redraw) that the headless gate can't see.

---

## 13. Observability: crash ring, log file, capture/video probes (no feature) [HW]

Etappe 2 gave the app a memory of its own failures — a panic hook + bounded
crash ring (E2.1), a supervisor that restarts long-lived tasks and records
when it had to (E2.2), a rotating file log (E2.3), a renderer-side ring of
failed IPC calls (E2.4), and a real capture/video probe wired back into
Diagnose (E2.5). None of it needs a feature flag; all of it is exercised here.

### Trigger a deliberate crash

```bash
export SUNDAYREC_TEST_PANIC=1
npm run tauri dev
```

- **Debug build only** (`#[cfg(debug_assertions)]`) — inert in a release build
  no matter what the env var says.
- **Quit any installed SundayRec first.** The single-instance plugin (all
  builds share the identifier `no.sundayrec.app`) means a second launch just
  focuses the existing window instead of starting a new process — with the
  installed app already running, the `tauri dev` process you just started
  never gets far enough to panic.
- Two seconds after startup a watched task panics on purpose
  ("SUNDAYREC_TEST_PANIC=1: deliberate panic to prove the crash ring"). The
  process itself does **not** crash — `crash::watch_handle` catches the panic
  at the `JoinHandle` boundary — but it is persisted exactly as a real one
  would be.
  - **Expected:** the terminal logs a `PANIC: …` line; a new
    `crash-<millis>-<seq>.json` appears in `<app-data>/crashes/`; opening
    **Lyd → Diagnose** shows finding **SR-CRASH-01** ("Appen har krasjet")
    with the count and the newest message.

### Where the files land

| What                                 | Path                                                  | Kept                                                                          |
| ------------------------------------ | ----------------------------------------------------- | ----------------------------------------------------------------------------- |
| Live log                             | `<app-data>/logs/sundayrec.log`                       | rotates at 2 MB                                                               |
| Rotated logs                         | `<app-data>/logs/sundayrec.1.log` … `sundayrec.4.log` | 5 files total, ~10 MB ceiling                                                 |
| Panics (process hook + watched-task) | `<app-data>/crashes/crash-<millis>-<seq>.json`        | newest 20                                                                     |
| Supervised-task restarts             | `<app-data>/crashes/restart-<millis>-<seq>.json`      | newest 20, own ring — a flapping task cannot evict the panic that explains it |

`<app-data>` is the platform app-data dir Tauri resolves (macOS:
`~/Library/Application Support/…`; Windows: `%APPDATA%\…`).

### «Vis logg» / «Kopier siste logg» (System-fanen → Hjelp og opplæring)

- **Vis logg** reveals the live log file in Finder/Explorer (`logs_reveal` →
  `tauri_plugin_opener::reveal_item_in_dir`), falling back to opening the
  folder itself before the first line has been written.
- **Kopier siste logg** pulls the tail of the live file (`logs_tail`, capped
  server-side at 512 KB regardless of what the UI asks for) and copies it to
  the clipboard, with a toast confirming the copy (or that the log is still
  empty).
  - VERIFIED-BY: e2e/system-support.spec.ts::«Kopier siste logg» puts the tail on the clipboard and confirms
  - VERIFIED-BY: e2e/system-support.spec.ts::an empty log is called out instead of copying nothing
- **Expected:** the file is plain text, newest lines at the bottom; secrets
  (SMTP passwords, OAuth tokens — and defensively RTMP keys) are redacted on the writer
  thread before a line ever reaches disk — confirm none show up if you have
  any of those configured.

### Capture-probe expectations in Diagnose

**Lyd → Diagnose** now runs a real ~2 s capture (and, with video on, grabs one
real camera frame) through the SAME backend a recording uses, then reports
`captureOk` / `videoOk` instead of the old permanent "ikke testet".

1. With nothing else using the mic, click **Diagnose**.
   - **Expected:** the probe runs and reports true/false. A `false` raises
     **SR-CAPTURE-02** ("Testopptaket fikk ingen lyd") — critical, and
     distinct from SR-CAPTURE-01 (a recording that happened but stuttered).
     With video on, a failed frame grab raises **SR-VIDEO-02** ("Kameraet ga
     ingen bilde").
2. **Refusal path 1 — during a recording.** Start a recording, then open
   Diagnose.
   - **Expected:** the probe does **not** run — the report's
     `capture_probe_skipped` reads "et opptak pågår — lydprøven ville tatt
     enheten"; nothing contends with the live take.
3. **Refusal path 2 — while the VU meter holds the input.** Open
   **Innstillinger → Lyd** (the channel grid opens the VU stream) and click
   **Diagnose** without stopping it.
   - **Expected:** the probe again does not run — `capture_probe_skipped`
     reads "nivåmåleren bruker mikrofonen — stopp den og kjør Diagnose
     igjen". Leave the Lyd tab (which stops the meter) and run Diagnose again
     to see the probe actually execute.

> [HW] The panic/crash-ring plumbing and the finding rules are unit-tested in
> `sundayrec-core::diagnostics` + `src-tauri/src/crash.rs`; only the
> end-to-end wiring — does a REAL panic reach a REAL file, does the REAL log
> rotate on a REAL disk, does the REAL device refuse the probe at the right
> moments — needs a rig. See `docs/NEEDS-RICHARD.md` for what is still
> owner-verified only.

---

## §PU-6 — ~~Episode prep + human-review queue~~ — REMOVED (R1 «Frivilligen først»)

The review queue, the reminder ladder, the tray callout and the editor's
review mode left the app 2026-08-23. The editor ALWAYS auto-detects the sermon
on open now (§12 step 8).

---

## §R7 — Auto-update (`updater`, IN DEFAULT) + settings completeness

### Auto-update

The status model (the localized `idle`/`checking`/`upToDate`/`available`/
`downloading`/`readyToInstall`/`error` phases), the dev-check guard, the
download-percent math, and the semver "is newer" decision are unit-tested in
`sundayrec-core::update`. The feed fetch + signature verify + install + relaunch
are **NETWORK/GUI-UNVERIFIED**, and a **real** update additionally needs a
SIGNED release + the updater public key in `tauri.conf.json` — see
docs/NEEDS-RICHARD.md. `updater` is in the **`default` feature set**, so it is
present in every release build and in a plain `npm run tauri dev`.

```bash
npm run tauri dev   # drive the Oppdateringer disclosure — updater is on by default
```

**«Oppdater automatisk»** (Generelt) is the privacy gate PRIVACY.md promises:
off = the app never contacts the update server on its own (no startup check,
no hourly repeat — not even in the window where the settings blob is still
loading, the #11 race fixed in PR #101); on = one immediate check plus exactly
one hourly schedule; flipping it mid-session arms/cancels the schedule without
firing extra checks. All four renderer paths are pinned in the browser tier:

- VERIFIED-BY: e2e/auto-update.spec.ts::off at startup: zero update_check even while settings load slowly (the #11 race)
- VERIFIED-BY: e2e/auto-update.spec.ts::on at startup: one immediate check, and the hourly repeat is scheduled
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling off while running stops the schedule and further checks
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling back on re-arms: an immediate check and a fresh schedule

**«Oppdateringskanal»** must reach the STORE with its value — the backend
(`update/mod.rs::current_channel`) reads sqlite, and the v0.11.1-beta.2 rig bug
#113 was precisely this select saying «Lagret ✓» while the machine silently
stayed on the stable feed. R4 removed the curated bridge that dropped it: the
renderer saves the full object through `settings_save`, sqlite is the one
store:

- VERIFIED-BY: e2e/update-channel.spec.ts::switching to beta reaches the store, not just the select
- VERIFIED-BY: e2e/update-channel.spec.ts::switching back to stable syncs too, and asks no question

1. Open the **Oppdateringer** disclosure and click **Se etter oppdateringer nå**.
   - **Expected in a default build:** a real check, not an error. Under
     `--no-default-features` the `update_check` command rejects with
     `feature_disabled`, and the panel has **no dedicated gate hint** for it —
     the rejection surfaces through the ordinary error path as «Kunne ikke
     sjekke for oppdateringer» (this runbook used to promise a calm «ikke
     bygget inn»-message that has never existed in this renderer). Seeing
     that error text in a default build with network is a BUG.
   - VERIFIED-BY: e2e/auto-update.spec.ts::a feature_disabled check surfaces as the ordinary error text
2. (dev build) **Se etter oppdateringer nå**.
   - **Expected:** the status reports **Du er oppdatert** — a dev build
     short-circuits the check (the `should_check` guard, unit-tested in
     `sundayrec-core::update`), so no error from a missing feed. The renderer
     half — an `upToDate` answer paints «Du er oppdatert» and retires any
     stale install button — is browser-tier pinned:
   - VERIFIED-BY: e2e/auto-update.spec.ts::an upToDate answer paints «Du er oppdatert» and retires stale buttons
3. (**release** build pointed at a real signed feed) check
   → **Last ned** → **↺ Start på nytt og installer**.
   - **Expected:** the panel walks `available` → `downloading {pct}` →
     `readyToInstall`; the relaunch applies the staged update. Needs the signed
     release + pubkey (NEEDS-RICHARD). // NETWORK/GUI-UNVERIFIED.

### Settings completeness (no feature)

R7 closed the gap between the Electron `store.ts` `Settings` and the Tauri model:
church profile (`churchName`/`responsiblePerson`), notification toggles
(`notifyStart`/`notifyStop`), and email config (`emailOnError`/`emailAddress`/
`emailSmtp`/`emailSmtpPort`/`emailSmtpUser` — the SMTP **password** stays in the
OS keychain via the `email` seam, never in the settings bag) plus the editor
intro/outro paths. All carry defaults + validation (`email_smtp_port` clamped
1..=65535) in `sundayrec-core::settings`.

1. Open **Generelt**, scroll to the **Menighet** / **Varsler** / **E-postvarsler**
   sections.
   - **Expected:** every field round-trips through `settings_save` (debounced)
     into SQLite and survives a relaunch; the port clamps to 1..=65535.
   - The church-profile round-trip (debounced save → storage → reload) and the
     port clamp:
   - VERIFIED-BY: e2e/settings.spec.ts::the church profile fields round-trip into storage and survive a reload
   - VERIFIED-BY: crates/sundayrec-core/src/settings.rs::validate_clamps_smtp_port
   - Since R4 there is no curated subset to drop a key from: `settings_save`
     carries the FULL object in one vocabulary, boot only reads, and a field
     written is a field read back (the #113/#115 class ends structurally):
   - VERIFIED-BY: e2e/settings-seam.spec.ts::boot performs no settings_save at all
   - VERIFIED-BY: e2e/settings-seam.spec.ts::one change saves the whole vocabulary — untouched fields keep their stored values
   - An existing install's localStorage blob is migrated into sqlite exactly
     once (old names translated, floats coerced, secrets stripped, key
     removed):
   - VERIFIED-BY: e2e/settings-migration.spec.ts::an old blob is imported once, translated, and the key removed
   - VERIFIED-BY: e2e/settings-migration.spec.ts::a corrupt blob yields defaults without crashing, and is not retried

---

## §R7b — Beta-søndag: the channel-promotion gate

Etappe 7's release flow is two rings (`RELEASE-CHECKLIST.md` §5): a tag first
goes to the `beta` channel, and nothing moves to `stable` — no matter how
green the headless gate is — until a beta tester has run a **real Sunday** on
the exact tag that was promoted, and it came back clean. This section is what
"clean" means: the checklist a beta tester (or whoever reads their report)
runs through before §5g (promote to `stable`) is authorized. It is a human
decision gate, not a code test.

**Before the service**

1. Confirm the machine is actually running the **promoted** tag, not whatever
   it happened to have installed. **Innstillinger → Generelt → Nåværende
   versjon** must read exactly the tag `node scripts/promote-release.mjs beta
vX.Y.Z-beta.N` promoted (`RELEASE-CHECKLIST.md` §5d/§5e).
   - **Expected:** version matches. If it's a build behind, either the update
     hasn't reached this machine yet (propagation — up to an hour for an
     already-running app, immediate on relaunch or a manual **Se etter
     oppdateringer nå**; see `ROLLBACK.md`) or **«Oppdater automatisk»** is
     off (§R7 above) — resolve which before treating today as a beta-ring
     result.
2. If the release touched `recorder/`, `capture.rs`, the editor, the meter
   loop, or boot ordering, this Sunday IS the §6a health gate
   (`RELEASE-CHECKLIST.md`) — a beta-ring release is not exempt from it.
   Catching a bad audio change here, before `stable`, is the entire reason
   the beta ring exists.

**During the service — a normal Sunday, not a synthetic test**

3. Record the actual service end-to-end — preroll (if enabled) through the
   real stop, at the length a real service runs, on the hardware this church
   actually uses (not a laptop mic standing in for the mixer).
4. Exercise whatever else this release changed for real, not just launch it —
   an editor change gets an edit, an
   email-alert change gets left running long enough to prove it fires (or
   correctly doesn't).

**After the service**

5. Open **Innstillinger → Lyd → Diagnose** (§5b) and read "Siste opptak
   (teknisk)".
   - **Expected:** `Dropp` / `xruns` / `IPC-overbelastning` at their healthy
     targets (≈0), clean exit, no `SR-CAPTURE-01`. Paste these numbers into
     the beta-ring report — the same numbers §6a asks for in the release
     notes, gathered a ring earlier.
6. Check the crash ring (§13, `<app-data>/crashes/`) for anything new since
   the beta build was installed.
   - **Expected:** nothing new. A crash the operator didn't notice at the
     time is exactly what the crash ring exists to surface after the fact.
7. Listen back to a few minutes of the actual recording.
   - **Expected:** it is complete, the right length, and sounds like the
     service. No telemetry number substitutes for someone actually checking
     the file that was the entire point of running SundayRec that morning.

**Go / no-go**

- [ ] Everything above came back clean → `RELEASE-CHECKLIST.md` §5g (promote
      to `stable`) is authorized.
- [ ] Anything did not come back clean → do **not** promote to `stable`. Fix
      it, cut a new `-beta.N`, promote that, and repeat this section on the
      new build. If a bad beta is already promoted and reaching testers, pause
      it first — see `ROLLBACK.md`.

---

## §P2b — Remaining Electron→Tauri IPC parity

These close the last handlers the Electron `src/main/ipc/*` exposed that the
earlier phases hadn't reached. Pure decisions live in `sundayrec-core` (unit-
tested); the I/O seams are annotated and, where they touch new hardware/network,
gated behind a default-off feature.

### ~~Sunday-suite integrations~~ — REMOVED (R1 «Frivilligen først»)

The Song/Plan/SundayEdit/Stage hand-offs, the integrations panel and the
`sundayrec://` scheme left the app 2026-08-23.

### Audio diagnostics (no feature) — `list_video_devices` + `diagnose_audio`

Mirrors `src/main/ipc/audio-devices.ts`. Both reuse the existing
`ffmpeg -list_devices` enumeration; the diagnostics shaping
(`build_audio_diagnostics`) is pure + tested.

1. Open **Generelt** → the camera dropdown / device probe.
   - **Expected:** `list_video_devices` returns the connected cameras and
     `diagnose_audio` returns the audio-input names (WASAPI loopback is not
     ported → `wasapi` empty, `wasapiAvailable` false). A missing ffmpeg / no
     devices yields empty lists, not an error. // HARDWARE-UNVERIFIED.

## What "passed" means

A green smoke test = §2–§6 all behave as the **Expected** lines say on a real
Mac with a real mic/camera, with no panic in the `tauri dev` stderr. §7 is a
bonus that needs a Google client. Record any deviation (which step, the stderr
log, the OS permission state) when reporting back.

For any build that touches **recording, capture, the editor, the meter loop, or
boot ordering**, also run **§5b** (recording-health telemetry) and **§12b**
(editor stability loop) and paste the diagnose "Siste opptak" numbers into the
release notes — that is the standing gate that stops unverified audio/editor
fixes from shipping (see `RELEASE-CHECKLIST.md`).
