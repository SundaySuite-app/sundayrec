# Distribution & auto-update

SundayRec ships as installers for macOS and Windows via GitHub Releases. The
release pipeline (`.github/workflows/release.yml`) is **live**: releases are
macOS-signed and auto-updating; the one remaining gap is **notarization**
(deliberately disabled — see below). This doc is the checklist.

## How it works

1. You bump the version (in `package.json`, `src-tauri/tauri.conf.json`, and
   `src-tauri/Cargo.toml` — keep them in sync) and push a tag `vX.Y.Z`.
2. `release.yml` builds on macOS (Apple Silicon) and Windows, fetches the
   ffmpeg/ffprobe sidecars, signs macOS (notarization is deliberately disabled
   pending the Apple agreement — see below), and creates a **draft** GitHub
   Release with the installers attached.
3. You review the draft and publish it as **Latest**.

> **Deploy gotcha (same as the Electron SundayRec):** the build uploads as a
> **draft** (with `prerelease: false`). "Publishing" is a separate manual step —
> review the draft, then publish it so it becomes **Latest**. A
> built-but-unpublished release is not served to anyone (the updater feed only
> sees published releases). This flow is proven in prod: every release since
> v0.4.x has been published as Latest this way (v0.8.0 is current).

## Phase status

| Capability                   | State                                                                                                            |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Build macOS + Windows on tag | ✅ wired (`release.yml`)                                                                                         |
| macOS signing                | ✅ LIVE since ~2026-07-31 (`MAC_CERTS`/`MAC_CERTS_PASSWORD` secrets set)                                         |
| macOS notarization           | ⏸ deliberately disabled — Apple PLA 403; env lines commented out in `release.yml:146-155` pending re-acceptance  |
| Windows signing              | ⏳ deferred (unsigned installer works; SmartScreen warns)                                                        |
| Auto-update (`latest.json`)  | ✅ LIVE since v0.4.x — plugin + pubkey + `includeUpdaterJson` + `TAURI_SIGNING_*` secrets; feed verified in prod |

macOS builds are **signed but not notarized**: Gatekeeper warns on first
launch → right-click ▸ Open. Windows is unsigned → "More info" ▸ "Run anyway".
Notarization returns once the Apple Program License Agreement is re-accepted
(see below).

## Required GitHub repository secrets

Settings → Secrets and variables → Actions → New repository secret.

### macOS code signing + notarization

You already did this for the Electron SundayRec — the same Developer ID cert
applies, and the workflow reuses the Electron-era secret names (mapped to
tauri-action's `APPLE_*` env vars in `release.yml:143-144`).
`APPLE_SIGNING_IDENTITY` is hardcoded in the workflow, not a secret.

| Secret                                                       | Value                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `MAC_CERTS`                                                  | ✅ set. Base64 of the "Developer ID Application" cert exported as `.p12`: `base64 -i cert.p12 \| pbcopy` (maps to `APPLE_CERTIFICATE`). Releases are signed since ~2026-07-31.                                                                                                                             |
| `MAC_CERTS_PASSWORD`                                         | ✅ set. The password from the `.p12` export (maps to `APPLE_CERTIFICATE_PASSWORD`).                                                                                                                                                                                                                        |
| `APPLE_ID` / `APPLE_APP_SPECIFIC_PASSWORD` / `APPLE_TEAM_ID` | ⏸ **Notarization — currently unused.** The env lines are commented out in `release.yml:146-155` because Apple's notary service returns 403 until the updated Program License Agreement is accepted on developer.apple.com (team 784GN847G4). Re-enable by uncommenting those lines once the PLA is signed. |

### Auto-update signing (✅ LIVE since v0.4.x)

The updater is **live in published releases**: the plugin is installed, the
public key + `endpoints` are in `tauri.conf.json` under `plugins.updater`,
`includeUpdaterJson: true` is set in `release.yml`, and the signing secrets are
in place — the `latest.json` feed is verified in prod (v0.8.0 is current).
Nothing here remains to set up; the only outstanding release-pipeline gap is
macOS **notarization** (previous section).

| Secret                               | Value                                                                                                          |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | ✅ set. Contents of `~/.tauri/sundayrec_updater.key` (key-id `4f08a2f48edd9a17`) — keep the local backup safe. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | ✅ set. The password chosen for that key (empty string if generated without one).                              |

> Keep the private key safe — losing it means existing installs can no longer
> auto-update (they'd need a manual reinstall with a new key).

### Windows code signing (deferred)

Not wired yet — the Windows installer is currently unsigned (it works, but
SmartScreen warns). Adding an EV/OV code-signing cert + the matching secrets is
a later task.

## Cut a release

```bash
# bump version in package.json AND src-tauri/tauri.conf.json AND src-tauri/Cargo.toml
git tag v0.1.0
git push origin v0.1.0
# → watch the run, review the draft Release, then publish it.
```

## CI (every push / PR)

`.github/workflows/ci.yml` runs on `main` pushes and PRs: frontend
lint/format/typecheck/tests, Rust fmt/clippy/tests across the workspace, a
ts-rs bindings drift check, and a `--no-bundle` compile of the whole app on
Linux. No secrets required.
