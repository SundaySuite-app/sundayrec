# Release checklist — SundayRec (Tauri)

Single, current-state launchpad. The code is gate-green (the full Rust test suite;
`npm run check` passes). Everything below that is **not** a code change is an owner action
(secrets / accounts / signing) or a rig verification. Distilled from
`NEEDS-RICHARD.md`, `DISTRIBUTION.md`, `archive/RELEASE-AUDIT-2026-06-01.md`,
`SMOKE-TEST.md`, `ROLLBACK.md`.

## State of the release pipeline (verified in repo)

| Item                                                                                   | State                                                                                                           |
| -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Build macOS + Windows on tag (`release.yml`)                                           | ✅ wired                                                                                                        |
| Beta ring: `-beta.N` tag → GitHub pre-release, automatic                               | ✅ wired (`release.yml`'s `prerelease:` follows the tag)                                                        |
| Auto-updater plugin + pubkey + endpoints (`tauri.conf.json`)                           | ✅ wired                                                                                                        |
| `uploadUpdaterJson: true` in `release.yml`                                             | ✅ set (it was `includeUpdaterJson` until v0.11.0-beta.1 — never a real tauri-action input; the run ignored it) |
| Channel promotion / kill-switch (`scripts/promote-release.mjs`)                        | ✅ wired — needs Keychain item `SundayRec telemetry admin key`                                                  |
| Worker update-channel admin API (`telemetry.sundaysuite.app/v1/admin/*`)               | ✅ live — brukt til å forfremme v0.11.0-beta.1; `promote-release.mjs` kjører mot den                            |
| Client update feed points at `updates.sundaysuite.app` (not GitHub `/releases/latest`) | ✅ shipped — `tauri.conf.json`'s endpoint and `sundayrec-core::update::DEFAULT_UPDATE_BASE` both name it        |
| `sundayrec://` deep-link scheme registered (config + Info.plist)                       | ✅ config done — GUI-UNVERIFIED                                                                                 |
| ts-rs bindings drift                                                                   | ✅ 0 diff (`npm run bindings`)                                                                                  |
| macOS signing                                                                          | 🔑 needs `MAC_CERTS` + `MAC_CERTS_PASSWORD` (identity is hardcoded in `release.yml`)                            |
| macOS notarization                                                                     | 🚫 DISABLED in `release.yml` (env lines commented out — Apple PLA 403). Secrets alone do NOT re-enable it — §2a |
| Updater signing                                                                        | 🔑 needs `TAURI_SIGNING_*` secrets                                                                              |
| Windows signing                                                                        | ⏳ deferred (unsigned installer works; SmartScreen warns)                                                       |

## 1. CI is not a blocker

Nothing to do here. The repo is **public**, so Actions minutes are free —
`ci.yml`'s own header says so, and it runs the full gate on every push to
`main`, every PR, `v*` tags and manual dispatch. The frozen-spending-limit
situation this section used to describe is over. (Historical fallback while it
lasted: a local `tauri build` — see `docs/archive/RELEASE-AUDIT-2026-06-01.md`.)

## 2. macOS signing + notarization (Apple secrets)

Settings → Secrets and variables → Actions. Team ID **784GN847G4** is on file.

> ⚠️ **Use the names below verbatim.** `release.yml` writes `APPLE_CERTIFICATE:`,
> `APPLE_CERTIFICATE_PASSWORD:` and `APPLE_PASSWORD:` — but those are the
> **env-var names tauri-action expects**, on the LEFT of the colon. The secrets
> it actually reads are the ones on the right (`secrets.MAC_CERTS` etc.), which
> are the Electron-era names kept so nothing had to be re-entered. Creating
> secrets called `APPLE_CERTIFICATE`/`APPLE_PASSWORD` produces four secrets
> nothing reads and a build that is still unsigned.

For signing (active today — `release.yml` lines 153–155):

- [ ] `MAC_CERTS` — base64 of the "Developer ID Application" `.p12`.
      ⚠️ The `.p12` on the Desktop reportedly has the **wrong password** —
      re-export from Keychain Access with a known password first.
- [ ] `MAC_CERTS_PASSWORD` — the new export password.
- ✅ **No signing-identity secret exists or is needed.** `APPLE_SIGNING_IDENTITY`
  is a **hardcoded literal** in `release.yml`
  (`"Developer ID Application: Richard Fossland (784GN847G4)"`). Changing the
  identity is a source edit, not a secret.

For notarization (**inactive** — see §2a):

- [ ] `APPLE_ID` — Apple Developer account email.
- [ ] `APPLE_APP_SPECIFIC_PASSWORD` — an **app-specific** password. ⚠️ The
      previous one was **leaked in chat** — revoke it at appleid.apple.com →
      Sign-In and Security → App-Specific Passwords, generate a fresh one, store
      only as this secret.
- [ ] `APPLE_TEAM_ID` — `784GN847G4`.

### 2a. Notarization needs a source edit, not a secret

Adding the three secrets above changes **nothing on its own**. The
`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` env lines in `release.yml` are
**commented out unconditionally** (lines 163–165), disabled 2026-07-31 because
Apple's notary service returns 403 _"A required agreement is missing or has
expired"_ until the updated Program License Agreement is accepted on
developer.apple.com for team 784GN847G4.

- [ ] Accept the Program License Agreement at developer.apple.com.
- [ ] **Uncomment those three lines in `release.yml`** and commit. Until that
      commit exists, every build is Developer ID-signed but NOT notarized, and
      first launch needs right-click ▸ Open.

## 3. Auto-update signing (plugin already wired — only secrets remain)

The keypair already exists (key-id `4f08a2f48edd9a17`, backup
`~/.tauri/sundayrec_updater.key`; pubkey is in `tauri.conf.json`). Just add:

- [ ] `TAURI_SIGNING_PRIVATE_KEY` — `cat ~/.tauri/sundayrec_updater.key`.
- [ ] `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — its password (empty if none).

> Losing the private key breaks auto-update for installed users — keep the backup.

## 4. Optional runtime features (not build blockers)

- [ ] **Google OAuth client (Desktop type)** for cloud backup + Gmail email path
      → `SUNDAYREC_GOOGLE_CLIENT_ID` (see `GOOGLE-OAUTH-SETUP.md`).
- [ ] **Anthropic API key** (OS keychain) for the live AI sermon-companion
      summary — the keyless extractive path works without it.

## 5. Cut the release — two rings, beta first

Since Etappe 7, "published on GitHub" and "reaches installed clients" are two
separate facts. **A release that is built and published but never promoted
serves nobody** — that is the standing failure mode to watch for now, and it
is dangerous precisely because it looks completely fine: draft reviewed,
published, "Latest" ticked, nothing red anywhere on GitHub. The channel just
silently keeps offering the previous tag. Steps 5d/5e (and 5g's repeat of
them) exist specifically to catch that — do not compress them into one
mental step called "promote", they check different things.

### Direct-to-stable (owner override — the exception, written down)

The **normal** path is beta first (§5a–§5f, then §5g). The owner can order a
release straight to `stable` — **v0.12.0 shipped that way** (2026-08-09), with
the beta ring left on v0.11.1-beta.2. That is an owner decision, not a
shortcut anyone else may take, and the minimum bar is what v0.12.0 actually
met:

- [ ] Full CI green on the release commit — all six jobs, including the
      complete Playwright e2e tier in CI (not just locally).
- [ ] `npm run check` green on merged `main`.
- [ ] Promote + verify exactly as §5d/§5e, but for `stable`
      (`node scripts/promote-release.mjs stable vX.Y.Z`, then the
      no-argument readback) — promote-release's manifest validation is the
      last automated gate.
- [ ] §6a still applies: if the release touched recording/editor/meter/boot
      code, the first real Sunday on it IS the health gate — now run from
      `stable`, with no ring underneath, so read `ROLLBACK.md` in advance.

### 5a. Bump + tag (beta ring)

- [ ] Bump version in lockstep to `vX.Y.Z-beta.N`: `package.json`,
      `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`.
- [ ] `git tag vX.Y.Z-beta.N && git push origin vX.Y.Z-beta.N`.
- [ ] Watch the run. A `-beta.N` tag builds as a GitHub **pre-release**
      automatically (`release.yml`'s `prerelease:` expression follows the tag
      name — see the comment above it for why that's safe now). It still
      lands as a **draft** either way.
- [ ] **Betas ship NSIS only on Windows, and that is deliberate.** MSI cannot
      express a beta version at all: Windows Installer's ProductVersion is three
      numeric fields, so tauri refuses with _"optional pre-release identifier in
      app version must be numeric-only"_. v0.11.0-beta.1's first run failed on
      exactly this — macOS built fine and the draft came out with mac assets
      only, which looks like a flake rather than a rule. `release.yml` now passes
      `--bundles nsis` for Windows beta tags. **Stable releases still ship both**,
      so a missing `.msi` on a stable draft IS a problem.

### 5b. Pin the Windows ffmpeg hash (one-off, only if an entry is missing)

- [ ] If the Windows job's "Fetch bundled ffmpeg/ffprobe sidecars" step prints
      `⚠ … no pinned SHA-256 … — computed <hash>`, copy those lines into
      `scripts/ffmpeg-checksums.json` and commit before publishing. See
      `DISTRIBUTION.md` ▸ "The bundled ffmpeg". (All four current
      macOS/Windows × ffmpeg/ffprobe hashes are pinned already — this is a
      no-op unless a future ffmpeg version bump drops an entry.)

### 5c. Review + publish the draft

- [ ] Review the draft Release, then publish it. **Publishing is a separate
      manual step from building** — a draft is served to no one (same gotcha
      as the Electron-era SundayRec).
- [ ] The pre-release flag is now set automatically from the tag, so there is
      nothing to toggle by hand for the update feed's sake any more.
- [ ] ⚠️ **Transition-period exception — still real, do not skip:** any
      install still on v0.10.0 or earlier reads GitHub's `/releases/latest`
      directly and never consults the promoted channel at all (see
      `ROLLBACK.md`). For those installs, whether GitHub calls this release
      "the latest" still genuinely matters. For everyone already on
      v0.11.0+, it does not — only step 5d/5g's promotion reaches them. This
      exception goes away once the whole fleet is confirmed past v0.11.0.

### 5d. PROMOTE the tag to `beta` — its own step, cannot be skipped

- [ ] `node scripts/promote-release.mjs beta vX.Y.Z-beta.N`

There is no code path that infers a promotion from a GitHub publish. This is
the only action that makes a v0.11.0+ install able to see the release at all.

### 5e. VERIFY the promotion took — do not skip this either

- [ ] `node scripts/promote-release.mjs` (no arguments) — confirm the `beta`
      channel now reports the tag you just cut, and is **not** paused. A
      checklist box that only says "promote" does not catch step 5d silently
      failing (wrong tag typo, network hiccup, stale key); this one does,
      because it reads the state back instead of trusting the previous step
      succeeded.

### 5f. Run a real Sunday on the beta ring before going further

- [ ] Have the beta tester run a real service on the promoted beta build and
      go through `SMOKE-TEST.md`'s **beta-søndag** section. Do not promote to
      `stable` until that section is clean.

### 5g. Repeat 5a–5e for the stable tag

- [ ] Bump to the plain `vX.Y.Z` (same lockstep bump as 5a), tag, push,
      review, publish (5c) — the plain tag builds as a normal (non-pre-)
      release automatically.
- [ ] **Promote**: `node scripts/promote-release.mjs stable vX.Y.Z`.
- [ ] **Verify**: `node scripts/promote-release.mjs` — confirm `stable`
      reports the new tag and is not paused.

> If a promoted release turns out to be bad after all, see `ROLLBACK.md`.
> Short version: pausing a channel stops NEW updates — it does not undo one
> that already happened. The only way back for an install that already
> updated is a NEWER version containing the fix; "rollback" in the literal
> sense is not an operation this system has.

## 6. Rig sign-off before publishing (needs hardware — `SMOKE-TEST.md`)

- [ ] §2–11 smoke test on a real Mac/Windows rig (capture, VU, editor ffmpeg,
      whisper, wake/scheduler).
- [ ] **Deep-link**: after a signed `tauri build`, open `sundayrec://…` and
      confirm it routes into the app (the config is in place but GUI-UNVERIFIED;
      requires the `tray` feature, which release builds include).

### 6a. Recording/editor health gate (HARD — for any build touching audio)

The headless `npm run check`/`ci` is **necessary but not sufficient**: it cannot
see audio stutter, recording-mode lag, or editor instability. So for any build
that changed **`recorder/`, `capture.rs`, the editor, the meter loop, or boot
ordering**, this is a publish blocker:

- [ ] Run **§5b** (record normally → Diagnose → "Siste opptak" numbers). Paste
      `Dropp / xruns / IPC-overbelastning` + the Trend into the release notes.
      Healthy = all ≈ 0, clean exit, no `SR-CAPTURE-01`.
- [ ] Confirm the telemetry **detects** a deliberately-stressed capture (§5b
      step 3) — if the numbers don't move under a CPU hog, the gate is blind.
- [ ] Run **§12b** (editor stability loop) — large file, rapid play/stop/seek,
      switch-files-mid-play, undo-mid-drag — no crash / stuck icon / wrong audio.
- [ ] If `SR-RATE-01` fires, confirm the device truly needs a forced rate;
      otherwise set sample rate to **Auto** before sign-off.

> Rule: no recording/editor change is "done" until these rig numbers are in the
> release notes. This is what stops unverified audio fixes from shipping again.
