import { describe, it, expect } from "vitest";
import {
  createDraftScheduler,
  type SchedulerClock,
  type TimerHandle,
} from "./draft-scheduler";

/** A hand-driven clock: `run()` fires every timer that is still armed. */
function fakeClock(): SchedulerClock & { run(): void; armed(): number } {
  const timers = new Map<number, () => void>();
  let next = 1;
  return {
    setTimeout(fn) {
      const id = next++;
      timers.set(id, fn);
      return id as unknown as TimerHandle;
    },
    clearTimeout(handle) {
      timers.delete(handle as number);
    },
    run() {
      // Snapshot first: a callback may arm a new timer, which must NOT run now.
      const due = [...timers.entries()];
      timers.clear();
      for (const [, fn] of due) fn();
    },
    armed: () => timers.size,
  };
}

type Write = { path: string; cuts: { start: number; end: number }[] };

function harness() {
  const clock = fakeClock();
  const writes: Write[] = [];
  const scheduler = createDraftScheduler<{ start: number; end: number }[]>({
    delayMs: 2000,
    clock,
    save: (path, cuts) => writes.push({ path, cuts }),
  });
  return { clock, writes, scheduler };
}

describe("createDraftScheduler", () => {
  it("debounces rapid edits into one write carrying the latest payload", () => {
    const { clock, writes, scheduler } = harness();
    // A drag calls this on every mutation; the point is to not spam IPC.
    scheduler.schedule("/rec/a.mp3", [{ start: 1, end: 2 }]);
    scheduler.schedule("/rec/a.mp3", [{ start: 1, end: 3 }]);
    scheduler.schedule("/rec/a.mp3", [{ start: 1, end: 4 }]);
    expect(writes).toEqual([]);
    expect(clock.armed()).toBe(1);

    clock.run();
    expect(writes).toEqual([
      { path: "/rec/a.mp3", cuts: [{ start: 1, end: 4 }] },
    ]);
  });

  it("writes to the path captured at SCHEDULE time, never a later one", () => {
    // The reported crash: a timer armed for A fires during B's load. Whatever
    // the editor now points at, the pending write can only describe A.
    const { clock, writes, scheduler } = harness();
    scheduler.schedule("/rec/a.mp3", [{ start: 5, end: 9 }]);
    // …user opens B here; the scheduler is not told, and must not care.
    clock.run();
    expect(writes).toEqual([
      { path: "/rec/a.mp3", cuts: [{ start: 5, end: 9 }] },
    ]);
  });

  it("cancel() drops a pending write entirely", () => {
    // What loadFile/teardown/clearEditorDraft rely on: no orphaned timer can
    // land an empty cut list on the file the user just opened.
    const { clock, writes, scheduler } = harness();
    scheduler.schedule("/rec/a.mp3", [{ start: 1, end: 2 }]);
    expect(scheduler.isPending()).toBe(true);

    scheduler.cancel();
    expect(scheduler.isPending()).toBe(false);
    clock.run();
    expect(writes).toEqual([]);
  });

  it("cancel() is safe with nothing armed, and repeatable", () => {
    const { writes, scheduler } = harness();
    scheduler.cancel();
    scheduler.cancel();
    expect(writes).toEqual([]);
    expect(scheduler.isPending()).toBe(false);
  });

  it("a switch to another file replaces the pending write rather than adding one", () => {
    // Belt two: even if the caller forgot to cancel on load, re-scheduling for B
    // disarms A's timer — one slot, so A can never be written from B's session.
    const { clock, writes, scheduler } = harness();
    scheduler.schedule("/rec/a.mp3", [{ start: 1, end: 2 }]);
    scheduler.schedule("/rec/b.mp3", []);
    expect(clock.armed()).toBe(1);

    clock.run();
    expect(writes).toEqual([{ path: "/rec/b.mp3", cuts: [] }]);
  });

  it("stops reporting pending once the write has fired", () => {
    const { clock, scheduler } = harness();
    scheduler.schedule("/rec/a.mp3", []);
    clock.run();
    expect(scheduler.isPending()).toBe(false);
  });
});
