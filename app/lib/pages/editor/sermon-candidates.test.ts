import { describe, it, expect } from "vitest";
import type { Suggestion } from "./state";
import {
  sermonCandidates,
  MIN_SERMON_CANDIDATE_SEC,
} from "./sermon-candidates";

const seg = (start: number, end: number, type: string): Suggestion => ({
  start,
  end,
  duration: end - start,
  label: type,
  type,
});

describe("sermonCandidates", () => {
  it("offers speech and sermon blocks, and nothing else", () => {
    const segments = [
      seg(0, 100, "silence"),
      seg(100, 300, "speech"),
      seg(300, 500, "music"),
      seg(500, 800, "sermon"),
      seg(800, 900, "mixed"),
      seg(900, 1000, "unknown"),
    ];
    expect(sermonCandidates(segments).map((c) => c.index)).toEqual([1, 3]);
  });

  it("drops blocks shorter than the one-minute floor", () => {
    const segments = [
      seg(0, MIN_SERMON_CANDIDATE_SEC - 1, "speech"),
      seg(100, 100 + MIN_SERMON_CANDIDATE_SEC, "speech"),
    ];
    expect(sermonCandidates(segments).map((c) => c.index)).toEqual([1]);
  });

  // ── The bug this module exists for ────────────────────────────────────────
  //
  // A 20 s speech block ahead of the real candidates. It is not offered, so the
  // offered list is three long while the speech-like list is four long — and the
  // old code numbered options off the former and resolved them against the
  // latter. Picking the third option promoted the second block.
  it("tags each offer with its SOURCE index, not its position in the offer list", () => {
    const segments = [
      seg(0, 20, "speech"), // 20 s — never offered
      seg(20, 200, "sermon"),
      seg(200, 400, "speech"),
      seg(400, 600, "speech"),
    ];
    const candidates = sermonCandidates(segments);
    expect(candidates).toHaveLength(3);
    // Display position 0,1,2 → source segments 1,2,3. Off by exactly the one
    // block that was filtered out, which is the whole bug.
    expect(candidates.map((c) => c.index)).toEqual([1, 2, 3]);
    // Picking the LAST offer must resolve to the block starting at 400 s.
    expect(segments[candidates[2].index].start).toBe(400);
  });

  it("sorts by start time, and the tag survives the sort", () => {
    const segments = [
      seg(400, 600, "speech"),
      seg(0, 200, "sermon"),
      seg(200, 400, "speech"),
    ];
    const candidates = sermonCandidates(segments);
    expect(candidates.map((c) => c.segment.start)).toEqual([0, 200, 400]);
    expect(candidates.map((c) => c.index)).toEqual([1, 2, 0]);
    // Resolving any offer gets back the very segment that was offered.
    for (const c of candidates) expect(segments[c.index]).toBe(c.segment);
  });

  it("leaves the source array untouched", () => {
    const segments = [seg(400, 600, "speech"), seg(0, 200, "sermon")];
    const before = segments.slice();
    sermonCandidates(segments);
    expect(segments).toEqual(before);
    expect(segments[0].start).toBe(400);
  });

  it("has nothing to offer for an unanalysed recording", () => {
    expect(sermonCandidates([])).toEqual([]);
  });
});
