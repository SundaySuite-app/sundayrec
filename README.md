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

## What it does

Scheduled + manual audio/video recording (crash-safe MKV capture with remux at
finalize, reconnect/split/pre-roll), an editor (cut plan, mastering presets,
chapters, export), whisper transcription, live RTMP streaming with overlays,
cloud backup + podcast publishing, OS wake-from-sleep scheduling, and a
menubar/tray — all behind default-off cargo features where a native dependency
is involved.

## Architecture

- **`crates/sundayrec-core`** — the pure domain core: GUI-free, Tauri-free,
  fs/network-free, clock injected by the caller. Every recorder/editor/
  streaming/whisper/publish _decision_ lives here and is unit-tested
  (~1000 tests). Ported knowledge from the Electron app (hardened ffmpeg
  arguments, device parsers, error classification, silence/watchdog logic) —
  rebuilt cleanly, not copied.
- **`src-tauri`** — the thin Tauri 2 shell: commands, events, processes,
  keyring, SQLite (sqlx), tracing. Impure paths that need a device/network/GUI
  are annotated `HARDWARE/NETWORK/GUI-UNVERIFIED` and covered by
  `docs/SMOKE-TEST.md`. Optional subsystems are gated behind default-off
  features (`editor`, `whisper`, `streaming`, `publish`, `email`, `tray`,
  `ndi`, `bridge`, `updater`, `asio`).
- **`legacy/`** — the shipping frontend: the ported Electron vanilla-TS
  renderer (`legacy/renderer` is the Vite root), its `types`/`shared`/`locales`
  trees, and `legacy/bindings/` — the committed ts-rs TypeScript bindings
  generated from the Rust types (`npm run bindings`; CI fails if they drift).
- **`docs/`** — living docs: migration plan (`MIGRATION-TAURI2.md`), completion
  state (`COMPLETION.md`), hardware smoke tests (`SMOKE-TEST.md`), the
  account/key checklist (`NEEDS-RICHARD.md`), and the current improvement
  backlog (`BACKLOG-AUDIT-2026-07-07.md`).

The original Electron app remains the **behavioural specification**, not a
template. See [`docs/MIGRATION-TAURI2.md`](docs/MIGRATION-TAURI2.md) for the
phase history.

## Build & test

```bash
npm install                          # toolchain + Tauri JS plugins
npm run ffmpeg                       # fetch ffmpeg/ffprobe sidecars (checksum-verified)

# Frontend
npm run dev                          # vite dev server (renderer only)
npm run tauri dev                    # full app
npm run build                        # tsc + vite production build

# Rust
cargo check --workspace              # type-check everything
cargo test -p sundayrec-core         # domain-core unit tests (fast, no GUI)
cargo test --workspace               # all Rust tests
npm run bindings                     # regenerate ts-rs bindings → legacy/bindings/

# The full gate (same steps as CI): prettier + eslint + tsc + vitest +
# version-sync + rustfmt + clippy -D warnings + cargo test
npm run check
bash scripts/ci-local.sh             # CI mirror incl. bindings drift + build
```

CI (`.github/workflows/ci.yml`) runs the same gate plus a dependency audit on
every push to `main`, every PR, `v*` tags, and manual dispatch (the repo is
public, so Actions minutes are free). Releases are built and
published as drafts by `.github/workflows/release.yml` (macOS arm64 + Windows;
signing/notarization activate when the secrets in `docs/NEEDS-RICHARD.md`
exist).
