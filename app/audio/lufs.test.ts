import { describe, expect, it } from "vitest";

import { formatLufs, isMeasuredLufs } from "./lufs";

describe("formatLufs", () => {
  it("rounds to whole LUFS with a real minus sign", () => {
    expect(formatLufs(-23.4)).toBe("−23");
    expect(formatLufs(-23.6)).toBe("−24");
    expect(formatLufs(-16)).toBe("−16");
  });

  it("says «under −70» at and below the meter's gate, silence included", () => {
    expect(formatLufs(-70)).toBe("under −70");
    expect(formatLufs(-120)).toBe("under −70");
  });

  it("is null for anything that was not measured", () => {
    for (const v of [null, undefined, NaN, Infinity, -Infinity, "−23"]) {
      expect(formatLufs(v)).toBeNull();
      expect(isMeasuredLufs(v)).toBe(false);
    }
  });
});
