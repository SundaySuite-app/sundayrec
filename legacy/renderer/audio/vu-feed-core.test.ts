// The backend VU feed's pure decisions: turning one `vu://levels` array into
// the two numbers a stereo meter draws (mirroring the capture engine's own
// routing), and the bookkeeping that keeps ONE engine session alive across
// however many meters happen to be on screen.
import { describe, expect, it } from 'vitest'
import {
  VU_FLOOR_DB,
  deviceAction,
  levelAt,
  mixDb,
  pickLR,
  refcountStep,
  resolveDevice,
  sameDevice,
} from './vu-feed-core'

describe('levelAt', () => {
  it('reads the requested channel', () => {
    expect(levelAt([-3, -20, -40], 1)).toBe(-20)
  })

  it('null (serde -inf) reads as the floor', () => {
    expect(levelAt([null, -20], 0)).toBe(VU_FLOOR_DB)
  })

  it('non-finite entries read as the floor', () => {
    expect(levelAt([Number.NaN, -20], 0)).toBe(VU_FLOOR_DB)
    expect(levelAt([-Infinity], 0)).toBe(VU_FLOOR_DB)
  })

  it('floors anything under -60 and caps at 0 dBFS', () => {
    expect(levelAt([-120], 0)).toBe(VU_FLOOR_DB)
    expect(levelAt([3.5], 0)).toBe(0)
  })

  it('clamps an out-of-range pick to the last channel, like build_route_plan', () => {
    // A settings pick of ch 31 on a device that came back with 2 channels.
    expect(levelAt([-10, -6], 31)).toBe(-6)
    expect(levelAt([-10, -6], -4)).toBe(-10)
  })

  it('an empty or missing payload is silence, not a crash', () => {
    expect(levelAt([], 0)).toBe(VU_FLOOR_DB)
    expect(levelAt(null, 0)).toBe(VU_FLOOR_DB)
    expect(levelAt(undefined, 3)).toBe(VU_FLOOR_DB)
  })
})

describe('mixDb', () => {
  it('mixes in the LINEAR domain: identical channels keep their level', () => {
    expect(mixDb(-6, -6)).toBeCloseTo(-6, 6)
  })

  it('a channel mixed with silence loses 6 dB', () => {
    // 0.5 * (10^(-6/20) + 0) → -12.02 dB, NOT the dB average (-33).
    expect(mixDb(-6, VU_FLOOR_DB)).toBeCloseTo(-12.006, 2)
  })

  it('two silent channels stay at the floor', () => {
    expect(mixDb(VU_FLOOR_DB, VU_FLOOR_DB)).toBeCloseTo(VU_FLOOR_DB, 6)
  })

  it('never reports above 0 dBFS', () => {
    expect(mixDb(0, 0)).toBe(0)
  })
})

describe('pickLR', () => {
  const levels = [-40, -30, -20, -10, -6, -3]

  it('stereo takes the two picked channels', () => {
    expect(pickLR(levels, 'stereo', 4, 5)).toEqual({ l: -6, r: -3 })
  })

  it('stereo defaults (0, 1) read the first two channels', () => {
    expect(pickLR(levels, 'stereo', 0, 1)).toEqual({ l: -40, r: -30 })
  })

  it('monoL shows the L pick in BOTH bars — the take is that one channel', () => {
    expect(pickLR(levels, 'monoL', 4, 5)).toEqual({ l: -6, r: -6 })
  })

  it('monoR shows the R pick in both bars', () => {
    expect(pickLR(levels, 'monoR', 4, 5)).toEqual({ l: -3, r: -3 })
  })

  it('monoMix mixes channels 0 & 1, ignoring the picks (build_route_plan parity)', () => {
    const got = pickLR(levels, 'monoMix', 4, 5)
    expect(got.l).toBe(got.r)
    expect(got.l).toBeCloseTo(mixDb(-40, -30), 6)
  })

  it('monoMix on a single-channel device mixes ch0 with itself', () => {
    expect(pickLR([-12], 'monoMix', 0, 1).l).toBeCloseTo(-12, 6)
  })

  it('a dual-mono pick (L = R) is allowed and shows the same channel twice', () => {
    expect(pickLR(levels, 'stereo', 2, 2)).toEqual({ l: -20, r: -20 })
  })

  it('silence on every channel is the floor, not -Infinity', () => {
    expect(pickLR([null, null], 'stereo', 0, 1)).toEqual({ l: VU_FLOOR_DB, r: VU_FLOOR_DB })
  })
})

describe('refcountStep', () => {
  it('the first subscriber starts the engine', () => {
    expect(refcountStep(0, 1)).toEqual({ count: 1, transition: 'start' })
  })

  it('further subscribers are free', () => {
    expect(refcountStep(1, 1)).toEqual({ count: 2, transition: 'none' })
    expect(refcountStep(2, -1)).toEqual({ count: 1, transition: 'none' })
  })

  it('the last release stops the engine', () => {
    expect(refcountStep(1, -1)).toEqual({ count: 0, transition: 'stop' })
  })

  it('a double release cannot drive the count negative', () => {
    expect(refcountStep(0, -1)).toEqual({ count: 0, transition: 'none' })
    // …and the next acquire is still a single start.
    expect(refcountStep(0, 1)).toEqual({ count: 1, transition: 'start' })
  })
})

describe('sameDevice', () => {
  it('null, undefined and blank all mean "the system default"', () => {
    expect(sameDevice(null, undefined)).toBe(true)
    expect(sameDevice(null, '')).toBe(true)
    expect(sameDevice('  ', undefined)).toBe(true)
  })

  it('ignores surrounding whitespace but not the name itself', () => {
    expect(sameDevice(' Qu-5 ', 'Qu-5')).toBe(true)
    expect(sameDevice('Qu-5', 'Qu-16')).toBe(false)
    expect(sameDevice('Qu-5', null)).toBe(false)
  })
})

describe('deviceAction', () => {
  it('not running: always a start', () => {
    expect(deviceAction(null, 'Qu-5', false)).toBe('start')
    expect(deviceAction('Qu-5', 'Qu-5', false)).toBe('start')
  })

  it('running on the same device: keep it', () => {
    expect(deviceAction('Qu-5', 'Qu-5', true)).toBe('keep')
    expect(deviceAction(null, null, true)).toBe('keep')
  })

  it('running on a different device: restart', () => {
    expect(deviceAction('Qu-5', 'Scarlett 18i20', true)).toBe('restart')
    expect(deviceAction(null, 'Qu-5', true)).toBe('restart')
  })
})

describe('resolveDevice', () => {
  it('no subscribers: the system default', () => {
    expect(resolveDevice([])).toBeNull()
  })

  it('the most recently acquired subscriber with an opinion wins', () => {
    expect(resolveDevice([{ deviceName: 'Qu-5' }, { deviceName: 'Scarlett' }])).toBe('Scarlett')
  })

  it('a subscriber without an opinion defers to the one before it', () => {
    expect(resolveDevice([{ deviceName: 'Qu-5' }, {}])).toBe('Qu-5')
  })

  it('an explicit null means "the system default" and DOES override', () => {
    expect(resolveDevice([{ deviceName: 'Qu-5' }, { deviceName: null }])).toBeNull()
  })

  it('a blank name is the system default too', () => {
    expect(resolveDevice([{ deviceName: '   ' }])).toBeNull()
  })
})
