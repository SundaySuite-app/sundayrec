import { describe, expect, it } from "vitest";

import {
  buildTrimCuts,
  effectiveTrim,
  formatHms,
  moveTrimEdge,
  parseHms,
  recordingPickerMeta,
  secToViewFrac,
  MIN_TRIM_GAP,
} from "./editor.helpers";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";

describe("parseHms / formatHms (existing seam, guarded)", () => {
  it("parses HH:MM:SS / MM:SS / SS and round-trips through formatHms", () => {
    expect(parseHms("00:00:30")).toBe(30);
    expect(parseHms("1:30")).toBe(90);
    expect(parseHms("90")).toBe(90);
    expect(parseHms("")).toBeNull();
    expect(parseHms("abc")).toBeNull();
    expect(formatHms(90)).toBe("00:01:30");
    expect(formatHms(3661)).toBe("01:01:01");
  });
});

describe("effectiveTrim", () => {
  const dur = 1934;

  it("falls back to file bounds when the fields are empty", () => {
    expect(effectiveTrim("", "", dur)).toEqual({ start: 0, end: dur });
  });

  it("parses the two fields into seconds", () => {
    expect(effectiveTrim("00:00:30", "00:25:00", dur)).toEqual({
      start: 30,
      end: 1500,
    });
  });

  it("clamps both edges into [0, duration]", () => {
    expect(effectiveTrim("99:99:99", "99:99:99", dur)).toEqual({
      start: dur,
      end: dur,
    });
  });

  it("orders the edges so start <= end even if the fields cross", () => {
    expect(effectiveTrim("00:25:00", "00:00:30", dur)).toEqual({
      start: 30,
      end: 1500,
    });
  });

  it("returns a zero window for a zero-duration file", () => {
    expect(effectiveTrim("00:00:30", "00:25:00", 0)).toEqual({
      start: 0,
      end: 0,
    });
  });
});

describe("moveTrimEdge", () => {
  const dur = 100;
  const win = { start: 10, end: 90 };

  it("moves the start edge and keeps the end fixed", () => {
    expect(moveTrimEdge("start", 25, win, dur)).toEqual({ start: 25, end: 90 });
  });

  it("moves the end edge and keeps the start fixed", () => {
    expect(moveTrimEdge("end", 60, win, dur)).toEqual({ start: 10, end: 60 });
  });

  it("clamps the start to the file lower bound", () => {
    expect(moveTrimEdge("start", -5, win, dur)).toEqual({ start: 0, end: 90 });
  });

  it("clamps the end to the file upper bound", () => {
    expect(moveTrimEdge("end", 250, win, dur)).toEqual({ start: 10, end: 100 });
  });

  it("never lets the start cross the end (keeps MIN_TRIM_GAP)", () => {
    const next = moveTrimEdge("start", 95, win, dur);
    expect(next.end).toBe(90);
    expect(next.start).toBeCloseTo(90 - MIN_TRIM_GAP, 6);
    expect(next.end - next.start).toBeGreaterThanOrEqual(MIN_TRIM_GAP - 1e-9);
  });

  it("never lets the end cross the start (keeps MIN_TRIM_GAP)", () => {
    const next = moveTrimEdge("end", 5, win, dur);
    expect(next.start).toBe(10);
    expect(next.end).toBeCloseTo(10 + MIN_TRIM_GAP, 6);
  });
});

describe("secToViewFrac", () => {
  it("maps a second to its fractional position in the viewport", () => {
    expect(secToViewFrac(50, 0, 100)).toBeCloseTo(0.5, 6);
    expect(secToViewFrac(25, 0, 100)).toBeCloseTo(0.25, 6);
  });

  it("respects a panned/zoomed viewport window", () => {
    expect(secToViewFrac(60, 40, 80)).toBeCloseTo(0.5, 6);
  });

  it("returns null when the second is outside the viewport", () => {
    expect(secToViewFrac(120, 0, 100)).toBeNull();
    expect(secToViewFrac(-5, 0, 100)).toBeNull();
  });

  it("returns null for a degenerate viewport", () => {
    expect(secToViewFrac(10, 50, 50)).toBeNull();
  });
});

describe("effectiveTrim → buildTrimCuts integration", () => {
  it("the marker window and the export cut plan agree on bounds", () => {
    const dur = 1934;
    const win = effectiveTrim("00:00:30", "00:25:00", dur);
    expect(win).toEqual({ start: 30, end: 1500 });
    const cuts = buildTrimCuts("00:00:30", "00:25:00", dur);
    expect(cuts[0]).toEqual({ start: 0, end: win.start });
    expect(cuts).toContainEqual({ start: win.end, end: dur });
  });
});

describe("recordingPickerMeta (recent-recordings open picker)", () => {
  const base: RecordingRow = {
    id: "r1",
    file_path: "/recordings/2026-05-17_pinse.wav",
    device_name: null,
    started_at: 0,
    duration_ms: null,
    byte_size: null,
    created_at: 0,
    note: null,
  };

  it("formats date + rounded duration when both are present", () => {
    const meta = recordingPickerMeta({
      ...base,
      started_at: Date.UTC(2026, 4, 17, 9, 2),
      duration_ms: 32 * 60_000,
    });
    expect(meta).toContain("·");
    expect(meta).toMatch(/32 min$/);
  });

  it("drops the duration when unknown/zero", () => {
    expect(recordingPickerMeta({ ...base, duration_ms: 0 })).toBe("");
    const dateOnly = recordingPickerMeta({
      ...base,
      started_at: Date.UTC(2026, 4, 17, 9, 2),
    });
    expect(dateOnly).not.toContain("·");
    expect(dateOnly.length).toBeGreaterThan(0);
  });

  it("renders sub-minute recordings as '< 1 min'", () => {
    const meta = recordingPickerMeta({ ...base, duration_ms: 4_000 });
    expect(meta).toBe("< 1 min");
  });

  it("returns an empty string when nothing is known", () => {
    expect(recordingPickerMeta(base)).toBe("");
  });
});
