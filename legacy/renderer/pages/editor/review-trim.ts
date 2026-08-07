/**
 * The kept sermon span, read back out of the editor's cuts. Pure — no DOM,
 * no IPC.
 *
 * Review mode opens by turning the proposed trim INTO cuts: one covering
 * `[0, startSec]` and one covering `[endSec, duration]` (see
 * `applyReviewModeDefaults`). This is the return leg — what span do the cuts
 * describe now that the operator has moved them?
 *
 * ## Why this is not "collapse the cuts into a trim"
 *
 * `editor-page.ts` carried a standing note that the trim was deliberately left
 * unwired, because cuts and `prep.suggestedTrim` are different models: cuts are
 * a list of removed ranges ANYWHERE in the file, the trim is one kept span, and
 * flattening the former into the latter would silently discard every cut but the
 * outer two.
 *
 * That objection is about which question you ask. "What single span replaces
 * these cuts?" is unanswerable and destructive. "Where does the kept material
 * begin and end?" is neither: it is decided by the leading and trailing cuts
 * alone, and INTERIOR cuts — a muffed sentence removed from the middle of the
 * sermon — do not move either boundary and are left exactly where they are. This
 * module answers only the second question, and the export still renders every
 * cut the operator made.
 *
 * The boundary is also the only part the detector proposed, so it is the only
 * part it can be judged on: `MIN_SERMON_START_SEC` and friends decide where the
 * sermon starts and stops, never what happens inside it.
 */

import type { Cut } from './state'

/** A kept span, in seconds from the start of the recording. */
export interface TrimSpan {
  startSec: number
  endSec: number
}

/**
 * How close to 0 (or to `duration`) a cut has to reach before it counts as the
 * leading (or trailing) one.
 *
 * Not zero, because the boundaries are dragged with a mouse on a waveform:
 * a "cut from the start" that a human made is a cut from 0.03 s. And review
 * mode's own pre-apply skips a cut it would have to make shorter than 0.5 s,
 * so an operator who accepts the proposal untouched leaves nothing at that end
 * at all — which is the `startSec = 0` case below, not a near-miss.
 */
export const EDGE_TOLERANCE_SEC = 0.5

/**
 * The span the operator ended up keeping, or `null` when the cuts do not
 * describe a usable one.
 *
 * `null` — rather than a guess — in three cases:
 *   - a non-finite or non-positive `duration` (no timeline to measure against),
 *   - cuts that between them remove everything,
 *   - a span that came out inverted or empty.
 *
 * A caller that gets `null` should push nothing: a trim nobody can act on is
 * worse in the queue than the proposal it would replace.
 */
export function keptSpanFromCuts(cuts: Cut[], durationSec: number): TrimSpan | null {
  if (!Number.isFinite(durationSec) || durationSec <= 0) return null

  let startSec = 0
  let endSec = durationSec

  for (const c of cuts) {
    if (!Number.isFinite(c.start) || !Number.isFinite(c.end) || c.end <= c.start) continue
    // A cut reaching the head pushes the start to wherever it ends; several
    // overlapping head cuts push it to the furthest of them.
    if (c.start <= EDGE_TOLERANCE_SEC) startSec = Math.max(startSec, c.end)
    // Mirror image at the tail.
    if (c.end >= durationSec - EDGE_TOLERANCE_SEC) endSec = Math.min(endSec, c.start)
  }

  startSec = Math.max(0, Math.min(startSec, durationSec))
  endSec = Math.max(0, Math.min(endSec, durationSec))
  if (!(endSec > startSec)) return null

  return { startSec, endSec }
}
