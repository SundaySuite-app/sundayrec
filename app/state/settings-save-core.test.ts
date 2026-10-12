import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { SAVE_COALESCE_MS } from "@lib/ui/bind-setting-core";
import { describe, expect, it } from "vitest";

import {
  IDLE_SAVE_TIMER,
  isDue,
  planFlush,
  planSave,
  patchFor,
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

describe("patchFor — only what changed crosses", () => {
  it("nothing changed → an empty patch", () => {
    expect(patchFor(SETTINGS_DEFAULTS, { ...SETTINGS_DEFAULTS })).toEqual({});
  });

  it("names exactly the changed fields, nested values by content", () => {
    const base = { ...SETTINGS_DEFAULTS };
    const now: typeof SETTINGS_DEFAULTS = {
      ...base,
      churchName: "Domkirken",
      deviceChannels: { x: { channelL: 1, channelR: 2 } },
    };
    expect(patchFor(now, base)).toEqual({
      churchName: "Domkirken",
      deviceChannels: { x: { channelL: 1, channelR: 2 } },
    });
    // An equal-by-content copy is not a change.
    expect(
      patchFor(
        {
          ...base,
          slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
        },
        {
          ...base,
          slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
        },
      ),
    ).toEqual({});
  });

  it("a field the baseline has never heard of counts as changed", () => {
    const now = { ...SETTINGS_DEFAULTS, futureField: 42 };
    expect(patchFor(now, SETTINGS_DEFAULTS)).toEqual({ futureField: 42 });
  });

  it("is a copy, so a change made while the IPC is in flight cannot follow it", () => {
    const now = { ...SETTINGS_DEFAULTS, churchName: "A" };
    const sent = patchFor(now, SETTINGS_DEFAULTS);
    now.churchName = "endret etterpå";
    expect(sent.churchName).toBe("A");
  });
});
