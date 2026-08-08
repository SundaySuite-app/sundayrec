#!/usr/bin/env node
// What the fleet's corrections say about the detector — the read half of the
// tuning ritual (docs/LEARNING.md).
//
// The ritual is: read the aggregates, propose a change to a detector constant,
// run the golden tests, look at which recordings changed answer, ship it as a
// normal release. This script is step one and nothing else. It cannot change a
// constant, cannot change a channel, cannot delete anything, and cannot reach
// any route that could — see "READ-ONLY" below.
//
// ── WHAT IT READS ───────────────────────────────────────────────────────────
//   GET /v1/admin/summary   — the aggregate view. `correctionBands` is the row
//                             set this tool exists for: signal × direction ×
//                             band, counted. `counterTotals` is the exposure —
//                             how often anyone was in a position to correct
//                             anything at all.
//   GET /v1/admin/channels  — which tag each ring serves. A correction can only
//                             come from a build that captures corrections, so
//                             "what is promoted" is part of reading the number.
//
// Both on `https://telemetry.sundaysuite.app`, which is the ADMIN half of the
// Worker; the public update feed lives on the other hostname and is not touched
// here. Same split, and the same reason, as scripts/promote-release.mjs.
//
// ── READ-ONLY, STRUCTURALLY ─────────────────────────────────────────────────
// There is exactly one HTTP function below, it hardcodes the method, and the
// only two paths in this file are the two GETs above. `tuning-report.test.mjs`
// asserts both properties against the file's own source text, so a later edit
// that adds a mutating call fails the gate rather than being noticed by a
// reviewer. A reporting tool that could also act is a tool nobody can run
// while thinking about something else.
//
// ── THE ADMIN KEY ───────────────────────────────────────────────────────────
// Never an argument, an env var, or a line in this file — read from the owner's
// macOS Keychain at run time and used only as the `x-admin-key` header. It is
// never printed, never logged, and never appears in an error message. This is
// promote-release.mjs's `readAdminKey`, deliberately identical down to the
// failure text: two scripts that fail differently on the same missing item send
// whoever hits it looking for two different problems.
//
//   security add-generic-password -s 'SundayRec telemetry admin key' \
//     -a sundayrec -w '<the admin key>'
//
// ── USAGE ───────────────────────────────────────────────────────────────────
//   node scripts/tuning-report.mjs           the report
//   node scripts/tuning-report.mjs --json    the raw summary, unrendered
//
// Exit 0 means the tool ran, whatever it found. "Nothing to read" is a
// successful run, exactly as it is for the A/B harness — the failure it would
// be reporting is not this tool's. Exit 1 is operational: no Keychain item, no
// network, an unauthorised or unparseable response.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

const ADMIN_BASE_URL = "https://telemetry.sundaysuite.app";
const KEYCHAIN_SERVICE = "SundayRec telemetry admin key";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// The band ladder, smallest first. Printed in THIS order rather than by count,
// for the reason `summary.ts` orders its query the same way: the shape of the
// distribution is the answer, and sorting by size scatters the ladder and hides
// the two things worth seeing — a lean toward one direction, and a pile-up in
// the widest band. The strings are the wire vocabulary owned by
// `crates/sundayrec-core/src/telemetry/corrections.rs`.
const BANDS = ["under_15s", "15_30s", "30_60s", "60_120s", "over_120s"];
const BAND_LABEL = {
  under_15s: "< 15 s",
  "15_30s": "15–30 s",
  "30_60s": "30–60 s",
  "60_120s": "60–120 s",
  over_120s: "≥ 120 s",
};

// What each signal is a correction OF, in the words a reader needs to turn a
// row into a proposal. The two families answer different questions and must not
// be added together: a `sermon_pick_*` row says the detector promoted the WRONG
// BLOCK, a `sermon_start`/`sermon_end` row says the block was right and the
// edges were not.
const SIGNALS = [
  ["sermon_start", "the trim's start edge (right block, wrong edge)"],
  ["sermon_end", "the trim's end edge (right block, wrong edge)"],
  [
    "sermon_pick_start",
    "a different block was promoted — measured at its start",
  ],
  ["sermon_pick_end", "the same correction, measured at its end"],
];

// What "earlier"/"later" means for a START edge, spelled out because the sign
// convention is the one thing here that is invisible when inverted: the counts
// stay plausible and the tuning goes the wrong way.
const DIRECTION_MEANING = {
  sermon_start: {
    earlier: "the detector opened TOO LATE and cut off the beginning",
    later: "the detector opened too early and swept in what came before",
  },
  sermon_end: {
    earlier: "the detector ran on past the end",
    later: "the detector closed too early and cut the ending",
  },
};

// The counters that say how much OPPORTUNITY there was to correct anything.
// Zero corrections against a zero here is not evidence about the detector; zero
// corrections against a large one is weak evidence, and the difference is the
// whole reason both numbers are printed.
const EXPOSURE_COUNTERS = ["editor.opened", "review.published"];

// ── The evidence bar, read from the harness rather than restated ─────────────
//
// `MIN_CORPUS_FOR_ANY_CONCLUSION` and `MIN_CORPUS_FOR_A_DIRECTION` are derived
// in `ab_eval.rs` from the sign test's own arithmetic, and their doc comments
// carry the derivation. Copying the numbers here would create a second place
// they live, and the copy would be the one that goes stale — so they are parsed
// out of the Rust source, and a parse failure is a hard error rather than a
// default. See docs/LEARNING.md for why the same two floors govern a fleet
// direction skew and not only the A/B corpus.
const AB_EVAL_PATH = "crates/sundayrec-core/src/ab_eval.rs";

export function parseEvidenceBar(source) {
  const read = (name) => {
    const m = source.match(new RegExp(`pub const ${name}: usize = (\\d+);`));
    return m ? Number(m[1]) : undefined;
  };
  const anyConclusion = read("MIN_CORPUS_FOR_ANY_CONCLUSION");
  const aDirection = read("MIN_CORPUS_FOR_A_DIRECTION");
  if (anyConclusion === undefined || aDirection === undefined) {
    throw new Error(
      `could not read the evidence bar from ${AB_EVAL_PATH} — the constants ` +
        `MIN_CORPUS_FOR_ANY_CONCLUSION / MIN_CORPUS_FOR_A_DIRECTION were not ` +
        `found. Refusing to print a report with an invented bar.`,
    );
  }
  return { anyConclusion, aDirection };
}

// ── The sign test, exactly ──────────────────────────────────────────────────
//
// Two-sided, over `n` corrections of one signal of which `k` fell the minority
// way: p = 2 · Σ_{i≤k} C(n,i) / 2ⁿ, capped at 1. This is the same test
// `ab_eval` reasons about, applied to the other axis — there, discordant
// recordings; here, the direction a boundary was dragged.
//
// BigInt rather than floats: the sum and the denominator are both exact
// integers, and 2⁻ⁿ underflows a double at n ≈ 1074. An approximation that
// silently returns 0 for a large corpus would be a p-value nobody could tell
// from a real one.
export function signTestP(n, k) {
  if (!Number.isInteger(n) || !Number.isInteger(k) || n < 0 || k < 0 || k > n) {
    return undefined;
  }
  if (n === 0) return undefined;
  let choose = 1n; // C(n, 0)
  let sum = 1n;
  for (let i = 1n; i <= BigInt(k); i++) {
    choose = (choose * (BigInt(n) - i + 1n)) / i;
    sum += choose;
  }
  const SCALE = 1_000_000_000_000n;
  const scaled = (2n * sum * SCALE) / 2n ** BigInt(n);
  return Math.min(1, Number(scaled) / Number(SCALE));
}

// ── Shaping the summary into the rows a human reads ─────────────────────────

/**
 * Fold `correctionBands` into one entry per signal: the counts per band, the
 * two direction totals, and the sign test over them.
 *
 * Pure, and takes the parsed summary rather than fetching one, so the renderer
 * can be tested against a corpus that does not exist yet — which today is the
 * only kind there is.
 */
export function summariseCorrections(summary, bar) {
  const rows = Array.isArray(summary?.correctionBands)
    ? summary.correctionBands
    : [];
  const out = [];
  for (const [signal, meaning] of SIGNALS) {
    const mine = rows.filter((r) => r.signal === signal);
    const byBand = {};
    for (const band of BANDS) {
      byBand[band] = { earlier: 0, later: 0 };
    }
    let earlier = 0;
    let later = 0;
    // A band or direction this build does not know is DROPPED, never folded
    // into a neighbour — the same rule as `CorrectionKey::from_wire`. But it is
    // counted and printed, because the only way it can happen is a client that
    // got ahead of this tool, and a vocabulary mismatch that reads as "fewer
    // corrections" is the kind of quiet loss this whole file exists to refuse.
    let unrecognised = 0;
    for (const r of mine) {
      const n = Number(r.n) || 0;
      const known =
        BANDS.includes(r.band) &&
        (r.direction === "earlier" || r.direction === "later");
      if (!known) {
        unrecognised += n;
        continue;
      }
      byBand[r.band][r.direction] += n;
      if (r.direction === "earlier") earlier += n;
      else later += n;
    }
    const total = earlier + later;
    const minority = Math.min(earlier, later);
    out.push({
      signal,
      meaning,
      total,
      unrecognised,
      earlier,
      later,
      byBand,
      // `undefined` below the floor, not a number nobody may act on. See the
      // renderer: a p-value printed under a corpus that cannot produce one is
      // the exact mistake this whole tool is built to avoid.
      p: total >= bar.anyConclusion ? signTestP(total, minority) : undefined,
      tier:
        total >= bar.aDirection
          ? "sufficient"
          : total >= bar.anyConclusion
            ? "provisional"
            : "impossible",
    });
  }
  return out;
}

// ── Rendering ───────────────────────────────────────────────────────────────

const pad = (s, w, fill = " ") => String(s).padEnd(w, fill);
const padStart = (s, w) => String(s).padStart(w);

function renderExposure(summary, lines) {
  const counters = Array.isArray(summary?.counterTotals)
    ? summary.counterTotals
    : [];
  const find = (name) =>
    Number(counters.find((c) => c.name === name)?.total ?? 0);
  const raw = summary?.raw ?? {};
  lines.push("WHAT IS REPORTING AT ALL");
  const row = (label, value) =>
    lines.push(`  ${pad(`${label} `, 27, ".")} ${value}`);
  row("events in the window", Number(raw.events ?? 0));
  row("distinct installs", Number(raw.installs ?? 0));
  for (const name of EXPOSURE_COUNTERS) row(name, find(name));
  lines.push("");
}

/**
 * The whole report as text. Pure: takes what the two GETs returned and the
 * evidence bar, returns a string. That is what lets the empty-corpus case be a
 * unit test rather than something only a live run can show.
 */
export function renderReport({ summary, channels, bar, generatedAt }) {
  const lines = [];
  const signals = summariseCorrections(summary, bar);
  const grandTotal = signals.reduce((a, s) => a + s.total, 0);
  const unrecognised = signals.reduce((a, s) => a + s.unrecognised, 0);

  lines.push(
    "SundayRec — what the fleet's corrections say about the sermon detector",
  );
  lines.push(`  source ....... ${ADMIN_BASE_URL}/v1/admin/summary (read-only)`);
  lines.push(`  read at ...... ${generatedAt}`);
  lines.push("");

  // Which build is on each ring, because a correction can only arrive from a
  // build that captures one. A ring serving an older tag explains an empty
  // report without anything being wrong.
  const rings = Array.isArray(channels?.channels) ? channels.channels : [];
  if (rings.length > 0) {
    lines.push("WHAT IS PROMOTED");
    for (const c of rings) {
      const paused = c?.paused ? "  — PAUSED" : "";
      lines.push(
        `  ${pad(c?.channel ?? "?", 8)} ${c?.tag ?? "(nothing promoted yet)"}${paused}`,
      );
    }
    lines.push("");
  }

  renderExposure(summary, lines);

  lines.push("THE EVIDENCE BAR");
  lines.push(
    `  ${bar.anyConclusion} corrections of one signal before ANY direction can be told from chance;`,
  );
  lines.push(
    `  ${bar.aDirection} before anything short of a clean sweep can. Both come from the sign`,
  );
  lines.push(
    `  test's own arithmetic — see MIN_CORPUS_FOR_ANY_CONCLUSION in ${AB_EVAL_PATH}.`,
  );
  lines.push("");

  if (grandTotal === 0) {
    // NO SHARES, NO LADDER, NO BARS. A zero share and an empty histogram both
    // read as a measurement, and there has been no measurement. Say what is
    // missing and stop.
    lines.push("CORRECTIONS");
    lines.push(
      "  NOTHING TO READ — not one correction has reached the aggregate.",
    );
    lines.push("");
    if (unrecognised > 0) {
      // Not the empty case at all: rows arrived, in a vocabulary this tool does
      // not know. Said before the explanation below, because the explanation
      // would be wrong.
      lines.push(
        `  ⚠ …except ${unrecognised} correction${unrecognised === 1 ? "" : "s"} in a signal, direction or band this`,
      );
      lines.push(
        "    tool does not recognise. The client is ahead of the tool — update BANDS/SIGNALS",
      );
      lines.push(
        "    here from crates/sundayrec-core/src/telemetry/corrections.rs.",
      );
      lines.push("");
    }
    lines.push(
      "  This is not a detector that is right. It is a detector nobody has",
    );
    lines.push(
      "  corrected: no install has both consented and moved a sermon boundary,",
    );
    lines.push(
      "  or the builds that capture corrections are not on anyone's machine yet.",
    );
    lines.push(
      "  No constant may move on this. Check the exposure counters above — an",
    );
    lines.push(
      "  editor nobody opened and an editor nobody corrected are different facts.",
    );
    lines.push("");
    lines.push(renderCompanion(summary));
    lines.push(renderWindowNote(summary));
    return lines.join("\n").trimEnd() + "\n";
  }

  lines.push("CORRECTIONS — counts, per signal, smallest band first");
  lines.push("");
  for (const s of signals) {
    lines.push(`  ${s.signal}  —  ${s.meaning}`);
    if (s.total === 0) {
      lines.push(
        s.unrecognised > 0
          ? `      ⚠ ${s.unrecognised} correction${s.unrecognised === 1 ? "" : "s"} arrived, all in a direction or band this tool does not recognise.`
          : "      (none)",
      );
      lines.push("");
      continue;
    }
    lines.push(
      `      ${pad("band", 10)}${padStart("earlier", 9)}${padStart("later", 8)}`,
    );
    for (const band of BANDS) {
      const cell = s.byBand[band];
      if (cell.earlier === 0 && cell.later === 0) continue;
      lines.push(
        `      ${pad(BAND_LABEL[band], 10)}${padStart(cell.earlier, 9)}${padStart(cell.later, 8)}`,
      );
    }
    lines.push(
      `      ${pad("total", 10)}${padStart(s.earlier, 9)}${padStart(s.later, 8)}   (${s.total})`,
    );
    if (s.unrecognised > 0) {
      lines.push(
        `      ⚠ ${s.unrecognised} more in a direction or band this tool does not recognise — dropped, not folded in.`,
      );
    }
    lines.push(`      ${renderSkew(s, bar)}`);
    lines.push("");
  }

  lines.push(renderCompanion(summary));
  lines.push(renderWindowNote(summary));
  return lines.join("\n").trimEnd() + "\n";
}

/**
 * One sentence per signal about whether the split is worth acting on, and — for
 * the two edge signals — what acting on it would MEAN. Shares appear here and
 * only here, and only above the floor: below it, a share is one correction
 * wearing a percentage (`ab_eval::OVER_THRESHOLDS_SEC` makes the same call for
 * the same reason).
 */
function renderSkew(s, bar) {
  if (s.tier === "impossible") {
    return `→ ${s.total} correction${s.total === 1 ? "" : "s"}: below the bar of ${bar.anyConclusion}. No direction can be stated.`;
  }
  const lean =
    s.earlier === s.later ? null : s.earlier > s.later ? "earlier" : "later";
  const share = Math.round((Math.max(s.earlier, s.later) / s.total) * 100);
  const p = s.p;
  const pText = p === undefined ? "" : ` (sign test p = ${p.toFixed(3)})`;
  if (lean === null) {
    return `→ dead even. Nothing to move.${pText}`;
  }
  const meaning = DIRECTION_MEANING[s.signal]?.[lean];
  const because = meaning ? ` — ${meaning}` : "";
  if (p !== undefined && p < 0.05) {
    const strength =
      s.tier === "sufficient"
        ? "This clears the bar"
        : `This clears α = 0.05 but the corpus is under ${bar.aDirection}, so only a clean sweep counts`;
    return `→ ${share} % lean ${lean}${because}. ${strength}${pText}.`;
  }
  return `→ ${share} % lean ${lean}${because}, but not distinguishable from a coin flip${pText}.`;
}

/** The companion's outcomes, which the same ritual reads and the same bar governs. */
function renderCompanion(summary) {
  const rows = Array.isArray(summary?.companionOutcomes)
    ? summary.companionOutcomes
    : [];
  const total = rows.reduce((a, r) => a + (Number(r.n) || 0), 0);
  if (total === 0) {
    return "COMPANION SUGGESTIONS\n  NOTHING TO READ — no outcome has been reported.\n";
  }
  const lines = ["COMPANION SUGGESTIONS — counts per kind"];
  const kinds = [...new Set(rows.map((r) => r.kind))].sort();
  for (const kind of kinds) {
    const mine = rows.filter((r) => r.kind === kind);
    const n = mine.reduce((a, r) => a + (Number(r.n) || 0), 0);
    const parts = mine
      .map((r) => `${r.outcome} ${Number(r.n) || 0}`)
      .join(", ");
    lines.push(`  ${pad(kind, 12)} ${parts}   (${n})`);
  }
  lines.push("");
  return lines.join("\n");
}

/**
 * What this view CANNOT see, printed every time rather than only when it bites.
 * Two limits, both structural:
 *
 *   - the summary's correction rows come from `event_corrections`, which is raw
 *     and purged at the retention cutoff. Everything older survives only in
 *     `agg_corrections`, which no admin route exposes — it takes a
 *     `wrangler d1 execute` session against the database;
 *   - a correction row carries no install id (by design — migration 0004), so
 *     nothing here can tell forty corrections from one church apart from four
 *     each from ten. The sign test above assumes they are independent, and that
 *     assumption cannot be checked from this data.
 */
function renderWindowNote(summary) {
  const days = summary?.retention?.retentionDays;
  const oldest = summary?.raw?.oldest;
  const lines = ["WHAT THIS VIEW CANNOT SEE"];
  lines.push(
    `  • Only the raw window${days ? ` (${days} days)` : ""}${
      oldest ? `, oldest row ${new Date(Number(oldest)).toISOString()}` : ""
    }. Older corrections live`,
  );
  lines.push(
    "    on in agg_corrections, which no admin route serves — read it with wrangler d1.",
  );
  lines.push(
    "  • No install id on a correction row (migration 0004). Forty corrections from",
  );
  lines.push(
    "    one church and four each from ten look identical here; the sign test above",
  );
  lines.push("    assumes independence and cannot check it.");
  lines.push(
    "  • Only corrections from installs that consented. A church that never corrects",
  );
  lines.push("    is not evidence the detector is right.");
  return lines.join("\n");
}

// ── Keychain ────────────────────────────────────────────────────────────────
// `spawnSync` runs no shell, so the key never round-trips through anything that
// could echo or log a command line. Returned for immediate use as a header
// value — never written to stdout/stderr anywhere in this file.
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

function fail(msg) {
  console.error(`✗ ${msg}`);
  process.exit(1);
}

// ── HTTP: one function, one method ──────────────────────────────────────────
async function get(path, key) {
  let res;
  try {
    res = await fetch(`${ADMIN_BASE_URL}${path}`, {
      method: "GET",
      headers: { "x-admin-key": key },
    });
  } catch (e) {
    fail(`request to ${path} failed: ${e.message}`);
  }
  const text = await res.text();
  if (!res.ok) {
    // The body may carry an error code; it never carries the key.
    fail(`GET ${path} returned HTTP ${res.status}: ${text.slice(0, 200)}`);
  }
  try {
    return JSON.parse(text);
  } catch (e) {
    fail(`GET ${path} did not return JSON (${e.message}).`);
  }
}

// ── main ────────────────────────────────────────────────────────────────────
async function main() {
  const wantJson = process.argv.slice(2).includes("--json");
  const bar = parseEvidenceBar(readFileSync(join(ROOT, AB_EVAL_PATH), "utf8"));

  const key = readAdminKey();
  const summary = await get("/v1/admin/summary", key);
  const channels = await get("/v1/admin/channels", key);

  if (wantJson) {
    console.log(JSON.stringify({ summary, channels }, null, 2));
    return;
  }

  process.stdout.write(
    renderReport({
      summary,
      channels,
      bar,
      generatedAt: new Date(
        Number(summary?.generatedAt) || Date.now(),
      ).toISOString(),
    }),
  );
}

// Only run when executed as a script, so the tests can import the pure parts
// without anything reaching for the Keychain or the network.
if (
  process.argv[1] &&
  realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))
) {
  await main();
}
