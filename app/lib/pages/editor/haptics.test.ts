import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { limitPulse, resetHaptics } from "./haptics";

const calls: string[] = [];

beforeEach(() => {
  calls.length = 0;
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-06T10:00:00Z"));
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      hapticPerform: (pattern: string) => {
        calls.push(pattern);
        return Promise.resolve();
      },
    },
  };
  resetHaptics();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("limitPulse", () => {
  it("uses the detent pattern, not the alignment one", () => {
    limitPulse();
    expect(calls).toEqual(["levelChange"]);
  });

  it("is throttled — a drag pinned at a limit does not buzz", () => {
    limitPulse();
    limitPulse();
    limitPulse();
    expect(calls).toEqual(["levelChange"]);
  });
});

describe("the missing-command case", () => {
  it("is a silent no-op when the shim has no hapticPerform", () => {
    (
      globalThis as unknown as { window: { api: Record<string, unknown> } }
    ).window = { api: {} };
    resetHaptics();
    expect(() => limitPulse()).not.toThrow();
  });
});
