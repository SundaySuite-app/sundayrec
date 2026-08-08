# Security Policy

SundayRec is a Tauri 2 desktop app for recording church services. This
document explains how to report a vulnerability, what's supported, and the
threat model the app's controls are designed against.

## Reporting a vulnerability

Please report security issues **privately**, not in a public issue:

Use this repository's Security tab → "Report a vulnerability" (GitHub
private security advisories:
https://github.com/SundaySuite-app/sundayrec/security/advisories/new). That
opens a private discussion with the maintainer before anything is public,
and it is the only reporting channel — there is no security mailing address.
If you cannot use advisories, open a regular issue asking for contact
**without** describing the vulnerability, and the maintainer will follow up
privately.

Please include what you found, the affected version, and reproduction steps.
This is a small, single-maintainer project — expect an initial response
within a few days, not an SLA.

## Supported versions

Only the **latest release on your channel** is supported. SundayRec
auto-updates on one of **two** operator-selectable channels —
`stable` (the default) and `beta` (`UpdateChannel` in
`crates/sundayrec-core/src/settings.rs`). The channel is a per-machine
setting, so the feed URL is chosen at RUN time
(`src-tauri/src/update/mod.rs`), not taken from `tauri.conf.json`; the config
still names the stable feed as a fallback for any path that bypasses that
code. There is no LTS branch and no backporting of fixes to older versions.
Please update before reporting an issue that may already be fixed.

## Threat model

SundayRec typically runs on a **volunteer-operated machine in a church**,
often started once and left unattended for the length of a service. The
operator is not a security professional, and the machine is not IT-managed.
Trust boundaries the app has to defend at:

- **`sundayrec://` deep links** — inbound custom-scheme URLs from other apps
  or a web page, which can be triggered without the operator's intent.
- **Media files and their sidecars** — recordings, intro/outro clips,
  subtitle files, transcripts — paths and content that ultimately come from
  outside the process (a picked file, an imported recording, another Sunday
  app).
- **User-configured URLs** — webhook, SMTP, and integration API endpoints the
  operator types in, which can point anywhere, including the local network.
- **The update feed** — a **first-party Cloudflare Worker** at
  `https://updates.sundaysuite.app/v1/update/{stable|beta}`, which the
  auto-updater polls, plus the signed artifact it downloads and installs.
  This is a trust boundary that MOVED: the feed used to be GitHub's
  `releases/latest/download/latest.json`, i.e. GitHub decided what every
  install was offered. It is now our own service, serving only manifests an
  operator has explicitly promoted. That is what makes the two rings and the
  kill-switch possible, and it also means the Worker — not GitHub — is now
  the thing an attacker would target to push a build at the whole fleet.
  There is deliberately **no fallback to the old GitHub feed** when the
  Worker is unreachable: a fallback would defeat the ability to STOP serving
  a bad version. The installers themselves are still hosted by GitHub
  Releases, and the minisign check below is what actually gates installation
  regardless of who served the manifest.
- **The update Worker's admin API** — the same Worker exposes operator-only
  routes (`/v1/admin/promote`, `/v1/admin/channel`, `/v1/admin/channels`) on
  its second custom domain, `https://telemetry.sundaysuite.app`. They decide
  which published tag each channel serves and whether a channel is paused.
  Authentication is a single shared **admin key** sent as the `x-admin-key`
  header; `scripts/promote-release.mjs` reads it from the owner's macOS
  Keychain (`SundayRec telemetry admin key`) at run time and never accepts it
  as an argument, an env var, or a literal in the file, and never logs it.
  Whoever holds that key controls what every install is offered next, so it
  is the highest-value secret in the release path. The Worker itself lives in
  the separate `sunday-telemetry` repo and its server-side controls are
  documented there, not here.
- **OS-level device access** — audio/video capture devices and the
  filesystem locations the app is granted.

**Non-goals:**

- Defending against a compromised OS or a compromised user account. If the
  machine itself is owned, SundayRec's own controls are not a second line of
  defense.
- Multi-tenant isolation. This is a single-operator desktop app; there is no
  concept of separating multiple untrusted users on the same install.

## Controls that exist

So a future auditor doesn't have to re-derive these from scratch:

- **No shell for media processing.** Every ffmpeg/ffprobe invocation uses
  `Command::new(path).arg(...)` with an argv array — no shell interpolation,
  so untrusted filenames/paths can't inject shell syntax.
- **Path guard + coverage ratchet** (`src-tauri/src/commands/path_guard.rs`).
  Renderer-supplied paths are validated against a named policy (absolute,
  `..`-free, canonicalized, checked against protected home directories and,
  where applicable, rooted under the configured save folder) before they
  reach the filesystem or ffmpeg. A test ratchet (E1.3) keeps commands that
  take a path from silently launching without going through it.
- **Stream-key redaction in logs.** RTMP/streaming keys are kept out of log
  output.
- **Whisper model integrity.** Downloaded transcription models are
  SHA-256-verified against a pinned hash and only renamed into place
  (`.partial` → final) after the hash matches; a mismatch deletes the partial
  instead of promoting it.
- **ffmpeg/ffprobe sidecar pinning.** Bundled binaries are fetched and
  checked against pinned SHA-256 hashes (`scripts/fetch-ffmpeg.mjs`,
  `scripts/ffmpeg-checksums.json`) before use.
- **OS keychain for credentials.** OAuth refresh tokens, the stream key, the
  SMTP password, and API keys are stored via the OS-native credential store
  (macOS Keychain / Windows Credential Manager through the `keyring` crate;
  `src-tauri/src/secrets/`) — never in plaintext settings files. (E1.6 closed
  a legacy gap where the SMTP password had leaked into a plaintext
  localStorage blob before this seam existed.)
- **Strict CSP, no unsafe-inline scripts.** `script-src 'self'` with no
  `unsafe-inline`/`unsafe-eval`; `style-src` allows `unsafe-inline` for CSS
  only. Duplicated between `tauri.conf.json` and the renderer's `index.html`
  meta tag, with a sync test (E1.7) so the two can't silently drift.
  Windows Steinberg ASIO SDK download is SHA-256-pinned as a hard-fail
  (E1.5) — the SDK is a fixed 2019 artifact, so an unexpected hash means the
  download was tampered with or moved.
- **PKCE + loopback for OAuth.** Google (Drive/YouTube/Gmail) OAuth uses the
  PKCE flow with a loopback redirect, avoiding a stored client secret in the
  desktop binary.
- **Updater signature verification.** Tauri's built-in updater verifies a
  minisign signature (`plugins.updater.pubkey` in `tauri.conf.json`) on every
  downloaded update before installing it.
- **Blocking dependency audits in CI.** `npm audit --audit-level=high` and
  `cargo audit` both run as a required CI job (`.github/workflows/ci.yml`,
  `audit`), not advisory-only.

## Known gaps / accepted risks

- **The shared Sunday session file has no Windows ACL.** `sunday-auth`
  (from the upstream `sunday-platform` repo) writes the cross-app session
  file atomically but does not yet restrict its permissions on Windows.
  Tracked upstream, not in this repo.
- **macOS builds are signed but not notarized.** Apple's notary service
  currently returns 403 pending re-acceptance of the Program License
  Agreement (see `docs/DISTRIBUTION.md`); Gatekeeper will warn on first
  launch until that's resolved.
