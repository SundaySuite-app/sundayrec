> **ARKIVERT (2026-07-08) — beskriver en tidligere tilstand.** Dette
> dokumentet ble skrevet 2026-06-01 mot v0.1.0 og motsier dagens konfig på de
> fleste punkter: versjonen er nå 0.4.2, `csp` er satt (ikke `null`),
> `assetProtocol` har en scoped allow/deny-liste, `plugins.updater`-blokken
> finnes (med `createUpdaterArtifacts: true` og `includeUpdaterJson: true` i
> release.yml), releases markeres ikke lenger pre-release, og CI trigges på
> `v*`-tagger (ikke push/PR). Se `.github/workflows/`, `src-tauri/tauri.conf.json`
> og `docs/NEEDS-RICHARD.md` for gjeldende tilstand.

# Release audit — pipeline as it stands

A concrete audit of the SundayRec release pipeline **as configured in the repo
today** (2026-06-01, after the `src/design` redesign + new-backend work). It
records what `tauri.conf.json` and the workflows declare, the secrets/env they
consume, what a local `tauri build` needs, and the gaps/mismatches that would
block or degrade a real release. Companion to `DISTRIBUTION.md` (the how-to) and
`NEEDS-RICHARD.md` (the account/key checklist).

## What `tauri.conf.json` declares

`src-tauri/tauri.conf.json`:

- `productName`: `SundayRec` (line 3)
- `version`: **`0.1.0`** (line 4) — matches `src-tauri/Cargo.toml` `version =
"0.1.0"` and `package.json` `"version": "0.1.0"`. ✅ all three in sync.
- `identifier`: `no.sundayrec.app` (line 5)
- `build`: `devUrl` `http://localhost:1420`, `frontendDist` `../dist`,
  before-dev `npm run dev`, before-build `npm run build` (lines 6–11).
- `bundle.active: true`, `bundle.targets: "all"` (lines 27–28) → builds every
  installer kind for the host OS.
- `bundle.externalBin`: `["binaries/ffmpeg", "binaries/ffprobe"]` (line 29) — the
  ffmpeg/ffprobe sidecars. These must be fetched into `src-tauri/binaries/` per
  target triple **before** bundling (CI does `node scripts/fetch-ffmpeg.mjs` /
  `npm run ffmpeg`).
- `bundle.icon`: the standard 5-icon set (lines 30–36).
- `app.windows`: one 1180×760 window, min 960×640 (lines 13–20).
- `app.security.csp: null` (line 24).

### Not declared (gaps)

- **No `plugins` block at all** → **no `plugins.updater`**, so **no `pubkey` and
  no `endpoints`**. Auto-update cannot resolve a feed even if a keypair existed.
- **No `plugins.deep-link`** scheme registration (`sundayrec://`) and no macOS
  `Info.plist` `CFBundleURLTypes` — the tray/deep-link `on_open_url` listener is
  wired in code but the OS won't deliver the scheme (see NEEDS-RICHARD PU-2).
- **No per-state tray icons** bundled (idle/recording/error) — the tray reuses the
  default window icon (NEEDS-RICHARD PU-2).

## What the workflows do

### `.github/workflows/release.yml`

- Trigger: push of a `v*` tag (or `workflow_dispatch`) (lines 25–29).
- Matrix: `macos-latest` (Apple Silicon / arm64) + `windows-latest` (lines 38–45).
  No Intel/universal mac, no Linux release target.
- Steps: checkout → Node 22 → stable Rust → cargo cache → `npm ci` →
  `node scripts/fetch-ffmpeg.mjs` → `tauri-apps/tauri-action@v0` (lines 49–100).
- Output: a **draft + prerelease** GitHub Release with the installers
  (`releaseDraft: true`, `prerelease: true`, lines 96–97). Publishing is a
  **separate manual step** (the documented Electron deploy gotcha).
- `includeUpdaterJson: false` (line 99) — **updater feed not emitted**.

### `.github/workflows/ci.yml`

- Trigger: push/PR on `main` (lines 3–7), concurrency-cancel.
- Ubuntu runner; installs the Tauri/GTK/ALSA dev libs (lines 38–47); fetches the
  ffmpeg sidecars; runs `npm run lint`/`format:check`/`typecheck`/`test`, then
  Rust `fmt --check` + `clippy --workspace -- -D warnings` + `cargo test
--workspace`, a **ts-rs bindings drift check** (fails if `src/lib/bindings`
  changed, lines 81–88), and a `tauri build --no-bundle` Linux compile.
- No secrets required.

## Secrets / env the pipeline expects (by name)

All consumed in `release.yml` env (lines 73–85). Absent secrets do **not** fail
the build — `tauri-action` skips signing and produces an unsigned installer.

| Secret / env                         | Used for                                   | Status        |
| ------------------------------------ | ------------------------------------------ | ------------- |
| `GITHUB_TOKEN`                       | create the draft Release                   | auto-provided |
| `APPLE_CERTIFICATE`                  | base64 of the Developer ID `.p12`          | **missing**   |
| `APPLE_CERTIFICATE_PASSWORD`         | the `.p12` export password                 | **missing** ¹ |
| `APPLE_SIGNING_IDENTITY`             | `Developer ID Application: … (784GN847G4)` | **missing**   |
| `APPLE_ID`                           | notarization account email                 | **missing**   |
| `APPLE_PASSWORD`                     | app-specific password for notarytool       | **missing** ² |
| `APPLE_TEAM_ID`                      | `784GN847G4`                               | known, unset  |
| `TAURI_SIGNING_PRIVATE_KEY`          | updater signing key                        | **missing** ³ |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | updater key password                       | **missing** ³ |

¹ Per project notes the Desktop `.p12` has the **wrong password** — re-export from
Keychain Access first.
² The earlier app-specific password was **leaked in chat** — revoke + regenerate
before storing.
³ Even with these set, the updater stays off until `tauri.conf.json` gains a
`plugins.updater` block and `release.yml` flips `includeUpdaterJson: true`.

No Google/SMTP/Anthropic secrets are consumed by the **release build** — those are
runtime feature config (see GOOGLE-OAUTH-SETUP.md / NEEDS-RICHARD.md).

## What a local `tauri build` needs

When CI is unavailable (the Actions billing block — see below), a release can be
produced locally on a Mac:

1. `npm ci`
2. `node scripts/fetch-ffmpeg.mjs` (or `npm run ffmpeg`) — populates
   `src-tauri/binaries/ffmpeg-<triple>` + `ffprobe` so `externalBin` resolves.
3. `npm run tauri build` — bundles for the host OS (`bundle.targets: "all"`).
4. Signing/notarization needs the same Apple env vars exported locally (or a
   Keychain identity) — otherwise the `.app`/`.dmg` is unsigned.
5. The build uploads nothing; attach the artifacts to a GitHub Release manually
   (and remember releases publish as **draft/prerelease** — flip to published).

## Gaps / mismatches found

Ordered roughly by release impact. File:line references are to this repo.

1. **GitHub Actions billing block (release-blocking, infra).** Per the suite
   notes the account's Actions billing/spending limit is blocked, so **neither
   `ci.yml` nor `release.yml` can run**. The build itself runs on Actions, so this
   blocks the whole CI-driven release until payment is fixed; local `tauri build`
   is the workaround.

2. **No code-signing / notarization secrets (release-blocking, accounts).** All
   six `APPLE_*` secrets are unset (`release.yml:80–85`), so a release build is
   **unsigned** (Gatekeeper-blocked on download). The `.p12` password is wrong and
   the app-specific password was leaked — both need remediation before the secrets
   can be set (NEEDS-RICHARD release checklist items 2–3).

3. **Updater not wired (degrades release, config).** `tauri.conf.json` has **no
   `plugins.updater` block** (no `pubkey`, no `endpoints`) and `release.yml:99`
   sets `includeUpdaterJson: false`. A shipped build has **no auto-update path** —
   users must manually reinstall. Needs the keypair + the config block + the flip
   (NEEDS-RICHARD release checklist item 4).

4. **Stale "Phase 9" comments in `release.yml` (cosmetic, not blocking).** The
   header (lines 12–21) and the line-98 comment still describe the updater as
   "Phase 9 … intentionally OFF" and reference `MIGRATION-TAURI2.md` phasing; the
   redesign superseded that narrative but the wiring (env at 76–77) is correct.
   Update the prose when the updater lands so the workflow self-documents.

5. **Mac coverage is arm64-only (scope, not blocking).** `release.yml:38–45`
   builds `macos-latest` (Apple Silicon) only — no Intel or universal binary, so
   Intel Macs get no native build. The workflow comment (lines 39–41) already
   flags this as a follow-up; the ffmpeg sidecar must match the chosen arch.

6. **Deep-link scheme not registered (feature gap, not build-blocking).** No
   `plugins.deep-link` in `tauri.conf.json` and no `CFBundleURLTypes` Info.plist
   entry, so the OS won't deliver `sundayrec://` URLs to the app even though the
   handler is wired (NEEDS-RICHARD PU-2).

7. **Version strings OK.** `tauri.conf.json:4`, `Cargo.toml`, and `package.json`
   all read `0.1.0` — no mismatch. The redesign did not bump the version;
   re-confirm the three stay in sync at the next `npm version` / tag (the
   `release.yml` header + DISTRIBUTION.md both call this out).

## Top release blockers

1. **GitHub Actions billing block** — no CI/release runs until payment is fixed.
2. **Apple signing + notarization** — re-export the `.p12` (wrong password),
   revoke + regenerate the leaked app-specific password, then set the six
   `APPLE_*` secrets; until then every build is unsigned.
3. **Updater unwired** — add the keypair + a `plugins.updater` block (pubkey +
   endpoints) to `tauri.conf.json` and flip `includeUpdaterJson: true`, or ship
   with no auto-update.
