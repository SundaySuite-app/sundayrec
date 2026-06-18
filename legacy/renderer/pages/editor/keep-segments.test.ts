import { describe, it, expect } from 'vitest'
import type { Cut } from './state'
import { computeKeepSegs, KEEP_EPSILON_SEC } from './keep-segments'

const cut = (start: number, end: number): Cut => ({ start, end })
const seg = (start: number, end: number) => ({ start, end })

describe('computeKeepSegs', () => {
  it('keeps the whole file when there are no cuts', () => {
    expect(computeKeepSegs([], 100)).toEqual([seg(0, 100)])
  })

  it('splits around a single middle cut', () => {
    expect(computeKeepSegs([cut(30, 40)], 100)).toEqual([seg(0, 30), seg(40, 100)])
  })

  it('drops the leading gap when a cut starts at 0', () => {
    expect(computeKeepSegs([cut(0, 20)], 100)).toEqual([seg(20, 100)])
  })

  it('drops the trailing tail when a cut runs to the end', () => {
    expect(computeKeepSegs([cut(80, 100)], 100)).toEqual([seg(0, 80)])
  })

  it('returns nothing when the whole file is cut', () => {
    expect(computeKeepSegs([cut(0, 100)], 100)).toEqual([])
  })

  it('merges overlapping cuts via the running cursor', () => {
    // 30-50 and 40-70 overlap → one kept gap before 30 and after 70
    expect(computeKeepSegs([cut(30, 50), cut(40, 70)], 100)).toEqual([seg(0, 30), seg(70, 100)])
  })

  it('sorts unsorted input', () => {
    expect(computeKeepSegs([cut(60, 70), cut(10, 20)], 100)).toEqual([
      seg(0, 10),
      seg(20, 60),
      seg(70, 100),
    ])
  })

  it('does not emit a sliver keep narrower than the epsilon', () => {
    // gap between cuts is exactly the epsilon → not kept (strict >)
    const cuts = [cut(10, 20), cut(20 + KEEP_EPSILON_SEC, 40)]
    expect(computeKeepSegs(cuts, 100)).toEqual([seg(0, 10), seg(40, 100)])
  })

  it('keeps a gap just wider than the epsilon', () => {
    const cuts = [cut(10, 20), cut(20 + KEEP_EPSILON_SEC + 0.01, 40)]
    const keeps = computeKeepSegs(cuts, 100)
    expect(keeps).toContainEqual(seg(20, 20 + KEEP_EPSILON_SEC + 0.01))
  })

  it('does not mutate the input cuts array', () => {
    const cuts = [cut(60, 70), cut(10, 20)]
    computeKeepSegs(cuts, 100)
    expect(cuts).toEqual([cut(60, 70), cut(10, 20)]) // original order preserved
  })
})
