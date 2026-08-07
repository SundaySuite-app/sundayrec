import type { Suggestion } from './state'

// Which analysed blocks the sermon picker may offer, and — the part that
// matters — WHICH SEGMENT each offer refers to.
//
// The picker used to build its <option> list from a filtered, sorted array while
// `setSermonSegment` indexed a differently-filtered, unsorted one. The two lists
// agree only when every speech block happens to be ≥ 60 s and already in time
// order; one short block earlier in the timeline shifts them apart and the user's
// correction lands on the wrong segment. Nothing reports it — the star moves to
// the block they picked, while a different block is promoted and trimmed to.
//
// So the mapping is extracted, pure, and tested: one list, and every offer
// carries the index of the segment it actually means.

/** Shortest block worth offering as the sermon. Below this it is an
 *  announcement or a reading, not the message — offering it is noise. */
export const MIN_SERMON_CANDIDATE_SEC = 60

/** One offerable block, paired with its index in the array it came from. */
export interface SermonCandidate {
  /** Index into the SOURCE array — the stable identity a picked option must
   *  carry, as opposed to its position in this filtered list. */
  index: number
  segment: Suggestion
}

/**
 * The speech-like blocks a recording offers as sermon candidates, in time order,
 * each tagged with its index in `segments`.
 *
 * Pure: no DOM, no shared state, and `segments` is not mutated or reordered.
 */
export function sermonCandidates(
  segments: readonly Suggestion[],
): SermonCandidate[] {
  return segments
    .map((segment, index) => ({ index, segment }))
    .filter(
      ({ segment }) =>
        (segment.type === 'speech' || segment.type === 'sermon') &&
        segment.duration >= MIN_SERMON_CANDIDATE_SEC,
    )
    .sort((a, b) => a.segment.start - b.segment.start)
}
