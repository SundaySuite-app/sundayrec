/**
 * The "next recording" STORE — one subscription, one state, many renderers.
 *
 * The backend has been emitting `scheduler://next`, `scheduler://missed` and
 * `scheduler://preflight` since the Tauri port, and nothing in the renderer ever
 * listened. Every surface polled `scheduler_status` on its own instead, which is
 * why the hero, the sidebar and the Tidsplan preview could each show a different
 * answer at the same moment — and why a missed recording was invisible unless
 * you happened to see the native notification.
 *
 * This module owns the subscription and the derived state; `next-recording-core`
 * owns the wording. Renderers do `subscribe(render)` and never compute.
 *
 * The three scheduler channels bypass `window.api.on` deliberately: EVENT_MAP is
 * the compatibility layer for OLD Electron channel names, and these three never
 * had one. New code talks to the backend's real event names.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { settings } from '../state'
import type { MissedRecordingInfo } from '../../bindings/MissedRecordingInfo'
import type { PreflightFinding } from '../../bindings/PreflightFinding'
import {
  buildNext,
  computeWake,
  emptyState,
  type NextRecordingState,
} from './next-recording-core'

/** Backend event names — see src-tauri/src/scheduler/mod.rs. */
const EV_NEXT = 'scheduler://next'
const EV_MISSED = 'scheduler://missed'
const EV_PREFLIGHT = 'scheduler://preflight'

/**
 * Fallback poll. The events are the primary path; this only covers a missed
 * emit (the supervisor restarting after a panic, an event fired before this
 * module was wired). One `scheduler_status` a minute is free.
 */
const POLL_MS = 60_000

let state: NextRecordingState = emptyState()
const listeners = new Set<(s: NextRecordingState) => void>()

/** The raw `scheduler://next` payload, kept so settings changes can re-derive
 *  the label/wake without another round-trip. */
let lastNextIso: string | null = null

let started = false
let pollTimer: ReturnType<typeof setInterval> | null = null
const unlisteners: Array<UnlistenFn | (() => void)> = []

// ── Derivation ───────────────────────────────────────────────────────────────

/**
 * Rebuild the derived parts of the state from `lastNextIso` + current settings.
 * Called on every event and every poll, so a schedule edit is reflected without
 * a second source of truth.
 */
function derive(): void {
  const specials = settings.specialRecordings ?? []
  const next = buildNext(lastNextIso, specials)
  state = {
    ...state,
    next,
    hasAnySchedule: (settings.slots ?? []).length > 0 || specials.length > 0,
    wake: computeWake(next, settings.wakeFromSleep !== false),
  }
}

function emit(): void {
  for (const cb of [...listeners]) {
    try {
      cb(state)
    } catch (err) {
      console.error('[next-recording] subscriber failed:', err)
    }
  }
}

function update(mutate: () => void): void {
  mutate()
  derive()
  emit()
}

// ── Public API ───────────────────────────────────────────────────────────────

/** The current truth. Never null — an unhydrated store is simply "nothing known". */
export function getNextRecordingState(): NextRecordingState {
  return state
}

/**
 * Render on every change. The callback fires IMMEDIATELY with the current state
 * so a renderer never has to paint a placeholder first, and the returned
 * function unsubscribes.
 */
export function subscribe(cb: (s: NextRecordingState) => void): () => void {
  listeners.add(cb)
  try {
    cb(state)
  } catch (err) {
    console.error('[next-recording] subscriber failed:', err)
  }
  return () => listeners.delete(cb)
}

/** Re-read the scheduler status now (after a schedule edit, on focus, …). */
export async function refreshNextRecording(): Promise<void> {
  try {
    const next = await window.api.getNextRecording()
    update(() => {
      lastNextIso = next?.date ?? null
    })
  } catch (err) {
    console.warn('[next-recording] status poll failed:', err)
    // Keep the last known state — a failed poll is not evidence that the
    // schedule is empty, and blanking the hero on a transient IPC error is
    // exactly the kind of lie this store exists to stop.
  }
}

/** Recompute from settings alone (a wake toggle, a slot added locally). */
export function syncScheduleSettings(): void {
  update(() => {})
}

/** Clear the missed-recording surface once the user has acknowledged it. */
export function dismissMissed(): void {
  if (state.missed.length === 0) return
  update(() => {
    state = { ...state, missed: [] }
  })
}

/**
 * Feed preflight findings from a source other than the scheduler — the
 * once-per-launch silent check on Home runs the same `run_preflight` the
 * backend fires 30 minutes before a scheduled start, and there is no reason for
 * the user to meet two different renderings of the same findings.
 */
export function setPreflightFindings(findings: PreflightFinding[]): void {
  update(() => {
    state = { ...state, preflight: findings }
  })
}

/** Clear the preflight surface. */
export function dismissPreflight(): void {
  if (state.preflight.length === 0) return
  setPreflightFindings([])
}

/** Note a recording start/stop from the recorder's own events. */
function setRecording(isRecording: boolean): void {
  if (state.isRecording === isRecording) return
  update(() => {
    state = { ...state, isRecording }
  })
}

/**
 * Wire the subscriptions. Idempotent — safe to call from every page setup.
 */
export function initNextRecordingStore(): void {
  if (started) return
  started = true

  const track = (p: Promise<UnlistenFn>): void => {
    p.then(u => unlisteners.push(u)).catch(err =>
      console.warn('[next-recording] listen failed:', err),
    )
  }

  // The nearest future start, recomputed by the supervisor on every settings
  // change / fired event. Payload is a zone-less local ISO string or null.
  track(
    listen<string | null>(EV_NEXT, e => {
      update(() => {
        lastNextIso = e.payload ?? null
      })
    }),
  )

  // Scheduled recordings that never ran. Emitted at startup and after a
  // suspected system sleep — the one event a church recorder must not swallow.
  track(
    listen<MissedRecordingInfo[]>(EV_MISSED, e => {
      const missed = Array.isArray(e.payload) ? e.payload : []
      if (missed.length === 0) return
      update(() => {
        state = { ...state, missed }
      })
    }),
  )

  // Fired ~30 minutes before a scheduled start (PREFLIGHT_LEAD_MIN), so there
  // is still time to plug the mixer back in.
  track(
    listen<PreflightFinding[]>(EV_PREFLIGHT, e => {
      const findings = Array.isArray(e.payload) ? e.payload : []
      update(() => {
        state = { ...state, preflight: findings }
      })
    }),
  )

  // isRecording comes from the recorder's OWN state events — the same source
  // `window.__isRecording` is set from, rather than the mutable global itself
  // (which no renderer can subscribe to).
  const recordingUnsubs = [
    window.api.on('recording-overlay-start', () => setRecording(true)),
    window.api.on('recording-overlay-stop', (data: unknown) => {
      // Mapped to `recording://state`, which fires on EVERY transition.
      const st = (data as { state?: string } | undefined)?.state
      if (st === 'recording' || st === 'reconnecting') setRecording(true)
      else if (st === 'stopped' || st === 'failed' || st === 'idle') setRecording(false)
    }),
    window.api.on('recording-finished', () => setRecording(false)),
    window.api.on('recording-error', () => setRecording(false)),
  ]
  for (const u of recordingUnsubs) if (u) unlisteners.push(u)

  void refreshNextRecording()
  pollTimer = setInterval(() => void refreshNextRecording(), POLL_MS)

  window.addEventListener('beforeunload', () => {
    if (pollTimer) clearInterval(pollTimer)
    pollTimer = null
    for (const u of unlisteners.splice(0)) {
      try {
        u()
      } catch {
        /* teardown is best-effort */
      }
    }
  })
}
