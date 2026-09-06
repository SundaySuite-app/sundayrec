// Debounced crash-recovery draft writer — the pure half of `scheduleDraftSave`.
//
// Split out of cuts.ts so the two invariants below can be asserted without a
// DOM, an `E` state object, or the IPC shim. Both were real bugs:
//
//   1. THE PENDING TIMER OUTLIVED THE FILE. `loadFile` resets `E.cuts`/`E.filePath`
//      synchronously but never cleared the debounce timer, so a timer armed while
//      editing file A fired 2 s later — in the middle of B's load — and wrote
//      whatever was in `E` at that instant to whatever `E.filePath` pointed at.
//      In practice: `{ cuts: [] }` into B's sidecar, moments before B's own
//      draft-restore tried to read it. The user's unsaved cuts for B were gone.
//   2. THE PAYLOAD WAS READ AT FIRE TIME. Even with the timer cleared on load,
//      reading the mutable global when the timer fires means the write is only
//      correct if nothing moved in the intervening 2 s. Capturing both the path
//      AND a snapshot at SCHEDULE time makes the write self-contained: it can
//      only ever describe the file it was armed for.
//
// Two belts, either of which alone fixes the reported crash; together they make
// a stale timer harmless rather than merely unlikely.

/** Whatever the host's timer API hands back. Never inspected — only handed back
 *  to `clearTimeout`, so this stays portable across DOM/node typings. */
export type TimerHandle = unknown;

/** The timer API, injectable so tests can drive time instead of waiting for it. */
export interface SchedulerClock {
  setTimeout(fn: () => void, ms: number): TimerHandle;
  clearTimeout(handle: TimerHandle): void;
}

const hostClock: SchedulerClock = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as Parameters<typeof clearTimeout>[0]),
};

export interface DraftScheduler<T> {
  /** Arm (or re-arm) the debounce for `filePath`, capturing `payload` now. */
  schedule(filePath: string, payload: T): void;
  /** Drop any pending write. Called on file switch, teardown, and after a
   *  successful export — anything that makes the pending write a lie. */
  cancel(): void;
  /** Whether a write is currently armed (test/diagnostic affordance). */
  isPending(): boolean;
}

/**
 * A single-slot debouncer: at most one write is ever armed, and it carries the
 * path + payload it was armed with.
 *
 * `save` is invoked with exactly what `schedule` was given — the scheduler never
 * reads back from application state, which is the whole point.
 */
export function createDraftScheduler<T>(opts: {
  delayMs: number;
  save: (filePath: string, payload: T) => void;
  clock?: SchedulerClock;
}): DraftScheduler<T> {
  const clock = opts.clock ?? hostClock;
  let handle: TimerHandle | null = null;

  const cancel = (): void => {
    if (handle !== null) {
      clock.clearTimeout(handle);
      handle = null;
    }
  };

  return {
    schedule(filePath, payload) {
      cancel();
      handle = clock.setTimeout(() => {
        handle = null;
        // `filePath`/`payload` are the ones captured HERE, at schedule time —
        // never whatever the editor happens to be showing 2 s later.
        opts.save(filePath, payload);
      }, opts.delayMs);
    },
    cancel,
    isPending: () => handle !== null,
  };
}
