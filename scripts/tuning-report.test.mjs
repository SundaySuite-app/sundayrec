// The tuning report's two promises: it says nothing over nothing, and it cannot
// act.
//
// The empty-corpus assertion mirrors `ab_eval`'s
// `an_empty_corpus_produces_a_report_that_says_so` — including the one that
// matters most, that no `%` appears anywhere. A zero share and a bar chart of
// nothing both read as measurements, and today there has been no measurement.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  parseEvidenceBar,
  renderReport,
  signTestP,
  summariseCorrections,
} from "./tuning-report.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(join(HERE, "tuning-report.mjs"), "utf8");

const BAR = { anyConclusion: 6, aDirection: 12 };

// The shape `GET /v1/admin/summary` returns when the tables are empty — which
// is what it returned on 2026-08-08, the day this was written.
const emptySummary = {
  ok: true,
  generatedAt: 1_754_600_000_000,
  retention: {
    retentionDays: 90,
    rawRowsOlderThanCutoff: 0,
    promiseHeld: true,
  },
  raw: { events: 0, installs: 0, oldest: null, newest: null },
  crashesByVersionOs: [],
  verdicts: [],
  findingsWeekOverWeek: [],
  counterTotals: [],
  wakeFailures: [],
  correctionBands: [],
  companionOutcomes: [],
  aggregates: { n: 0, oldest_day: null },
};

const channels = {
  channels: [
    { channel: "stable", tag: "v0.11.0", paused: false },
    { channel: "beta", tag: "v0.11.1-beta.2", paused: false },
  ],
};

const band = (signal, direction, band_, n) => ({
  signal,
  direction,
  band: band_,
  n,
  pctOfSignal: 0,
});

describe("the empty corpus", () => {
  const text = renderReport({
    summary: emptySummary,
    channels,
    bar: BAR,
    generatedAt: "2026-08-08T00:00:00.000Z",
  });

  it("says there is nothing to read", () => {
    expect(text).toContain("NOTHING TO READ");
    expect(text).toContain("not one correction has reached the aggregate");
  });

  it("prints no percentage at all", () => {
    expect(text).not.toContain("%");
  });

  it("refuses to read silence as a detector that is right", () => {
    expect(text).toContain("This is not a detector that is right");
    expect(text).toContain("No constant may move on this");
  });

  it("still states the evidence bar, so the distance is legible", () => {
    expect(text).toContain("THE EVIDENCE BAR");
    expect(text).toContain("6 corrections of one signal");
    expect(text).toContain("12 before anything short of a clean sweep");
  });

  it("still says what the view cannot see", () => {
    expect(text).toContain("WHAT THIS VIEW CANNOT SEE");
    expect(text).toContain("assumes independence");
  });
});

describe("a corpus below the bar", () => {
  const summary = {
    ...emptySummary,
    raw: { events: 4, installs: 1 },
    correctionBands: [
      band("sermon_start", "earlier", "30_60s", 3),
      band("sermon_start", "later", "under_15s", 1),
    ],
  };
  const text = renderReport({
    summary,
    channels,
    bar: BAR,
    generatedAt: "2026-08-08T00:00:00.000Z",
  });

  it("prints the counts", () => {
    expect(text).toContain("30–60 s");
  });

  it("states no direction and no share below the floor", () => {
    expect(text).toContain("below the bar of 6");
    expect(text).not.toContain("%");
  });
});

describe("a corpus that clears the bar", () => {
  const summary = {
    ...emptySummary,
    raw: { events: 40, installs: 3 },
    correctionBands: [
      band("sermon_start", "earlier", "30_60s", 10),
      band("sermon_start", "earlier", "15_30s", 4),
      band("sermon_start", "later", "under_15s", 2),
    ],
  };
  const text = renderReport({
    summary,
    channels,
    bar: BAR,
    generatedAt: "2026-08-08T00:00:00.000Z",
  });

  it("names the direction and what it means for the detector", () => {
    expect(text).toContain("lean earlier");
    expect(text).toContain("opened TOO LATE");
  });

  it("prints the sign test rather than only the share", () => {
    expect(text).toMatch(/sign test p = 0\.\d{3}/);
  });
});

describe("summariseCorrections", () => {
  it("keeps the two correction families apart", () => {
    const rows = summariseCorrections(
      {
        correctionBands: [
          band("sermon_start", "earlier", "30_60s", 3),
          band("sermon_pick_start", "later", "over_120s", 5),
        ],
      },
      BAR,
    );
    const byName = Object.fromEntries(rows.map((r) => [r.signal, r]));
    expect(byName["sermon_start"].total).toBe(3);
    expect(byName["sermon_pick_start"].total).toBe(5);
    expect(byName["sermon_end"].total).toBe(0);
  });

  it("drops a band this build cannot express rather than folding it into a neighbour", () => {
    // Same rule as `CorrectionKey::from_wire` — but counted separately, because
    // the only way this happens is a client ahead of this tool, and a
    // vocabulary mismatch that reads as "fewer corrections" is exactly the
    // quiet loss the tool exists to refuse.
    const [first] = summariseCorrections(
      { correctionBands: [band("sermon_start", "earlier", "7_9s", 4)] },
      BAR,
    );
    expect(first.total).toBe(0);
    expect(first.earlier).toBe(0);
    expect(first.unrecognised).toBe(4);
  });

  it("says so rather than reporting an empty corpus", () => {
    const text = renderReport({
      summary: {
        ...emptySummary,
        correctionBands: [band("sermon_start", "sideways", "30_60s", 9)],
      },
      channels,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    expect(text).toContain("does not recognise");
    expect(text).toContain("The client is ahead of the tool");
    expect(text).not.toContain("%");
  });
});

describe("a signal this tool does not know", () => {
  // The wire vocabulary can grow a NEW signal name before this tool learns it —
  // the exact "client ahead of the tool" case the band/direction rule already
  // covers. The same discipline must govern the signal axis: dropping is right
  // (there is nothing to attribute the rows to), dropping SILENTLY reads as
  // "fewer corrections", which is the quiet loss the file's own header refuses.
  const alien = band("sermon_pick", "earlier", "30_60s", 40);

  it("does not read as an empty corpus without a word about the rows", () => {
    const text = renderReport({
      summary: { ...emptySummary, correctionBands: [alien] },
      channels,
      bar: BAR,
      generatedAt: "2026-08-09T00:00:00.000Z",
    });
    expect(text).toContain("does not recognise");
    expect(text).toContain("The client is ahead of the tool");
    expect(text).not.toContain("%");
  });

  it("is called out beside real counts too, not only over an otherwise-empty corpus", () => {
    const text = renderReport({
      summary: {
        ...emptySummary,
        correctionBands: [band("sermon_start", "earlier", "30_60s", 7), alien],
      },
      channels,
      bar: BAR,
      generatedAt: "2026-08-09T00:00:00.000Z",
    });
    expect(text).toContain("does not recognise");
  });

  it("history rows under an unknown signal are counted the same way", () => {
    const text = renderReport({
      summary: emptySummary,
      channels,
      history: {
        corrections: {
          rows: [
            {
              signal: "sermon_pick",
              direction: "earlier",
              band: "30_60s",
              total: 4,
            },
          ],
          byVersion: [],
          span: { days: 1, total: 4 },
        },
        companion: { rows: [], span: { days: 0 } },
      },
      bar: BAR,
      generatedAt: "2026-08-09T00:00:00.000Z",
    });
    expect(text).toContain("does not recognise");
  });
});

describe("signTestP — the arithmetic the evidence bar rests on", () => {
  it("reproduces the numbers ab_eval's constants are derived from", () => {
    // "At n = 5 that is 0.0625: a clean sweep still would not be significant.
    //  At n = 6 it is 0.031."
    expect(signTestP(5, 0)).toBeCloseTo(0.0625, 6);
    expect(signTestP(6, 0)).toBeCloseTo(0.03125, 6);
    // "At n = 12 a 10–2 split reaches p = 0.039 … (9–3 does not: p > 0.05)"
    expect(signTestP(12, 2)).toBeCloseTo(0.0386, 3);
    expect(signTestP(12, 3)).toBeGreaterThan(0.05);
  });

  it("caps at 1 for an even split", () => {
    expect(signTestP(10, 5)).toBe(1);
  });

  it("does not underflow to a fake zero on a large corpus", () => {
    // 2⁻ⁿ underflows a double past n ≈ 1074; BigInt does not.
    expect(signTestP(2000, 900)).toBeGreaterThan(0);
    expect(signTestP(2000, 900)).toBeLessThan(1);
  });
});

describe("parseEvidenceBar", () => {
  it("reads the constants out of the real ab_eval.rs", () => {
    const source = readFileSync(
      join(HERE, "..", "crates/sundayrec-core/src/ab_eval.rs"),
      "utf8",
    );
    const bar = parseEvidenceBar(source);
    expect(bar.anyConclusion).toBe(6);
    expect(bar.aDirection).toBe(12);
  });

  it("throws rather than defaulting when the constants move", () => {
    expect(() => parseEvidenceBar("// nothing here")).toThrow(
      /could not read the evidence bar/,
    );
  });

  it("does not read a commented-out declaration as the live constant", () => {
    // The plausible wrong state: someone comments the old line out while moving
    // or renaming the constant. A parser that reads the comment reports a bar
    // nobody derived; failing loud is the whole point of the hard error.
    const ghost = [
      "// pub const MIN_CORPUS_FOR_ANY_CONCLUSION: usize = 99;",
      "// pub const MIN_CORPUS_FOR_A_DIRECTION: usize = 5;",
    ].join("\n");
    expect(() => parseEvidenceBar(ghost)).toThrow(
      /could not read the evidence bar/,
    );
  });
});

describe("read-only, structurally", () => {
  it("names no HTTP method but GET", () => {
    const methods = [...SOURCE.matchAll(/method:\s*"([A-Z]+)"/g)].map(
      (m) => m[1],
    );
    // Two fetch helpers (`get`, `getOptional`), one verb between them. The
    // assertion is on the SET: any second verb is a write path and must fail.
    expect(methods.length).toBeGreaterThan(0);
    expect([...new Set(methods)]).toEqual(["GET"]);
  });

  it("names no route but the three GETs it reads", () => {
    const routes = new Set(
      [...SOURCE.matchAll(/\/v1\/[a-z/]+/g)].map((m) => m[0]),
    );
    expect([...routes].sort()).toEqual([
      "/v1/admin/channels",
      "/v1/admin/history",
      "/v1/admin/summary",
    ]);
  });
});

// ── The history route: the fold beyond the raw window ───────────────────────
//
// Three promises: history rows COUNT (evidence does not expire), a 404 is said
// out loud rather than papered over, and every discipline above — no shares
// under the floor, unknown vocabulary dropped-and-counted — governs the merged
// corpus exactly as it governed the raw one.
describe("the folded history", () => {
  const histRow = (signal, direction, band_, total) => ({
    signal,
    direction,
    band: band_,
    total,
  });

  it("merges history rows into the same fold as the raw window", () => {
    const summary = {
      ...emptySummary,
      raw: { events: 2, installs: 1 },
      correctionBands: [band("sermon_start", "earlier", "30_60s", 2)],
    };
    const history = {
      beyondRawWindow: true,
      corrections: {
        rows: [
          histRow("sermon_start", "earlier", "30_60s", 9),
          histRow("sermon_start", "later", "under_15s", 1),
        ],
        byVersion: [],
        span: {
          first_day: "2026-01-01",
          last_day: "2026-03-01",
          days: 12,
          total: 10,
        },
      },
      companion: { rows: [], span: { days: 0 } },
    };
    const rows = summariseCorrections(summary, BAR, history);
    const s = rows.find((r) => r.signal === "sermon_start");
    expect(s.earlier).toBe(11);
    expect(s.later).toBe(1);
    expect(s.total).toBe(12);
    // 12 total meets the direction bar, so the sign test may now speak —
    // history evidence alone lifted it over the floor.
    expect(s.tier).toBe("sufficient");

    const text = renderReport({
      summary,
      channels,
      history,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    expect(text).toContain(
      "(2 in the raw window + 10 from the folded history)",
    );
    expect(text).toContain("+ /v1/admin/history");
    expect(text).toContain("Nothing by age");
  });

  it("a 404 (history: null) says the view is truncated, in so many words", () => {
    const text = renderReport({
      summary: emptySummary,
      channels,
      history: null,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    expect(text).toContain("is not");
    expect(text).toContain("deployed on this Worker yet");
    expect(text).toContain("wrangler d1");
    expect(text).not.toContain("+ /v1/admin/history");
  });

  it("below-the-floor stays percent-free even when history contributes", () => {
    const summary = {
      ...emptySummary,
      raw: { events: 1, installs: 1 },
      correctionBands: [band("sermon_start", "earlier", "30_60s", 1)],
    };
    const history = {
      corrections: {
        rows: [histRow("sermon_start", "earlier", "60_120s", 2)],
        byVersion: [],
        span: { days: 2, total: 2 },
      },
      companion: { rows: [], span: { days: 0 } },
    };
    const text = renderReport({
      summary,
      channels,
      history,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    // 3 total: under anyConclusion. The merged corpus obeys the same floor.
    expect(text).not.toContain("%");
    expect(text).toContain("below the bar");
  });

  it("history rows in an unknown vocabulary are dropped and counted, not folded", () => {
    const history = {
      corrections: {
        rows: [histRow("sermon_start", "sideways", "30_60s", 4)],
        byVersion: [],
        span: { days: 1, total: 4 },
      },
      companion: { rows: [], span: { days: 0 } },
    };
    const rows = summariseCorrections(emptySummary, BAR, history);
    const s = rows.find((r) => r.signal === "sermon_start");
    expect(s.total).toBe(0);
    expect(s.unrecognised).toBe(4);
  });

  it("companion outcomes are reported as not collected since v0.15, with the leftover count", () => {
    // The AI companion left in v0.15 and `companionOutcomes` left the wire
    // with it. Rows the Worker still holds from older builds are counted as
    // history, never rendered as if a constant could move on them.
    const summary = {
      ...emptySummary,
      companionOutcomes: [{ kind: "title", outcome: "kept", n: 3 }],
    };
    const history = {
      corrections: { rows: [], byVersion: [], span: { days: 0 } },
      companion: {
        rows: [{ kind: "title", outcome: "kept", total: 7 }],
        span: { days: 5, total: 7 },
      },
    };
    const text = renderReport({
      summary,
      channels,
      history,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    expect(text).toContain("COMPANION SUGGESTIONS — NOT COLLECTED since v0.15");
    expect(text).toContain("10 historical outcome(s)");
    expect(text).not.toContain("kept 3");

    const none = renderReport({
      summary: emptySummary,
      channels,
      history: null,
      bar: BAR,
      generatedAt: "2026-08-08T00:00:00.000Z",
    });
    expect(none).toContain("NOT COLLECTED since v0.15");
    expect(none).not.toContain("historical outcome");
  });
});
