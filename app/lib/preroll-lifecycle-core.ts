/**
 * The pre-roll rolling buffer's DECISIONS — pure, no IPC, no DOM, no clock.
 *
 * `decidePreroll` (the "should the buffer be running?" question) has been pure
 * and unit-tested since it landed. The rest of the lifecycle was not, and the
 * rest of the lifecycle is where the microphone's one-owner invariant is
 * actually enforced:
 *
 *   • WHAT to do about a decision — apply it now, do nothing, or wait for the
 *     device to go quiet first (`planReconcile`). The wait is not cosmetic: a
 *     recording that just ended has a driver that has not let go yet, and
 *     re-grabbing the input ahead of it is how format renegotiation bites.
 *   • WHETHER a recorder event means "live" (`liveFromRecordingState`). The
 *     `recording://state` event fires on EVERY transition, so the mapping from
 *     state name to live/not-live/no-opinion decides who owns the microphone.
 *
 * Both were a handful of branches inside an async shell that touches
 * `window.api` and `setTimeout`, i.e. unreachable from the node-env unit gate.
 * They are the same shape as the rest of the renderer's `*-core` modules now:
 * explicit inputs, no globals, table-tested.
 *
 * `preroll-lifecycle.ts` is the shell that reads the settings singleton, holds
 * the timer, and calls the backend.
 */

/** Everything the run/stop decision depends on. */
export interface PrerollConditions {
  /** The advanced opt-in. Off ⇒ the buffer never runs, whatever else is true. */
  enabled: boolean;
  /** `settings.preRollSeconds` — 0 means the feature is off. */
  seconds: number;
  /** An input device is configured (the buffer has nothing to address without one). */
  deviceKnown: boolean;
  /** A recording is in progress — the capture engine owns the mic, full stop. */
  isRecording: boolean;
}

/** What the shell should do about the buffer. */
export type PrerollDecision = "run" | "stop";

/**
 * Should the rolling buffer be running right now?
 *
 * `stop` is the safe answer and therefore the default for every doubt: the cost
 * of a wrong `stop` is a pre-roll clip nobody notices missing, while the cost of
 * a wrong `run` is a second owner on the microphone during a service.
 */
export function decidePreroll(c: PrerollConditions): PrerollDecision {
  if (!c.enabled) return "stop";
  if (!(c.seconds > 0)) return "stop";
  if (!c.deviceKnown) return "stop";
  if (c.isRecording) return "stop";
  return "run";
}

/**
 * How long to wait before re-opening the buffer after a recording ended. The
 * capture engine releases the device on its terminal state, but a real driver
 * takes a moment; the overlay's own meter restart uses 3 s for exactly this
 * reason. Stopping is never delayed.
 */
export const RESTART_SETTLE_MS = 3000;

/** What `reconcilePreroll` should actually do. */
export type PrerollAction =
  /** The decision has not changed and nothing was forced — no IPC. */
  | "none"
  /** Issue the command now. */
  | "apply"
  /** Coming back up after a stop: wait `RESTART_SETTLE_MS`, then re-decide. */
  | "defer-restart";

export interface ReconcilePlan {
  action: PrerollAction;
  /** What the shell should record as the decision it has ACTED on. For
   *  `defer-restart` that is already `run`, so a second reconcile arriving
   *  during the wait does not queue a second timer. */
  applied: PrerollDecision | null;
}

export interface ReconcileInput {
  /** The last decision the shell acted on; `null` at app start and after a
   *  failed apply (unknown backend state ⇒ decide from scratch). */
  previous: PrerollDecision | null;
  /** What `decidePreroll` says right now. */
  decision: PrerollDecision;
  /** Re-issue even when the decision has not changed — used at app start (a
   *  previous run may have left a loop behind) and after a device change,
   *  where "run" means "run on a DIFFERENT device". */
  force: boolean;
}

/**
 * Plan one reconcile.
 *
 * Order matters and is the order the shell has always used: the no-op check
 * first (and `force` defeats only that one), then the settle-delay check.
 * A forced `stop → run` therefore still WAITS — forcing means "do not skip this
 * because nothing changed", never "grab the microphone immediately".
 */
export function planReconcile(input: ReconcileInput): ReconcilePlan {
  const { previous, decision, force } = input;
  if (decision === previous && !force)
    return { action: "none", applied: previous };
  if (decision === "run" && previous === "stop") {
    return { action: "defer-restart", applied: "run" };
  }
  return { action: "apply", applied: decision };
}

/**
 * Does a `recording://state` payload mean a recording is live?
 *
 * `null` = no opinion: an unknown state must leave the current belief alone
 * rather than guess. Guessing "not recording" would release the buffer's
 * restraint mid-service; guessing "recording" would strand the buffer down
 * forever.
 */
export function liveFromRecordingState(
  state: string | undefined,
): boolean | null {
  switch (state) {
    case "preparing":
    case "recording":
    case "reconnecting":
    case "stopping":
      return true;
    case "stopped":
    case "failed":
    case "idle":
      return false;
    default:
      return null;
  }
}
