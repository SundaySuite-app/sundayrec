import type { Cut } from './state'

// Pure cut-list mutations — adding a cut + the overlap/adjacency merge. Extracted
// from cuts.ts so the merge invariants are unit-testable (a wrong merge = the
// user's cuts silently combine or split incorrectly). No DOM, no shared state.

/** A cut shorter than this (after clamping) is ignored — a stray click / tiny drag. */
export const MIN_CUT_LENGTH_SEC = 0.1
/** Cuts whose gap is within this are merged — near-adjacent cuts become one. */
export const MERGE_EPSILON_SEC = 0.01

const clampToFile = (sec: number, duration: number): number =>
  Math.max(0, Math.min(duration, sec))

/**
 * Sort by start and collapse overlapping / near-adjacent (within
 * [`MERGE_EPSILON_SEC`]) cuts into single regions. Returns a fresh,
 * non-aliasing list; does not mutate the input.
 */
export function mergeCuts(cuts: Cut[]): Cut[] {
  const sorted = [...cuts].sort((a, b) => a.start - b.start)
  const merged: Cut[] = []
  for (const c of sorted) {
    const prev = merged[merged.length - 1]
    if (prev && c.start <= prev.end + MERGE_EPSILON_SEC) {
      prev.end = Math.max(prev.end, c.end)
    } else {
      merged.push({ start: c.start, end: c.end })
    }
  }
  return merged
}

/**
 * Add a new cut (`s..e`, any order, clamped to `[0, duration]`) to `cuts` and
 * return the merged result — or `null` when the cut is too short to keep
 * (< [`MIN_CUT_LENGTH_SEC`]). Pure; does not mutate the input.
 */
export function addCutToList(
  cuts: Cut[],
  s: number,
  e: number,
  duration: number,
): Cut[] | null {
  if (e < s) [s, e] = [e, s]
  s = clampToFile(s, duration)
  e = clampToFile(e, duration)
  if (e - s < MIN_CUT_LENGTH_SEC) return null
  return mergeCuts([...cuts, { start: s, end: e }])
}
