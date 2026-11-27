import { describe, expect, it } from "vitest";

import {
  FLAT_BAR,
  flatBars,
  isBars,
  PREVIEW_BARS,
  previewBars,
} from "./peaks-preview";

describe("previewBars", () => {
  it("er en flat linje uten topper", () => {
    expect(previewBars([])).toEqual(flatBars());
    expect(previewBars(new Float32Array(0))).toHaveLength(PREVIEW_BARS);
  });

  it("er en flat linje for ren stillhet og for ikke-endelige tall", () => {
    expect(previewBars(new Float32Array(1000))).toEqual(flatBars());
    expect(previewBars([NaN, Infinity, -1])).toEqual(flatBars());
  });

  it("tar høyeste topp per hull og skalerer mot den høyeste stolpen", () => {
    const peaks = new Float32Array(640);
    peaks[5] = 0.2; // hull 0
    peaks[325] = 0.1; // hull 32
    const bars = previewBars(peaks);
    expect(bars[0]).toBe(100);
    expect(bars[32]).toBe(50);
    expect(bars[10]).toBe(FLAT_BAR);
  });

  it("gir alltid n stolper, også når det er færre topper enn stolper", () => {
    const bars = previewBars([1, 0.5], 8);
    expect(bars).toHaveLength(8);
    expect(Math.max(...bars)).toBe(100);
  });

  it("holder seg i FLAT_BAR..100", () => {
    const peaks = Float32Array.from({ length: 5000 }, (_, i) =>
      Math.abs(Math.sin(i / 50)),
    );
    for (const b of previewBars(peaks)) {
      expect(b).toBeGreaterThanOrEqual(FLAT_BAR);
      expect(b).toBeLessThanOrEqual(100);
    }
  });
});

describe("isBars", () => {
  it("godtar det previewBars skriver, avviser alt annet", () => {
    expect(isBars(previewBars([1, 2, 3]))).toBe(true);
    expect(isBars([1, 2, 3])).toBe(false);
    expect(isBars(null)).toBe(false);
    expect(isBars(Array(PREVIEW_BARS).fill("x"))).toBe(false);
    expect(isBars(Array(PREVIEW_BARS).fill(101))).toBe(false);
  });
});
