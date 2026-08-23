import { describe, expect, it } from 'vitest'
import {
  BACKWARD_EPS,
  createEtaEstimator,
  etaBucket,
  formatEta,
  formatPercent,
  GROW_TOLERANCE,
  MIN_SAMPLES,
  MIN_SPAN_MS,
  STALL_MS,
  type EtaReading,
} from './progress-core'

/** A translator that echoes the key, so assertions name the branch taken. */
const tKey = (key: string): string => key
/** The Norwegian fallbacks, so assertions can read the actual sentence. */
const tFallback = (_key: string, fallback = ''): string => fallback

/** `tf` counterparts: same near-identity, then real substitution. */
const interp = (str: string, params: Record<string, string | number>): string =>
  Object.entries(params).reduce((acc, [k, v]) => acc.replaceAll(`{${k}}`, String(v)), str)
const tfKey = (key: string): string => key
const tfFallback = (
  _key: string,
  params: Record<string, string | number>,
  fallback = '',
): string => interp(fallback, params)

/**
 * Drive a run at a constant rate. Returns the reading after the last sample.
 * `rate` is fraction per second; samples land every `stepMs`.
 */
function runConstant(
  est: ReturnType<typeof createEtaEstimator>,
  opts: { rate: number; stepMs: number; steps: number; startMs?: number; startF?: number },
): { reading: EtaReading; nowMs: number; fraction: number } {
  let now = opts.startMs ?? 0
  let f = opts.startF ?? 0
  let reading = est.push(f, now)
  for (let i = 0; i < opts.steps; i++) {
    now += opts.stepMs
    f = Math.min(1, f + (opts.rate * opts.stepMs) / 1000)
    reading = est.push(f, now)
  }
  return { reading, nowMs: now, fraction: f }
}

describe('createEtaEstimator — warm-up gate', () => {
  it('says nothing at all from a single sample', () => {
    const est = createEtaEstimator()
    expect(est.push(0, 0)).toEqual({ etaMs: null, stable: false })
  })

  it('withholds an estimate until MIN_SAMPLES moves AND MIN_SPAN_MS have passed', () => {
    // Three moves, but crammed into 300 ms — the span gate must still hold.
    const fast = createEtaEstimator()
    fast.push(0, 0)
    fast.push(0.1, 100)
    fast.push(0.2, 200)
    expect(fast.push(0.3, 300).stable).toBe(false)

    // Enough span, but only two moves — the sample gate holds.
    const sparse = createEtaEstimator()
    sparse.push(0, 0)
    sparse.push(0.1, 2000)
    expect(sparse.push(0.2, 4000).stable).toBe(false)

    // Both satisfied → an estimate appears.
    const ok = createEtaEstimator()
    ok.push(0, 0)
    ok.push(0.1, 1000)
    ok.push(0.2, 2000)
    const r = ok.push(0.3, 3000)
    expect(r.stable).toBe(true)
    expect(r.etaMs).not.toBeNull()
  })

  it('needs at least MIN_SAMPLES moves — repeated identical fractions are not moves', () => {
    const est = createEtaEstimator()
    est.push(0.1, 0)
    est.push(0.1, 1000)
    est.push(0.1, 2000)
    est.push(0.1, 3000)
    expect(est.push(0.1, 4000).stable).toBe(false)
    expect(MIN_SAMPLES).toBe(3)
    expect(MIN_SPAN_MS).toBe(2000)
  })
})

describe('createEtaEstimator — constant rate', () => {
  it('lands within a few percent of the truth', () => {
    // 2 %/s → a full run is 50 s. After 20 s we are at 40 %, so 30 s remain.
    const est = createEtaEstimator()
    const { reading } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    expect(reading.stable).toBe(true)
    expect(reading.etaMs!).toBeGreaterThan(29_000)
    expect(reading.etaMs!).toBeLessThan(31_000)
  })

  it('counts down between events, without inventing progress', () => {
    const est = createEtaEstimator()
    const { reading, nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const later = est.read(nowMs + 3000)
    expect(later.stable).toBe(true)
    // Three seconds of coasting, and not a millisecond more.
    expect(later.etaMs!).toBeCloseTo(reading.etaMs! - 3000, -1)
  })
})

describe('createEtaEstimator — a rate change mid-run', () => {
  it('re-converges on the new rate instead of averaging the whole run', () => {
    const est = createEtaEstimator()
    // Phase 1: a slow measure pass, 1 %/s for 20 s (0 → 20 %).
    let now = 0
    let f = 0
    for (let i = 0; i < 40; i++) {
      now += 500
      f += 0.005
      est.push(f, now)
    }
    const slow = est.read(now)
    // At 1 %/s with 80 % left, the honest answer is ~80 s.
    expect(slow.etaMs!).toBeGreaterThan(70_000)

    // Phase 2: the encode, ten times faster (10 %/s), for 6 s (20 % → 80 %).
    for (let i = 0; i < 12; i++) {
      now += 500
      f += 0.05
      est.push(f, now)
    }
    const fast = est.read(now)
    // A naive from-the-start average would still say ~30 s (26 s elapsed for
    // 80 %). The EMA has re-converged: 20 % left at ~10 %/s is a couple of s.
    expect(fast.etaMs!).toBeLessThan(8_000)
  })
})

describe('createEtaEstimator — the monotone display rule', () => {
  it('holds instead of jumping up when the raw estimate wobbles slightly', () => {
    const est = createEtaEstimator()
    const { reading, nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const before = reading.etaMs!
    // One sample that barely moves: the raw estimate ticks up a little. The
    // display must not follow it upward — it may only coast down.
    const after = est.push(0.4005, nowMs + 500)
    expect(after.etaMs!).toBeLessThanOrEqual(before)
  })

  it('eases — never snaps — when a real slowdown pushes the estimate up', () => {
    const est = createEtaEstimator()
    const { reading, nowMs, fraction } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const before = reading.etaMs!
    // Four seconds of nothing: the rate decays, the raw estimate explodes.
    let now = nowMs
    let prev = before
    const seen: number[] = []
    for (let i = 0; i < 8; i++) {
      now += 500
      const r = est.push(fraction, now)
      if (!r.stable) break
      seen.push(r.etaMs!)
      // Growth is allowed, but never as a single leap to the raw value.
      expect(r.etaMs!).toBeLessThan(prev * 4)
      prev = r.etaMs!
    }
    expect(seen.length).toBeGreaterThan(0)
    expect(seen[seen.length - 1]).toBeGreaterThan(before - 4000)
    expect(GROW_TOLERANCE).toBeGreaterThan(1)
  })
})

describe('createEtaEstimator — stalls', () => {
  it('grows gracefully, then admits it does not know', () => {
    const est = createEtaEstimator()
    const { nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    // Just under the stall threshold: still an estimate.
    expect(est.read(nowMs + STALL_MS - 1).stable).toBe(true)
    // At the threshold: «beregner …».
    const stalled = est.read(nowMs + STALL_MS)
    expect(stalled.stable).toBe(false)
    expect(stalled.etaMs).toBeNull()
    expect(formatEta(stalled.etaMs, tKey, tfKey)).toBe('progress.etaCalculating')
  })

  it('recovers as soon as the job moves again', () => {
    const est = createEtaEstimator()
    const { nowMs, fraction } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    expect(est.read(nowMs + 9000).stable).toBe(false)
    const resumed = est.push(fraction + 0.05, nowMs + 9500)
    expect(resumed.stable).toBe(true)
    expect(resumed.etaMs).not.toBeNull()
  })
})

describe('createEtaEstimator — bursts and odd input', () => {
  it('survives a burst of samples inside one millisecond', () => {
    const est = createEtaEstimator()
    const { nowMs, fraction } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    let r: EtaReading = est.read(nowMs)
    for (let i = 0; i < 20; i++) r = est.push(fraction + i * 0.001, nowMs)
    expect(r.stable).toBe(true)
    expect(Number.isFinite(r.etaMs!)).toBe(true)
  })

  it('treats float-noise backward drift as a hold, not a restart', () => {
    // `bytesRead / expected` re-emitting the same value an ulp lower must not
    // wipe the estimate — only a real rewind does.
    const est = createEtaEstimator()
    const { nowMs, fraction } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const drifted = est.push(fraction - 1e-15, nowMs + 200)
    expect(drifted.stable).toBe(true)
    expect(est.push(fraction - BACKWARD_EPS * 2, nowMs + 400).stable).toBe(false)
  })

  it('catches up without a backwards jump when a burst lands after a gap', () => {
    const est = createEtaEstimator()
    const { reading, nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const before = reading.etaMs!
    // 2 s of silence then a leap to 80 % — twice the expected progress.
    const after = est.push(0.8, nowMs + 2000)
    expect(after.stable).toBe(true)
    expect(after.etaMs!).toBeLessThan(before)
  })

  it('ignores NaN and Infinity rather than poisoning the rate', () => {
    const est = createEtaEstimator()
    const { reading, nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    est.push(Number.NaN, nowMs + 100)
    est.push(Number.POSITIVE_INFINITY, nowMs + 200)
    const after = est.read(nowMs)
    expect(after.stable).toBe(true)
    expect(after.etaMs!).toBeCloseTo(reading.etaMs!, -2)
  })

  it('re-warms when progress jumps backward (a phase restarted)', () => {
    const est = createEtaEstimator()
    const { nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    const back = est.push(0.05, nowMs + 500)
    expect(back.stable).toBe(false)
    expect(back.etaMs).toBeNull()
  })

  it('snaps to zero at 100 % and on complete()', () => {
    const est = createEtaEstimator()
    const { nowMs } = runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    expect(est.push(1, nowMs + 500)).toEqual({ etaMs: 0, stable: true })

    const other = createEtaEstimator()
    other.push(0.1, 0)
    expect(other.complete()).toEqual({ etaMs: 0, stable: true })
    expect(other.read(999_999)).toEqual({ etaMs: 0, stable: true })
  })

  it('reset() puts it back to knowing nothing', () => {
    const est = createEtaEstimator()
    runConstant(est, { rate: 0.02, stepMs: 500, steps: 40 })
    est.reset()
    expect(est.read(10_000)).toEqual({ etaMs: null, stable: false })
  })
})

describe('etaBucket', () => {
  const table: Array<[number | null, ReturnType<typeof etaBucket>]> = [
    [null, { kind: 'unknown' }],
    [Number.NaN, { kind: 'unknown' }],
    [Number.POSITIVE_INFINITY, { kind: 'unknown' }],
    [0, { kind: 'under10s' }],
    [9_999, { kind: 'under10s' }],
    [10_000, { kind: 'seconds', value: 10 }],
    [17_000, { kind: 'seconds', value: 20 }],
    [44_000, { kind: 'seconds', value: 40 }],
    [54_999, { kind: 'seconds', value: 50 }],
    [55_000, { kind: 'minutes', value: 1 }],
    [89_999, { kind: 'minutes', value: 1 }],
    [90_000, { kind: 'minutes', value: 2 }],
    [8 * 60_000, { kind: 'minutes', value: 8 }],
    [12 * 60_000, { kind: 'minutes', value: 10 }],
    [13 * 60_000, { kind: 'minutes', value: 15 }],
    [47 * 60_000, { kind: 'minutes', value: 45 }],
    [3600_000, { kind: 'hours', hours: 1, minutes: 0 }],
    [3600_000 + 20 * 60_000, { kind: 'hours', hours: 1, minutes: 20 }],
    [2 * 3600_000 + 59 * 60_000, { kind: 'hours', hours: 3, minutes: 0 }],
  ]
  for (const [ms, want] of table) {
    it(`${ms} → ${JSON.stringify(want)}`, () => {
      expect(etaBucket(ms)).toEqual(want)
    })
  }

  it('never claims a precision it does not have', () => {
    // Every seconds bucket is a whole ten; every hours bucket a whole ten of
    // minutes. Nothing in between ever reaches a user.
    for (let ms = 10_000; ms < 55_000; ms += 137) {
      const b = etaBucket(ms)
      expect(b.kind).toBe('seconds')
      expect((b as { value: number }).value % 10).toBe(0)
    }
    for (let ms = 3600_000; ms < 6 * 3600_000; ms += 60_001) {
      const b = etaBucket(ms) as { kind: string; minutes: number }
      expect(b.kind).toBe('hours')
      expect(b.minutes % 10).toBe(0)
      expect(b.minutes).toBeLessThan(60)
    }
  })
})

describe('formatEta', () => {
  it('uses one key per bucket', () => {
    expect(formatEta(null, tKey, tfKey)).toBe('progress.etaCalculating')
    expect(formatEta(5_000, tKey, tfKey)).toBe('progress.etaUnder10s')
    expect(formatEta(20_000, tKey, tfKey)).toBe('progress.etaSeconds')
    expect(formatEta(120_000, tKey, tfKey)).toBe('progress.etaMinutes')
    expect(formatEta(3600_000, tKey, tfKey)).toBe('progress.etaHours')
    expect(formatEta(3600_000 + 20 * 60_000, tKey, tfKey)).toBe('progress.etaHoursMinutes')
  })

  it('renders the Norwegian sentences with the numbers substituted', () => {
    expect(formatEta(null, tFallback, tfFallback)).toBe('beregner …')
    expect(formatEta(3_000, tFallback, tfFallback)).toBe('under 10 s igjen')
    expect(formatEta(21_000, tFallback, tfFallback)).toBe('ca. 20 s igjen')
    expect(formatEta(150_000, tFallback, tfFallback)).toBe('ca. 3 min igjen')
    expect(formatEta(2 * 3600_000, tFallback, tfFallback)).toBe('ca. 2 t igjen')
    expect(formatEta(3600_000 + 31 * 60_000, tFallback, tfFallback)).toBe('ca. 1 t 30 min igjen')
  })
})

describe('formatPercent', () => {
  it('rounds, clamps and never emits NaN', () => {
    expect(formatPercent(0)).toBe(0)
    expect(formatPercent(0.4271)).toBe(43)
    expect(formatPercent(1)).toBe(100)
    expect(formatPercent(1.7)).toBe(100)
    expect(formatPercent(-3)).toBe(0)
    expect(formatPercent(Number.NaN)).toBe(0)
  })
})
