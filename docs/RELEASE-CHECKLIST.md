# Release checklist — SundayRec (Tauri)

Single, current-state launchpad. The code is gate-green (the full Rust test suite;
`npm run check` passes). Everything below that is **not** a code change is an owner action
(secrets / accounts / signing) or a rig verification. Distilled from
`NEEDS-RICHARD.md`, `DISTRIBUTION.md`, `RELEASE-AUDIT.md`, `SMOKE-TEST.md`,
`ROLLBACK.md`.

## State of the release pipeline (verified in repo)

| Item                                                                                   | State                                                                                                           |
| -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Build macOS + Windows on tag (`release.yml`)                                           | ✅ wired                                                                                                        |
| Beta ring: `-beta.N` tag → GitHub pre-release, automatic                               | ✅ wired (`release.yml`'s `prerelease:` follows the tag)                                                        |
| Auto-updater plugin + pubkey + endpoints (`tauri.conf.json`)                           | ✅ wired                                                                                                        |
| `includeUpdaterJson: true` in `release.yml`                                            | ✅ set                                                                                                          |
| Channel promotion / kill-switch (`scripts/promote-release.mjs`)                        | ✅ wired — needs Keychain item `SundayRec telemetry admin key`                                                  |
| Worker update-channel admin API (`telemetry.sundaysuite.app/v1/admin/*`)               | ⏳ Etappe 7 Worker-side rollout — see `sunday-telemetry` repo                                                   |
| Client update feed points at `updates.sundaysuite.app` (not GitHub `/releases/latest`) | ⏳ pending — see the `qa/e7-update-channel` work; `tauri.conf.json` still points at GitHub as of this checklist |
| `sundayrec://` deep-link scheme registered (config + Info.plist)                       | ✅ config done — GUI-UNVERIFIED                                                                                 |
| ts-rs bindings drift                                                                   | ✅ 0 diff (`npm run bindings`)                                                                                  |
| macOS signing + notarization                                                           | 🔑 needs Apple secrets                                                                                          |
| Updater signing                                                                        | 🔑 needs `TAURI_SIGNING_*` secrets                                                                              |
| Windows signing                                                                        | ⏳ deferred (unsigned installer works; SmartScreen warns)                                                       |

## 1. Unblock CI (P0 — gates everything else)

- [ ] **GitHub Actions billing**: `ci.yml` and `release.yml` run on Actions and
      cannot start while the spending limit is frozen. Raise it / fix payment,
      then re-run on a tag. Fallback while blocked: local `tauri build` (see
      `RELEASE-AUDIT.md`).

## 2. macOS signing + notarization (Apple secrets)

Settings → Secrets and variables → Actions. Team ID **784GN847G4** is on file.

- [ ] `APPLE_CERTIFICATE` — base64 of the "Developer ID Application" `.p12`.
      ⚠️ The `.p12` on the Desktop reportedly has the **wrong password** —
      re-export from Keychain Access with a known password first.
- [ ] `APPLE_CERTIFICATE_PASSWORD` — the new export password.
- [ ] `APPLE_SIGNING_IDENTITY` — `Developer ID Application: … (784GN847G4)`.
- [ ] `APPLE_ID` — Apple Developer account email.
- [ ] `APPLE_PASSWORD` — an **app-specific** password. ⚠️ The previous one was
      **leaked in chat** — revoke it at appleid.apple.com → Sign-In and Security
      → App-Specific Passwords, generate a fresh one, store only as this secret.
- [ ] `APPLE_TEAM_ID` — `784GN847G4`.

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

### 5a. Bump + tag (beta ring)

- [ ] Bump version in lockstep to `vX.Y.Z-beta.N`: `package.json`,
      `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`.
- [ ] `git tag vX.Y.Z-beta.N && git push origin vX.Y.Z-beta.N`.
- [ ] Watch the run. A `-beta.N` tag builds as a GitHub **pre-release**
      automatically (`release.yml`'s `prerelease:` expression follows the tag
      name — see the comment above it for why that's safe now). It still
      lands as a **draft** either way.

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
      whisper, wake/scheduler, streaming).
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
