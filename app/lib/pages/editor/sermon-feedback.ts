import type { Suggestion } from "./state";
import { sermonCandidates } from "./sermon-candidates";
import type { EditorSermonPickRequest } from "../../../../legacy/bindings/EditorSermonPickRequest";

// The renderer's half of E8: turn "the human just corrected the sermon pick"
// into the record the backend stores. Pure — no DOM, no shared state — because
// the one thing that can go wrong here is silent: a payload assembled AFTER the
// promotion has already flipped the two `type` fields describes a world where
// the detector picked the block the human picked, and every record written from
// it says the detector was right.
//
// Everything that is a JUDGEMENT (is this a correction at all, does it replace
// the last one, which block does it mean after a re-analysis) lives in
// `sundayrec_core::feedback`. This file only shapes the evidence.

/**
 * The correction payload for `editor_record_sermon_pick`.
 *
 * `segments` must be the list AS THE USER SAW IT — call this BEFORE promoting
 * the chosen block, so `autoIndex` still points at the block carrying the
 * detector's own `sermon` label.
 *
 * `autoIndex` is the DETECTOR's pick, which after a restored correction is not
 * the block currently promoted. Pass `null` when the detector found no sermon;
 * "we found nothing and the human found something" is its own signal.
 */
export function buildSermonPickRequest(
  segments: readonly Suggestion[],
  autoIndex: number | null,
  chosenIndex: number,
  durationSec: number,
): EditorSermonPickRequest {
  return {
    // COPIES, not the live objects. `setSermonSegment` promotes by mutating the
    // very segments this list points at, and the IPC call that carries them
    // leaves after that mutation — so a request holding the originals describes
    // the world as it is a microsecond too late, with the human's block already
    // labelled `sermon`. The one lie this record must never tell.
    segments: segments.map((s) => ({ ...s })),
    // What the picker OFFERED — the alternatives the choice was made among.
    // Derived from the same function the dropdown builds its options from, so
    // the record cannot describe a menu the user never saw.
    candidateIndices: sermonCandidates(segments).map((c) => c.index),
    autoIndex: autoIndex !== null && autoIndex >= 0 ? autoIndex : null,
    chosenIndex,
    durationSec,
  };
}

/** Index of the block the detector itself promoted, or `null` when it found no
 *  sermon. Read off a FRESH detection result, before any stored correction is
 *  applied on top — that is the only moment the detector's own answer is
 *  visible. */
export function autoSermonIndex(
  segments: readonly Suggestion[],
): number | null {
  const i = segments.findIndex((s) => s.type === "sermon");
  return i >= 0 ? i : null;
}
