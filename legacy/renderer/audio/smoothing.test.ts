// The one property every meter in the app now depends on: how a level MOVES
// must not depend on how often we get to draw it. The old per-frame constants
// failed exactly here — at 30 fps the release took twice as long as at 60, so a
// dropped frame changed the motion law and read as a stutter.
import { describe, expect, it } from 'vitest'
import { RELEASE_TAU_MS, alphaFor, createLevelSmoother } from './smoothing'

/** Drive a smoother over a fixed 2 s trajectory at `fps` and report how long
 *  (in ms of simulated wall clock) the fall from 0 dBFS to −60 dBFS takes to
 *  cover 90 % of its distance. Same trajectory, same clock — only the sampling
 *  rate differs. */
function timeTo90PercentFall(fps: number): number {
  const dt = 1000 / fps
  const s = createLevelSmoother() // instant attack, τ = 80 ms release
  // First second: target pinned at 0 dBFS, so the meter sits at the ceiling.
  for (let t = 0; t < 1000; t += dt) s.step(0, dt)
  // Second second: target drops to the floor. Measure the release.
  const target90 = 0 + (-60 - 0) * 0.9 // −54 dBFS
  let elapsed = 0
  for (let t = 0; t < 1000; t += dt) {
    elapsed += dt
    if (s.step(-60, dt) <= target90) return elapsed
  }
  return Infinity
}

describe('alphaFor', () => {
  it('composes — two half steps leave the same distance as one full step', () => {
    const half = alphaFor(8.35, RELEASE_TAU_MS)
    expect(1 - (1 - half) * (1 - half)).toBeCloseTo(alphaFor(16.7, RELEASE_TAU_MS), 12)
  })

  it('treats τ ≤ 0 as "snap" and dt ≤ 0 as "no time passed"', () => {
    expect(alphaFor(16.7, 0)).toBe(1)
    expect(alphaFor(0, RELEASE_TAU_MS)).toBe(0)
    expect(alphaFor(-5, RELEASE_TAU_MS)).toBe(0)
  })
})

describe('createLevelSmoother — frame-rate independence', () => {
  it('settles in the same wall-clock time at 30, 60 and 120 fps', () => {
    const times = [30, 60, 120].map(timeTo90PercentFall)
    const mean = times.reduce((a, b) => a + b, 0) / times.length
    for (const [i, ms] of times.entries()) {
      expect(Number.isFinite(ms), `fps index ${i} never settled`).toBe(true)
      // ±10 %: the only spread left is the sampling grid (one frame of
      // quantisation), not a difference in the motion law.
      expect(Math.abs(ms - mean) / mean).toBeLessThan(0.1)
    }
    // …and it lands on the analytic answer: −τ·ln(0.1) ≈ 184 ms.
    expect(mean).toBeGreaterThan(160)
    expect(mean).toBeLessThan(210)
  })

  it('reaches the same value after the same elapsed time, whatever the rate', () => {
    // Exactly 240 ms of simulated time at every rate (a trailing partial step
    // closes the grid), so any difference left would be the motion law itself.
    const after = (fps: number): number => {
      const frame = 1000 / fps
      const s = createLevelSmoother({ initial: 0 })
      for (let elapsed = 0; elapsed < 240; ) {
        const dt = Math.min(frame, 240 - elapsed)
        s.step(-60, dt)
        elapsed += dt
      }
      return s.value
    }
    const values = [24, 30, 60, 120, 240].map(after)
    for (const v of values) expect(Math.abs(v - values[0])).toBeLessThan(1e-9)
  })
})

describe('createLevelSmoother — shape', () => {
  it('rises instantly by default (a lagging meter under-reports)', () => {
    const s = createLevelSmoother()
    expect(s.step(-3, 16.7)).toBe(-3)
  })

  it('eases the fall instead of snapping', () => {
    const s = createLevelSmoother({ initial: 0 })
    const v = s.step(-60, 16.7)
    expect(v).toBeLessThan(0)
    expect(v).toBeGreaterThan(-60)
  })

  it('honours a custom attack τ', () => {
    const s = createLevelSmoother({ attackTau: 40, initial: -60 })
    const v = s.step(0, 16.7)
    expect(v).toBeGreaterThan(-60)
    expect(v).toBeLessThan(0)
  })

  it('never overshoots, and a backgrounded tab resumes instead of teleporting', () => {
    const s = createLevelSmoother({ initial: 0 })
    // A 5-second gap is clamped to MAX_STEP_MS, so the meter is still on its
    // way down rather than pinned to the floor as if nothing had happened.
    const v = s.step(-60, 5000)
    expect(v).toBeGreaterThan(-60)
    expect(v).toBeLessThan(-50)
  })

  it('ignores a non-finite target rather than poisoning its state', () => {
    const s = createLevelSmoother({ initial: -12 })
    expect(s.step(Number.NaN, 16.7)).toBe(-12)
    expect(s.value).toBe(-12)
  })

  it('resets to the initial value', () => {
    const s = createLevelSmoother({ initial: -60 })
    s.step(-3, 16.7)
    s.reset()
    expect(s.value).toBe(-60)
  })
})
