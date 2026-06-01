import { describe, expect, it } from "vitest";

import { levelLabel } from "./hooks";

/**
 * `levelLabel` buckets a live dBFS peak into the four signal-strength labels
 * the recording/home signal indicator shows. The thresholds must match the
 * meter scale exactly so the label and the bars never disagree.
 */
describe("levelLabel", () => {
  it("reads null / non-finite as weak (silence, no telemetry yet)", () => {
    expect(levelLabel(null)).toBe("weak");
    expect(levelLabel(undefined)).toBe("weak");
    expect(levelLabel(Number.NaN)).toBe("weak");
    expect(levelLabel(-Infinity)).toBe("weak");
  });

  it("classifies < -24 dBFS as weak", () => {
    expect(levelLabel(-60)).toBe("weak");
    expect(levelLabel(-30)).toBe("weak");
    expect(levelLabel(-24.1)).toBe("weak");
  });

  it("classifies -24 .. < -12 dBFS as ok (inclusive lower bound)", () => {
    expect(levelLabel(-24)).toBe("ok");
    expect(levelLabel(-18)).toBe("ok");
    expect(levelLabel(-12.1)).toBe("ok");
  });

  it("classifies -12 .. < -6 dBFS as good", () => {
    expect(levelLabel(-12)).toBe("good");
    expect(levelLabel(-9)).toBe("good");
    expect(levelLabel(-6.1)).toBe("good");
  });

  it("classifies >= -6 dBFS as loud", () => {
    expect(levelLabel(-6)).toBe("loud");
    expect(levelLabel(-3)).toBe("loud");
    expect(levelLabel(0)).toBe("loud");
  });
});
