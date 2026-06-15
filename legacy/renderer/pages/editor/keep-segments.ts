import type { Cut } from './state'

// The KEPT (non-cut) segments of a recording — the single source of truth for
// both preview playback (skip over cuts) and export (the cut-plan). A bug here =
// the wrong audio survives an export, so it's extracted pure + unit-tested.

/**
 * Tolerance for emitting a kept segment. A keep-gap (or the trailing tail)
 * shorter than this is dropped — avoids sliver segments from near-adjacent cuts
 * or a cut that ends at the file's very end. Mirrors the editor's inline 0.05 s.
 */
export const KEEP_EPSILON_SEC = 0.05

/**
 * Compute the kept segments of a `duration`-second recording given its cuts.
 * Cuts are sorted and collapsed via a running cursor, so overlapping/adjacent
 * cuts merge. Pure — no DOM, no shared state.
 */
export function computeKeepSegs(
  cuts: Cut[],
  duration: number,
): { start: number; end: number }[] {
  const sorted = [...cuts].sort((a, b) => a.start - b.start)
  const keeps: { start: number; end: number }[] = []
  let cursor = 0
  for (const c of sorted) {
    if (c.start > cursor + KEEP_EPSILON_SEC) keeps.push({ start: cursor, end: c.start })
    cursor = Math.max(cursor, c.end)
  }
  if (cursor < duration - KEEP_EPSILON_SEC) keeps.push({ start: cursor, end: duration })
  return keeps
}
