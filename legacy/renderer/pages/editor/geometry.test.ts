import { describe, it, expect, beforeEach } from 'vitest'
import { E } from './state'
import {
  clampMain,
  clampPlayable,
  effIntroDur,
  effOutroDur,
  minPlayableSec,
  maxPlayableSec,
} from './geometry'

// These read the shared editor state `E`. Reset the fields we touch before each
// test so the singleton can't bleed between cases. A truthy stand-in is enough
// for the intro/outro buffers — the code only checks their presence.
const fakeBuf = {} as unknown as AudioBuffer

beforeEach(() => {
  E.duration = 100
  E.includeIntroOutro = false
  E.introBuffer = null
  E.outroBuffer = null
  E.introDuration = 0
  E.outroDuration = 0
})

describe('clampMain', () => {
  it('clamps to [0, duration]', () => {
    expect(clampMain(-5)).toBe(0)
    expect(clampMain(50)).toBe(50)
    expect(clampMain(150)).toBe(100)
  })
})

describe('intro/outro effective durations', () => {
  it('are zero unless includeIntroOutro AND a buffer is loaded', () => {
    E.introDuration = 8
    E.outroDuration = 5
    expect(effIntroDur()).toBe(0) // includeIntroOutro false
    E.includeIntroOutro = true
    expect(effIntroDur()).toBe(0) // no buffer yet
    E.introBuffer = fakeBuf
    expect(effIntroDur()).toBe(8) // enabled + buffer present
    E.outroBuffer = fakeBuf
    expect(effOutroDur()).toBe(5)
  })
})

describe('playable range', () => {
  it('is [0, duration] with no intro/outro', () => {
    // toBeCloseTo: `-effIntroDur()` yields -0 here, and toBe uses Object.is
    // (-0 !== 0). Numerically it's zero; the app uses it only in Math.min/max.
    expect(minPlayableSec()).toBeCloseTo(0)
    expect(maxPlayableSec()).toBe(100)
    expect(clampPlayable(-10)).toBeCloseTo(0)
    expect(clampPlayable(200)).toBe(100)
    expect(clampPlayable(42)).toBe(42)
  })

  it('extends into the intro (negative) + outro tail when enabled', () => {
    E.includeIntroOutro = true
    E.introBuffer = fakeBuf
    E.outroBuffer = fakeBuf
    E.introDuration = 8
    E.outroDuration = 5
    expect(minPlayableSec()).toBe(-8)
    expect(maxPlayableSec()).toBe(105)
    expect(clampPlayable(-20)).toBe(-8) // can't seek before the intro
    expect(clampPlayable(200)).toBe(105) // can't seek past the outro tail
    expect(clampPlayable(-3)).toBe(-3) // inside the intro is fine
  })
})
