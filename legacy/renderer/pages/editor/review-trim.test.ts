import { describe, expect, it } from 'vitest'
import { EDGE_TOLERANCE_SEC, keptSpanFromCuts } from './review-trim'
import type { Cut } from './state'

const DURATION = 3600

function cut(start: number, end: number): Cut {
  return { start, end }
}

describe('keptSpanFromCuts', () => {
  it('reads back exactly what review mode applied', () => {
    // The round trip that matters: `applyReviewModeDefaults` turns a proposal
    // of 300..2000 into these two cuts, and an operator who changes nothing
    // must produce the same numbers back — otherwise every untouched episode
    // would report a phantom correction.
    const span = keptSpanFromCuts([cut(0, 300), cut(2000, DURATION)], DURATION)
    expect(span).toEqual({ startSec: 300, endSec: 2000 })
  })

  it('follows a start the operator dragged later', () => {
    expect(keptSpanFromCuts([cut(0, 330), cut(2000, DURATION)], DURATION)).toEqual({
      startSec: 330,
      endSec: 2000,
    })
  })

  it('follows an end the operator dragged later', () => {
    expect(keptSpanFromCuts([cut(0, 300), cut(2100, DURATION)], DURATION)).toEqual({
      startSec: 300,
      endSec: 2100,
    })
  })

  it('leaves interior cuts out of the boundary entirely', () => {
    // The objection the old code raised, answered: a removed passage inside the
    // sermon must not move either boundary, and must not be flattened away.
    const span = keptSpanFromCuts(
      [cut(0, 300), cut(900, 960), cut(1400, 1420), cut(2000, DURATION)],
      DURATION,
    )
    expect(span).toEqual({ startSec: 300, endSec: 2000 })
  })

  it('treats a file with no cuts as keeping everything', () => {
    expect(keptSpanFromCuts([], DURATION)).toEqual({ startSec: 0, endSec: DURATION })
  })

  it('handles a head cut with no tail cut, and vice versa', () => {
    // Review mode skips a pre-applied cut shorter than 0.5 s, so an accepted
    // proposal that starts at 0 genuinely has no leading cut.
    expect(keptSpanFromCuts([cut(2000, DURATION)], DURATION)).toEqual({
      startSec: 0,
      endSec: 2000,
    })
    expect(keptSpanFromCuts([cut(0, 300)], DURATION)).toEqual({
      startSec: 300,
      endSec: DURATION,
    })
  })

  it('counts a cut that starts a hair after zero as the leading one', () => {
    const span = keptSpanFromCuts([cut(EDGE_TOLERANCE_SEC / 2, 300)], DURATION)
    expect(span).toEqual({ startSec: 300, endSec: DURATION })
  })

  it('does not mistake an early interior cut for the leading one', () => {
    // Just past the tolerance: the kept material still begins at 0.
    const span = keptSpanFromCuts([cut(EDGE_TOLERANCE_SEC + 0.1, 300)], DURATION)
    expect(span).toEqual({ startSec: 0, endSec: DURATION })
  })

  it('takes the furthest of several overlapping head or tail cuts', () => {
    const span = keptSpanFromCuts(
      [cut(0, 200), cut(0.2, 300), cut(2000, DURATION), cut(2100, DURATION - 0.1)],
      DURATION,
    )
    expect(span).toEqual({ startSec: 300, endSec: 2000 })
  })

  it('refuses a timeline it cannot measure against', () => {
    expect(keptSpanFromCuts([cut(0, 300)], 0)).toBeNull()
    expect(keptSpanFromCuts([cut(0, 300)], -1)).toBeNull()
    expect(keptSpanFromCuts([cut(0, 300)], NaN)).toBeNull()
  })

  it('refuses cuts that between them remove everything', () => {
    expect(keptSpanFromCuts([cut(0, DURATION)], DURATION)).toBeNull()
    expect(keptSpanFromCuts([cut(0, 2000), cut(1800, DURATION)], DURATION)).toBeNull()
  })

  it('ignores individually malformed cuts instead of poisoning the span', () => {
    const span = keptSpanFromCuts(
      [cut(0, 300), cut(NaN, 500), cut(700, 700), cut(900, 800), cut(2000, DURATION)],
      DURATION,
    )
    expect(span).toEqual({ startSec: 300, endSec: 2000 })
  })
})
