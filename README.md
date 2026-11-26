# SundayRec

Recording for church services — the Tauri 2 rebuild of the Electron SundayRec,
on the same foundation as the rest of the Sunday suite (Tauri 2 + Rust).

> **This is the official SundayRec.** It supersedes the original Electron app
> (versions ≤ 4.55.0), which is no longer maintained or distributed.
>
> **Upgrading from 4.x:** download the latest installer from
> [Releases](https://github.com/SundaySuite-app/sundayrec/releases/latest). The
> new app replaces the old one. Your **recordings are safe** (they live in your
> chosen save folder); app **settings must be re-entered** (the new version
> stores them separately).

> **Running this on a Sunday without a developer in the room?** This README is
> for people working on the code. The old volunteer guide
> ([`docs/archive/FRIVILLIG.md`](docs/archive/FRIVILLIG.md), Norwegian) describes
> features that «rent bord» removed and is archived until the core has been rig
> tested; see [`docs/PLAN.md`](docs/PLAN.md).

## What it does

Four pages, one job each:

- **Ta opp** — the big record button, the source and its level, the camera.
  Crash-safe capture straight to WAV with reconnect, split and pre-roll.
- **Opptak** — the recordings list. A click opens the editor on the same page:
  a waveform with two handles, cut away the rest, and one «Podkastklar lyd»
  switch (−16 LUFS). «Eksporter» is a sheet with optional intro/outro.
- **Plan** — a week view with draggable blocks for the weekly recordings, auto
  start with the machine, and OS wake-from-sleep so a scheduled recording
  happens with nobody there.
- **Innstillinger** — sound and channels, camera, save folder and format,
  notifications, language (Norwegian or English), updates, privacy and log.

First run is three steps: sound card and channels (with a live meter), save
folder, first weekly recording. A native notification on the machine tells you
when a take fails or goes silent; the app itself uploads nothing.

SundayRec is deliberately a small RECORDING app. «Rent bord» (v0.31.0) cut it
back to this core: no learning from your corrections, no sermon finder, no
diagnostics/test recordings, no «Legg ut», no church profile or calendar, no
special recordings, and only Norwegian and English. The full list of what was
removed, and why it should stay out, is in [`docs/PLAN.md`](docs/PLAN.md).

## Architecture

- **`crates/sundayrec-core`** — the pure domain core: GUI-free, Tauri-free,
  fs/network-free, clock injected by the caller. Every recorder/editor
  _decision_ lives here and is unit-tested
  (~1100 tests as of R2; the `src-tauri` shell carries a further ~700).
  Ported knowledge from the Electron app (hardened ffmpeg
  arguments, device parsers, error classification, silence/watchdog logic) —
  rebuilt cleanly, not copied.
- **`src-tauri`** — the thin Tauri 2 shell: commands, events, processes,
  SQLite (sqlx), tracing. Impure paths that need a device/network/GUI
  are annotated `HARDWARE/NETWORK/GUI-UNVERIFIED` and covered by
  `docs/SMOKE-TEST.md`. Subsystems are cargo features; `default` is
  `editor`, `tray`, `updater`. Default-OFF and opt-in: `asio`.
  `src-tauri/Cargo.toml`'s `[features]` block is the authority — it
  explains why each one sits where it does. Since R2 the workspace has **no
  C/C++ toolchain dependency** (whisper-rs/libwhisper was the only one): a
  stock Rust toolchain plus the ffmpeg sidecar builds everything except the
  Windows `asio` feature (Steinberg SDK, see `docs/BUILD_ASIO.md`).
- **`app/`** — the shipping frontend: a Preact shell and the Vite root. Four
  pages — **Ta opp · Opptak · Plan · Innstillinger** — plus first run
  (`pages/firstrun`). Signals at module scope for state, CSS Modules over one
  token file for every colour, JSX from the compiler (no Babel plugin), and one
  door into Tauri: `window.api`. The long, historical description of the shell
  before «rent bord» is [`docs/archive/APP-SHELL.md`](docs/archive/APP-SHELL.md).
- **`app/lib/`** — the shared INVENTORY the shell reaches through the `@lib/*`
  alias, and the only part of the old Electron renderer that survived fase B:
  the IPC shim (`api-shim.ts`, the one file in the repo that imports Tauri's
  `invoke`), the locale loader, and the pure `*-core` decision modules the
  screens are assembled from. It is a **verbatim port** and is treated as one —
  its own ESLint block, left out of `prettier`, tightened file by file as each
  is opened for a real reason. It never imports the shell around it; the shell
  never reaches it by relative path.
- **`legacy/`** — what is left when the inventory moved out: `legacy/bindings/`,
  the committed ts-rs TypeScript bindings generated from the Rust types
  (`npm run bindings`; CI fails if they drift), the two locale catalogues (`no`, `en`) in
  `legacy/locales/`, and the `types`/`shared` trees. `app/` reaches all of it
  through the `@legacy/*` alias. Generated code and data, in other words — the
  Electron vanilla-TS renderer that used to live here is gone entirely.
- **`docs/`** — start at [`docs/README.md`](docs/README.md), the map of every
  document, and [`docs/PLAN.md`](docs/PLAN.md), the one register of what is
  left to do and whose it is. Beside them: hardware smoke tests
  (`SMOKE-TEST.md`), a one-day rig checklist (`RIG-DAY.md`) and the release
  runbooks. Session reports and
  superseded snapshots live in `docs/archive/`.

The original Electron app remains the **behavioural specification**, not a
template. See [`docs/archive/MIGRATION-TAURI2.md`](docs/archive/MIGRATION-TAURI2.md) for the
phase history.

## Build & test

```bash
npm install                          # toolchain + Tauri JS plugins
npm run ffmpeg                       # fetch ffmpeg/ffprobe sidecars (checksum-verified)

# Frontend — the app shell in app/, on :1420
npm run dev                          # vite dev server (frontend only)
npm run tauri dev                    # full app, in a real WKWebView
npm run build                        # tsc + vite → dist/ (what a release bundles)

# Rust
cargo check --workspace              # type-check everything
cargo test -p sundayrec-core         # domain-core unit tests (fast, no GUI)
cargo test --workspace               # all Rust tests
npm run bindings                     # regenerate ts-rs bindings → legacy/bindings/

# Browser tier (Playwright) — UI journeys, NOT part of `npm run check`
npm run e2e:install                  # one-off: download the chromium binary
npm run e2e                          # starts vite itself, then runs e2e/
npm run e2e:headed                   # …watching it happen
npm run e2e:ui                       # …in Playwright's picker/inspector

# The local gate. See package.json's `check` script for the exact chain —
# it is not reproduced here, because a prose copy goes stale silently.
# CI runs it PLUS several steps this does not (see ci.yml).
npm run check
bash scripts/ci-local.sh             # closer CI mirror incl. bindings drift + build
```

### The browser tier

`npm run check` is node-env-only by design, which leaves every rendered screen
untestable. `e2e/` closes that: the shell boots in a plain browser
(`api-shim.ts` falls back on every rejected `invoke`), `?goto=<page>`
deep-links into any screen, and the E5.1 fixture seam
(`window.__SUNDAYREC_FIXTURES__`, keyed by Tauri command name) lets a spec drive
it with populated state instead of empty ones. No Tauri, no ffmpeg, no device —
Playwright starts the Vite server itself, so `npm run e2e` is the whole command.

The spec files under `e2e/` are the inventory — one file per surface, each with
a header saying exactly what it pins and why (boot and the shell, the three
destinations, first run and consent, the recorder seam, the library and the
trash, the editor's three steps, settings and the shell→sqlite settings seam,
auto-update, the update channel, system support, telemetry preview).
`npx playwright test --list` gives the current count; an enumerated prose copy
here rotted twice, so there isn't one any more. ONE project since fase B: there
were two while both shells existed, and the app copies carried byte-identical
test titles so the `VERIFIED-BY:` pointers in `docs/SMOKE-TEST.md` could move by
changing nothing at all. Those pointers are gate-checked
(`npm run smoke-verified`), and the gate now requires them to resolve to a real
`test(…)` title rather than to the name appearing anywhere in the file.

Deliberately not wired into `npm run check`: it needs a browser binary
(`npm run e2e:install`, ~95 MB) that the local gate should not require, and it
tests a different thing at a different cadence. See the header of
`playwright.config.ts` for the reasoning behind each config choice.

CI (`.github/workflows/ci.yml`) runs five parallel jobs on every push to
`main`, every PR, `v*` tags, and manual dispatch (the repo is public, so
Actions minutes are free): **check** (the `npm run check` chain plus a
feature-off `cargo clippy`), **build-smoke** (a `--no-bundle` Tauri build), **e2e** (the
Playwright tier), **windows-check** (Windows `cargo check` + clippy, incl. an
asio-less approximation of the release feature combo), and **audit**
(npm + cargo dependency audit). Releases are built and published as drafts by
`.github/workflows/release.yml` (macOS arm64 + Windows). macOS **signing**
activates once the `MAC_CERTS`/`MAC_CERTS_PASSWORD` secrets exist;
**notarization** is gated on the repo _variable_ `NOTARIZE_MAC` (off by
default — Apple's Program License Agreement is still pending), read by the
`[notarize-switch]` step in `release.yml`; flipping it to `true` re-enables
notarization with no commit needed. See `docs/RELEASE-CHECKLIST.md` §2/§2a.

## Lisens

SundayRec er lisensiert under [MIT-lisensen](LICENSE).

De medfølgende `ffmpeg`/`ffprobe`-sidecar-binærene er GPL-bygg fra tredjepart
(lastet ned og sjekksum-verifisert av `scripts/fetch-ffmpeg.mjs`). De kjøres
som SEPARATE prosesser og lenkes ikke inn i appen, så appens egen kode kan
være MIT; binærenes egen lisens følger binærene.
