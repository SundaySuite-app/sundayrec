// The IPC-failure ring and its surfacing policy (E2.4) — pure, no DOM, no Tauri.
//
// ## The problem this exists for
//
// `api-shim.ts`'s `call<T>(cmd, args, fallback)` catches every error and
// returns `fallback` with a `console.warn`. That is the RIGHT resilience shape:
// the renderer must not die because one command is missing, and a partially
// wired backend must degrade to an empty state rather than a white screen.
//
// But it makes a backend failure indistinguishable from "no data". Combined
// with the fact that (until E2.1) a Rust panic became exactly such a rejected
// invoke, the operator's experience of a crashed backend was: an empty history
// list, a device dropdown with nothing in it, a diagnose panel that says
// everything is fine. Nothing anywhere said "this failed".
//
// So: keep the fallback, keep the console.warn, and ALSO remember what failed
// and tell the operator once.
//
// ## Why the policy needs to be careful
//
// Several commands are polled — `recording_status` ~1×/s, the VU feed, the
// preview frame ~4×/s. A backend that is down fails all of them, forever. A
// toast per failure would be a hundred toasts a minute stacked over the UI,
// which is not "surfacing a problem", it is a denial of service. Hence: one
// toast per command per cooldown, and a global cap so a dozen DIFFERENT broken
// commands still cannot paper over the screen.
//
// The ring itself is unconditional — every failure is remembered, whether or
// not it was shown, because "siste IPC-feil" in the diagnose panel wants the
// pattern, not the one that happened to win the rate limit.

/** One remembered failure. */
export interface IpcFailure {
  /** The Tauri command name, e.g. `recordings_list`. */
  cmd: string;
  /** The human-readable error text (`ipcErrText` output). */
  error: string;
  /** `Date.now()` at the moment it failed. */
  at: number;
}

/**
 * How many failures are remembered. Bounded because this lives for the whole
 * session in a renderer that may run for a nine-hour Sunday: a polled command
 * failing 4×/s would otherwise grow an unbounded array until the tab dies —
 * which would turn a diagnostic into a second outage.
 *
 * 50 is chosen to hold a burst AND the handful of unrelated failures around it,
 * which is what makes the list diagnostic rather than just repetitive.
 */
export const RING_MAX = 50;

/** A command that just toasted stays quiet for this long. */
export const DEDUP_MS = 60_000;

/** At most this many toasts inside {@link WINDOW_MS}, across ALL commands. */
export const MAX_TOASTS_PER_WINDOW = 3;
export const WINDOW_MS = 60_000;

/** The mutable state. Held by the shim; passed in explicitly so this is pure. */
export interface IpcFailureState {
  /** Newest LAST — the order the crash ring and telemetry history use. */
  ring: IpcFailure[];
  /** `cmd` → the time it was last toasted. */
  lastToastByCmd: Record<string, number>;
  /** Times of the recent toasts, for the global cap. Newest last. */
  recentToasts: number[];
}

export function createIpcFailureState(): IpcFailureState {
  return { ring: [], lastToastByCmd: {}, recentToasts: [] };
}

/**
 * Whether this failure should reach the operator as a toast.
 *
 * Pure predicate — it reads the state but does not change it, so a caller can
 * ask without committing (and so it is trivially testable). {@link noteSurfaced}
 * is what records the decision.
 */
export function shouldSurface(state: IpcFailureState, cmd: string, now: number): boolean {
  const last = state.lastToastByCmd[cmd];
  // The dedup: the FIRST failure in a burst speaks, the rest of the burst does
  // not. A poll failing every 250 ms produces one toast a minute, not 240.
  if (last !== undefined && now - last < DEDUP_MS) return false;
  // The global cap: three different broken commands is already a clear enough
  // signal; the fourth would just be noise on top of a screen the operator is
  // trying to read.
  const inWindow = state.recentToasts.filter((t) => now - t < WINDOW_MS).length;
  return inWindow < MAX_TOASTS_PER_WINDOW;
}

/** Record that `cmd` was toasted at `now`, and forget toasts older than the window. */
export function noteSurfaced(state: IpcFailureState, cmd: string, now: number): void {
  state.lastToastByCmd[cmd] = now;
  state.recentToasts = state.recentToasts.filter((t) => now - t < WINDOW_MS);
  state.recentToasts.push(now);
}

/**
 * Remember a failure and decide whether to surface it.
 *
 * Returns `true` when the caller should toast. The ring is appended to either
 * way — a failure that lost the rate limit is still a failure the diagnose
 * panel should be able to show.
 */
export function recordFailure(
  state: IpcFailureState,
  cmd: string,
  error: string,
  now: number,
): boolean {
  state.ring.push({ cmd, error, at: now });
  if (state.ring.length > RING_MAX) state.ring.splice(0, state.ring.length - RING_MAX);
  const surface = shouldSurface(state, cmd, now);
  if (surface) noteSurfaced(state, cmd, now);
  return surface;
}

/** A copy of the ring, newest FIRST — the order a "siste feil" list reads in. */
export function recentFailures(state: IpcFailureState): IpcFailure[] {
  return [...state.ring].reverse();
}

/**
 * A one-line summary for the diagnose panel: how many failures, how many
 * distinct commands, and when the newest one was.
 */
export function failureSummary(state: IpcFailureState): {
  count: number;
  commands: string[];
  newestAt: number | null;
} {
  const commands = [...new Set(state.ring.map((f) => f.cmd))].sort();
  return {
    count: state.ring.length,
    commands,
    newestAt: state.ring.length ? state.ring[state.ring.length - 1].at : null,
  };
}
