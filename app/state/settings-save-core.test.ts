import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { SAVE_COALESCE_MS } from "@lib/ui/bind-setting-core";
import { describe, expect, it } from "vitest";

import {
  IDLE_SAVE_TIMER,
  isDue,
  planFlush,
  planSave,
  payloadFor,
  type SaveTimerState,
} from "./settings-save-core";

describe("planSave", () => {
  it("arms a timer when nothing is pending", () => {
    const plan = planSave(IDLE_SAVE_TIMER, 1_000, 120);
    expect(plan.action).toBe("arm");
    expect(plan.next.dueAtMs).toBe(1_120);
  });

  it("coalesces onto the LAST request, not the first", () => {
    // Trailing, not leading: the value that ends up on disk is the one the
    // user stopped on.
    const plan = planSave({ dueAtMs: 1_120 }, 1_050, 120);
    expect(plan.action).toBe("coalesce");
    expect(plan.next.dueAtMs).toBe(1_170);
  });

  it("treats a negative delay as immediate rather than as a time machine", () => {
    expect(planSave(IDLE_SAVE_TIMER, 500, -50).next.dueAtMs).toBe(500);
  });
});

describe("a burst of changes", () => {
  /** Drive the planner the way the shell does and count the writes. */
  function writesFor(requestTimes: number[], delayMs: number): number[] {
    let state: SaveTimerState = IDLE_SAVE_TIMER;
    const writes: number[] = [];
    for (const at of requestTimes) {
      // Time passes between requests; a due timer fires before the next one.
      while (isDue(state, at)) {
        writes.push(state.dueAtMs as number);
        state = IDLE_SAVE_TIMER;
      }
      state = planSave(state, at, delayMs).next;
    }
    if (state.dueAtMs !== null) writes.push(state.dueAtMs);
    return writes;
  }

  it("collapses a slider drag into ONE write", () => {
    // 40 events, 10 ms apart, 120 ms coalescing window.
    const drag = Array.from({ length: 40 }, (_, i) => i * 10);
    expect(writesFor(drag, SAVE_COALESCE_MS)).toEqual([390 + SAVE_COALESCE_MS]);
  });

  it("does NOT collapse changes made minutes apart", () => {
    expect(writesFor([0, 60_000], SAVE_COALESCE_MS)).toEqual([
      SAVE_COALESCE_MS,
      60_000 + SAVE_COALESCE_MS,
    ]);
  });

  it("writes exactly once for a toggle that reveals a select the user then sets", () => {
    // The case SAVE_COALESCE_MS exists for: two different controls, one breath.
    expect(writesFor([0, 30], SAVE_COALESCE_MS)).toEqual([
      30 + SAVE_COALESCE_MS,
    ]);
  });
});

describe("planFlush", () => {
  it("sends what is pending", () => {
    const plan = planFlush({ dueAtMs: 5_000 });
    expect(plan.action).toBe("send");
    expect(plan.next.dueAtMs).toBeNull();
  });

  it("does nothing when nothing is pending", () => {
    // A flush that wrote anyway would turn every page change into a disk write.
    expect(planFlush(IDLE_SAVE_TIMER).action).toBe("none");
  });
});

describe("payloadFor — the R4 invariant", () => {
  it("sends the WHOLE vocabulary, not a selection", () => {
    const sent = payloadFor(SETTINGS_DEFAULTS);
    expect(Object.keys(sent).sort()).toEqual(
      Object.keys(SETTINGS_DEFAULTS).sort(),
    );
    // A guard on the guard: if the defaults ever shrank to a handful, the
    // assertion above would pass while proving nothing.
    expect(Object.keys(sent).length).toBeGreaterThan(40);
  });

  it("filters nothing at all — even a field this build has never heard of", () => {
    // An older/newer backend's extra field must travel back untouched rather
    // than being dropped and re-defaulted on the next read (#113/#115).
    const sent = payloadFor({ ...SETTINGS_DEFAULTS, futureField: 42 });
    expect(sent).toHaveProperty("futureField", 42);
  });

  it("is a copy, so a change made while the IPC is in flight cannot follow it", () => {
    const before = { ...SETTINGS_DEFAULTS };
    const sent = payloadFor(before);
    before.churchName = "endret etterpå";
    expect(sent.churchName).toBe("");
  });
});
