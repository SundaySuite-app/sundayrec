# Rollback — what it actually means for SundayRec, and what to run

This is a runbook, meant to be followed at 09:40 on a Sunday morning by
someone who just realised the release they promoted an hour ago is bad. It is
deliberately honest rather than reassuring: "rollback" is not a button this
system has, and pretending otherwise here would waste exactly the minutes
that matter.

## The one fact that shapes everything below

The updater's own guard is a semver **"is this genuinely newer"** check
(`is_newer` in `crates/sundayrec-core/src/update.rs`) — a client only ever
moves to a HIGHER version than the one it is currently running. There is no
code path, on the client or in the Worker, that offers an app a version lower
than the one it already has.

**So "roll back to the last good version" is not an operation this system can
perform.** You cannot make an already-updated install go backwards. What you
have instead are two separate, narrower levers, and neither of them is undo:

1. **The kill-switch** — pause a channel so nobody ELSE gets the bad version.
2. **Ship forward** — cut a new tag, HIGHER than the bad one, containing the
   previous good code, and promote that.

Both are real and both work. Neither reaches a machine that already updated.

## What the kill-switch does NOT do

- **It does not touch a machine that already downloaded and applied the bad
  version.** Those installs are running the bad build until you ship
  something newer — pausing stops the bleeding, it does not heal the wound.
- **Unpublishing or deleting the GitHub release does not help either**, for
  two independent reasons: the update feed is served by the Worker now, not
  by GitHub's `/releases/latest` — deleting the GitHub release does not
  un-promote it from the Worker's channel state. And separately, the
  installers anyone already downloaded are already sitting on their disks;
  removing the GitHub asset does not reach back and un-run an installer that
  already ran.
- **Propagation is not instant, but it is fast.** A running app re-checks for
  updates about once an hour (`legacy/renderer/pages/general-page.ts`'s
  startup-plus-hourly check), and the update feed itself is cached for 60
  seconds. So pausing a channel reaches an already-running installation
  within the hour, and a freshly-launched or manually-checked one
  immediately.

## The transition-period gap (until the whole fleet is past v0.11.0)

Every build up to and including **v0.10.0** has the OLD GitHub endpoint
(`https://github.com/SundaySuite-app/sundayrec/releases/latest/download/latest.json`)
compiled directly into it. Those installs never consult the Worker-served
feed at all — they ask GitHub's own `/releases/latest` forever, no matter
what you pause on the Worker.

**So during the transition, the kill-switch protects only the new fleet
(v0.11.0 and later).** The only lever over a pre-0.11.0 install is what it
has always been: what GitHub itself considers "latest" (see step 2 below).
Pausing the `stable` channel on the Worker has zero effect on a machine still
running v0.9.x or v0.10.0.

This stops mattering once every install has moved past v0.11.0 — there is no
pre-0.11 fleet left to worry about at that point. Until then, do not assume
the kill-switch alone protects everyone.

## Runbook: a bad release just went out

Run these in order.

### 1. Pause the channel immediately

```bash
node scripts/promote-release.mjs --pause stable      # or: beta
```

Confirm it took:

```bash
node scripts/promote-release.mjs
```

Expect the paused channel to show **PAUSED**, with its promoted tag
unchanged — pausing stops serving updates from that channel, it does not
un-promote the tag.

This protects the v0.11.0+ fleet within the hour (see propagation note
above). It does **not** protect anyone still on a pre-0.11.0 build — that is
step 2.

### 2. (only while a pre-0.11.0 fleet still exists) Un-latest the bad GitHub release

If the bad tag might still reach pre-0.11.0 installs: open the release on
GitHub → **Edit release** → untick **"Set as the latest release"** (or mark
it a pre-release). This does nothing for anyone who already updated, and
nothing for the v0.11.0+ fleet (which never looks at GitHub). It only stops a
pre-0.11 install that hasn't updated yet from being offered the bad build.
Skip this step entirely once the fleet is confirmed on v0.11.0+.

### 3. Fix the problem in a branch, as normal

Whatever caused the bad release — fix it, review it, merge it, same as any
other change.

### 4. Cut a NEW tag, higher than the bad one, containing the fix

```bash
# bump package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml in lockstep
git tag v0.11.1
git push origin v0.11.1
```

This is the only way to get already-updated installs onto good code — there
is no lower-version path. If the bad release was a beta (`v0.11.0-beta.1`),
the fix can be another beta (`v0.11.0-beta.2`) if you want another beta-ring
pass first, or go straight to a stable tag once you're confident — either way
it must be a HIGHER version than the bad one, on the SAME channel it is
replacing.

### 5. Let the release build, then promote the fixed tag

```bash
node scripts/promote-release.mjs stable v0.11.1
```

### 6. Confirm the channel is actually un-paused

Pausing and promoting are independent controls (separate fields, separate
endpoints) — promoting a new tag does not automatically clear a pause you set
in step 1.

```bash
node scripts/promote-release.mjs
```

If it still shows **PAUSED**, resume it explicitly:

```bash
node scripts/promote-release.mjs --resume stable
```

### 7. Verify

```bash
node scripts/promote-release.mjs
```

Confirm the channel shows the new, fixed tag and is **not** paused. Then run
the normal verification for that channel before considering this done — see
`RELEASE-CHECKLIST.md` and `SMOKE-TEST.md`'s beta-søndag section.

## Summary

| You want to…                                       | Can you?        | How                                                                         |
| -------------------------------------------------- | --------------- | --------------------------------------------------------------------------- |
| Stop NEW updates to a bad version (v0.11.0+ fleet) | Yes             | `node scripts/promote-release.mjs --pause <channel>`                        |
| Stop a pre-0.11.0 install from updating to it      | Yes, separately | Un-latest / pre-release the GitHub release itself (step 2)                  |
| Undo an update on a machine that already took it   | **No**          | Not possible — ship a newer version instead (steps 3–5)                     |
| Remove the bad installer from GitHub               | Doesn't help    | The manifest is Worker-served; already-downloaded installers are unaffected |

## Why the update feed lives on a different host than the admin API

`GET /v1/update/{channel}` is served from `https://updates.sundaysuite.app`,
not `https://telemetry.sundaysuite.app` where the admin routes in this
runbook live (same Worker, second custom domain). That split is deliberate:
an update check happens whether or not the operator ever consented to
telemetry (see `PRIVACY.md`), so it must not be served from a host whose name
implies it only exists for people who opted in.
