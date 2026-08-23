import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { limitPulse, resetHaptics, snapPulse } from './haptics'

const calls: string[] = []

beforeEach(() => {
  calls.length = 0
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-08-06T10:00:00Z'))
  ;(globalThis as unknown as { window: unknown }).window = {
    api: {
      hapticPerform: (pattern: string) => {
        calls.push(pattern)
        return Promise.resolve()
      },
    },
  }
  resetHaptics()
})

afterEach(() => {
  vi.useRealTimers()
})

/** Move past the throttle window without moving so far that a test reads as
 *  "much later" — 200 ms is a slow, deliberate drag between two boundaries. */
function laterMs(ms = 200): void {
  vi.advanceTimersByTime(ms)
}

describe('snapPulse', () => {
  it('does not tap when nothing snapped', () => {
    expect(snapPulse(12.34, 12.34)).toBe(false)
    expect(calls).toEqual([])
  })

  it('taps "alignment" when a value is claimed by a boundary', () => {
    expect(snapPulse(12.3, 12.5)).toBe(true)
    expect(calls).toEqual(['alignment'])
  })

  it('does not re-tap while hovering on the SAME boundary', () => {
    snapPulse(12.3, 12.5)
    laterMs()
    expect(snapPulse(12.31, 12.5)).toBe(false)
    laterMs()
    expect(snapPulse(12.29, 12.5)).toBe(false)
    expect(calls).toEqual(['alignment'])
  })

  it('taps again on a DIFFERENT boundary', () => {
    snapPulse(12.3, 12.5)
    laterMs()
    expect(snapPulse(20.1, 20.0)).toBe(true)
    expect(calls).toEqual(['alignment', 'alignment'])
  })

  it('re-arms after free movement, so returning to a boundary taps', () => {
    snapPulse(12.3, 12.5)
    laterMs()
    snapPulse(15.0, 15.0) // free — no boundary near
    laterMs()
    expect(snapPulse(12.3, 12.5)).toBe(true)
    expect(calls).toEqual(['alignment', 'alignment'])
  })

  it('throttles a fast drag across many boundaries into single taps', () => {
    // A 125 Hz drag: an event every 8 ms, each landing on a new boundary.
    for (let i = 0; i < 20; i++) {
      snapPulse(i + 0.01, i + 0.5)
      vi.advanceTimersByTime(8)
    }
    // 20 events over 160 ms, floor of 80 ms between taps → at most 3.
    expect(calls.length).toBeLessThanOrEqual(3)
    expect(calls.length).toBeGreaterThan(0)
  })
})

describe('limitPulse', () => {
  it('uses the detent pattern, not the alignment one', () => {
    limitPulse()
    expect(calls).toEqual(['levelChange'])
  })

  it('shares the throttle with snapPulse — a drag pinned at a limit does not buzz', () => {
    limitPulse()
    limitPulse()
    limitPulse()
    expect(calls).toEqual(['levelChange'])
  })
})

describe('the missing-command case', () => {
  it('is a silent no-op when the shim has no hapticPerform', () => {
    ;(globalThis as unknown as { window: { api: Record<string, unknown> } }).window = { api: {} }
    resetHaptics()
    expect(() => snapPulse(1, 2)).not.toThrow()
    expect(() => limitPulse()).not.toThrow()
  })
})
