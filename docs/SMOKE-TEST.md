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
npm install            # pulls ffmpeg-static + @ffprobe-installer/ffprobe
npm run ffmpeg         # copies them to src-tauri/binaries/<name>-<host-triple>
ls src-tauri/binaries  # expect ffmpeg-… and ffprobe-… for your host triple
```

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
npm run check          # lint + typecheck + vitest + clippy + cargo test
cargo build            # debug build of the Tauri binary
npm run build          # tsc + vite frontend build
```

All four must be green before a smoke test is meaningful. As of this runbook the
gate is green: 332 Rust tests + the vitest suite + clippy `-D warnings`.

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

Expect at boot: `SundayRec backend ready (db at …/sundayrec.sqlite)` and (with no
Google client configured) `cloud upload worker idle: Google OAuth client not
configured`. The cloud worker idling cleanly with no config is itself a thing to
verify here — there should be **no** repeated cloud log spam.

---

## 3. Pick an input device → VU meter moves [HW]

1. Open the device picker.
2. Choose a microphone from the audio input list (enumerated via ffmpeg
   `avfoundation` on macOS).
   - **Expected:** the list is non-empty and names match your real inputs.
3. Speak / tap the mic.
   - **Expected:** the VU meter (cpal-driven, per channel) moves in real time. A
     dead-flat meter while you speak = the OS denied mic access (see §0) or the
     wrong device is selected.

---

## 4. Camera preview [HW]

1. Select a camera/video device.
   - **Expected:** the MJPEG preview (ffmpeg avfoundation → base64 frames over the
     Tauri event channel) shows live video within a second or two.
2. No preview + the app still alive = check camera permission (§0). App vanishes
   = permission string missing/denied and the OS killed it.

---

## 5. Record 30 s → stop → history row [HW]

1. Start a recording with mic (+ camera if testing A/V).
   - **Expected:** status flips to recording; with `RUST_LOG=debug` you see ffmpeg
     `size=` progress lines being parsed.
2. Let it run ~30 seconds, talking so the silence-watcher does **not** fire.
3. Stop the recording.
   - **Expected:** a graceful stop (a `q` is sent to ffmpeg's stdin, not a kill),
     and a **new history row** appears with a plausible **duration (~30 s)** and
     **file size (> 0)**.
4. Confirm the file exists on disk at the path shown.

> [HW] Reconnect/split/preroll fusion paths are wired but unproven on a device.
> A basic single-segment 30 s capture is the smoke-test target here.

---

## 6. Add a note → reveal in folder [HW]

1. On the new history row, add a note and save.
   - **Expected:** the note persists (it round-trips through `recording_update_note`
     into SQLite; relaunching the app shows it again).
2. Use "reveal in folder" / open.
   - **Expected:** the OS file manager opens at the recording (via the `opener`
     plugin — capability `opener:allow-open-path` is granted).

---

## 7. (Optional) Cloud connect + upload [HW][NET]

Requires a Google Desktop OAuth client — see
[`docs/GOOGLE-OAUTH-SETUP.md`](GOOGLE-OAUTH-SETUP.md) to create one and set
`SUNDAYREC_GOOGLE_CLIENT_ID` (+ optional `SUNDAYREC_GOOGLE_CLIENT_SECRET`) before
launching:

```bash
export SUNDAYREC_GOOGLE_CLIENT_ID="…apps.googleusercontent.com"
export SUNDAYREC_GOOGLE_CLIENT_SECRET="…"   # optional for Desktop clients
npm run tauri dev
```

1. Trigger **cloud connect** (Drive).
   - **Expected:** the system browser opens Google's consent screen; after you
     approve, the loopback redirect (`http://127.0.0.1:<ephemeral-port>`)
     completes and the service shows as connected. A "client not configured"
     error here means the env var didn't reach the process.
2. **Enqueue a backup** of the recording from §5, then watch the upload.
   - **Expected:** the queue entry transitions through uploading → done; the file
     appears in Google Drive (`drive.file` scope = only files this app created).
   - With `RUST_LOG=sundayrec=debug` the worker logs each resumable chunk.

> [NET] The whole cloud worker (`reqwest` PUTs, keychain token read, chunk reads)
> is NETWORK-UNVERIFIED — only the decision logic (queue ordering, chunk math,
> token/error classification) is unit-tested. This step is the first real
> exercise of the wire path.

---

## What "passed" means

A green smoke test = §2–§6 all behave as the **Expected** lines say on a real
Mac with a real mic/camera, with no panic in the `tauri dev` stderr. §7 is a
bonus that needs a Google client. Record any deviation (which step, the stderr
log, the OS permission state) when reporting back.
