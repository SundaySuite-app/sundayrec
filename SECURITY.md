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

This channel is for **vulnerabilities only**. An ordinary bug (a crash, a
wrong setting, a confusing screen) is not a security report — see
[CONTRIBUTING.md](CONTRIBUTING.md) for where those go (a public issue, or
dev@sundaysuite.app). Two channels, two purposes: that address is read by a
person and is not private, so a real vulnerability described there instead
of through advisories would sit in the open until someone notices.

## Supported versions

Only the **latest release on your channel** is supported. There is no LTS
branch and no backporting of fixes to older versions. Please update before
reporting an issue that may already be fixed.

SundayRec auto-updates from one of two rings — `stable` and `beta` — chosen per
install under **Oppsett → Avansert → «Oppdateringer»**. Every install is on
`stable` unless somebody deliberately moved it. The feed URL is built at run time from that
setting (`channel_feed_url` in `crates/sundayrec-core/src/update.rs`), not from
`tauri.conf.json`; the `plugins.updater` block there names the stable feed only
as a fallback for a build that somehow bypasses that path.

A fix for a security issue lands on `beta` first and is promoted to `stable`
once it has been through a real service somewhere. If you are reporting against
a `-beta.N` build, say so — the two rings can be several commits apart.

## Threat model

SundayRec typically runs on a **volunteer-operated machine in a church**,
often started once and left unattended for the length of a service. The
operator is not a security professional, and the machine is not IT-managed.
Trust boundaries the app has to defend at:

- **Media files and their sidecars** — recordings, intro/outro clips, the
  `.meta`/`.cuts-draft`/`.feedback` JSON — paths and content that ultimately
  come from outside the process (a picked file, an imported recording).
- (The `sundayrec://` deep-link scheme, the chat webhook and the integration
  API endpoints were removed in R1 of «Frivilligen først», and the SMTP host
  the operator typed in went with e-mail alerts after that — fewer boundaries
  to defend. The app no longer talks to any endpoint the user configures.
  «Legg ut»'s own-page link is no exception: the app never fetches it, it
  hands it to the system browser after vetting — see the «Legg ut» bullet
  below.)
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
  take a path from silently launching without going through it — a path
  PARAMETER, and since finding E1 also a path-shaped FIELD of a struct a
  command takes (`commands/path_ratchet.rs`, `PATH_FIELDS`), which is where
  the recorder's output path used to hide.
- **The recording's output location is always computed in Rust.**
  `start_recording` takes a `ManualStartRequest` — the take's name, an
  auto-stop cap and the video toggle — and nothing else from the webview. The
  save folder, the file name, the format and the separate-audio sidecar's
  extension are planned in Rust from the persisted settings, by the same
  composition the scheduler uses (`recorder::opts::build_opts_in`). Until
  finding E1 the renderer asked `plan_recording_opts` (since deleted) for the
  full `RecordingOpts` and handed them straight back, so `output_path` was a
  raw IPC string the recorder created folders for and wrote into — any path
  the user can write to, for a compromised page. `RecordingOpts` is now not
  `Deserialize` (a compile-time assertion in `recorder/engine/payloads.rs`),
  so no command can take it again; golden tests in `commands/recorder.rs` pin
  that every legitimate manual start hands the engine byte-identical opts to
  before, and that a path smuggled into the request goes nowhere.
  What a compromised page can still decide, precisely: the **folder**
  (`settings_save` stores it, vetted as described in the opener bullet
  below); the **file-name stem**, through `customName` — reduced to a single
  path component by `sanitize_filename` (`/ \ : * ? " < > |` become `_`,
  surrounding blanks and trailing dots are trimmed, Windows device names are
  prefixed), after which Rust appends `_<YYYY-MM-DD>.<ext>`, and `_2`, `_3`, …
  if that name is taken, so nothing is overwritten; and the **extension**,
  only from the closed set the stored format and the video toggle allow
  (`mp3`, `wav`, `flac`, `aac`, or `mp4` with a camera). Not a path, and not
  any other extension. (The stem is not otherwise restricted: control
  characters, bidirectional-override characters and over-long names pass
  `sanitize_filename` unchanged. That function also names scheduled
  recordings, so hardening it is a separate change, not part of the E1 fix.)
- **Secret redaction in logs.** Credential-shaped values (`key=…`, Bearer
  tokens, and — defensively, though SundayRec no longer streams — the trailing
  key segment of RTMP URLs) are kept out of log output
  (`crates/sundayrec-core/src/redact.rs`).
- (**Whisper model integrity** — the SHA-256-verified model download — left
  with transcription in R2 of «Frivilligen først». The app downloads no
  models any more; the VAD model is vendored and verified at build + load.)
- **ffmpeg/ffprobe sidecar pinning.** Bundled binaries are fetched and
  checked against pinned SHA-256 hashes (`scripts/fetch-ffmpeg.mjs`,
  `scripts/ffmpeg-checksums.json`) before use.
- **No stored credentials, and no keychain access.** The last secret the app
  kept — the SMTP password, in the OS-native credential store — went with
  e-mail alerts. v0.23.0 and v0.24.0 deleted it once on upgrade; from v0.25.0
  the `keyring` crate and the `secrets` module are gone and the app never
  touches the keychain. Retired entries an older build left behind are listed
  in `src-tauri/src/settings/email_cleanup.rs` for manual removal. (E1.6 had
  earlier closed a legacy gap where that password leaked into a plaintext
  localStorage blob.)
- **Strict CSP, no unsafe-inline scripts.** `script-src 'self'` with no
  `unsafe-inline`/`unsafe-eval`; `style-src` allows `unsafe-inline` for CSS
  only. Duplicated between `tauri.conf.json` and the renderer's `index.html`
  meta tag, with a sync test (E1.7) so the two can't silently drift.
  Windows Steinberg ASIO SDK download is SHA-256-pinned as a hard-fail
  (E1.5) — the SDK is a fixed 2019 artifact, so an unexpected hash means the
  download was tampered with or moved.
- ~~**PKCE + loopback for OAuth.**~~ **No OAuth in this app any more.** The
  Sunday Account (SSO) login used PKCE with a loopback redirect, avoiding a
  stored client secret in the desktop binary; V1/PR3 deleted the whole login
  (five commands with no caller and no screen) together with the `sunday-auth`
  dependency. The Google Drive/YouTube/Gmail OAuth client that followed the
  same pattern left with cloud backup in R1 of «Frivilligen først». SundayRec
  now holds no OAuth client and mints no token.
- **«Legg ut» opens one page, never one the webview names.** The export
  receipt's button calls `publish_open_upload_page`, which takes no argument:
  it reads the stored channel and asks `sundayrec_core::publish::upload_page_url`,
  which answers with a fixed `https://` address (SoundCloud, YouTube, Spotify
  for Creators) or the church's own link after `custom_upload_url` has vetted
  it (`https://` only, no userinfo, one line, at most 2048 characters). The
  webview holds no `opener` permission at all (next bullet), and nothing is
  uploaded — the volunteer drags the file in, logged in to the church's own
  account in their own browser.
- **The webview cannot reach the OS opener.** `capabilities/default.json`
  grants no `opener:` permission, and a Rust test
  (`commands::recordings_open::tests::the_webview_holds_no_opener_permission`)
  fails if one comes back — in any file Tauri loads capabilities from (the
  whole `capabilities/` tree, nested folders and `.toml` included) or inline in
  any app config file — and fails on any such file it cannot read, so a new
  format cannot slip past it. The plugin's `reveal_item_in_dir` has no scope
  check at all, and its `open_path` scope was never configured (so the old
  grant both let any page reveal any path and silently refused every folder
  open). Every open/reveal now goes through a Rust command that decides what
  may be shown: `recordings_open_folder` takes no argument and opens only the
  resolved recordings folder, and never a package — on macOS anything the OS
  itself calls one (`NSWorkspace isFilePackageAtPath`: apps, installers,
  plug-ins, and the document packages installed apps declare, such as
  `.key`/`.logicx`), on every OS an extension list and an `Info.plist` check;
  `recordings_reveal` only _reveals_ (never opens) an existing file that is
  inside the recordings folder, known to the recording history, or an export
  delivered in this session — compared as canonical paths, and refused with an
  error that does not echo the path. A NEW save folder from the renderer is
  vetted before it is stored (`settings_save`; a profile import keeps the
  stored folder instead): absolute, outside the protected home folders, not a
  package, and not the file-system root, the home folder or a folder above it.
  Paths are compared through one key that also folds macOS' firmlink spelling
  (`/System/Volumes/Data/Users/…` is `/Users/…`) and case where the file
  system ignores it, and — on macOS and Linux — by file identity (device and
  inode) wherever the file exists; the protected-folder check every path guard
  shares (`path_guard::deny_sensitive_under`) uses the same pair. Both commands,
  and the save-folder vet, do their filesystem checks off the async runtime.
  The plugin's injected `<a target=_blank>` click handler is switched off
  (`open_js_links_on_click(false)`).
  What remains after the follow-up to the review of #302 (2026-10-02), none of
  it running code from the folder: a save folder stored before the vet existed
  is never re-judged — on purpose, so an installation goes on recording where
  it always did — so one planted earlier still decides grant 2 (what may be
  _revealed_), though the tray will not open it if it is a package; a NEW
  folder that does not exist yet can only be judged by its extension (the OS
  has nothing to look at until the recorder creates it — the tray asks the OS
  again before opening it); Windows has no identity check, so a second
  spelling of a local file that canonicalisation does not unify (a loopback
  network path) is compared by name only; and a network share that stops
  answering still leaves the click waiting until the OS gives up, now on a
  blocking-pool thread rather than a runtime worker.
  A related gap that predates #302, found in its review: `settings_export_to_file`
  writes to a path the renderer passes rather than one a dialog opened by Rust
  returned, and its guard (`checked_path`) judges that path only up to its
  deepest existing folder and only against the protected list — so a
  compromised renderer could create or overwrite other files the user can
  write. The firmlink fold above does close the `/System/Volumes/Data`
  spelling of the protected folders for that guard too; the real fix (Rust
  opens the dialog) is a high-priority row in `docs/PLAN.md`.
- **Updater signature verification.** Tauri's built-in updater verifies a
  minisign signature (`plugins.updater.pubkey` in `tauri.conf.json`) on every
  downloaded update before installing it.
- **Blocking dependency audits in CI.** `npm audit --audit-level=high` and
  `cargo audit` both run as a required CI job (`.github/workflows/ci.yml`,
  `audit`), not advisory-only.

## Known gaps / accepted risks

- ~~**The shared Sunday session file has no Windows ACL.**~~ **No longer this
  app's risk.** `sunday-auth` (upstream `sunday-platform`) writes the cross-app
  session file atomically without restricting its Windows permissions — but
  V1/PR3 removed the dependency along with the login that used it, so SundayRec
  neither writes nor reads that file. The gap is still real for whichever
  Sunday app does; it is tracked upstream, and it is not in this repo's threat
  model any more.
- **macOS builds are signed but not notarized.** Apple's notary service
  currently returns 403 pending re-acceptance of the Program License
  Agreement (see `docs/DISTRIBUTION.md`); Gatekeeper will warn on first
  launch until that's resolved.
