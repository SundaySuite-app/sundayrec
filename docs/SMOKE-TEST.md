# SundayRec — Smoke Test Runbook

A hands-on, hardware-in-the-loop checklist for proving that the Tauri rebuild
actually records. Everything below the line **cannot** be exercised in the
headless CI gate — it needs a real display, microphone, and camera. This doc is
the bridge from "compiles + unit-tests pass" to "validated on a real rig".

> Legend: **[HW]** = HARDWARE-UNVERIFIED in code — never run against a device in
> the gate, only here. **[NET]** = needs network + a Google OAuth client.

## The navigation this runbook walks

Fase B of «Frivilligen først» replaced the shipped shell. The old five pages
with their eight tabs are gone; there are **three destinations** on the rail —
**Opptak · Bibliotek · Oppsett** — plus **Rediger**, which is not a destination
at all but a screen a recording opens into. Every step below has been re-walked
against the shell that actually ships; this table is the translation for
anybody holding an older report.

| where it used to be                              | where it is now                                                   |
| ------------------------------------------------ | ----------------------------------------------------------------- |
| Hjem                                             | **Opptak**                                                        |
| Historikk (+ the search box, the chips)          | **Bibliotek** (search kept; the chips are gone)                   |
| Innstillinger → Lyd (the 32-tile channel grid)   | **Oppsett › «Hvilken lyd?»** — device, channel pair, hearing test |
| Innstillinger → Filer                            | **Oppsett › «Hvor skal opptakene?»** + «Hvilken kvalitet?»        |
| Innstillinger → Generelt (Menighet)              | **Oppsett › «Hvilken kirke?»**                                    |
| Innstillinger → Deling / Varsler                 | **Oppsett › «Hvem får beskjed hvis noe går galt?»**               |
| Innstillinger → Video                            | **Oppsett › Tillegg › «Ta med kamera»**                           |
| Tidsplan (month calendar + day detail)           | **Oppsett › «Ta opp automatisk»**, and Avansert for the rest      |
| Innstillinger → System (log, profile, telemetry) | **Oppsett › Avansert**                                            |
| Oppdateringer / Oppdateringskanal                | **Oppsett › Avansert › Oppdateringer**                            |
| E-postserver (SMTP), inside the alerts card      | **Oppsett › Avansert › «E-postserver (SMTP)»**                    |
| Rediger, three tabs (Klipp / Lyd / Innhold)      | **Rediger**, three STEPS (Klipp → Lyd → Eksporter)                |
| The export MODAL                                 | the **Eksporter** step — there is no modal                        |
| «Nåværende versjon» in Generelt                  | the version line at the foot of the rail                          |

Surfaces that were removed rather than moved are listed at the bottom of this
file under **«Flater som ikke finnes lenger»** — read that before reporting a
missing screen as a bug.

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
npx playwright test    # the browser tier — every screen, driven the way a volunteer would
cargo build            # debug build of the Tauri binary
npm run build          # tsc + vite → dist/ (the shell a release bundles)
```

All four must be green before a smoke test is meaningful. As of this runbook the
gate is green: the full Rust test suite (`cargo test --workspace`) + a **vitest**
frontend suite (pure logic like the editor cut-history state machine; grows as
more pure logic is extracted) + clippy `-D warnings`. Every feature also compiles
in isolation — `cargo build -p sundayrec --features <flag>` for
`email`/`tray`/`editor`/`updater`. (Since R2 «Frivilligen først» the
workspace has no C/C++ toolchain dependency: `whisper` is gone.)

⚠️ **Which of these are actually in the shipping build.** `src-tauri/Cargo.toml`
sets `default = ["editor", "tray", "updater", "email"]`,
so **all four are ON in a plain `npm run tauri dev` / `cargo build` and in every
release**. The `--features <flag>` lines below them are redundant, not
prerequisites, and a `feature_disabled` response from any of those four is a BUG
to report — not the expected result. Only `asio`/`vad` are genuinely
default-off and need an explicit `--features`. To exercise a disabled path
deliberately, build with `--no-default-features`. (v0.14: `streaming`/`ndi`/
`bridge` were removed with the Direkte page — their old §R3/§R3b sections are
gone from this runbook. R1 «Frivilligen først» 2026-08-23: cloud backup (§7),
podcast RSS + `publish` (§10), the live cue bridge (§10c), the review queue
(§PU-6) and the Sunday-suite integrations (§P2b) followed; §8 e-post is
SMTP-only and §9's deep links are gone. R2 «Frivilligen først» 2026-08-23:
whisper transcription (§10b), the AI companion + chapters (§12 step 9 and the
chapter half of the editor), the learning cards (§R7 settings completeness, step 2)
and the Video tab's quality knobs (§12 step 4c) followed; §6b is a metadata
search only.)

---

## 2. Launch [HW]

```bash
npm run tauri dev
```

`predev` fetches ffmpeg if needed; vite serves on the fixed port **1420**
(`strictPort`); Tauri opens the window titled "SundayRec".

**What proves the bridge is up.** The rail paints **Opptak · Bibliotek ·
Oppsett** with the church name at the top and the version at the foot, and the
status line under it says one of five things — «Alt er klart» (green), «Lyden er
ikke koblet til» / «Lite plass igjen» (amber), «Neste opptak …» (grey) or «Tar
opp» (red). A status line at all means `settings_get` answered and the database
opened without panicking; the old header's literal "backend OK" is gone, because
a line that only ever says one thing is not a status. If `settings_get` actually
FAILED, the shell says so out loud rather than rendering factory defaults as if
they were yours — that is `hydrateError`, and it is the one case where a
factory-fresh screen would otherwise be indistinguishable from a broken store.

**Where logs go:** the backend uses `tracing` to **stderr** of the terminal
running `tauri dev`. Bump verbosity with the env filter:

```bash
RUST_LOG=debug npm run tauri dev          # everything
RUST_LOG=sundayrec=debug npm run tauri dev # just our crates
```

Expect at boot: `SundayRec backend ready (db at …/sundayrec.sqlite)` and no
repeated background-task log spam.

**First run:** a fresh install (no `onboardingDone`) boots into the first-run
sequence; a settled install goes straight to **Opptak**. First run is no longer
a wizard with screens of its own — it is the five real Oppsett screens shown in
order, with a progress line and «Fortsett uten lyd» as the emergency exit, so
nothing a volunteer learns there has to be unlearned afterwards. The consent
question (E3.6) asks with the «Aldri»-list on display, records the answer —
yes _or_ no — through `telemetry_consent_set`, treats a decline as fully equal,
and cannot trap the operator if the backend rejects the answer. The renderer half of all of that is pinned in the browser
tier; only the native window/DB boot itself stays a rig observation:

- VERIFIED-BY: e2e/onboarding.spec.ts::first run shows the wizard; a settled install does not
- VERIFIED-BY: e2e/onboarding.spec.ts::the consent step exists, and says what is and is not collected
- VERIFIED-BY: e2e/onboarding.spec.ts::«Ja, del anonymt» grants consent and finishes the wizard
- VERIFIED-BY: e2e/onboarding.spec.ts::«Nei takk» declines — and the app is otherwise identical
- VERIFIED-BY: e2e/onboarding.spec.ts::a backend that cannot record the answer does not trap the operator

---

## 3. «Hvilken lyd?» → the meter moves → the channel pair [HW]

Fase B folded the 32-tile channel grid into ONE screen with three things on it:
which device, which channel pair, and a hearing test that answers the only
question a volunteer actually has — _do we hear it?_

1. Open **Oppsett → «Hvilken lyd?»**.
   - **Expected:** the devices are listed as cards — «Maskinens egen mikrofon»
     (marked "Kun for test, eller hvis dere ikke har mikser"), «USB / Ekstern»,
     and any mixer as «Miksebord · N kanaler». A device the machine no longer
     has says «Finner ikke {name}» in amber, not «Tilkoblet ✓».
2. Speak / tap the mic (or send signal on a mixer channel).
   - **Expected:** the meter moves and the WORD under it changes — «Vi hører
     ingenting» → «Vi hører lyd» → «For høyt». The word is read from PEAK, not
     RMS, so "too loud" is about the peaks. A dead-flat meter while you speak =
     the OS denied mic access (see §0) or the wrong device is active.
3. On a multi-channel device, pick the **channel pair** («Hvilke kanaler?» —
   the ones with signal light up), then press **«Bruk denne»**.
   - **Expected:** device, name, channel count and pair land in ONE save with
     ONE «Lagret ✓». They are four keys that must arrive together, so this is a
     deliberate button rather than an auto-apply: `useSetting` owns one key and
     has one receipt, and four keys landing separately is how half a device
     choice gets stored. Pressing it with nothing changed says «Ingenting er
     endret.» instead of a false receipt.
   - **Expected:** the status line and the question agree. «Alt er klart» while
     question 1 sits amber saying «Finner ikke Behringer X32» is the seam P1a
     closed — if you ever see the two disagree, that is a bug worth the report.
4. With no device at all: **«Finner ingen lydenheter»** and a **«Søk igjen»**
   button — not an empty list that looks like a still-loading one.

---

## 4. Camera preview [HW]

The shipped renderer polls `recording_preview_frame` **during recording only** —
there is no idle device-select preview surface. Verify the preview as part of a
video recording:

1. Turn the camera on under **Oppsett → Tillegg → «Ta med kamera»**, pick the
   camera, then start a recording (§5 with video).
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

1. On **Opptak**, press **«Start opptak»** (there is no modal any more — the
   source, the camera and the filename were all decided in Oppsett, so start is
   one button).
   - **Expected:** the overlay comes up and the status line turns red («Tar
     opp» — red never means anything else in this app); with `RUST_LOG=debug`
     the progress you see comes from the **native writer's byte counter** (not
     ffmpeg `size=` lines — those only appear in video /
     `classic_ffmpeg_audio` sessions).
   - **Also expected, before you press it:** with no source chosen the button
     is off AND says why («Start er sperret til lyden er valgt…») — it is
     `aria-disabled`, so a keyboard user can still reach it to hear the reason.
     A grey button with no explanation is the failure this replaced.
   - The start seam (`plan_recording_opts` + `start_recording`, once each), and
     the refusal path where the engine says no and the operator gets the
     localized reason on a screen that stays put:
   - VERIFIED-BY: e2e/recorder.spec.ts::manual start flips the app into the recording overlay
   - VERIFIED-BY: e2e/recorder.spec.ts::a start the engine refuses keeps the modal open and says why
2. Let it run ~30 seconds, talking so the silence-watcher does **not** fire.
3. Stop the recording.
   - **Expected:** the confirmation is the way round it should be — **«Fortsett
     å ta opp» is the primary button and the Enter choice**, «Stopp» is the
     secondary. A dialog whose default answer ends the service recording is the
     one this replaced.
   - **Expected:** a graceful stop — the engine raises the stop flag and does a
     **bounded join** of the capture/writer threads (no process kill). ffmpeg
     only appears at stop if a **delivery encode** (e.g. WAV → FLAC/AAC) runs.
     The finalizing overlay stays up and says the file is safe meanwhile; then
     a **«Opptaket er lagret»** card stays on screen (it does not fade like the
     old toast did), and a **new row** appears in **Bibliotek** with a plausible
     **duration (~30 s)** and **file size (> 0)**.
   - The stop seam (confirm guard → `stop_recording` once → an explicit
     finalizing overlay that waits for a terminal engine event):
   - VERIFIED-BY: e2e/recorder.spec.ts::stop is guarded by a confirm and then holds a finalizing overlay
4. Confirm the file exists on disk at the path shown — **«Vis i Finder»** on
   the receipt card is the shortest way, and it says so honestly («Fant ikke
   fila på disken.») if the file is not where the row claims.

**Also worth doing once, because it is the promise the window makes:** start a
recording and then close the window with the red button. The recording must
keep running (the app lives on in the tray) — that sentence was untrue until
P3 made it true in Rust. Then press ⌘Q while recording: it must ask ONE more
time, and on «Stopp» it waits until the file is safely written before the
process exits.

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

⚠️ **There is no Diagnose screen any more.** The modal this section used to
open (`Innstillinger → Lyd → Diagnose`) had no place in the three-destination
navigation and was not rebuilt — see «Flater som ikke finnes lenger» at the
bottom of this file. **The measurement is untouched**: the backend still writes
every number below on every recording, and `build_audio_diagnostics` still
composes the report. What is missing is a screen that shows it, so this section
is read **off disk** until one exists.

**Read the numbers after a normal recording:**

1. Record a service (or ~15 s of speech) normally, then stop.
2. Read `<app-data>/last-recording.json` (and the rolling
   `<app-data>/recording-telemetry-history.json` for the trend) — the same JSON
   the report is composed from. `<app-data>` is resolved in §13's table.
   - **Expected:** the fields behind the report's **"Siste opptak (teknisk)"**
     section — `Dropp`, `xruns`, `IPC-overbelastning (tapte nivå-oppdateringer)`
     and `Avsluttet rent` — with the newest recording first in the history file.
   - The report content and the SR-CAPTURE-01 rule are still gated in Rust; only
     the surface that displayed them is gone:
   - VERIFIED-BY: crates/sundayrec-core/src/diagnostics.rs::degraded_last_recording_warns_and_renders_section
   - **Healthy target:** `IPC-overbelastning 0` + clean exit, and per session
     type: **native audio** → `ring_overrun_samples 0` + the frame-count
     cross-check verdict exact; **video / ffmpeg** → `Dropp 0`, `xruns 0`.
   - A degraded recording also raises finding **SR-CAPTURE-01** with the counts.

**Prove the telemetry has teeth (it must DETECT a bad recording):**

3. Start a CPU hog (e.g. `yes > /dev/null &` ×4, or a heavy export), record ~15 s,
   stop, re-read the file.
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

## 6. The library row → reveal in folder [HW]

⚠️ **Notes can no longer be EDITED** — an owner decision in P3. A note that is
already in the database is still shown on its row; there is no modal to write a
new one, and nothing calls `recording_update_note`. See «Flater som ikke finnes
lenger».

1. Open **Bibliotek**. The new recording is the top row.
   - **Expected:** the row is titled by WHEN, not by filename — «Søndag 16.
     august 2026 · 11:00» — with the filename underneath. A camera session
     writes two files (`{stem}.mp4` and the `{stem}.wav` sidecar) and must show
     as **ONE** row with a «Video» chip, not two.
   - **Expected:** a recording under half a minute reads «Under 1 min», never
     «0 min». A recording whose duration the database does not know reads «—»
     and claims nothing. Those are two different facts and the row must not
     spell them the same way.
   - **Expected:** an existing note (written by an older build) still shows on
     its row.
2. Use **«Vis i Finder»** on the row.
   - **Expected:** the OS file manager opens at the recording (via the `opener`
     plugin — capability `opener:allow-open-path` is granted).
3. Press **«Slett»** on a row.
   - **Expected:** no question — the row moves to the trash and a toast offers
     **«Angre»**. Delete is undoable, so asking first would be a question with
     no stakes. The **Papirkurv** entry is always visible (it says «Papirkurven
     er tom» when it is), because a link that hides itself when empty is a link
     nobody learns exists. Inside it, «Legg tilbake» restores and «Slett nå» is
     the one genuinely dangerous button — and there **CANCEL is the Enter
     choice** and the confirm is a red SECONDARY.

---

## 6b. History search [HW]

The filter/grouping/stats math and the substring search are pure and
gate-tested (`history-core`); what a rig confirms is that the search box
wiring behaves on real data. (The transcript half of this section — hits
inside sermon text — left with whisper in R2 «Frivilligen først».)

1. Record two or three sessions (repeat §5) so **Bibliotek** has several rows.
2. Type into the search box («Søk etter dato eller navn»).
   - **Expected:** the list filters live by filename, date, or note text
     (case-insensitive); the count line above it describes the **filtered
     view** — the same rows as the list, so the two can never disagree. A query
     that matches nothing says «Ingen treff for «…»» with the explanation of
     where it looked, which is a different sentence from the
     never-recorded-anything empty state.
   - VERIFIED-BY: e2e/history.spec.ts::the search box filters live, and a miss says so in its own words

⚠️ **The Lyd / Video filter chips are gone.** «Video» survives as a FACT on the
row (it says a session has a camera file), never as a filter. Sortable columns
went with them. See «Flater som ikke finnes lenger».

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

Two screens drive this since fase B, and the split is deliberate. **Oppsett →
«Hvem får beskjed hvis noe går galt?»** is the volunteer's half: one toggle, one
address, one **«Send en test»**. **Oppsett → Avansert → «E-postserver (SMTP)»**
is the technical half: host · port · user · from, and the password (which goes
to the OS keychain, never into the settings bag).

The toggle on the volunteer screen sits behind a **Gate** that says «Krever en
e-postserver (SMTP). Sett opp under Avansert.» when no transport is configured —
because the canvas's «E-posten sendes via SundaySuite» is not true: there is no
such relay, and without the congregation's own SMTP server nothing arrives no
matter what is in the address field. `email_status` is read up-front (works in
every build) to show whether this binary has the `email` feature at all, and
`email_send_test` carries the recipient and the chosen language.
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

1. **SMTP test message.** Under **Avansert → «E-postserver (SMTP)»** configure a
   host (587 STARTTLS or 465 implicit TLS) and save the password to the
   keychain; then under **«Hvem får beskjed hvis noe går galt?»** enter the
   address, save it, and press **«Send en test»**. (Pressing test before saving
   an address says so — «Skriv inn en adresse og trykk Lagre først.» — rather
   than sending to nobody.)
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

In the new shell a tray action is a **signal**, not a synthesised click: the
router sets `pendingAction`, navigates to where the action belongs, and the
screen picks it up when it is ready. The old hooks clicked a button that had to
exist, on a page that had to be showing, in a DOM that had to be finished —
three assumptions that have each failed separately.

1. Launch; confirm a SundayRec item appears in the macOS menubar / Windows tray.
   - **Expected:** the menu shows status → open → start/stop → folder → check
     system → diagnostics → quit, in the UI language.
2. Click **Stopp opptak** while recording.
   - **Expected:** the recording stops (the `RecorderEngine::stop()` path), the
     overlay comes down and a new row appears in **Bibliotek**, even with the
     window unfocused.
3. While recording, the menu swaps "Start" → "Stop" and the icon turns red.
4. Click **Start opptak** from the tray with no source chosen.
   - **Expected:** it navigates to **Opptak** and does NOTHING else — the card
     above the button says why. Starting from the menubar on a source nobody
     chose would be the same lie, just in a different place.
5. Click **Åpne opptaksmappen**.
   - **Expected:** the folder opens AND the app lands on **Bibliotek**, so you
     also see the recordings you just asked to see.
6. Click **Sjekk systemet**.
   - **Expected:** it opens **Oppsett → «Hvilken lyd?»**, which is where every
     answer a preflight would give you now lives.

⚠️ **Diagnostikk** in the tray navigates to **Oppsett** and stops there: the
screen it used to open does not exist. Not a regression introduced by the tray —
see «Flater som ikke finnes lenger».

> [GUI] The `tauri::tray` item install and the menu paint need a real desktop
> session — se markøren i §9-innledningen. The dedicated tray icon assets aren't bundled yet
> (the app's default window icon is reused) — see docs/NEEDS-RICHARD.md PU-2.

---

## 10. ~~Podcast RSS publish~~ — REMOVED (R1 «Frivilligen først»)

The RSS feed, the `publish` feature and the Podcast card left the app
2026-08-23.

---

## 10b. ~~Whisper transcription~~ — REMOVED (R2 «Frivilligen først»)

Transcription (the `whisper` feature, whisper-rs/libwhisper — the build's only
C/C++ dependency — the model download, the Transkribering panel, SRT/VTT/TXT
export and the transcript search) left the app 2026-08-23. Transcripts are
better served by tools built for them; the section number stays so
cross-references still resolve.

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

The weekly time lives on **Oppsett → «Ta opp automatisk»** (one day, one start,
one duration). **Oppsett → Avansert → «Flere tider og spesialopptak»** holds the
rest: extra fixed times, and single-date specials (a concert, a Christmas Eve).

⚠️ **The month calendar, the day detail and the wake-diagnostics card are gone**
— the same information is two lists and one sentence now. The wake sentence is
on Avansert next to the toggle: «Denne maskinen kan vekkes fra dvale.» /
«… kan ikke vekkes — la den stå på.» / «… spør om administratorpassord første
gang.» / «Vi vet ikke ennå …».

⚠️ Turning **«Ta opp automatisk»** off no longer deletes the time. `Settings`
grew a real `auto_record_enabled` flag in P1b (read in ONE place,
`active_slots()`, so a flag honoured in five of six readers cannot wake the
machine at 10:50 for a recording it then refuses to make). Specials are NOT
gated by it — they are dates somebody entered for one concert.

1. Add a slot a couple of minutes ahead; leave the app running.
   - **Expected:** at the slot time the recorder starts unattended; the tray and
     the status line's «Neste opptak …» update; a reminder notification fires
     `reminder_minutes` before (that reminder is configured on **Oppsett →
     «Hvem får beskjed…» → «Påminnelse før automatisk opptak»**, and it is gated
     off with a stated reason until «Ta opp automatisk» is on).
2. **Also verify the flag round-trips:** turn «Ta opp automatisk» off, relaunch,
   turn it back on.
   - **Expected:** the time is still there. A profile written before the flag
     existed defaults to ON (`serde(default = "default_true")`) — `false` would
     have silently disarmed every congregation that already had a Sunday time.
3. **macOS:** enable wake-from-sleep under Avansert, reschedule (accept the
   admin prompt), sleep the Mac just before a slot.
   - **Expected:** the machine wakes and records. Cross-check with
     `pmset -g sched` — and note that on Apple Silicon the IOKit read and
     `pmset` can legitimately disagree; the app is deliberately pessimistic, so
     "missing" means "schedule it again", not "broken".
4. **Windows:** enable wake-from-sleep, reschedule (no prompt should appear),
   confirm `powercfg -waketimers` lists a timer set by `[PROCESS] …SundayRec.exe`,
   then sleep the machine just before a slot.
   - **Expected:** the machine resumes and records. If it does not, check
     "Tillat vekketimere" in the power options first — an armed timer with that
     setting off fires without waking anything, and nothing in the arming call
     reports it.
5. **Windows, the honest limit:** quit SundayRec entirely, then sleep the machine.
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

**The shape of the screen changed in P4.** 47 controls in three tabs plus 25 in
an export modal became **three steps** — **Klipp → Lyd → Eksporter** — with one
question each. There is no export modal, no «Normaliser» toggle, no mastering
apply panel and no intro/outro jingle rows; each of those is listed under
«Flater som ikke finnes lenger» with what replaced it. The BACKEND is unchanged:
the same `editor_*` commands over the same ffmpeg sidecar.

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

1. Record (or import) a short service so it shows in **Bibliotek**, then open
   it in **Rediger** — from the row, from the «Opptaket er lagret» receipt's
   «Åpne i Rediger», or by dragging a file onto the empty editor («Dra et
   opptak hit»). The drop zone is ONE element that is ALWAYS there: Tauri
   catches OS drags itself, so an overlay that only appears on `dragenter` is
   not there to be hit when the event arrives.
   - **Expected:** step 1 is **«Klipp»**, and it opens on the only question a
     volunteer has — _is this the sermon?_ The suggestion card is already there
     («Vi tror prekenen er her — fra … til … »), so keeping just the sermon is
     ONE click, not two through a tab.
   - **Expected:** the duration paints almost immediately (ffprobe reads
     container headers only), then the waveform. The loading line says which
     phase it is in («Leter etter prekenen …»). Press play: it must sound like
     the file, not like a telephone — that is the `asset://` transport on the
     original. No quality notice for a normal wav/flac/mp3/m4a.
   - **Expected:** the clock reads `h:mm:ss` the whole way. A service of 1 h
     2 min must not jump from «59:59» to «1:00:00» — the digits are
     tabular and the format does not change width mid-playback.
     1b. **Reopen the same file.** — **Expected:** the waveform is back in a blink
     and no "Analyserer bølgeform…" line appears (the peaks sidecar answered).
     `ls` next to the recording shows `<stem>.peaks.json`. Delete it and reopen to
     watch the first-open path again.
     1c. **Open an `.ogg`/`.opus`/`.webm`** (a container WKWebView can't decode).
   - **Expected:** a "Klargjør avspilling…" line, then playback works and a
     notice says it is going through a temporary file at full quality, and that
     export still uses the original.
2. Adjust the gold sermon window by **dragging its handles**, and — if the
   detector picked the wrong block — choose the right one from the block
   dropdown.
   - **Expected:** the handles are real focusable controls
     (`role="slider"`, «Der prekenen begynner» / «Der prekenen slutter»), so
     arrow keys move them. A volunteer who does not use a mouse must still be
     able to say where the sermon starts; a drawn rectangle cannot be focused,
     read aloud or nudged.
   - **Expected:** the correction is remembered («Vi husker det til neste
     gang») — see step 8.
3. Press **«Behold bare prekenen»** — or open **«Klipp manuelt»** and mark cuts
   by dragging on the waveform. Remove one with **✕**.
   - **Expected:** the cut list shows each kept region as `{name} · {span}`, and
     the result line reads «Resultat: … (av …)». Red cut bands overlay the
     waveform (the canvas paint itself is still // GUI-UNVERIFIED); removed rows
     disappear. Undo/redo say so honestly when there is nothing to undo.
   - VERIFIED-BY: e2e/editor.spec.ts::a cut row shows its range and the ✕ really removes it
4. Press **«Neste: Lyd»** — step 2. Choose **«Automatisk lydforbedring»**:
   **Tale** (the recommended default), **Tale og musikk**, or **Ingen**.
   - **Expected:** three cards with a REASON each («For preken og liturgi.
     Anbefalt.» / «Når lovsang skal være med.» / «Eksporter lyden slik den ble
     tatt opp.»), not five presets named after loudness targets. The three
     words map to real numbers in `sound-profiles.ts`; the mapping table is in
     `docs/APP-SHELL.md` §P4b.
   - Press **«Lytt»** and A/B **Før** / **Etter**.
   - **Expected:** a 20-second sample taken **from the sermon** (not from the
     first 20 seconds of the file, which is often an empty room) renders and
     plays. It is a real render through the real chain — and it writes to a
     temp file, never next to the original.
   - **«Avansert: åpne mikseren»** opens the full chain (low cut, denoise,
     dereverb, gate, compressor, de-esser, limiter, output level, three EQ
     bands).
   - **Expected:** the mixer says it **REPLACES** the profile («Mikseren
     erstatter «Tale». Ingen mastring legges oppå — du styrer hele kjeden.»).
     A mixer that layered on top of a preset would be two things fighting over
     the same gain.
5. Press **«Neste: Eksporter»** — step 3. Pick a **format** and leave the
   destination alone, then export.
   - **Expected:** the destination reads **«Samme mappe som opptaket»** and a
     `*_redigert.<fmt>` file lands next to the source (no "path must be
     absolute"). Each format states its trade-off («Liten fil. Passer for nett
     og deling.» / «Samme kvalitet som opptaket, mindre fil.» / «Ukomprimert.
     Størst fil.») and the size estimate «ca. N MB» is computed **from the
     file**, not from the settings. The progress bar moves for real. On playback
     the marked regions are gone and the level is on target.
   - **Expected:** when it is done, the receipt offers three ways on — «Vis i
     Finder», «Eksporter i annet format», «Til biblioteket».
   - The destination half of the old modal's honesty claim:
   - VERIFIED-BY: e2e/editor.spec.ts::eksportsteget er ærlig om hvor filen havner
     5b. **Cancel a long export** mid-render. — **Expected:** it stops within a
     second or two and says «Eksporten stoppet. Prøv igjen.» — not a frozen bar.
     5c. **Video file:** open an mp4 and leave **«Ta med video (MP4)»** on.
   - **Expected:** the mp4 out has both streams and honours the cuts, and the
     format row states the consequence rather than letting you pick an
     impossible one: «Formatet er MP4 så lenge videoen er med.»
   - On macOS the export tries VideoToolbox first, automatically (R2 removed
     the «Maskinvare-koding» toggle — it guarded nothing). If VideoToolbox
     refuses, the log shows a warning and the export completes in software
     anyway — a failed hardware render must never cost the user the export.
6. **P1 reopen-ability (cuts-draft sidecar):** with cuts marked, close the
   editor (or reselect another recording) then reselect the same recording.
   - **Expected:** the cut rows are back — restored **silently** from the
     cuts-draft sidecar the autosave writes every 2 s (with a 7-day freshness
     guard; the old «Fant lagrede kutt fra forrige økt»-banner + Gjenopprett
     button were dropped — the restore is automatic now, see
     editor/loader.ts). After a successful **Eksporter** the draft is deleted,
     so a later reopen restores nothing.
   - VERIFIED-BY: e2e/editor.spec.ts::unsaved cuts from a previous session come back on reopen
     ⚠️ **The standalone mastering panel is gone** (the old steps 6 and 7 — the
     `_mastert` file, «Forhåndsvis mastering (15 s)», the apply-with-progress and its
     Avbryt). `editor_master_apply` is no longer reached from anywhere; the preview
     mechanism survives as step 2's **«Lytt»**, which renders a 20-second sample
     through the same chain. See «Flater som ikke finnes lenger».

7. **E8 sermon-pick correction survives a reopen:** on a recording where the
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
8. ~~**E8 the companion signal reaches the same file**~~ — REMOVED (R2
   «Frivilligen først»): the AI companion and its `companionSuggestions` /
   `companionOutcomes` left the app 2026-08-23. What remains of this step:
   - (The `trimAdjustments` signal was only ever written by the review queue's
     publish step — gone in R1 — so nothing writes it; existing sidecars keep
     theirs. `docs/LEARNING.md` §Status says what is live, dormant and gone.)
   - With diagnostics ON, **Oppsett → Avansert → «Del anonym diagnostikk» →
     «Hva sendes»** should list `corrections` (a signal, a direction and a
     coarse band) with a count after step 7 — and the caption must NOT say
     «Ingenting å sende akkurat nå.» while it is on screen. With diagnostics OFF, do the same edit and
     confirm the collection stays empty: nothing is accumulated for someone who
     has not opted in, not even in memory.
   - The preview surface's half of this (the collection rendered, the caption
     honest while it is on screen) — the accumulation gating stays
     backend-verified:
   - VERIFIED-BY: e2e/telemetry-preview.spec.ts::a payload carrying corrections shows them, not «ingenting å sende»
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
     3b. **Leave step 1 while playing** (press «Neste: Lyd»), then come back.
   - **Expected:** playback STOPS on leaving — the same rule as leaving the
     page — the canvas is torn down and rebuilt, and the peaks are still in
     memory so the return draws without decoding anything again. A waveform that
     "freezes" after a step change is a canvas that remounted and left a draw
     loop painting into an element nobody sees.
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
  - **Expected:** the terminal logs a `PANIC: …` line and a new
    `crash-<millis>-<seq>.json` appears in `<app-data>/crashes/`.
  - ⚠️ The finding **SR-CRASH-01** ("Appen har krasjet") is still RAISED by
    `sundayrec-core::diagnostics`, but there is no screen that shows findings —
    read the crash file instead. See «Flater som ikke finnes lenger».

### Where the files land

| What                                 | Path                                                  | Kept                                                                          |
| ------------------------------------ | ----------------------------------------------------- | ----------------------------------------------------------------------------- |
| Live log                             | `<app-data>/logs/sundayrec.log`                       | rotates at 2 MB                                                               |
| Rotated logs                         | `<app-data>/logs/sundayrec.1.log` … `sundayrec.4.log` | 5 files total, ~10 MB ceiling                                                 |
| Panics (process hook + watched-task) | `<app-data>/crashes/crash-<millis>-<seq>.json`        | newest 20                                                                     |
| Supervised-task restarts             | `<app-data>/crashes/restart-<millis>-<seq>.json`      | newest 20, own ring — a flapping task cannot evict the panic that explains it |

`<app-data>` is the platform app-data dir Tauri resolves (macOS:
`~/Library/Application Support/…`; Windows: `%APPDATA%\…`).

### «Vis» / «Kopier» (Oppsett → Avansert → «Logg»)

- **Vis** reveals the live log file in Finder/Explorer (`logs_reveal` →
  `tauri_plugin_opener::reveal_item_in_dir`), falling back to opening the
  folder itself before the first line has been written.
- **Kopier** pulls the tail of the live file (`logs_tail`, capped
  server-side at 512 KB regardless of what the UI asks for) and copies it to
  the clipboard, with a toast confirming the copy (or that the log is still
  empty).
  - VERIFIED-BY: e2e/system-support.spec.ts::«Kopier siste logg» puts the tail on the clipboard and confirms
  - VERIFIED-BY: e2e/system-support.spec.ts::an empty log is called out instead of copying nothing
- **Expected:** the file is plain text, newest lines at the bottom; secrets
  (SMTP passwords, OAuth tokens — and defensively RTMP keys) are redacted on the writer
  thread before a line ever reaches disk — confirm none show up if you have
  any of those configured.

### ⚠️ Capture probe — backend only, no surface

`run_diagnostics` still runs a real ~2 s capture (and, with video on, grabs one
real camera frame) through the SAME backend a recording uses, and still refuses
politely while a recording is running or while the VU meter holds the input
(`capture_probe_skipped`). **Nothing in the shell calls it.** The command is
registered, tested in Rust and unreachable from the UI, so these three paths
cannot be exercised by hand at all until a screen exists. They are listed under
«Flater som ikke finnes lenger» with the rest, and the Rust tests are what keeps
them honest meanwhile.

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

**«Oppdater automatisk»** (Oppsett → Avansert → «Oppdateringer») is the privacy
gate PRIVACY.md promises:
off = the app never contacts the update server on its own (no startup check,
no hourly repeat — not even in the window where the settings blob is still
loading, the #11 race fixed in PR #101); on = one immediate check plus exactly
one hourly schedule; flipping it mid-session arms/cancels the schedule without
firing extra checks. The toggle and the timer live together on purpose — P1b
shipped without the timer, which meant **the shell did not check for updates by
itself at all**, and that is the same road the beta ring's kill-switch travels.
All four renderer paths are pinned in the browser tier:

- VERIFIED-BY: e2e/auto-update.spec.ts::off at startup: zero update_check even while settings load slowly (the #11 race)
- VERIFIED-BY: e2e/auto-update.spec.ts::on at startup: one immediate check, and the hourly repeat is scheduled
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling off while running stops the schedule and further checks
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling back on re-arms: an immediate check and a fresh schedule

**«Oppdateringer: Stabil / Beta»** (same card) must reach the STORE with its
value — the backend
(`update/mod.rs::current_channel`) reads sqlite, and the v0.11.1-beta.2 rig bug
#113 was precisely this select saying «Lagret ✓» while the machine silently
stayed on the stable feed. R4 removed the curated bridge that dropped it: the
renderer saves the full object through `settings_save`, sqlite is the one
store:

- VERIFIED-BY: e2e/update-channel.spec.ts::switching to beta reaches the store, not just the select
- VERIFIED-BY: e2e/update-channel.spec.ts::switching back to stable syncs too, and asks no question

1. Open **Oppsett → Avansert → «Oppdateringer»** and click **«Se etter
   oppdateringer nå»**.
   - **Expected in a default build:** a real check, not an error. Under
     `--no-default-features` the `update_check` command rejects with
     `feature_disabled`, and the panel has **no dedicated gate hint** for it —
     the rejection surfaces through the ordinary error path as «Kunne ikke
     sjekke for oppdateringer» (this runbook used to promise a calm «ikke
     bygget inn»-message that has never existed in this renderer). Seeing
     that error text in a default build with network is a BUG.
   - VERIFIED-BY: e2e/auto-update.spec.ts::a feature_disabled check surfaces as the ordinary error text
2. (dev build) **«Se etter oppdateringer nå»**.
   - **Expected:** the status reports **Du er oppdatert** — a dev build
     short-circuits the check (the `should_check` guard, unit-tested in
     `sundayrec-core::update`), so no error from a missing feed. The renderer
     half — an `upToDate` answer paints «Du er oppdatert» and retires any
     stale install button — is browser-tier pinned:
   - VERIFIED-BY: e2e/auto-update.spec.ts::an upToDate answer paints «Du er oppdatert» and retires stale buttons
3. (**release** build pointed at a real signed feed) check
   → **«Last ned og installer»** → **«Start på nytt og installer»**.
   - **Expected:** it walks `available` → `downloading {pct}` →
     `readyToInstall`; the relaunch applies the staged update. Needs the signed
     release + pubkey (NEEDS-RICHARD). // NETWORK/GUI-UNVERIFIED.
   - **Expected:** the announcement is ONE banner over whatever page you are on,
     keyed `update`, so «tilgjengelig» → «laster ned 40 %» → «klar» rewrites the
     same strip instead of stacking three. It is `warn`, not `bad`: an update
     waiting is not something that is wrong, and `bad` is `role="alert"`, which
     interrupts a screen reader mid-sentence.
   - **Expected:** if the relaunch does not happen, it says so («Omstarten
     skjedde ikke. Avslutt appen og åpne den på nytt …») rather than sitting on
     «Starter på nytt …» forever.

### Settings completeness (no feature)

R7 closed the gap between the Electron `store.ts` `Settings` and the Tauri model:
church profile (`churchName`/`responsiblePerson`), notification toggles
(`notifyStart`/`notifyStop`), and email config (`emailOnError`/`emailAddress`/
`emailSmtp`/`emailSmtpPort`/`emailSmtpUser` — the SMTP **password** stays in the
OS keychain via the `email` seam, never in the settings bag) plus the editor
intro/outro paths. All carry defaults + validation (`email_smtp_port` clamped
1..=65535) in `sundayrec-core::settings`.

1. Walk **Oppsett → «Hvilken kirke?»** (name + language), **«Hvem får beskjed
   hvis noe går galt?»** (the OS toggle, the address) and **Avansert →
   «E-postserver (SMTP)»** (host · port · user · from).
   - **Expected:** every field round-trips through `settings_save` (debounced)
     into SQLite and survives a relaunch; the port clamps to 1..=65535.
   - **Expected:** a save that FAILS rolls the control back to what is actually
     stored and toasts about it. The old shell left the new value standing, so
     the screen claimed one thing while sqlite held another and the change
     "disappeared" at the next launch. The two exceptions are deliberate: the
     alert address and the weekly time are explicit-save fields, and there a
     failed write does NOT throw away what you typed.
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
2. **R2 «Frivilligen først» — the dead fields are really gone, tolerantly.**
   Import a pre-v0.15 profile (one exported by v0.14 carries `hasLaunched`,
   `sampleRate`, `inputVolume`, the EQ/compressor/limiter fields, `avSync`,
   `minimizeToTray`, `videoBitrate`, `outputMode`, `trimSilence`,
   `showLiveLevels`, `separateAudioFormat`, `localAdaptivity`,
   `videoResolution`/`videoFramerate`/`videoContainer`/`videoCodec`/
   `videoEncoder`, `editorHwEncode`).
   - **Expected:** the import succeeds, every neighbour keeps its value, and a
     fresh export no longer carries any of them. **Oppsett → Tillegg → «Ta med
     kamera»** shows the on/off, the camera pick and «Behold separat lydfil» —
     nothing else — and Avansert has no «Hva appen har lagt merke til» / «Hva
     appen har justert» cards.
   - Import/export themselves are on **Avansert → «Innstillingsprofil»**, and
     the import asks first («Dette erstatter innstillingene på denne maskinen
     …») because it is the one settings action that is not undoable.
   - VERIFIED-BY: crates/sundayrec-core/src/settings.rs::legacy_blob_with_v015_dead_fields_imports_cleanly
   - VERIFIED-BY: legacy/renderer/migrate-legacy-settings-core.test.ts::drops the v0.15 dead settings fields tolerantly — the rest imports cleanly

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
   it happened to have installed. The **version line at the foot of the rail**
   must read exactly the tag `node scripts/promote-release.mjs beta
vX.Y.Z-beta.N` promoted (`RELEASE-CHECKLIST.md` §5d/§5e).
   - **Expected:** version matches. If it's a build behind, either the update
     hasn't reached this machine yet (propagation — up to an hour for an
     already-running app, immediate on relaunch or a manual **«Se etter
     oppdateringer nå»** under Avansert; see `ROLLBACK.md`) or **«Oppdater
     automatisk»** is off (§R7 above) — resolve which before treating today as
     a beta-ring result.
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

5. Read `<app-data>/last-recording.json` (§5b — the Diagnose screen it used to
   be read from no longer exists).
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

1. Open **Oppsett → Tillegg → «Ta med kamera»** → the camera picker.
   - **Expected:** `list_video_devices` returns the connected cameras, and the
     card states what the chosen one can actually deliver («Kameraet leverer
     maks 1080p · 30 bilder i sekundet»), or says plainly that it could not read
     that. No camera at all reads «Finner ikke noe kamera», with the advice to
     connect one or turn the option off. A missing ffmpeg / no devices yields
     empty lists, not an error. // HARDWARE-UNVERIFIED.
   - ⚠️ `diagnose_audio` (the audio-input name list, WASAPI loopback not ported)
     is no longer reached from any screen — see «Flater som ikke finnes
     lenger».

## What "passed" means

A green smoke test = §2–§6 all behave as the **Expected** lines say on a real
Mac with a real mic/camera, with no panic in the `tauri dev` stderr. Record any
deviation (which step, the stderr log, the OS permission state) when reporting
back.

**One thing to watch for that is specific to the new shell.** WKWebView's UA
string here carries **no `Safari` token**. Any library that branches on the UA
sees "unknown engine" and takes its slowest path — that is the exact fact behind
SundayEdit's 42× regression, invisible in Chromium. Nothing ships that branches
on it today, but if a screen feels slow in the real window and fast in the
browser tier, that is the first thing to suspect.

For any build that touches **recording, capture, the editor, the meter loop, or
boot ordering**, also run **§5b** (recording-health telemetry) and **§12b**
(editor stability loop) and paste the diagnose "Siste opptak" numbers into the
release notes — that is the standing gate that stops unverified audio/editor
fixes from shipping (see `RELEASE-CHECKLIST.md`).

---

## Flater som ikke finnes lenger

Fase B deleted the shipped shell and replaced it with «Frivilligen først»'s. Not
everything the old one had was rebuilt. This list is what a tester will look for
and not find, so that «I could not find the Diagnose screen» is answered here
instead of filed as a bug — and so that nothing on it can quietly be forgotten.
Every entry names what still works underneath, because in most cases the
BACKEND is untouched and only the surface is gone.

The standing list of what is owed lives in `docs/APP-SHELL.md` §«Etter byttet».

| gone                                                        | what is left, and where                                                                                                                                         |
| ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **The Diagnose modal** (Innstillinger → Lyd)                | `run_diagnostics` / `diagnose_audio` still run and are Rust-tested; nothing calls them. The recording numbers are in `<app-data>/last-recording.json` (§5b).    |
| **The capture/video probe's three paths**                   | still in the backend, refusals and all — unreachable by hand until a screen exists (§13).                                                                       |
| **Editing a recording's note**                              | owner decision (P3). An existing note still SHOWS on its row; `recording_update_note` is unreached.                                                             |
| **The Lyd / Video filter chips, sortable columns**          | search is kept and does the same job; «Video» survives as a fact on the row, not a filter (§6b).                                                                |
| **The month calendar + day detail + wake-diagnostics card** | two lists and one sentence on Avansert — «Flere tider og spesialopptak» and the wake line (§11).                                                                |
| **The export MODAL**                                        | the **Eksporter** step. Its destination honesty is re-pinned; the LEVEL row went with normalisation.                                                            |
| **The «Normaliser» toggle**                                 | removed with the mastering targets. Level is decided by the profile (Tale / Tale og musikk / Ingen) or by the mixer — never by two things at once (§12 step 4). |
| **The mastering apply panel** (`_mastert`)                  | `editor_master_apply` is unreached. The preview survives as step 2's «Lytt» (§12).                                                                              |
| **Intro/outro jingle rows**                                 | not built in any step. A P-restanse, not a removal on purpose — see APP-SHELL §P4b.                                                                             |
| **The editor's three TABS**                                 | three STEPS with the same names for two of them; the chosen step is NOT remembered across a reopen, deliberately — every open starts at «is this the sermon?».  |
| **`#modal-manual`** (source/camera/filename)                | those are Oppsett's answers now; start is one button (§5).                                                                                                      |
| **The «backend OK» header**                                 | the status line, which says one of five true things (§2).                                                                                                       |

### Claims this runbook used to point at a test for, and no longer can

These four VERIFIED-BY pointers were removed rather than moved. Each described a
screen that is gone, so a pointer at the replacement would have claimed coverage
of something nobody built.

- `e2e/system-support.spec.ts::the Diagnose modal shows the audio rows and the full backend report`
- `e2e/history.spec.ts::a note reaches the backend and shows on the row`
- `e2e/history.spec.ts::a filter that matches nothing says so in its own words`
- `e2e/editor.spec.ts::the export modal is honest about destination and level`

⚠️ Three of those four would have kept passing the `smoke-verified` gate, because
the new specs quote the old titles VERBATIM in comments explaining why they were
not carried over — and the gate matched anywhere in the file. It matches inside a
`test(…)` / `it(…)` / `describe(…)` title now, which is what a VERIFIED-BY
pointer always meant.
