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
- **A user-chosen location comes from a dialog Rust opens; the webview holds
  it only as a token.** The rule: when the operator picks a file or folder,
  the native dialog is opened by the process, never by the webview — a path
  the webview sends is only a claim that a dialog was shown, and with no
  per-command ACL a compromised webview can send any path with no dialog at
  all. Where the pick and its use are one step (the settings profile, below),
  the command simply acts on the dialog's answer. Where they are separate
  clicks, Rust keeps the picked place and hands the webview an opaque session
  token for it
  (`commands/chosen_paths.rs`, `ChosenPaths`): a random v4 UUID minted only
  after the dialog Rust opened answered and the answer passed `path_guard`,
  kept in memory for this session only (a restart forgets every token),
  bounded (64, oldest evicted), typed (a folder token never resolves where a
  file is asked for), and RE-VALIDATED when used — the place must still exist,
  still be that kind, still canonicalise to the very place that was picked (a
  folder swapped for a symlink since then is not that folder), and still pass
  `path_guard`. The webview is shown the place's last component only. The
  guarantee is carried by types, not by convention: only `vet` can make a
  `Vetted` place, `mint` takes one and consumes it (a path nobody vetted is not
  a type `mint` accepts), the raw lookup is private, and the one way a token
  gives its place back is `ChosenPaths::resolve(token, kind)` — lookup, kind
  check and re-validation in one step. Finding A2 is closed this way for the
  export FOLDER: the editor's export folder used to be
  `editor_export`'s `output_folder`, the answer of a folder picker the webview
  opened — so ffmpeg rendered into any folder the user can write to that the
  protected-folder list did not name. `editor_pick_output_folder` now opens
  the picker in Rust and answers with a token and the folder's name, and
  `editor_export` takes only `output_folder_token`: none = next to the source
  (resolved exactly as before, pinned by a golden test), an unknown token is
  refused (`export_folder_unknown`), a folder gone since the pick
  (`export_folder_missing`) or refused by the guard (`export_folder_refused`)
  too. An old-shape `outputFolder` is an unknown key serde ignores — that
  export lands next to its source. Unlike the plugin's own `open` command, the
  Rust picker does not widen the `asset://` scope to the picked folder.
  `commands::path_ratchet` judges a place-shaped name plus `_token` as a path
  field — in a request struct (`PATH_FIELDS`) or as a direct command parameter
  (`PARAM_TOKENS`) — and holds it to a `Token` entry naming the resolver the
  command must call (and following its calls must reach `ChosenPaths::resolve`)
  and the test that feeds it a forged token. Its lexical rules read the source
  with comments stripped, so a comment that names a guard cannot make a command
  look guarded.
  **A2, second half (the recording): closed.** The editor's recording used to
  be a path the webview sent to every `editor_*` command (`input_path`) and to
  the export (`EditorExportRequest.input_path`, `intro_path`, `outro_path`),
  held only by `path_guard` — so a compromised webview could point ffprobe and
  ffmpeg at any readable file, and, because «ved siden av kilden»
  (`ExportFolder::BesideSource`) is derived from the source, choose that file's
  folder as the destination. Now a recording enters the editor by exactly three
  doors, and each mints a File token for a file Rust decided on:
  `editor_open_recording` (the open dialog, opened in Rust — it takes no
  argument), `editor_open_known` (a history row, by the ROW'S id: the database
  holds the path; rows are written by the recorder and by startup recovery,
  which reads a manifest only if it is named the way the recorder names its own
  (`<session_id>.json`, one function for both) and the id is what the recorder
  makes — its start time in ms: 1–20 ASCII digits, so no sidecar name such as
  `<stem>.meta.json` written through `editor_write_sidecar` can ever carry its
  own matching id — and puts every file in it, the
  pre-roll clip and the row's final path through `path_guard::checked_input_file`
  first. A manifest that fails the name or the guard gets no row and nothing in
  it is deleted: it is warned about once and renamed `<name>.refused`, so it
  neither warns at every start nor counts as a recording in flight in the
  scheduler's missed-recording check (`pending_windows_in` applies the same
  name rule). It is never deleted, so a later version or support can fetch it.
  Pinned by the tests in `recorder::recovery`), and a file dropped on the
  window (the process catches the drop itself, `window::on_event` →
  `editor::note_drop`, and tells the page with `editor://file-dropped` — the
  path of a drop never reaches the page). `editor_load_recording`,
  `editor_peaks`, `editor_extract_playback_proxy`, `editor_segments`,
  `editor_diagnose_channels`, `editor_auto_process`, `editor_mastering_analyze`,
  `editor_master_preview` and `editor_export` take `source_token` and nothing
  else, and resolve it with `resolve_source` (`ChosenPaths::resolve(token,
File)`: typed, looked up, re-validated). «Ved siden av kilden» is derived from
  the RESOLVED source. The intro and outro clips are not in the request at all:
  `use_intro`/`use_outro` are switches, Rust reads the clip from the SAVED
  settings and vets it again at use (`export_clip_unusable`), and
  `settings_save` (and the localStorage hand-over) keeps the stored clips
  whatever the webview sends — they change only through `settings_pick_editor_intro`/
  `_outro`, which open a dialog in Rust, and the clears. The refusals have codes
  and no paths: `source_unknown`, `source_missing`, `source_refused`. Old-shape
  payloads fail closed: `inputPath` is an unknown key and the missing token
  refuses the request.
  **D2, the webview's own `asset://` scope: closed.** `editor_allow_asset_path`
  let the webview widen its own asset scope to any file `path_guard` let
  through, which made every readable file playable (and so readable) from the
  page. It is gone (`REPLACED` by `editor_open_recording`): the scope grows by
  one file only when RUST opens a recording — the vetted canonical place, and
  nothing else — and for the temp files Rust itself renders (the playback proxy,
  the mastering preview). The static `assetProtocol.scope.allow` in
  `tauri.conf.json` is **empty** (the protocol stays enabled, and its `deny`
  list of protected home folders stays): the only things the page plays are the
  opened recording (`loader.ts`), the proxy Rust renders for it (`loader.ts`)
  and the master preview Rust renders (`sound.ts`), and each is granted per
  file by `grant_asset_file`; no `<img>`/`<video>` loads from `asset://` (the
  camera preview is `data:`/`getUserMedia`). The old `$DOCUMENT`/`$DOWNLOAD`/
  `$VIDEO`/`$AUDIO`/`$DESKTOP`/`$APPDATA`/`$APPLOCALDATA`/`$TEMP` globs made
  everything in those folders readable from the page, including the app's own
  database and recovery folder, whatever Rust had been asked. Pinned by
  `editor::tests::the_static_asset_scope_allows_no_folder_and_keeps_its_deny_list`,
  and `grant_asset_file` itself against tauri's own scope
  (`the_asset_grant_opens_the_one_file_and_nothing_beside_it`: a grant widened
  to the file's folder fails it). New code that plays a file from `asset://`
  must be granted the same way, or it will not play. `commands::path_ratchet`
  pins the rest, from the outside: every `*_token` parameter or field is
  classified (a place token with
  its resolver and a forged-token proof, or a non-place with the reason — the
  old rule asked for a path-shaped word in front of `_token`, which
  `source_token` is not); a guard counts only as CODE in a command's body, not as
  a comment or a string literal that names it; `.mint(` may only appear in a
  CLOSED list of functions (`MINTERS`, each with its reason: `open_source`,
  `choose_output_folder`), which may only be reached through a closed list of
  doors (`MINT_DOORS`: the open dialog, the history row, the window's own drop
  handler, the folder dialog), and a door's place must be bound from the
  dialog's answer or the row's path and from nothing else — merely CALLING
  `recording_file_path(` or a dialog no longer anchors a mint (a command that
  mints from something the webview sent is a failing test); and `editor_export`
  must hand the seam the places `run_export` resolved, with no `ExportFolder`
  or `ResolvedExport` of its own.
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
  `recordings_reveal` and `recordings_reveal_export` only _reveal_ (never
  open) an existing file — the file of a history row the webview names by its
  id, or the export a token from the export's own result stands for; neither
  takes a path (B-family, below) — and refuse with an error that does not echo
  the path. A NEW save folder is vetted before it is stored
  (`settings_pick_save_folder`, which opens the dialog in Rust; a profile import
  keeps the stored folder instead): absolute, outside the protected home
  folders, not a package, and not the file-system root, the home folder or a
  folder above it.
  Paths are compared through one key that also folds macOS' firmlink spelling
  (`/System/Volumes/Data/Users/…` is `/Users/…`) and case where the file
  system ignores it, and — on macOS and Linux — by file identity (device and
  inode) wherever the file exists; the protected-folder check every path guard
  shares (`path_guard::deny_sensitive_under`) uses the same pair. The open and
  reveal commands, and the save-folder vet, do their filesystem checks off the
  async runtime.
  The plugin's injected `<a target=_blank>` click handler is switched off
  (`open_js_links_on_click(false)`).
  What remains after the follow-up to the review of #302 (2026-10-02), none of
  it running code from the folder: a save folder stored before the vet existed
  is never re-judged — on purpose, so an installation goes on recording where
  it always did — so one planted earlier still decides where the recorder
  writes, though the tray will not open it if it is a package; a NEW
  folder that does not exist yet can only be judged by its extension (the OS
  has nothing to look at until the recorder creates it — the tray asks the OS
  again before opening it); Windows has no identity check, so a second
  spelling of a local file that canonicalisation does not unify (a loopback
  network path) is compared by name only; and a network share that stops
  answering still leaves the click waiting until the OS gives up, now on a
  blocking-pool thread rather than a runtime worker.
- **A settings file's location comes from a dialog Rust opens.** The settings
  profile («Innstillingsprofil») is exported and imported by
  `settings_export_profile` and `settings_import_profile`, which take no
  argument: each opens the native save/open dialog from Rust and touches only
  the file that dialog answered. Until the review of #302 found it (A1), the
  webview opened the dialog itself and passed the picked path to
  `settings_export_to_file(path)` — and with no per-command ACL a compromised
  webview could pass any path with no dialog at all, so the export was an
  arbitrary file create/overwrite with content it shaped (every free-text
  setting lands in the JSON): a `.cmd` in the Windows Startup folder, an
  overwritten `~/.zshrc`. Its guard only knew the protected home folders. The dialog's
  filter names now come from Rust too, in the stored UI language. The path the
  dialog answers still meets the `UserChosenWrite`/`UserChosenRead` guard
  (absolute, no `..`, not in a protected folder) as defence in depth. A
  profile never carries this machine's own settings — its sound card and
  routing, camera, save folder, start-at-login, wake and updates
  (`settings::profile::MACHINE_LOCAL`): they are left out of the export and
  ignored by the import, so a file cannot move where this machine records or
  stop it starting. Nor can an import switch automatic deletion on or shorten
  it. The import reads at most 1 MiB, refuses a file that
  is not a settings profile (`profile_not_settings`) without writing anything,
  and lays the profile over the stored settings rather than over the defaults
  — a wrong file used to reset everything, the schedule included. `commands::path_ratchet` (`REPLACED`) fails if a
  path-taking twin of either command comes back, or if one of them grows a
  path parameter.
  **A3/A4, the B-family and B1: closed (PR-D).** The last commands that took a
  path from the webview — the editor's sidecars and sermon pick, «Vis i
  Finder» and the papirkurv — were held only by `path_guard`, which judges a
  path against the protected home folders and nothing else. None takes one any
  more; each names a thing Rust already knows:
  - `editor_read_sidecar`, `editor_write_sidecar`, `editor_delete_sidecar`,
    `editor_record_sermon_pick` and `editor_sermon_pick` take `source_token` —
    the File token of the recording (finding A2) — and nothing else. Rust
    resolves it with `resolve_source` (typed, looked up, re-validated) and
    builds `<stem>.meta.json`, `<stem>.cuts-draft.json` and
    `<stem>.feedback.json` itself, with the suffixes
    `sundayrec_core::editor::sidecar_path` has always used. A sidecar can
    therefore only land beside a recording Rust opened; a forged token never
    reaches the seam (`with_source`, pinned by
    `a_sidecar_command_with_a_forged_token_never_touches_the_disk`). The
    generic write and delete still refuse the feedback file.
  - `recordings_reveal` takes a history ROW's id (`store::recording_file_path`
    — the database, written only by the recorder), and a new
    `recordings_reveal_export` takes the token `editor_export` put in its
    result for the file the engine had just delivered. That token is a
    `ChosenKind::Export` token in the same store as the others: typed (it opens
    nothing in the editor and names no folder), session-scoped, bounded and
    re-validated when used (a file swapped for a symlink elsewhere is not the
    one delivered). The set of «delivered exports» and the three grants it
    served (inside the recordings root, a recording the history knows, a
    delivered export — the middle one a scan that canonicalised every history
    row) are gone. The three callers each have a road without a path: the
    library row and «Siste opptak» pass their row's id, the recording receipt
    finds its row by the path in Rust's own `recording://finished` event, and
    the export receipt passes the token.
  - `trash_move` takes the ids of history rows. An id with no row refuses the
    whole call (`recording_unknown`) before anything moves, so it moves only a
    recording the history knows and the sidecars beside it. Retention
    (`recordings_prune`) never comes through it — it runs in Rust over its own
    rows — and nothing in the app moves an export to the papirkurv.
  - **The recordings folder** is picked by `settings_pick_save_folder`: Rust
    opens the folder dialog, vets the folder with `vet_new_save_folder` and
    stores it. `settings_save` keeps the stored folder whatever the webview
    sends (as it keeps the intro and outro clips), so the folder cannot be set
    from a string — before, the page sent the dialog's answer in
    `settings_save`, and a compromised page could send any folder the vet
    accepted with no dialog at all.

  **The `dialog:` lock (PR-F).** With nothing in the webview opening a dialog,
  `capabilities/default.json` grants no `dialog:` permission, the npm package
  `@tauri-apps/plugin-dialog` is gone from `package.json` and the lock file,
  and two Rust tests next to the opener one fail if either comes back:
  `the_webview_holds_no_dialog_permission` (any capability file or inline
  config, read exactly like the opener tripwire) and
  `nothing_the_webview_is_built_from_names_the_dialog_plugin` (`package.json`,
  the lock file and every source under `app/`, `legacy/`, `e2e/`, `scripts/`).
  A vitest in `app/lib/api-shim-files.test.ts` says the same from the other
  side. `tauri-plugin-dialog` stays a Rust dependency: Rust is who asks.

  **What a path still is, and what remains.** No `#[tauri::command]` takes a
  path-shaped parameter any more — by name (`path`, `dest`, `target`, `src`,
  `source`, `location`, `uri`, `dir` …) or by type (`PathBuf`, `Path`, `OsString`)
  (`commands::path_ratchet` fails on one; its
  `GUARDED` list is gone, because a guard is not an answer, and `EXEMPT` is
  empty), and the old path-shaped parameters are pinned retired. The webview
  still receives paths — `OpenedRecording.path`, the export's `outputPath`, the
  history rows' `file_path` — for DISPLAY and for `<audio src>`, whose
  `asset://` address needs one over a scope Rust widened to exactly that file;
  no command accepts one back. One door still takes a save folder the webview
  names, and it is counted by Rust: `settings_import`, the one-shot
  localStorage hand-over for an upgrade from the old app. It takes JSON and not
  a path parameter, and the ratchet holds that shape to a closed list
  (`PARSED_PLACE_STRUCTS`: a command that parses `Settings` — or a struct with
  a place in it — out of a `String`/`Value` must be listed with its reason).
  Until the #314 review (B1) its «only once» was a flag in the webview's own
  localStorage, which a compromised page ignores, so it could move the recordings
  folder whenever it liked — the review's proof moved it to `<app-data>/recovery`,
  the first link of a chain that ended in deleted files. Now:
  - **Rust counts it.** `import` claims the `legacy_import_done` row in the SAME
    transaction that writes the settings; a second call is refused with
    `settings_import_done` and writes nothing, and a failed write rolls the
    claim back so the retry at the next launch still has its chance. The gate is
    this flag and not «a settings row exists»: a build older than R4 bridged a
    subset into sqlite and kept the full blob in localStorage, so a row can sit
    next to a hand-over that is still due. (Nothing in `setup` writes the row
    before the page does — `email_cleanup` and the scheduler's prune only
    rewrite a row that exists.)
  - **The first `settings_get` closes it** (`settings::close_legacy_import`), for
    the installs that never needed a hand-over — a fresh one, or one that did it
    years ago and has no flag. The page awaits its migration before it reads its
    settings, so a hand-over that was due has happened by then. The cost: an
    import that failed and is retried after the page was already read is
    answered `settings_import_done` and dropped; the page treats that as done
    (`app/lib/migrate-legacy-settings.ts`).
  - **The folder in it is held to more than a dialog's folder**
    (`vet_handover_save_folder`): the usual vet, and it must EXIST, be a folder
    and take a probe file — and no save folder at all (dialog or hand-over) may be
    the app's own data folder, inside it or above it (`save_folder_app_data`).
    A refusal keeps the stored folder and imports the rest. The intro and outro
    clips are never carried.
    What remains: on an install where the hand-over has not been closed yet — from
    process start until the page's first read — one call can still name an existing,
    writable folder outside the protected and app folders.

  **The `updater:` lock (#314, S2).** The webview holds no updater permission.
  `build.rs` used to generate `capabilities/updater.generated.json`
  (`updater:default`) whenever the `updater` feature was on, which let the page
  call `plugin:updater|check` — with `allowDowngrades`, `proxy` and `headers` —
  and so be offered an older, validly signed release without these fixes. The
  frontend never used it: updating is `update_check`/`update_install`, which call
  the plugin from Rust and need no capability. The generator is gone (the build
  script only deletes a stale file), no npm package is installed, and three
  Rust tests beside the dialog ones fail if it returns:
  `the_webview_holds_no_updater_permission`,
  `nothing_the_webview_is_built_from_names_the_updater_plugin` and
  `the_build_script_no_longer_writes_an_updater_capability`.

  **Smaller, from the same review.** A token that is USED is the newest in
  `ChosenPaths`, so the store evicts what nobody asks for, not what is in use. A
  place whose plain Windows spelling (verbatim prefix stripped) does not
  canonicalise back to it — names ending in a dot or a space — is refused
  (`plain_names_the_same_place`). A file dropped on the window opens, and widens
  the `asset://` scope, only if it is audio or video by its canonical name
  (`AUDIO_EXT`/`VIDEO_EXT`). The recording receipt's buttons act on the history
  row id `recording://finished` carries; with no row they are off, with a reason.

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
