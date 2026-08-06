import { describe, expect, it } from 'vitest'

import { decidePreroll, type PrerollConditions } from './preroll-lifecycle'

const ready: PrerollConditions = {
  enabled: true,
  seconds: 30,
  deviceKnown: true,
  isRecording: false,
}

describe('decidePreroll', () => {
  it('runs only when every condition holds', () => {
    expect(decidePreroll(ready)).toBe('run')
  })

  it('never runs without the advanced opt-in', () => {
    expect(decidePreroll({ ...ready, enabled: false })).toBe('stop')
  })

  it('treats pre-roll = 0 seconds as off', () => {
    expect(decidePreroll({ ...ready, seconds: 0 })).toBe('stop')
    expect(decidePreroll({ ...ready, seconds: -5 })).toBe('stop')
  })

  it('will not open an unknown device', () => {
    expect(decidePreroll({ ...ready, deviceKnown: false })).toBe('stop')
  })

  it('yields the microphone to a running recording — the one-owner invariant', () => {
    expect(decidePreroll({ ...ready, isRecording: true })).toBe('stop')
  })

  it('stops on ANY failing condition, not just the first', () => {
    // Every single-condition failure, and the all-off case.
    const failures: Array<Partial<PrerollConditions>> = [
      { enabled: false },
      { seconds: 0 },
      { deviceKnown: false },
      { isRecording: true },
      { enabled: false, seconds: 0, deviceKnown: false, isRecording: true },
    ]
    for (const f of failures) {
      expect(decidePreroll({ ...ready, ...f })).toBe('stop')
    }
  })

  it('is pure — the same input always gives the same answer', () => {
    const input = { ...ready }
    const first = decidePreroll(input)
    expect(decidePreroll(input)).toBe(first)
    expect(input).toEqual(ready)
  })
})
