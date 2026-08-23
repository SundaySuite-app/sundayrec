/**
 * Pure decisions for the backend VU feed — no DOM, no `window.api`, fully
 * unit-tested in vu-feed-core.test.ts.
 *
 * The renderer no longer opens ANY microphone of its own (see audio/vu-feed.ts
 * for the why). Every meter in the app now reads the same `vu://levels`
 * packets, which carry one peak + one RMS entry per NATIVE device channel — a
 * Qu-5's 32, not getUserMedia's 2. Turning that array into the two numbers a
 * stereo meter draws is the interesting decision, and it lives here.
 */

import type { ChannelMode } from '../../types'

/** The dBFS floor every meter in the app shares (audio/vu.ts, channel-grid). */
export const VU_FLOOR_DB = -60

/** A picked stereo pair, in dBFS, floored at [VU_FLOOR_DB]. */
export interface StereoPick {
  l: number
  r: number
}

/**
 * One channel's level, clamped exactly the way the capture path clamps its
 * channel indices (`audio::asio::build_route_plan`): a stale settings pick can
 * never read out of bounds — it reads the last valid channel instead.
 *
 * `null` is how serde_json serialises `f32::NEG_INFINITY` (digital silence);
 * so is any non-finite value that slips through. Both read as the floor.
 */
export function levelAt(
  levels: readonly (number | null | undefined)[] | null | undefined,
  index: number,
): number {
  if (!levels || levels.length === 0) return VU_FLOOR_DB
  const i = Math.min(Math.max(0, Math.trunc(index)), levels.length - 1)
  const v = levels[i]
  if (v == null || !Number.isFinite(v)) return VU_FLOOR_DB
  return Math.max(VU_FLOOR_DB, Math.min(0, v))
}

/**
 * Average two dBFS levels the way the capture engine averages the samples
 * themselves — in the LINEAR domain.
 *
 * `MixHalf` (audio/asio.rs) and the ffmpeg `pan=mono|c0=0.5*c0+0.5*c1` filter
 * both compute `0.5·(a + b)` on amplitudes. Averaging the dB numbers instead
 * would be a different function entirely: two identical −6 dB channels mix to
 * −6 dB (same amplitude, doubled then halved), which the dB average happens to
 * get right, but a −6 dB channel mixed with silence is −12 dB, where the dB
 * average would claim −33.
 */
export function mixDb(a: number, b: number): number {
  const linear = 0.5 * (Math.pow(10, a / 20) + Math.pow(10, b / 20))
  if (linear <= 0) return VU_FLOOR_DB
  return Math.max(VU_FLOOR_DB, Math.min(0, 20 * Math.log10(linear)))
}

/**
 * The two numbers a stereo meter should draw for one `vu://levels` array.
 *
 * Mirrors `audio::asio::build_route_plan` — the routing the NATIVE capture
 * engine (the default path since v0.6.0) actually applies to the recording —
 * so the meter shows the signal that will land in the file, not the signal the
 * device happens to expose:
 *   - stereo   → the user's L/R channel picks,
 *   - monoL    → the L pick in both bars (the take is that one channel),
 *   - monoR    → the R pick in both bars,
 *   - monoMix  → channels 0 & 1 averaged, in both bars.
 *
 * ⚠️ `monoMix` deliberately ignores the L/R picks: `build_route_plan` pins it
 * to `MixHalf(0, 1)` and `capture::channel_map_filter` to `c0/c1`. A meter that
 * showed the picked channels there would be lying about the recording.
 */
export function pickLR(
  levels: readonly (number | null | undefined)[] | null | undefined,
  mode: ChannelMode,
  chL: number,
  chR: number,
): StereoPick {
  const l = levelAt(levels, chL)
  const r = levelAt(levels, chR)
  switch (mode) {
    case 'monoL':
      return { l, r: l }
    case 'monoR':
      return { l: r, r }
    case 'monoMix': {
      const m = mixDb(levelAt(levels, 0), levelAt(levels, 1))
      return { l: m, r: m }
    }
    default:
      return { l, r }
  }
}

// ── Feed bookkeeping ─────────────────────────────────────────────────────────

/** What a subscriber count change means for the engine. */
export type FeedTransition = 'start' | 'stop' | 'none'

/**
 * Refcount step for the shared feed. The engine is a process-wide singleton, so
 * the FIRST subscriber starts it and the LAST one stops it; everything in
 * between is free.
 *
 * A release that arrives twice (a page's stop path running after its own
 * teardown) must not drive the count negative — that would make the next
 * acquire look like 0 → 1 twice and leak a second engine start.
 */
export function refcountStep(
  count: number,
  delta: 1 | -1,
): { count: number; transition: FeedTransition } {
  const prev = Math.max(0, count)
  const next = Math.max(0, prev + delta)
  if (prev === 0 && next > 0) return { count: next, transition: 'start' }
  if (prev > 0 && next === 0) return { count: next, transition: 'stop' }
  return { count: next, transition: 'none' }
}

/** Two device names refer to the same device. `null` = "the system default". */
export function sameDevice(a: string | null | undefined, b: string | null | undefined): boolean {
  const norm = (v: string | null | undefined): string | null => {
    const s = (v ?? '').trim()
    return s === '' ? null : s
  }
  return norm(a) === norm(b)
}

/** What the feed must do to serve `requested`. */
export type DeviceAction = 'start' | 'restart' | 'keep'

/**
 * Restart decision on a device change.
 *
 * `start_vu` is stop-first-then-start on the Rust side, so 'restart' and
 * 'start' issue the same call — they are kept apart because only one of them
 * is a device SWITCH, and that is the case worth logging and worth guarding
 * with a generation token.
 */
export function deviceAction(
  currentDevice: string | null,
  requestedDevice: string | null,
  running: boolean,
): DeviceAction {
  if (!running) return 'start'
  return sameDevice(currentDevice, requestedDevice) ? 'keep' : 'restart'
}

/**
 * Which device the feed should run on, given every live subscriber.
 *
 * The most RECENTLY acquired subscriber with an explicit device wins: the
 * channel grid opening on a just-tapped mixer must re-point the feed, and the
 * home meter (which follows the saved settings) must not drag it back. A
 * subscriber that leaves `deviceName` undefined has no opinion at all —
 * "whatever is running" — while an explicit `null` means "the system default".
 */
export function resolveDevice(
  subs: readonly { deviceName?: string | null }[],
): string | null {
  for (let i = subs.length - 1; i >= 0; i--) {
    const d = subs[i].deviceName
    if (d === undefined) continue
    return d === null || d.trim() === '' ? null : d
  }
  return null
}
