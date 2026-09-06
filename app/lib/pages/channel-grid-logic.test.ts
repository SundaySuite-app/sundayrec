// The channel grid's pure decisions: the tap reducer (two taps assign L then
// R), the mono-mode coupling, the signal hysteresis, and the dB→fraction
// meter mapping.
import { describe, expect, it } from "vitest";
import {
  armedForMode,
  dbToFraction,
  nextAssignment,
  nextSignalState,
  type Assignment,
} from "./channel-grid-logic";

const start: Assignment = { l: 0, r: 1, armed: "l" };

describe("nextAssignment", () => {
  it("stereo: two taps assign L then R and re-arm L", () => {
    const afterL = nextAssignment(start, 16, "stereo");
    expect(afterL).toEqual({ l: 16, r: 1, armed: "r" });
    const afterR = nextAssignment(afterL, 17, "stereo");
    expect(afterR).toEqual({ l: 16, r: 17, armed: "l" });
  });

  it("re-arming lets one side be redone without touching the other", () => {
    // User taps the R chip (armed=r), then a column.
    const rearmed: Assignment = { l: 16, r: 17, armed: "r" };
    expect(nextAssignment(rearmed, 20, "stereo")).toEqual({
      l: 16,
      r: 20,
      armed: "l",
    });
  });

  it("monoL always writes the L slot and stays armed on L", () => {
    let a = start;
    a = nextAssignment(a, 5, "monoL");
    expect(a).toEqual({ l: 5, r: 1, armed: "l" });
    a = nextAssignment(a, 9, "monoL");
    expect(a).toEqual({ l: 9, r: 1, armed: "l" });
  });

  it("monoR always writes the R slot", () => {
    const a = nextAssignment(start, 7, "monoR");
    expect(a).toEqual({ l: 0, r: 7, armed: "r" });
  });

  it("monoMix behaves like stereo (both channels feed the mix)", () => {
    const a = nextAssignment(start, 3, "monoMix");
    expect(a).toEqual({ l: 3, r: 1, armed: "r" });
  });

  it("L and R may share a channel (dual-mono)", () => {
    const afterL = nextAssignment(start, 4, "stereo");
    const afterR = nextAssignment(afterL, 4, "stereo");
    expect(afterR.l).toBe(4);
    expect(afterR.r).toBe(4);
  });

  it("mode switches preserve the stored channels (no data loss)", () => {
    const stereo = { l: 16, r: 17, armed: "l" as const };
    // Switching to monoL then tapping only moves L; R survives for the switch back.
    const mono = nextAssignment(stereo, 2, "monoL");
    expect(mono.r).toBe(17);
  });
});

describe("armedForMode", () => {
  it("pins the armed slot for mono modes and keeps it for stereo/mix", () => {
    expect(armedForMode("monoL", "r")).toBe("l");
    expect(armedForMode("monoR", "l")).toBe("r");
    expect(armedForMode("stereo", "r")).toBe("r");
    expect(armedForMode("monoMix", "l")).toBe("l");
  });
});

describe("nextSignalState (hysteresis)", () => {
  it("turns on above −50, off below −55, holds in between", () => {
    expect(nextSignalState(false, -45)).toBe(true);
    expect(nextSignalState(true, -60)).toBe(false);
    // The dead band keeps the previous state — no flicker at the threshold.
    expect(nextSignalState(true, -52)).toBe(true);
    expect(nextSignalState(false, -52)).toBe(false);
  });
});

describe("dbToFraction", () => {
  it("maps the −60 dB floor to 0 and 0 dBFS to 1, clamped", () => {
    expect(dbToFraction(-60)).toBe(0);
    expect(dbToFraction(0)).toBe(1);
    expect(dbToFraction(-30)).toBeCloseTo(0.5);
    expect(dbToFraction(3)).toBe(1);
    expect(dbToFraction(-90)).toBe(0);
  });

  it("treats null/undefined/−∞ (serde null) as silence", () => {
    expect(dbToFraction(null)).toBe(0);
    expect(dbToFraction(undefined)).toBe(0);
    expect(dbToFraction(Number.NEGATIVE_INFINITY)).toBe(0);
  });
});
