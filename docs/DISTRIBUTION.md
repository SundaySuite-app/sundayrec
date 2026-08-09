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
> v0.4.x has been published as Latest this way (v0.11.0-beta.1 is the newest tag; v0.10.0 is the newest stable).

## Phase status

| Capability                   | State                                                                                                           |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Build macOS + Windows on tag | ✅ wired (`release.yml`)                                                                                        |
| macOS signing                | ✅ LIVE since ~2026-07-31 (`MAC_CERTS`/`MAC_CERTS_PASSWORD` secrets set)                                        |
| macOS notarization           | ⏸ deliberately disabled — Apple PLA 403; env lines commented out in `release.yml:146-155` pending re-acceptance |
| Windows signing              | ⏳ deferred (unsigned installer works; SmartScreen warns)                                                       |
| Auto-update (`latest.json`)  | ✅ LIVE since v0.4.x — plugin + pubkey + `uploadUpdaterJson` + `TAURI_SIGNING_*` secrets; feed verified in prod |

macOS builds are **signed but not notarized**: Gatekeeper warns on first
launch → right-click ▸ Open. Windows is unsigned → "More info" ▸ "Run anyway".
Notarization returns once the Apple Program License Agreement is re-accepted
(see below).

## The bundled ffmpeg (what we ship, and under what licence)

Every installer carries an `ffmpeg` + `ffprobe` sidecar. `scripts/fetch-ffmpeg.mjs`
downloads them per platform; `src-tauri/tauri.conf.json` bundles them via
`externalBin`.

**Version: 8.1.2** (since 2026-08-06). Before that we shipped **6.0** — a 2023
release — not by choice but because the sidecar came from the `ffmpeg-static`
npm package, which has been frozen there for years. That package (and
`@ffprobe-installer/ffprobe`) is gone.

**Not 9.0.** ffmpeg 9.0 shipped 2026-08-04, two days before this upgrade. A
church recording gets one take; a two-day-old major release is not what it runs
on. 8.1 is the current maintained release branch and is where we stay until
9.x has a few point releases behind it.

| Platform   | Source                                                                   | Archive                                                                                                                               |
| ---------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------- |
| macOS      | [ffmpeg.martin-riedl.de](https://ffmpeg.martin-riedl.de) release channel | `/download/macos/<arch>/<buildid>_8.1.2/{ffmpeg,ffprobe}.zip` — signed + notarized by the publisher                                   |
| Linux (CI) | same                                                                     | `/download/linux/<arch>/<buildid>_8.1.2/…` — never shipped; the ubuntu job needs a real binary so the real-ffmpeg smokes actually run |
| Windows    | [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) release **essentials**   | `/builds/packages/ffmpeg-8.1.2-essentials_build.zip` (`<stem>/bin/*.exe`)                                                             |

Both publish a `.sha256` next to the archive, and both keep versioned archives
around — the URLs above are pinned to an exact build, not to "latest", so a
rebuild a year from now fetches the same bytes.

**Licensing: GPL, unchanged.** Both builds are configured `--enable-gpl
--enable-version3`, same as the 6.0 build we shipped before, so SundayRec's
distribution obligations are what they already were: the GPL applies to the
ffmpeg binaries we redistribute, and anyone we hand an installer to is entitled
to the corresponding source. Point them at the publisher's build page (linked
above) and ffmpeg's own release tarball for 8.1.2. Nothing here is `--enable-nonfree`.
The gyan "essentials" build is a subset of the full one; it carries everything
this app asks for (native aac/flac/pcm/mjpeg, libmp3lame, libx264, dshow,
lavfi, and every filter we use is built-in).

**Installer size grew.** ffmpeg 8 static builds are simply larger than 6.0's:
the macOS pair went ~63 MB → ~131 MB, Windows ~110 MB → ~204 MB before
installer compression. That is inherent to the version, not to the source
chosen — there is no small 8.x static build. It is also the size the updater
downloads per release.

### Two-layer integrity pinning

1. **Archive** — the publisher's SHA-256 for each `.zip` is hard-pinned in
   `scripts/fetch-ffmpeg.mjs`. A mismatch aborts before anything is unpacked.
   Bumping `FFMPEG_VERSION` means re-reading those `.sha256` files.
2. **Bundled binary** — `scripts/ffmpeg-checksums.json` pins the SHA-256 of the
   exact unpacked bytes, keyed `<name>-<rust host triple>`. A mismatch is a hard
   failure; a **missing** key logs the computed hash and proceeds.

That missing-key behaviour is the pinning workflow: **macOS arm64 is pinned**
(computed on the owner's machine). **Windows and Linux are not yet** — their
hashes get printed by the first CI/release run that fetches them. Copy the two
`⚠ … computed <hash>` lines out of that run's "Fetch … sidecars" step into
`scripts/ffmpeg-checksums.json` and commit. Linux is optional (it never ships);
Windows should be pinned before the next release is published, exactly as the
6.0-era Windows pins were captured.

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
`uploadUpdaterJson: true` is set in `release.yml`, and the signing secrets are
in place — the `latest.json` feed is verified in prod (v0.11.0-beta.1 is the newest tag; v0.10.0 is the newest stable).
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

`.github/workflows/ci.yml` runs on `main` pushes, PRs, `v*` tags and manual
dispatch, as six parallel jobs: **check** (frontend lint/format/typecheck/
tests, version/i18n/smoke-pointer/reachability checks, Rust fmt/clippy/tests,
ts-rs bindings drift, feature-off clippy), **vad** (clippy + tests with the
`vad` feature), **build-smoke** (a `--no-bundle` compile of the whole app on
Linux), **e2e** (the Playwright renderer tier), **windows-check** (Windows
cargo check + clippy), and **audit** (npm + cargo advisories). No secrets
required.
