import { describe, it, expect } from 'vitest'
import type { Cut } from './state'
import {
  pushSnapshot,
  undoSnapshot,
  redoSnapshot,
  MAX_CUT_HISTORY,
  type CutHistoryState,
} from './cut-history'

const cut = (start: number, end: number): Cut => ({ start, end })
const empty = (): CutHistoryState => ({ history: [], idx: -1 })

describe('pushSnapshot', () => {
  it('records the first snapshot at idx 0', () => {
    const s = pushSnapshot(empty(), [cut(1, 2)])
    expect(s.idx).toBe(0)
    expect(s.history).toEqual([[cut(1, 2)]])
  })

  it('deep-copies — later mutation of the live array does not alias history', () => {
    const live = [cut(1, 2)]
    const s = pushSnapshot(empty(), live)
    live[0].end = 999
    expect(s.history[0][0].end).toBe(2)
  })

  it('discards redo states ahead of the pointer', () => {
    // build A,B,C then undo twice (idx 0), then push D → C/B are dropped
    let s = pushSnapshot(empty(), [cut(0, 1)]) // idx0 = A
    s = pushSnapshot(s, [cut(0, 2)]) // idx1 = B
    s = pushSnapshot(s, [cut(0, 3)]) // idx2 = C
    s = { ...s, idx: 0 } // simulate undo back to A
    s = pushSnapshot(s, [cut(0, 9)]) // push D
    expect(s.idx).toBe(1)
    expect(s.history).toEqual([[cut(0, 1)], [cut(0, 9)]])
  })

  it('caps the stack at MAX_CUT_HISTORY, dropping the oldest', () => {
    let s = empty()
    for (let i = 0; i < MAX_CUT_HISTORY + 5; i++) s = pushSnapshot(s, [cut(i, i + 1)])
    expect(s.history.length).toBe(MAX_CUT_HISTORY)
    expect(s.idx).toBe(MAX_CUT_HISTORY - 1)
    // oldest kept is snapshot #5 (0..4 dropped), newest is the last push
    expect(s.history[0]).toEqual([cut(5, 6)])
    expect(s.history[MAX_CUT_HISTORY - 1]).toEqual([cut(MAX_CUT_HISTORY + 4, MAX_CUT_HISTORY + 5)])
  })
})

describe('undoSnapshot', () => {
  it('steps back one snapshot', () => {
    let s = pushSnapshot(empty(), [cut(0, 1)])
    s = pushSnapshot(s, [cut(0, 2)])
    const r = undoSnapshot(s, 1)
    expect(r).toEqual({ idx: 0, cuts: [cut(0, 1)] })
  })

  it('at idx 0 with live cuts, undoes to the empty pre-history state', () => {
    const s = pushSnapshot(empty(), [cut(0, 1)]) // idx 0
    expect(undoSnapshot(s, 1)).toEqual({ idx: -1, cuts: [] })
  })

  it('is a no-op at idx 0 when already empty, and at idx -1', () => {
    const s = pushSnapshot(empty(), [cut(0, 1)])
    expect(undoSnapshot(s, 0)).toBeNull()
    expect(undoSnapshot(empty(), 0)).toBeNull()
  })

  it('returns a copy — mutating it does not corrupt history', () => {
    let s = pushSnapshot(empty(), [cut(0, 1)])
    s = pushSnapshot(s, [cut(0, 2)])
    const r = undoSnapshot(s, 1)!
    r.cuts[0].end = 999
    expect(s.history[0][0].end).toBe(1)
  })
})

describe('redoSnapshot', () => {
  it('steps forward one snapshot', () => {
    let s = pushSnapshot(empty(), [cut(0, 1)])
    s = pushSnapshot(s, [cut(0, 2)])
    s = { ...s, idx: 0 } // undone
    expect(redoSnapshot(s)).toEqual({ idx: 1, cuts: [cut(0, 2)] })
  })

  it('is a no-op at the head of the stack', () => {
    let s = pushSnapshot(empty(), [cut(0, 1)])
    s = pushSnapshot(s, [cut(0, 2)])
    expect(redoSnapshot(s)).toBeNull()
  })

  it('undo→redo round-trips to the same state', () => {
    let s = pushSnapshot(empty(), [cut(0, 1)])
    s = pushSnapshot(s, [cut(5, 6)])
    const u = undoSnapshot(s, 1)!
    const r = redoSnapshot({ ...s, idx: u.idx })!
    expect(r).toEqual({ idx: 1, cuts: [cut(5, 6)] })
  })
})
