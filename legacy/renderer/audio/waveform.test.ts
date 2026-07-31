// Pure-helper specs for the recording waveform's pacing + bloom budget — the
// two knobs that turned "jank that scales with loudness" (2026-07-31) into a
// constant, affordable per-frame cost.
import { describe, expect, it } from 'vitest'
import { BLOOM_BUDGET, DRAW_INTERVAL_MS, bloomStride, nextDrawGate } from './waveform'
import { easeFallAlpha } from '../pages/recording'

describe('nextDrawGate', () => {
  it('holds the gate inside the interval (no draw)', () => {
    expect(nextDrawGate(1000, 1000 + DRAW_INTERVAL_MS - 1)).toBe(1000)
  })

  it('advances by exactly one interval on a due draw — steady cadence', () => {
    const g1 = nextDrawGate(1000, 1000 + DRAW_INTERVAL_MS + 3)
    expect(g1).toBe(1000 + DRAW_INTERVAL_MS)
    // The +3 ms jitter must NOT drift the phase: the next draw is due exactly
    // one interval after the PREVIOUS GATE, not after the jittered timestamp.
    expect(nextDrawGate(g1, g1 + DRAW_INTERVAL_MS - 2)).toBe(g1)
    expect(nextDrawGate(g1, g1 + DRAW_INTERVAL_MS)).toBe(g1 + DRAW_INTERVAL_MS)
  })

  it('resyncs to now after a long stall instead of replaying missed draws', () => {
    const now = 1000 + DRAW_INTERVAL_MS * 10
    expect(nextDrawGate(1000, now)).toBe(now)
  })
})

describe('bloomStride', () => {
  it('stamps everything when under budget', () => {
    expect(bloomStride(10, BLOOM_BUDGET)).toBe(1)
    expect(bloomStride(BLOOM_BUDGET, BLOOM_BUDGET)).toBe(1)
  })

  it('caps the stamp count at the budget for any qualifying count', () => {
    for (const qualifying of [BLOOM_BUDGET + 1, 100, 478, 5000]) {
      const stride = bloomStride(qualifying, BLOOM_BUDGET)
      const stamped = Math.ceil(qualifying / stride)
      expect(stamped).toBeLessThanOrEqual(BLOOM_BUDGET)
      // …and we never over-decimate below half the budget (haze stays visible).
      expect(stamped).toBeGreaterThanOrEqual(BLOOM_BUDGET / 2)
    }
  })
})

describe('easeFallAlpha', () => {
  it('is frame-rate independent: two 16.7 ms steps ≈ one 33.4 ms step', () => {
    const one = 1 - (1 - easeFallAlpha(16.7)) * (1 - easeFallAlpha(16.7))
    expect(one).toBeCloseTo(easeFallAlpha(33.4), 10)
  })

  it('covers most of the distance within a few hundred ms (responsive release)', () => {
    expect(easeFallAlpha(240)).toBeGreaterThan(0.9)
    expect(easeFallAlpha(0)).toBe(0)
  })
})
