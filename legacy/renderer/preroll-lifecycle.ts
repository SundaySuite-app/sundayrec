/**
 * The pre-roll rolling buffer's lifecycle.
 *
 * `preRollSeconds` has been a settings dropdown since the port, and `start_recording`
 * dutifully harvests a pre-roll clip on every manual start — but NOTHING has ever
 * called `preroll_start`, so the rolling buffer never ran and the harvest always
 * came back empty. The setting promised to capture the seconds before the button
 * press and captured nothing.
 *
 * ## Why this is opt-in (the microphone has exactly ONE owner)
 *
 * The rolling buffer is a CONTINUOUS ffmpeg capture on the same input device the
 * live meters use. Backend-side the hand-offs are already correct — `preroll_start`
 * stops the cpal VU engine first, and `start_recording` harvests + `preroll.stop()`s
 * + `vu.stop()`s + settles 400 ms before the capture engine opens the device — but
 * the RENDERER's own `getUserMedia` meters (Home "Lydnivå — live", the Direktesending
 * meter, onboarding) are not covered by any of that, and Home is the default page.
 * A buffer that runs while the app is idle would therefore sit on the mic next to
 * the home meter for essentially the whole time the app is open. That is precisely
 * the second-owner pattern the 2026-07-31 audit traced 15–56 % sample loss to, and
 * the Qu-5 rig incident showed a webview-held device can even make the recorder's
 * open fail outright.
 *
 * Making that safe means redesigning where the home meter gets its levels from,
 * which is not a change to make next to a hardware-verified capture engine. So the
 * buffer ships behind a default-OFF advanced toggle (`prerollEnabled`), the
 * settings copy says plainly what it costs, and Home shows a chip whenever it is
 * running so it is never a silent background microphone.
 *
 * ## Shape
 *
 * `decidePreroll` is PURE — the whole "should the buffer be running?" question,
 * unit-tested with no IPC. `reconcilePreroll` is the thin shell that acts on it.
 */

import { settings } from './state'

/** Everything the decision depends on. */
export interface PrerollConditions {
  /** The advanced opt-in. Off ⇒ the buffer never runs, whatever else is true. */
  enabled: boolean
  /** `settings.preRollSeconds` — 0 means the feature is off. */
  seconds: number
  /** An input device is configured (the buffer has nothing to address without one). */
  deviceKnown: boolean
  /** A recording is in progress — the capture engine owns the mic, full stop. */
  isRecording: boolean
}

/** What the shell should do about the buffer. */
export type PrerollDecision = 'run' | 'stop'

/**
 * Should the rolling buffer be running right now?
 *
 * `stop` is the safe answer and therefore the default for every doubt: the cost
 * of a wrong `stop` is a pre-roll clip nobody notices missing, while the cost of
 * a wrong `run` is a second owner on the microphone during a service.
 */
export function decidePreroll(c: PrerollConditions): PrerollDecision {
  if (!c.enabled) return 'stop'
  if (!(c.seconds > 0)) return 'stop'
  if (!c.deviceKnown) return 'stop'
  if (c.isRecording) return 'stop'
  return 'run'
}

/** This module's own view of whether a recording is running, fed by the recorder
 *  events. Kept separate from `window.__isRecording` because both are written by
 *  handlers on the SAME events and listener order is not something to depend on
 *  when the answer decides who owns the microphone. Either saying "recording"
 *  is enough to keep the buffer down. */
let recordingSeen = false

/** Read the live conditions out of the settings cache + recorder state. */
export function currentConditions(): PrerollConditions {
  return {
    enabled: settings.prerollEnabled === true,
    seconds: settings.preRollSeconds ?? 0,
    deviceKnown: !!(settings.deviceName ?? settings.deviceId),
    isRecording: recordingSeen || window.__isRecording === true,
  }
}

// ── Shell ────────────────────────────────────────────────────────────────────

/** The last decision we ACTED on, so a reconcile that changes nothing is free
 *  (this runs on every settings save, device pick and recorder transition). */
let applied: PrerollDecision | null = null
/** Whether the backend reports the loop as actually running, for the Home chip.
 *  `preroll_start` can answer `false` (no device match, pre-roll off in the
 *  BACKEND's copy of the settings), and a chip that claims otherwise would be
 *  the same kind of lie this whole feature is fixing. */
let lastActive = false

const chipListeners = new Set<(active: boolean, seconds: number) => void>()

/** Render the "Forhåndsbuffer aktiv (N s)" chip. Fires immediately with the
 *  current state so a subscriber never has to paint a placeholder. */
export function subscribePrerollStatus(
  cb: (active: boolean, seconds: number) => void,
): () => void {
  chipListeners.add(cb)
  try {
    cb(lastActive, settings.preRollSeconds ?? 0)
  } catch (err) {
    console.error('[preroll] status subscriber failed:', err)
  }
  return () => chipListeners.delete(cb)
}

function publish(active: boolean): void {
  lastActive = active
  const secs = settings.preRollSeconds ?? 0
  for (const cb of [...chipListeners]) {
    try {
      cb(active, secs)
    } catch (err) {
      console.error('[preroll] status subscriber failed:', err)
    }
  }
}

/** How long to wait before re-opening the buffer after a recording ended. The
 *  capture engine releases the device on its terminal state, but a real driver
 *  takes a moment; the overlay's own meter restart uses 3 s for exactly this
 *  reason, and re-grabbing the mic ahead of it is how format renegotiation
 *  bites. Stopping is never delayed. */
const RESTART_SETTLE_MS = 3000

let restartTimer: ReturnType<typeof setTimeout> | null = null

/**
 * Bring the backend's buffer in line with the current conditions. Idempotent and
 * safe to call from anywhere — a settings save, a device pick, a recorder state
 * change, app start.
 *
 * `force` re-issues the command even when the decision hasn't changed (used at
 * app start, where we don't know what a previous run left behind, and after a
 * device change, where "run" means "run on a DIFFERENT device").
 */
export async function reconcilePreroll(force = false): Promise<void> {
  if (restartTimer) { clearTimeout(restartTimer); restartTimer = null }
  const decision = decidePreroll(currentConditions())
  if (decision === applied && !force) return

  // Coming back up after a recording: let the device go quiet first. A stop is
  // always immediate — yielding the microphone must never wait.
  if (decision === 'run' && applied === 'stop') {
    applied = decision
    restartTimer = setTimeout(() => {
      restartTimer = null
      // Re-decide: three seconds is long enough for a new recording to start.
      if (decidePreroll(currentConditions()) === 'run') void apply('run')
    }, RESTART_SETTLE_MS)
    return
  }

  applied = decision
  await apply(decision)
}

async function apply(decision: PrerollDecision): Promise<void> {
  try {
    if (decision === 'run') {
      const started = (await window.api.prerollStart?.()) ?? false
      publish(started)
    } else {
      await window.api.prerollStop?.()
      publish(false)
    }
  } catch (err) {
    console.warn('[preroll] reconcile failed:', err)
    // Unknown backend state — re-decide from scratch next time rather than
    // trusting a cached decision that may never have landed.
    applied = null
    publish(false)
  }
}

/** Ask the backend whether the loop is really running and republish. Cheap; used
 *  after a start to confirm, and by the settings panel's indicator. */
export async function refreshPrerollStatus(): Promise<void> {
  try {
    const st = await window.api.prerollStatus?.()
    publish(st?.active === true)
  } catch {
    publish(false)
  }
}

/**
 * Wire the lifecycle. Idempotent.
 *
 * The recorder transitions are the load-bearing ones: the buffer must be down
 * before the capture engine opens the device, and may come back once the session
 * is over. `start_recording` also stops it backend-side (belt and braces — that
 * is the guarantee, this is the fast path).
 */
let started = false
export function initPrerollLifecycle(): void {
  if (started) return
  started = true

  const setRecording = (live: boolean): void => {
    recordingSeen = live
    void reconcilePreroll()
  }

  window.api.on('recording-overlay-start', () => setRecording(true))
  // Mapped to `recording://state` — fires on EVERY transition, so read the state
  // rather than assuming "stop".
  window.api.on('recording-overlay-stop', (data: unknown) => {
    const st = (data as { state?: string } | undefined)?.state
    if (st === 'preparing' || st === 'recording' || st === 'reconnecting' || st === 'stopping') {
      setRecording(true)
    } else if (st === 'stopped' || st === 'failed' || st === 'idle') {
      setRecording(false)
    }
  })
  window.api.on('recording-finished', () => setRecording(false))
  window.api.on('recording-error', () => setRecording(false))

  // App start: force, because a previous run (or a crash) may have left a loop
  // behind that the current settings say should not exist.
  void reconcilePreroll(true)
}
