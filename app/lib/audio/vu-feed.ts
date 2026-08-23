/**
 * THE VU feed — one backend metering session, many subscribers.
 *
 * ## Why this module exists
 *
 * Every meter in the app used to open its OWN `getUserMedia` stream: the home
 * meter, the Direkte meter, the onboarding audio test, and — for a blink on
 * every device enumeration — the device picker. Each of those made the WEBVIEW
 * an owner of the microphone, and that is the failure class behind the Qu-5
 * incident of 2026-07-31: WebKit tears a CoreAudio input down ASYNCHRONOUSLY,
 * so a multi-channel device that had been opened by the webview stayed pinned
 * to the 2-channel format gUM negotiated, long after the meter was "stopped".
 * The recorder then opened a 32-channel mixer and got a stereo pair.
 *
 * There is now exactly one owner of an input device in this process: the Rust
 * side. `start_vu` opens a cpal stream at the device's FULL native channel
 * count and emits `vu://levels` ~30×/s; `start_recording` stops it before the
 * capture engine opens the device. The renderer only ever LISTENS.
 *
 * ## The contract
 *
 * - `acquireVuFeed(sub)` returns a release function. First acquire starts the
 *   engine, last release stops it — the engine is a process-wide singleton, so
 *   two meters on screen must not mean two sessions.
 * - Exactly ONE `vu-levels` listener exists, fanned out to subscribers.
 * - Packets are delivered whoever emitted them. `vu://levels` is not a
 *   `start_vu` private channel — the native pre-roll buffer is an alternate
 *   emitter — so "levels are flowing" is itself the liveness signal, not "our
 *   startVu call resolved".
 * - A subscriber that wants a different device restarts the session
 *   (`start_vu` is stop-first-then-start on the Rust side).
 */

import type { VuLevels } from '../../bindings/VuLevels'
import type { ChannelMode } from '../../types'
import { pickLR, deviceAction, refcountStep, resolveDevice } from './vu-feed-core'

/** What the feed is doing right now, for the subscriber's status line. */
export type VuFeedState =
  /** No engine session (nobody is subscribed, or we're mid-teardown). */
  | 'idle'
  /** `start_vu` is in flight — the device is being opened. */
  | 'connecting'
  /** Packets are flowing. */
  | 'live'
  /** `start_vu` failed twice; the device would not open. */
  | 'failed'

/** Which channels a subscriber's two bars should show. Read per packet, so a
 *  mode radio or a channel tap takes effect on the next frame. */
export interface VuPick {
  mode: ChannelMode
  chL: number
  chR: number
}

export interface VuFeedSubscriber {
  /**
   * The device this subscriber wants metered. `undefined` = no opinion (meter
   * whatever is running); `null` = the system default.
   */
  deviceName?: string | null
  /** The channel pick for THIS subscriber's bars. Omitted = plain stereo 0/1. */
  pick?: () => VuPick
  /**
   * One packet. `l`/`r` are the subscriber's picked RMS levels in dBFS (floored
   * at −60) — the number the bars draw. `raw` is the whole per-native-channel
   * payload, for meters that draw every channel (the grid) or need the PEAK
   * pair (`pickLR(raw.peak_dbfs, …)` — peak-hold markers, clip lights).
   */
  onLevels?: (l: number, r: number, raw: VuLevels) => void
  /** State changes, plus the negotiated native channel count (0 until known). */
  onState?: (state: VuFeedState, channels: number) => void
}

const DEFAULT_PICK: VuPick = { mode: 'stereo', chL: 0, chR: 1 }

interface Entry {
  sub: VuFeedSubscriber
  alive: boolean
}

const subs: Entry[] = []
let unlisten: (() => void) | null = null
/** The device the current session was started for. */
let device: string | null = null
/** We have asked the engine to run (and haven't seen it fail). */
let running = false
/** Bumped on every (re)start + teardown so a stale `start_vu` completion from a
 *  previous device can't publish its channel count over the current one. */
let gen = 0
let feedState: VuFeedState = 'idle'
let channels = 0
/** What subscribers were last told, so `publish()` stays a no-op in the steady
 *  state (a 30 Hz packet stream must not fan out an unchanged status). */
let notifiedState: VuFeedState | null = null
let notifiedChannels = -1

/**
 * Subscribe to the backend VU feed. Returns a release function; call it exactly
 * once (extra calls are harmless no-ops).
 */
export function acquireVuFeed(sub: VuFeedSubscriber): () => void {
  const entry: Entry = { sub, alive: true }
  const { transition } = refcountStep(subs.length, 1)
  subs.push(entry)
  if (transition === 'start') attachListener()
  reconcile()
  // Tell the newcomer where things stand without waiting for a transition.
  try {
    sub.onState?.(feedState, channels)
  } catch {
    /* a subscriber's own render error must not break the feed */
  }
  return () => release(entry)
}

function release(entry: Entry): void {
  if (!entry.alive) return
  entry.alive = false
  const i = subs.indexOf(entry)
  if (i >= 0) subs.splice(i, 1)
  const { transition } = refcountStep(subs.length + 1, -1)
  if (transition === 'stop') {
    teardown()
    return
  }
  // A remaining subscriber may want a different device than the one that left.
  reconcile()
}

/** How many meters currently hold the feed. Exported for diagnostics/tests. */
export function vuFeedSubscriberCount(): number {
  return subs.length
}

// ── Engine lifecycle ─────────────────────────────────────────────────────────

/**
 * Every `start_vu`/`stop_vu` runs on ONE serial queue.
 *
 * Without it, the last meter closing while a `start_vu` was still in flight
 * would issue `stop_vu` first and let the start land AFTERWARDS — leaving a
 * cpal stream open on the device with nobody left to close it. That is the
 * "device stays held" failure this whole phase exists to eliminate, so it must
 * not be reintroduced by a race in the client. Serialising also means a stale
 * start that does open a session is always followed by the operation that
 * supersedes it (the next start's stop-first, or the teardown's stop).
 */
let queue: Promise<unknown> = Promise.resolve()
function enqueue(op: () => Promise<void>): void {
  queue = queue.then(op, op).catch(() => {})
}

function reconcile(): void {
  const wanted = resolveDevice(subs.map(e => e.sub))
  if (deviceAction(device, wanted, running) === 'keep') return
  device = wanted
  requestStart(wanted)
}

function requestStart(dev: string | null): void {
  const my = ++gen
  running = true
  setState('connecting')
  enqueue(async () => {
    // Superseded while we waited our turn — the newer request owns the state.
    if (my !== gen) return
    // A recording owns the device outright (`start_recording` calls `vu.stop()`
    // itself). Racing it with a start_vu would only be a stop-first-then-start
    // fight over the same hardware — the meter that matters during a take reads
    // `recording://levels` instead.
    if (window.__isRecording) {
      running = false
      setState('idle')
      return
    }
    try {
      let count: number
      try {
        count = await window.api.startVu(dev)
      } catch {
        // One retry after 400 ms: the device may still be settling out of a
        // just-released format hold (the same grace the channel grid has always
        // taken, now shared by every meter).
        await new Promise<void>(r => setTimeout(r, 400))
        // Nothing is open at this point, so bailing here leaks nothing.
        if (my !== gen) return
        count = await window.api.startVu(dev)
      }
      // A session IS open now. If we were superseded mid-call, leave it: the
      // operation that superseded us is already queued behind this one and will
      // either restart (stop-first) or stop it.
      if (my !== gen) return
      channels = count
      setState('live')
    } catch {
      if (my !== gen) return
      running = false
      setState('failed')
    }
  })
}

function teardown(): void {
  gen++
  running = false
  device = null
  channels = 0
  if (unlisten) {
    try {
      unlisten()
    } catch {
      /* already gone */
    }
    unlisten = null
  }
  setState('idle')
  enqueue(async () => {
    await window.api.stopVu().catch(() => {})
  })
}

// ── Packet fan-out ───────────────────────────────────────────────────────────

function attachListener(): void {
  if (unlisten) return
  const un = window.api.on('vu-levels', onPacket)
  unlisten = typeof un === 'function' ? un : null
}

function onPacket(payload: unknown): void {
  const raw = payload as VuLevels | null
  const peak = raw?.peak_dbfs
  if (!raw || !Array.isArray(peak)) return
  // Packets are the liveness signal, not our own `start_vu` resolution: the
  // native pre-roll buffer emits on this same channel, so levels can arrive
  // for a session this module never started. Deliver them regardless.
  if (feedState !== 'live') {
    if (channels === 0) channels = peak.length
    setState('live')
  }
  // `rms_dbfs` is what the bars draw (the old getDbFS was an RMS over the time
  // domain); fall back to peak if a future emitter ships peaks only.
  const rms = Array.isArray(raw.rms_dbfs) ? raw.rms_dbfs : peak
  // Snapshot: a subscriber is allowed to release itself from its own callback.
  for (const e of subs.slice()) {
    if (!e.alive || !e.sub.onLevels) continue
    const p = e.sub.pick?.() ?? DEFAULT_PICK
    const { l, r } = pickLR(rms, p.mode, p.chL, p.chR)
    try {
      e.sub.onLevels(l, r, raw)
    } catch {
      /* one meter's render error must not starve the others */
    }
  }
}

/**
 * Set the state and tell every subscriber — but only when something they can
 * see actually changed. The channel COUNT is part of that: when packets arrive
 * before `start_vu` resolves (the grid's width comes from the resolution), the
 * state is already `live` and only the count moves.
 */
function setState(next: VuFeedState): void {
  feedState = next
  if (notifiedState === feedState && notifiedChannels === channels) return
  notifiedState = feedState
  notifiedChannels = channels
  for (const e of subs.slice()) {
    if (!e.alive || !e.sub.onState) continue
    try {
      e.sub.onState(feedState, channels)
    } catch {
      /* see onPacket */
    }
  }
}
