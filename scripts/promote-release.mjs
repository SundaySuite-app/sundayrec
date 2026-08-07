#!/usr/bin/env node
// Promote a tag to an update channel, pause/resume a channel (kill-switch),
// or show the current state of both channels — the owner's one lever over
// which already-published GitHub release installed clients get offered.
//
// GitHub still builds and hosts the installers
// (`.github/workflows/release.yml` creates a DRAFT release on every `v*`
// tag, then you publish it by hand). This script is the SEPARATE step after
// that: it tells the Sunday telemetry Worker's admin API which published tag
// each channel should serve. Nothing here builds, tags, or publishes a
// GitHub release — see docs/RELEASE-CHECKLIST.md for the full sequence, and
// docs/ROLLBACK.md for what to run when a promoted release turns out bad.
//
// ── TWO HOSTS, ON PURPOSE (not a typo) ──────────────────────────────────────
// The PUBLIC update feed clients poll — `GET /v1/update/{channel}` — is
// served from `https://updates.sundaysuite.app`. That check happens whether
// or not the operator ever consented to telemetry, so it deliberately does
// NOT live on a host named "telemetry" (see PRIVACY.md). The ADMIN routes
// this script calls (`/v1/admin/promote`, `/v1/admin/channel`,
// `/v1/admin/channels`) are operator-only and stay on
// `https://telemetry.sundaysuite.app` — same Worker, same deployment, second
// custom domain, different half of the API.
//
// ── THE RULE (enforced here AND server-side — this is the fast, local check
//    that saves a round trip and a confusing audit-log entry) ──────────────
//   a `vX.Y.Z-beta.N` tag can only be promoted to the "beta"   channel
//   a plain     `vX.Y.Z` tag can only be promoted to the "stable" channel
//
// ── THE ADMIN KEY ────────────────────────────────────────────────────────────
// Never an argument, an env var, or a line in this file — it is read from the
// owner's macOS Keychain at run time and used only as the `x-admin-key`
// request header. It is never printed, never logged, and never appears in an
// error message.
//
//   security add-generic-password -s 'SundayRec telemetry admin key' \
//     -a sundayrec -w '<the admin key>'
//
// (one-time setup on a machine — ask wherever ADMIN_KEY was generated/stored
// for the value; see the sunday-telemetry repo's README "Secrets" section.)
//
// The service name has spaces because that is what the item is ACTUALLY called
// in the owner's Keychain — it was created by hand, before this script existed.
// Renaming the Keychain item to suit the script would be the wrong direction:
// the item is the thing that exists, and a script that guesses a tidier name is
// a script that fails on the one machine it has to work on.
//
// ── USAGE ────────────────────────────────────────────────────────────────────
//   node scripts/promote-release.mjs                   show both channels' state
//   node scripts/promote-release.mjs <channel> <tag>    promote <tag> to <channel>
//   node scripts/promote-release.mjs --pause <channel>  kill-switch: pause a channel
//   node scripts/promote-release.mjs --resume <channel> un-pause a channel
//
//   <channel> is "stable" or "beta".
//
// Examples:
//   node scripts/promote-release.mjs beta v0.11.0-beta.1
//   node scripts/promote-release.mjs stable v0.11.0
//   node scripts/promote-release.mjs --pause stable
//   node scripts/promote-release.mjs --resume stable

import { spawnSync } from "node:child_process";

const ADMIN_BASE_URL = "https://telemetry.sundaysuite.app";
const KEYCHAIN_SERVICE = "SundayRec telemetry admin key";
const CHANNELS = new Set(["stable", "beta"]);

const STABLE_TAG = /^v\d+\.\d+\.\d+$/;
const BETA_TAG = /^v\d+\.\d+\.\d+-beta\.\d+$/;

function fail(msg) {
  console.error(`✗ ${msg}`);
  process.exit(1);
}

// ── Keychain ─────────────────────────────────────────────────────────────
// `spawnSync` runs no shell, so the key never round-trips through anything
// that could echo or log a command line. The key is returned to the caller
// for immediate use as a header value — it is never written to stdout/stderr
// anywhere in this file.
function readAdminKey() {
  const r = spawnSync(
    "security",
    ["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
    { encoding: "utf8" },
  );
  if (r.error) {
    fail(
      `could not run \`security\` (${r.error.message}) — this script only works on macOS, ` +
        `reading from Keychain Access.`,
    );
  }
  if (r.status !== 0) {
    fail(
      `no Keychain item named "${KEYCHAIN_SERVICE}" — add it once with:\n` +
        `    security add-generic-password -s '${KEYCHAIN_SERVICE}' -a sundayrec -w '<the admin key>'`,
    );
  }
  const key = r.stdout.trim();
  if (!key) fail(`Keychain item "${KEYCHAIN_SERVICE}" exists but is empty.`);
  return key;
}

// ── HTTP ─────────────────────────────────────────────────────────────────
async function call(method, path, key, body) {
  let res;
  try {
    res = await fetch(`${ADMIN_BASE_URL}${path}`, {
      method,
      headers: {
        "x-admin-key": key,
        ...(body ? { "content-type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch (e) {
    fail(`request to ${path} failed: ${e.message}`);
  }
  const text = await res.text();
  let payload;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = text; // not JSON — print whatever came back verbatim
  }
  return { status: res.status, ok: res.ok, payload };
}

function printResult({ status, ok, payload }) {
  console.log(`${ok ? "✓" : "✗"} HTTP ${status}`);
  if (payload !== null && payload !== undefined) {
    console.log(
      typeof payload === "string" ? payload : JSON.stringify(payload, null, 2),
    );
  }
  if (!ok) process.exit(1);
}

// ── channel/tag agreement — checked locally, before any request or even a
// Keychain read, so a typo never costs a network round trip ────────────────
function assertTagMatchesChannel(channel, tag) {
  if (!BETA_TAG.test(tag) && !STABLE_TAG.test(tag)) {
    fail(
      `"${tag}" doesn't look like a release tag (expected "vX.Y.Z" or "vX.Y.Z-beta.N").`,
    );
  }
  if (BETA_TAG.test(tag) && channel !== "beta") {
    fail(
      `"${tag}" is a beta tag (-beta.N) — it can only be promoted to "beta", not "${channel}".`,
    );
  }
  if (STABLE_TAG.test(tag) && channel !== "stable") {
    fail(
      `"${tag}" is a plain release tag — it can only be promoted to "stable", not "${channel}".`,
    );
  }
}

// ── best-effort friendly summary of GET /v1/admin/channels — printResult()
// below always prints the raw JSON too, so a shape this doesn't recognise is
// still fully visible, just less pretty ───────────────────────────────────
function summarizeChannels(payload) {
  // The Worker returns `channels` as an ARRAY of rows, each carrying its own
  // `channel` name. Indexing it by name yields undefined and prints nothing —
  // which is how this line silently went blank the first time it ran.
  const rows = payload && typeof payload === "object" ? payload.channels : null;
  if (!Array.isArray(rows)) return;
  for (const c of rows) {
    if (!c || typeof c !== "object") continue;
    const paused = c.paused ? "  — PAUSED (kill-switch on)" : "";
    // `behind` is the whole point of reading this before and after a release:
    // it is how "published the release but forgot to promote it" stops being
    // silent. A ring serving nothing at all is always behind, so say which.
    const behind = c.behind
      ? `  ⚠ NEWER RELEASE NOT PROMOTED: ${c.latestTag ?? "(unknown)"}`
      : "";
    console.log(
      `  ${c.channel}: ${c.tag ?? "(nothing promoted yet)"}${paused}${behind}`,
    );
  }
}

// ── main ─────────────────────────────────────────────────────────────────
async function main() {
  const args = process.argv.slice(2);

  if (args.length === 0) {
    const key = readAdminKey();
    console.log(`Channel state (${ADMIN_BASE_URL}/v1/admin/channels):`);
    const result = await call("GET", "/v1/admin/channels", key);
    summarizeChannels(result.payload);
    console.log();
    printResult(result);
    return;
  }

  if (args[0] === "--pause" || args[0] === "--resume") {
    const paused = args[0] === "--pause";
    const channel = args[1];
    if (!CHANNELS.has(channel)) {
      fail(`usage: node scripts/promote-release.mjs ${args[0]} <stable|beta>`);
    }
    const key = readAdminKey();
    console.log(`${paused ? "Pausing" : "Resuming"} channel "${channel}" …`);
    printResult(
      await call("POST", "/v1/admin/channel", key, { channel, paused }),
    );
    return;
  }

  if (args.length === 2) {
    const [channel, tag] = args;
    if (!CHANNELS.has(channel)) {
      fail(
        `unknown channel "${channel}" — expected "stable" or "beta".\n` +
          `usage: node scripts/promote-release.mjs <channel> <tag>`,
      );
    }
    assertTagMatchesChannel(channel, tag);
    const key = readAdminKey();
    console.log(`Promoting ${tag} → ${channel} …`);
    printResult(await call("POST", "/v1/admin/promote", key, { channel, tag }));
    return;
  }

  fail(
    "usage:\n" +
      "  node scripts/promote-release.mjs                   show channel state\n" +
      "  node scripts/promote-release.mjs <channel> <tag>    promote a tag\n" +
      "  node scripts/promote-release.mjs --pause <channel>  kill-switch\n" +
      "  node scripts/promote-release.mjs --resume <channel> un-pause",
  );
}

await main();
