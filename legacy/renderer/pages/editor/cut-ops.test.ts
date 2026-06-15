import { describe, it, expect } from 'vitest'
import type { Cut } from './state'
import { addCutToList, mergeCuts, MIN_CUT_LENGTH_SEC } from './cut-ops'

const cut = (start: number, end: number): Cut => ({ start, end })

describe('mergeCuts', () => {
  it('sorts and leaves disjoint cuts untouched', () => {
    expect(mergeCuts([cut(50, 60), cut(10, 20)])).toEqual([cut(10, 20), cut(50, 60)])
  })

  it('merges overlapping cuts', () => {
    expect(mergeCuts([cut(10, 30), cut(20, 40)])).toEqual([cut(10, 40)])
  })

  it('merges near-adjacent cuts within the epsilon, keeps the longer end', () => {
    expect(mergeCuts([cut(10, 20), cut(20.005, 25)])).toEqual([cut(10, 25)])
    // a fully-contained cut does not shrink the outer one
    expect(mergeCuts([cut(10, 40), cut(15, 20)])).toEqual([cut(10, 40)])
  })

  it('does not mutate the input', () => {
    const input = [cut(20, 40), cut(10, 30)]
    mergeCuts(input)
    expect(input).toEqual([cut(20, 40), cut(10, 30)])
  })
})

describe('addCutToList', () => {
  it('normalizes reversed start/end', () => {
    expect(addCutToList([], 40, 20, 100)).toEqual([cut(20, 40)])
  })

  it('clamps to [0, duration]', () => {
    expect(addCutToList([], -10, 120, 100)).toEqual([cut(0, 100)])
  })

  it('rejects a cut shorter than the minimum (returns null)', () => {
    expect(addCutToList([cut(10, 20)], 50, 50 + MIN_CUT_LENGTH_SEC / 2, 100)).toBeNull()
  })

  it('adds + merges into an existing overlapping cut', () => {
    expect(addCutToList([cut(10, 30)], 25, 50, 100)).toEqual([cut(10, 50)])
  })

  it('inserts a disjoint cut in sorted order', () => {
    expect(addCutToList([cut(60, 70)], 10, 20, 100)).toEqual([cut(10, 20), cut(60, 70)])
  })

  it('does not mutate the existing cut list', () => {
    const existing = [cut(10, 30)]
    addCutToList(existing, 25, 50, 100)
    expect(existing).toEqual([cut(10, 30)])
  })
})
