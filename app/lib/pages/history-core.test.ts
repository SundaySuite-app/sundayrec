import { describe, expect, it } from "vitest";
import {
  baseNoExt,
  filterRecordings,
  historyTotals,
  isVideoRow,
  pairRecordings,
  sortRecordings,
  type RecordingEntry,
} from "./history-core";

/** A row with only the fields the function under test looks at. */
function row(p: Partial<RecordingEntry> & { path?: string }): RecordingEntry {
  return { status: "ok", ...p };
}

// The recorder writes the separate-audio sidecar next to the video with the
// SAME stem in the SAME directory, so these are one session.
const VIDEO = "/rec/gudstjeneste-2026-05-31.mp4";
const AUDIO = "/rec/gudstjeneste-2026-05-31.flac";

describe("isVideoRow", () => {
  it("decides by container extension", () => {
    expect(isVideoRow(row({ path: VIDEO }))).toBe(true);
    expect(isVideoRow(row({ path: AUDIO }))).toBe(false);
    expect(isVideoRow(row({ path: "/rec/a.MOV" }))).toBe(true);
  });

  it("falls back to the legacy Electron note when there is no path", () => {
    expect(isVideoRow(row({ note: "Video" }))).toBe(true);
    expect(isVideoRow(row({ note: "Preken om nåde" }))).toBe(false);
  });
});

describe("baseNoExt", () => {
  it("strips only the final extension and tolerates a missing path", () => {
    expect(baseNoExt(VIDEO)).toBe("/rec/gudstjeneste-2026-05-31");
    expect(baseNoExt(AUDIO)).toBe("/rec/gudstjeneste-2026-05-31");
    expect(baseNoExt(undefined)).toBe("");
  });

  it("does not eat a dot that belongs to a directory name", () => {
    expect(baseNoExt("/rec/v1.2/opptak")).toBe("/rec/v1.2/opptak");
  });
});

describe("pairRecordings", () => {
  it("collapses the audio + video of one session into a single row", () => {
    // Video first, as the newest-first history lists it.
    const rows = [row({ path: VIDEO }), row({ path: AUDIO })];
    const out = pairRecordings(rows);
    expect(out).toHaveLength(1);
    expect(out[0].r.path).toBe(AUDIO);
    expect(out[0].videoEntry?.path).toBe(VIDEO);
  });

  it("pairs regardless of which half comes first", () => {
    const out = pairRecordings([row({ path: AUDIO }), row({ path: VIDEO })]);
    expect(out).toHaveLength(1);
    expect(out[0].r.path).toBe(AUDIO);
    expect(out[0].videoEntry?.path).toBe(VIDEO);
  });

  it("leaves a video-only session as its own row", () => {
    const out = pairRecordings([row({ path: VIDEO })]);
    expect(out).toHaveLength(1);
    expect(out[0].r.path).toBe(VIDEO);
    expect(out[0].videoEntry).toBeNull();
  });

  it("does NOT pair two recordings that merely share a start time", () => {
    // This is the case the old adjacency heuristic got wrong: same minute,
    // different recordings (two rooms, or a re-take).
    const rows = [
      row({
        path: "/rec/sal-a.mp4",
        date: "2026-05-31",
        startTime: "11:00",
        note: "Video",
      }),
      row({ path: "/rec/sal-b.flac", date: "2026-05-31", startTime: "11:00" }),
    ];
    const out = pairRecordings(rows);
    expect(out).toHaveLength(2);
    expect(out.every((p) => p.videoEntry === null)).toBe(true);
  });

  it("pairs one audio with one video and keeps the extras when three rows collide", () => {
    const rows = [
      row({ path: VIDEO }),
      row({ path: AUDIO }),
      row({ path: "/rec/gudstjeneste-2026-05-31.wav" }),
    ];
    const out = pairRecordings(rows);
    expect(out).toHaveLength(2);
    // The pair anchors where the first of its two halves sat.
    expect(out[0].r.path).toBe(AUDIO);
    expect(out[0].videoEntry?.path).toBe(VIDEO);
    // The third file is never swallowed.
    expect(out[1].r.path).toBe("/rec/gudstjeneste-2026-05-31.wav");
    expect(out[1].videoEntry).toBeNull();
  });

  it("keeps the incoming (sorted) order and anchors a pair at its first half", () => {
    const rows = [
      row({ path: "/rec/nyest.flac" }),
      row({ path: VIDEO }),
      row({ path: AUDIO }),
      row({ path: "/rec/eldst.flac" }),
    ];
    expect(pairRecordings(rows).map((p) => p.r.path)).toEqual([
      "/rec/nyest.flac",
      AUDIO,
      "/rec/eldst.flac",
    ]);
  });

  it("never pairs rows that have no path to match on", () => {
    const rows = [row({ note: "Video", date: "x" }), row({ date: "x" })];
    expect(pairRecordings(rows)).toHaveLength(2);
  });

  it("returns an empty list for no rows", () => {
    expect(pairRecordings([])).toEqual([]);
  });
});

describe("sortRecordings", () => {
  const rows = [
    row({ path: "/a.flac", timestamp: 200, durationSec: 60 }),
    row({ path: "/b.flac", timestamp: 100, durationSec: 3600 }),
    row({ path: "/c.flac", timestamp: 300, durationSec: 900 }),
  ];

  it("defaults to newest first when asked for descending time", () => {
    expect(sortRecordings(rows, "time", "desc").map((r) => r.path)).toEqual([
      "/c.flac",
      "/a.flac",
      "/b.flac",
    ]);
  });

  it("sorts oldest first ascending", () => {
    expect(sortRecordings(rows, "time", "asc").map((r) => r.path)).toEqual([
      "/b.flac",
      "/a.flac",
      "/c.flac",
    ]);
  });

  it("sorts by real duration seconds, longest first", () => {
    expect(sortRecordings(rows, "duration", "desc").map((r) => r.path)).toEqual(
      ["/b.flac", "/c.flac", "/a.flac"],
    );
  });

  it("does not mutate the input", () => {
    const before = rows.map((r) => r.path);
    sortRecordings(rows, "duration", "asc");
    expect(rows.map((r) => r.path)).toEqual(before);
  });

  it("is stable for ties", () => {
    const tied = [
      row({ path: "/first.flac", durationSec: 10 }),
      row({ path: "/second.flac", durationSec: 10 }),
    ];
    expect(sortRecordings(tied, "duration", "desc").map((r) => r.path)).toEqual(
      ["/first.flac", "/second.flac"],
    );
  });

  it("falls back to the date string when there is no numeric timestamp", () => {
    const byDate = [
      row({ path: "/old.flac", date: "2026-01-01T10:00:00.000Z" }),
      row({ path: "/new.flac", date: "2026-06-01T10:00:00.000Z" }),
    ];
    expect(sortRecordings(byDate, "time", "desc").map((r) => r.path)).toEqual([
      "/new.flac",
      "/old.flac",
    ]);
  });
});

describe("filterRecordings", () => {
  const rows = [
    row({ path: VIDEO }),
    row({ path: AUDIO }),
    row({ path: "/rec/preken.flac" }),
  ];

  it("passes everything through on «Alle»", () => {
    expect(filterRecordings(rows, "all")).toHaveLength(3);
  });

  it("keeps only audio containers on «Lyd»", () => {
    expect(filterRecordings(rows, "audio").map((r) => r.path)).toEqual([
      AUDIO,
      "/rec/preken.flac",
    ]);
  });

  it("keeps only video containers on «Video»", () => {
    expect(filterRecordings(rows, "video").map((r) => r.path)).toEqual([VIDEO]);
  });

  it("does not mutate the input", () => {
    const copy = filterRecordings(rows, "all");
    copy.pop();
    expect(rows).toHaveLength(3);
  });
});

describe("historyTotals", () => {
  it("sums the real seconds instead of re-parsing the display label", () => {
    // The old code regexed "1t 30m" back out of the formatted string and
    // skipped anything that did not match — including this "45 s" row.
    const rows = [
      row({
        durationSec: 5400,
        duration: "1t 30m",
        date: "2026-05-31T11:00:00.000Z",
        timestamp: 3,
      }),
      row({
        durationSec: 45,
        duration: "45 s",
        date: "2026-05-24T11:00:00.000Z",
        timestamp: 2,
      }),
      row({
        durationSec: 600,
        duration: "10m",
        date: "2026-05-17T11:00:00.000Z",
        timestamp: 1,
      }),
    ];
    const totals = historyTotals(rows);
    expect(totals.count).toBe(3);
    expect(totals.totalSec).toBe(6045);
    expect(totals.lastDate).toBe("2026-05-31T11:00:00.000Z");
  });

  it("ignores failed rows — they have no file and no duration", () => {
    const rows = [
      row({ durationSec: 600, timestamp: 2, date: "d2" }),
      row({ status: "error", durationSec: 99999, timestamp: 3, date: "d3" }),
    ];
    const totals = historyTotals(rows);
    expect(totals.count).toBe(1);
    expect(totals.totalSec).toBe(600);
    expect(totals.lastDate).toBe("d2");
  });

  it("finds the newest date even when the rows are not sorted", () => {
    const rows = [
      row({ timestamp: 100, date: "gammel", durationSec: 1 }),
      row({ timestamp: 900, date: "nyest", durationSec: 1 }),
      row({ timestamp: 500, date: "midt", durationSec: 1 }),
    ];
    expect(historyTotals(rows).lastDate).toBe("nyest");
  });

  it("reports zeroes for an empty list", () => {
    expect(historyTotals([])).toEqual({
      count: 0,
      totalSec: 0,
      lastDate: undefined,
    });
  });
});
