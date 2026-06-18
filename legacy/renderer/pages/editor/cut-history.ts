import type { Cut } from './state'

// Pure undo/redo state machine for the editor's cut list — extracted from
// `cuts.ts` so the invariants are UNIT-TESTABLE without a DOM (the renderer side
// just stores the result + repaints). `import type` keeps this free of any
// `document`-touching code so the tests run in a plain node environment.
//
// Model: `history` is a stack of cut-list snapshots; `idx` points at the live
// one. `idx === -1` means "no history yet" (the initial empty state).

/** Cap on undo depth — older snapshots are dropped. */
export const MAX_CUT_HISTORY = 50

/** A history stack + the pointer into it. */
export interface CutHistoryState {
  history: Cut[][]
  idx: number
}

/** Deep-copy a cut list so snapshots can't alias the live array. */
function clone(cuts: Cut[]): Cut[] {
  return cuts.map((c) => ({ start: c.start, end: c.end }))
}

/**
 * Record `cuts` as a new snapshot after a mutation. Discards any redo states
 * ahead of `idx`, appends a copy, and caps the stack at [`MAX_CUT_HISTORY`]
 * (dropping the oldest). Returns the new `{ history, idx }`.
 */
export function pushSnapshot(state: CutHistoryState, cuts: Cut[]): CutHistoryState {
  let history = state.history.slice(0, state.idx + 1)
  history.push(clone(cuts))
  if (history.length > MAX_CUT_HISTORY) {
    history = history.slice(history.length - MAX_CUT_HISTORY)
  }
  return { history, idx: history.length - 1 }
}

/**
 * Undo: returns the new `idx` and the cut list to restore, or `null` when there
 * is nothing to undo. At `idx === 0` with live cuts, undoes to the empty
 * pre-history state (`idx -1`, `cuts []`). `liveCutCount` is the current live
 * cut count (so an already-empty state at idx 0 is a no-op).
 */
export function undoSnapshot(
  state: CutHistoryState,
  liveCutCount: number,
): { idx: number; cuts: Cut[] } | null {
  if (state.idx <= 0) {
    if (state.idx === 0 && liveCutCount > 0) return { idx: -1, cuts: [] }
    return null
  }
  const idx = state.idx - 1
  return { idx, cuts: clone(state.history[idx]) }
}

/**
 * Redo: returns the new `idx` and cut list, or `null` when nothing is ahead of
 * the current pointer.
 */
export function redoSnapshot(state: CutHistoryState): { idx: number; cuts: Cut[] } | null {
  if (state.idx >= state.history.length - 1) return null
  const idx = state.idx + 1
  return { idx, cuts: clone(state.history[idx]) }
}
