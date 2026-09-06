import { describe, expect, it } from "vitest";
import {
  createIpcFailureState,
  failureSummary,
  noteSurfaced,
  recentFailures,
  recordFailure,
  shouldSurface,
  DEDUP_MS,
  MAX_TOASTS_PER_WINDOW,
  RING_MAX,
  WINDOW_MS,
} from "./ipc-failures-core";

describe("the IPC failure ring", () => {
  it("remembers failures newest-last and hands them back newest-first", () => {
    const s = createIpcFailureState();
    recordFailure(s, "recordings_list", "boom", 1000);
    recordFailure(s, "trash_list", "bang", 2000);
    expect(s.ring.map((f) => f.cmd)).toEqual(["recordings_list", "trash_list"]);
    expect(recentFailures(s).map((f) => f.cmd)).toEqual([
      "trash_list",
      "recordings_list",
    ]);
    expect(recentFailures(s)[0]).toEqual({
      cmd: "trash_list",
      error: "bang",
      at: 2000,
    });
  });

  it("is bounded, so a 4-per-second poll cannot grow it without limit", () => {
    // The failure mode this cap exists for: a nine-hour Sunday with a polled
    // command failing every 250 ms is ~130 000 entries without it.
    const s = createIpcFailureState();
    for (let i = 0; i < RING_MAX * 3; i++)
      recordFailure(s, "recording_status", `e${i}`, i);
    expect(s.ring.length).toBe(RING_MAX);
    // …and it keeps the NEWEST, not the first ones it happened to see.
    expect(s.ring[s.ring.length - 1].error).toBe(`e${RING_MAX * 3 - 1}`);
    expect(s.ring[0].error).toBe(`e${RING_MAX * 2}`);
  });

  it("summarises count, distinct commands and the newest timestamp", () => {
    const s = createIpcFailureState();
    expect(failureSummary(s)).toEqual({
      count: 0,
      commands: [],
      newestAt: null,
    });
    recordFailure(s, "b_cmd", "x", 10);
    recordFailure(s, "a_cmd", "y", 20);
    recordFailure(s, "b_cmd", "z", 30);
    expect(failureSummary(s)).toEqual({
      count: 3,
      commands: ["a_cmd", "b_cmd"],
      newestAt: 30,
    });
  });
});

describe("the surfacing policy", () => {
  it("surfaces the first failure of a burst and then goes quiet", () => {
    const s = createIpcFailureState();
    expect(recordFailure(s, "recording_status", "e", 0)).toBe(true);
    // A poll failing 4×/s for the rest of the minute says nothing more.
    for (let t = 250; t < DEDUP_MS; t += 250) {
      expect(recordFailure(s, "recording_status", "e", t)).toBe(false);
    }
    // …but every one of them is still remembered.
    expect(s.ring.length).toBe(RING_MAX);
  });

  it("speaks again once the cooldown has passed", () => {
    const s = createIpcFailureState();
    expect(recordFailure(s, "recording_status", "e", 0)).toBe(true);
    expect(recordFailure(s, "recording_status", "e", DEDUP_MS - 1)).toBe(false);
    expect(recordFailure(s, "recording_status", "e", DEDUP_MS)).toBe(true);
  });

  it("dedups per command, so a second broken command is still heard", () => {
    const s = createIpcFailureState();
    expect(recordFailure(s, "recordings_list", "e", 0)).toBe(true);
    expect(recordFailure(s, "recordings_list", "e", 100)).toBe(false);
    // A DIFFERENT command failing is new information.
    expect(recordFailure(s, "trash_list", "e", 100)).toBe(true);
  });

  it("caps the total number of toasts so a broken backend cannot paper the screen", () => {
    const s = createIpcFailureState();
    for (let i = 0; i < MAX_TOASTS_PER_WINDOW; i++) {
      expect(recordFailure(s, `cmd_${i}`, "e", i)).toBe(true);
    }
    // The fourth distinct broken command adds nothing the operator can use.
    expect(recordFailure(s, "cmd_overflow", "e", 10)).toBe(false);
    // Once the window has rolled past, the budget is back.
    expect(recordFailure(s, "cmd_later", "e", WINDOW_MS + 1)).toBe(true);
  });

  it("shouldSurface is a pure read — asking does not spend the budget", () => {
    const s = createIpcFailureState();
    expect(shouldSurface(s, "x", 0)).toBe(true);
    expect(shouldSurface(s, "x", 0)).toBe(true);
    expect(shouldSurface(s, "x", 0)).toBe(true);
    expect(s.recentToasts).toEqual([]);
    noteSurfaced(s, "x", 0);
    expect(shouldSurface(s, "x", 0)).toBe(false);
  });

  it("forgets toast timestamps older than the window instead of growing forever", () => {
    const s = createIpcFailureState();
    for (let i = 0; i < 100; i++) noteSurfaced(s, `cmd_${i}`, i * WINDOW_MS);
    expect(s.recentToasts.length).toBe(1);
  });
});
