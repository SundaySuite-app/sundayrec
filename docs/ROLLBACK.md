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

## The guarantee holds for strict semver, and only for strict semver

The guard delegates to the `semver` crate (the same one Cargo and
`tauri-plugin-updater` use), so it enforces semver.org exactly: build metadata
is ignored for precedence, `0.11.0-beta.2` supersedes `-beta.1`, `-beta.10`
supersedes `-beta.9`, and the stable `0.11.0` supersedes every `0.11.0-*`.

For a version string that is **not** valid semver, the guard has nothing to
order by and falls back to "the strings differ, so it is newer". That fallback
is not an ordering — it answers _newer_ in both directions — so **the "only
ever moves to a HIGHER version" promise above does not cover it.**

In practice you cannot reach it, and three separate things have to break at
once before you could:

- The running version comes from the app's own package metadata, and **Cargo
  refuses to build a package whose version is not strict semver** (a
  zero-padded date like `2026.05.31` fails with "invalid leading zero in minor
  version number").
- The offered version comes from `tauri-plugin-updater`, which has already
  parsed the manifest into a `semver::Version` before the guard sees it. A
  manifest the crate cannot parse never gets this far.
- `scripts/promote-release.mjs` requires `latest.json`'s `version` to equal the
  tag without its `v`, and every tag this project has cut is
  `vMAJOR.MINOR.PATCH[-beta.N]`.

**The operating rule: tag releases as strict semver.** `v0.13.0`,
`v0.14.0-beta.1`. Not `v2026.05.31`, not `v1.0`, not a leading zero in any
field. A date-shaped tag is the one shape that looks reasonable and is not
covered — `v2026.5.31` would be fine, `v2026.05.31` would not, and nothing in
the pipeline will tell you which one you picked.

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
  updates about once an hour (`app/state/auto-update.ts`'s startup-plus-hourly
  check, over `@lib/pages/auto-update-schedule-core`), and the feed itself is
  cached for 60
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

Not at the Mac that has this script's Keychain item, or not the owner at
all? See "Nødprosedyre uten Mac (10 min)" near the end of this file — same
two routes, raw `curl`.

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

## Nødprosedyre uten Mac (10 min)

Everything above assumes `node scripts/promote-release.mjs`, which — until
now — only ever worked on the owner's Mac, reading the admin key from that
one Mac's Keychain. If the person who needs to pull the kill-switch right
now is not at that Mac (a different operator, a phone with `curl` and no
Node, a Windows laptop, anything), the admin API itself does not care what
called it: these are the same three routes the script calls, written out
raw. `scripts/promote-release.mjs` also has a second way to reach them
without a Mac at all — `SUNDAYREC_ADMIN_KEY` as an environment variable,
checked before the Keychain — if Node happens to be available; the `curl`
below needs neither Node nor the script.

**The admin key.** Set it as an environment variable in the shell you're
using, once, and never paste the value anywhere else — not chat, not a
shared doc, not an issue:

```bash
export ADMIN_KEY='<the admin key>'
```

Who hands you this key, and how, if you are not the owner and not at the
owner's Mac, is the owner's decision, made at the time — that is
deliberately not written down in this repo. Nothing below assumes an answer
to it.

**1. Read the current state of both channels** — confirms you're talking to
the right thing before changing anything:

```bash
curl -sS https://telemetry.sundaysuite.app/v1/admin/channels \
  -H "x-admin-key: $ADMIN_KEY"
```

**2. Pause the bad channel** — the kill-switch, the same one-line effect as
`--pause` in the script. `"channel"` is `"stable"` or `"beta"`, whichever is
serving the bad release:

```bash
curl -sS -X POST https://telemetry.sundaysuite.app/v1/admin/channel \
  -H "x-admin-key: $ADMIN_KEY" \
  -H "content-type: application/json" \
  -d '{"channel":"stable","paused":true}'
```

**3. Confirm it took** — re-run step 1 and look for `"paused":true` on the
channel you just touched. Pausing does not clear or change the promoted
tag — see "What the kill-switch does NOT do" above, it applies here too.

**4. Resume it later**, once steps 3–7 of the runbook above have happened on
a machine that has the script (same route, `"paused":false`):

```bash
curl -sS -X POST https://telemetry.sundaysuite.app/v1/admin/channel \
  -H "x-admin-key: $ADMIN_KEY" \
  -H "content-type: application/json" \
  -d '{"channel":"stable","paused":false}'
```

⚠️ **Both hosts matter here too.** These routes are on
`telemetry.sundaysuite.app` (the admin host), not `updates.sundaysuite.app`
(the public feed clients poll) — see "Why the update feed lives on a
different host" above. The admin key is not accepted on the public host and
would do nothing there.

This covers the kill-switch and reading channel state only — the same two
things `--pause`/`--resume`/no-args cover in the script. Promoting a NEW tag
(`POST /v1/admin/promote`) additionally requires validating that tag's
`latest.json` first — `scripts/promote-release.mjs`'s `manifestProblems()` —
which is not something to hand-reconstruct in a raw `curl` command under
time pressure. Step 5 above ("Cut a NEW tag…") waits for a machine that has
the script.
