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
// Two sources, tried in order (see readAdminKey() below):
//   1. SUNDAYREC_ADMIN_KEY, an env var — the emergency path for a machine
//      that isn't the owner's Mac (or isn't a Mac at all), documented in
//      docs/ROLLBACK.md "Nødprosedyre uten Mac". Never a CLI argument, never
//      a line in this file.
//   2. The owner's macOS Keychain, at run time — the original, normal path.
// Whichever source wins, the key is used ONLY as the `x-admin-key` request
// header. It is never printed, never logged, and never appears in an error
// message, on either path.
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
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

const ADMIN_BASE_URL = "https://telemetry.sundaysuite.app";
const KEYCHAIN_SERVICE = "SundayRec telemetry admin key";
const CHANNELS = new Set(["stable", "beta"]);

const STABLE_TAG = /^v\d+\.\d+\.\d+$/;
const BETA_TAG = /^v\d+\.\d+\.\d+-beta\.\d+$/;

// ── The manifest a tag actually carries ──────────────────────────────────────
// `latest.json` is a PUBLIC asset on the (already published) GitHub release, so
// this needs no token — the same unauthenticated GET the Worker itself does.
const MANIFEST_URL = (tag) =>
  `https://github.com/SundaySuite-app/sundayrec/releases/download/${tag}/latest.json`;

// The platform keys `tauri-plugin-updater` looks up, and therefore the ones a
// promotable manifest MUST carry. It searches `{os}-{arch}-{installer}` first
// and falls back to `{os}-{arch}` (2.10.1, updater.rs `get_urls`); a key that
// is absent is not "no update available", it is a hard `TargetsNotFound` error
// on every check that install ever makes.
//
// `windows-x86_64-msi` is deliberately NOT required: MSI cannot express a
// `-beta.N` version (Windows Installer's ProductVersion is three numeric
// fields), so release.yml builds Windows betas with `--bundles nsis` and the
// beta manifest legitimately has four entries, not five. NSIS is what the
// updater installs from either way.
const REQUIRED_PLATFORMS = [
  "darwin-aarch64",
  "darwin-aarch64-app",
  "windows-x86_64",
  "windows-x86_64-nsis",
];

// ── Manifest validation (pure — unit-tested in promote-release.test.mjs) ─────
//
// WHY THIS EXISTS. release.yml's matrix runs with `fail-fast: false`, so the
// two platform legs upload INDEPENDENTLY. When v0.11.0-beta.1's first run
// failed on Windows (run 31206918593), the macOS leg still finished and
// uploaded its own `latest.json` — a manifest with only the two darwin keys —
// to the draft. The run showed red, but the draft it left behind was complete
// enough to look publishable and would have been accepted for promotion
// without a murmur, and every Windows install on that ring would have started
// erroring on every update check.
//
// Nothing between "the workflow decides what to build" and "the operator
// promotes a tag" was checking that those two agreed. This is that check.
export function manifestProblems(manifest, tag) {
  const problems = [];

  if (!manifest || typeof manifest !== "object") {
    return ["latest.json is not a JSON object"];
  }

  // The tag and the version the manifest announces must be the same release.
  // The updater compares the manifest's `version` against the installed one —
  // a mismatch here means clients are offered a version whose bytes came from
  // somewhere else entirely.
  const expected = tag.replace(/^v/, "");
  if (manifest.version !== expected) {
    problems.push(
      `latest.json says version "${manifest.version}" but the tag is "${tag}" (expected "${expected}")`,
    );
  }

  const platforms =
    manifest.platforms && typeof manifest.platforms === "object"
      ? manifest.platforms
      : null;
  if (!platforms) {
    return [...problems, "latest.json has no `platforms` object"];
  }

  for (const key of REQUIRED_PLATFORMS) {
    const entry = platforms[key];
    if (!entry || typeof entry !== "object") {
      problems.push(
        `no "${key}" entry — that platform's installs get TargetsNotFound on every update check`,
      );
      continue;
    }
    // An unsigned artifact is the other silent release failure: the build
    // succeeds, the assets upload, and every client rejects the update at the
    // signature check. tauri-action emits an empty string rather than omitting
    // the field when TAURI_SIGNING_PRIVATE_KEY is absent.
    if (typeof entry.signature !== "string" || entry.signature.trim() === "") {
      problems.push(`"${key}" has no signature — clients would reject it`);
    }
    if (typeof entry.url !== "string" || entry.url.trim() === "") {
      problems.push(`"${key}" has no download url`);
    }
  }

  return problems;
}

// Fetch + validate. FAILS CLOSED: an unreachable or unparseable manifest is a
// refusal, never a shrug — the whole point is that "we could not check" must
// not read the same as "we checked and it was fine".
async function assertManifestIsPromotable(tag) {
  const url = MANIFEST_URL(tag);
  let res;
  try {
    res = await fetch(url);
  } catch (e) {
    fail(
      `could not fetch ${url} (${e.message}).\n` +
        `  Refusing to promote a tag whose updater manifest could not be checked.`,
    );
  }
  if (res.status === 404) {
    fail(
      `${tag} has no latest.json asset (HTTP 404 from ${url}).\n` +
        `  Either the release is still a DRAFT (publish it first — draft assets are not\n` +
        `  publicly downloadable), or the build never uploaded an updater manifest.`,
    );
  }
  if (!res.ok) {
    fail(`fetching ${url} returned HTTP ${res.status} — refusing to promote.`);
  }

  let manifest;
  try {
    manifest = JSON.parse(await res.text());
  } catch (e) {
    fail(`${tag}'s latest.json is not valid JSON (${e.message}).`);
  }

  const problems = manifestProblems(manifest, tag);
  if (problems.length > 0) {
    fail(
      `${tag}'s latest.json is not fit to promote:\n` +
        problems.map((p) => `    • ${p}`).join("\n") +
        `\n  Found platforms: ${Object.keys(manifest.platforms ?? {}).join(", ") || "(none)"}\n` +
        `  This usually means one leg of release.yml's matrix failed while the other\n` +
        `  still uploaded (the matrix runs fail-fast: false). Re-run the failed job,\n` +
        `  let it overwrite latest.json, then promote.`,
    );
  }
  console.log(
    `✓ ${tag} latest.json: version ${manifest.version}, platforms ${Object.keys(manifest.platforms).sort().join(", ")}`,
  );
}

function fail(msg) {
  console.error(`✗ ${msg}`);
  process.exit(1);
}

// ── Admin key: env override, then Keychain ─────────────────────────────────
// SUNDAYREC_ADMIN_KEY wins if set, and the Keychain is never touched in that
// case — `security` isn't even spawned, which is the whole point: this is
// the path for a machine (or a person) that has no Keychain item to read,
// documented in docs/ROLLBACK.md "Nødprosedyre uten Mac (10 min)".
//
// Otherwise: `security find-generic-password` via `spawnSync`, which runs no
// shell, so the key never round-trips through anything that could echo or
// log a command line.
//
// Either path returns the key to the caller for immediate use as a header
// value — it is never written to stdout/stderr anywhere in this file.
export function readAdminKey() {
  const envKey = process.env.SUNDAYREC_ADMIN_KEY;
  if (envKey && envKey.trim() !== "") {
    console.log(
      "Using SUNDAYREC_ADMIN_KEY from the environment (emergency path — see docs/ROLLBACK.md).",
    );
    return envKey.trim();
  }

  const r = spawnSync(
    "security",
    ["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
    { encoding: "utf8" },
  );
  if (r.error) {
    fail(
      `could not run \`security\` (${r.error.message}) — this script only works on macOS, ` +
        `reading from Keychain Access. Not on a Mac? Set SUNDAYREC_ADMIN_KEY instead — see ` +
        `docs/ROLLBACK.md "Nødprosedyre uten Mac".`,
    );
  }
  if (r.status !== 0) {
    fail(
      `no Keychain item named "${KEYCHAIN_SERVICE}" — add it once with:\n` +
        `    security add-generic-password -s '${KEYCHAIN_SERVICE}' -a sundayrec -w '<the admin key>'\n` +
        `  or set SUNDAYREC_ADMIN_KEY instead — see docs/ROLLBACK.md "Nødprosedyre uten Mac".`,
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
    // Checked BEFORE the Keychain read: a tag that isn't fit to promote should
    // cost neither a secret nor a round trip to the admin API.
    await assertManifestIsPromotable(tag);
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

// Only run when executed as a script, so the unit tests can import
// `manifestProblems` without `main()` reaching for the Keychain.
if (
  process.argv[1] &&
  realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))
) {
  await main();
}
