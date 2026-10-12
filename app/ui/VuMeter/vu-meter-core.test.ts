import { describe, expect, it } from "vitest";

import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";

import { STALE_MS, effectiveLevel, needsRepaint } from "./vu-meter-core";

describe("effectiveLevel — strømmen stopper", () => {
  it("følger siste pakke så lenge pakker kommer", () => {
    expect(effectiveLevel(-12, 1000, 1000 + STALE_MS)).toBe(-12);
  });

  it("faller til gulvet når strømmen har vært stille lenger enn STALE_MS", () => {
    expect(effectiveLevel(-12, 1000, 1000 + STALE_MS + 1)).toBe(VU_FLOOR_DB);
  });

  it("uten noen pakke ennå er nivået uendret (gulvet fra start)", () => {
    expect(effectiveLevel(VU_FLOOR_DB, null, 99999)).toBe(VU_FLOOR_DB);
  });
});

describe("needsRepaint", () => {
  const a = { l: -30, r: -30, mono: false };

  it("tegner første gang og når noe er skittent", () => {
    expect(needsRepaint(null, a, false)).toBe(true);
    expect(needsRepaint(a, a, true)).toBe(true);
  });

  it("tegner ikke på nytt når ingenting har beveget seg", () => {
    expect(needsRepaint(a, { ...a, l: -30.01 }, false)).toBe(false);
  });

  it("tegner når en stolpe flytter seg eller mono bytter", () => {
    expect(needsRepaint(a, { ...a, r: -20 }, false)).toBe(true);
    expect(needsRepaint(a, { ...a, mono: true }, false)).toBe(true);
  });
});
