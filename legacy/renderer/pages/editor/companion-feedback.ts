// Passive learning signal for the AI sermon companion (../editor-companion.ts).
//
// editor-companion.ts already computes whether a suggestion landed — that's
// what picks the toast text ("Lagt i metadata" vs "Allerede i metadata", "N
// kapitler lagt til" vs "Alle funne kapitler finnes allerede") — and then
// throws the computation away the moment the toast fades. This module gives
// that outcome somewhere to go: a per-suggestion record of which KIND of
// thing the companion offered and what became of it. Split out (like
// draft-scheduler.ts) so the state machine is unit-testable without a DOM,
// an `E`, or the IPC shim.
//
// PRIVACY, not as an afterthought but as the reason this type looks the way
// it does: a CompanionSuggestionEvent can never carry the suggested text,
// the user's rewrite, the transcript, the sermon, the recording's name, or a
// file path. There is nowhere on the type to put any of it — see the doc
// comment on CompanionSuggestionEvent below.

/**
 * The three things the companion actually puts up for a decision
 * (`SermonCompanion` in `legacy/bindings/SermonCompanion.ts`, consumed by
 * `../editor-companion.ts`):
 *   - `title`       → the suggested episode title (applied by "→ Bruk i metadata")
 *   - `description` → the suggested summary, folded into E.meta.description
 *                      by the SAME button
 *   - `chapters`    → the suggested chapter markers ("→ Legg til N kapitler")
 *
 * `highlights` is deliberately NOT a kind here: the panel only lets you SEEK
 * to a highlight (`data-seek`, pure playback navigation) — there is no
 * button that takes a highlight into the recording's metadata, so there is
 * no accept/reject decision to observe, only a click that plays audio.
 */
export type CompanionSuggestionKind = 'title' | 'description' | 'chapters'

/**
 * `rejected` is part of the vocabulary on purpose — a future companion
 * surface (or a redesigned panel) may grow an explicit dismiss gesture, and
 * the sidecar this feeds should not need a schema change when it does. But
 * as of this module, editor-companion.ts has no dismiss button, only a
 * "use it" button per kind; nothing in this file ever emits `rejected`. See
 * `createCompanionFeedbackTracker`'s doc comment for what actually produces
 * `left_alone` instead.
 */
export type CompanionSuggestionOutcome = 'accepted' | 'rejected' | 'left_alone'

/**
 * One companion suggestion's fate. Categories and outcomes ONLY — no
 * suggestion text, no user rewrite, no transcript, no sermon content, no
 * recording name, no file path. That is a property of the shape (there is
 * simply no field to put any of it in), not a filter applied when writing
 * this out, so a future edit to this file cannot accidentally start leaking
 * content through it.
 */
export interface CompanionSuggestionEvent {
  kind: CompanionSuggestionKind
  outcome: CompanionSuggestionOutcome
  /** Only ever `true` alongside `outcome === 'accepted'`. Separates the more
   *  interesting of the two accepted cases — the user took the suggestion
   *  as a starting point and rewrote it — from one kept exactly as offered. */
  editedAfterAccept: boolean
}

/**
 * WRITE SEAM — stub.
 *
 * `.feedback.json`, the sidecar this is meant to land in, is being built by a
 * parallel Etappe 8 stage, not this one (this phase is TypeScript-only and
 * does not touch Rust or create sidecars). Until that stage exists, events
 * are dropped on the floor on purpose: this module has to be usable, and
 * testable, on its own, without depending on work landing elsewhere at the
 * same time.
 *
 * To connect it: replace the function `../editor-companion.ts` passes as
 * `write` when it calls `createCompanionFeedbackTracker` (search that file
 * for `companionFeedbackWriteStub`) with one that appends `event` to the
 * CURRENT recording's Feedback sidecar — e.g. under a
 * `companionSuggestions: CompanionSuggestionEvent[]` field. Two things the
 * replacement needs to get right:
 *   - which recording: `event` deliberately carries no file path (see
 *     CompanionSuggestionEvent's doc comment), so the replacement must close
 *     over the current file path itself, the way `cuts.ts` binds
 *     `draftSaver`'s `save` to `window.api.editorSaveCutsDraft(fp, …)`.
 *   - never block the panel: mirror the fire-and-forget
 *     `.catch(() => {})` pattern `cuts.ts` uses for its own sidecar write —
 *     a failed feedback write must never surface as a companion error.
 */
export const companionFeedbackWriteStub: (event: CompanionSuggestionEvent) => void = () => {
  // Intentionally empty — see doc comment above.
}

export interface CompanionFeedbackTracker {
  /** Call when a fresh companion result is rendered. Finalizes whatever the
   *  PREVIOUS result left undecided (as `left_alone`) before tracking the
   *  new one — a rebuild always both closes the old batch and opens a new one. */
  shown(): void
  /** Call the instant a suggestion is actually applied: `title`/`description`
   *  written into E.meta, or `chapters` once at least one chapter was merged
   *  in. Idempotent — a kind already decided this batch is left alone. */
  accepted(kind: CompanionSuggestionKind): void
  /** Call when a field the tracker was told is `accepted` gets changed again
   *  afterwards — the caller owns detecting that (e.g. a one-shot `input`
   *  listener on the metadata field), this only records the fact. A no-op
   *  for a kind that was never `accepted` this batch (nothing to correct),
   *  and for `chapters`: there is no single field to watch a rewrite of a
   *  chapter list on (rows are un-attributed — a companion-added chapter and
   *  a hand-added one look identical once merged), so `chapters` can only
   *  ever reach `editedAfterAccept: false`. */
  edited(kind: CompanionSuggestionKind): void
  /** Call when the panel is torn down WITHOUT a new result replacing this one
   *  (file switch, explicit clear). Finalizes anything still undecided as
   *  `left_alone` — the same finalize `shown()` runs for the next batch, kept
   *  as its own method because "leaving" and "replacing" are different call
   *  sites in editor-companion.ts and neither should have to fake the other. */
  reset(): void
}

const KINDS: readonly CompanionSuggestionKind[] = ['title', 'description', 'chapters']

type Status = 'pending' | 'accepted' | 'decided'

/**
 * One companion result's outcome, tracked kind-by-kind and written out
 * exactly once per kind per batch — never twice, so the write seam is a
 * plain append-only log rather than something that has to reconcile
 * corrections. `edited()` writes immediately (an edit is final: text cannot
 * become un-edited), everything else is decided and written at `finalize`
 * (the batch boundary — `shown()` for the next result, `reset()` for none).
 */
export function createCompanionFeedbackTracker(opts: {
  write: (event: CompanionSuggestionEvent) => void
}): CompanionFeedbackTracker {
  // null = no batch currently open. Distinguishing this from "a batch with
  // nothing accepted" matters: without it, reset() followed by a later
  // shown() (companion torn down, then genuinely rebuilt) would re-finalize
  // an empty placeholder batch and emit three bogus `left_alone` events for
  // suggestions that were never shown.
  let batch: Map<CompanionSuggestionKind, Status> | null = null

  function finalize(): void {
    if (!batch) return
    for (const [kind, status] of batch) {
      if (status === 'pending') {
        opts.write({ kind, outcome: 'left_alone', editedAfterAccept: false })
      } else if (status === 'accepted') {
        // Accepted, and untouched again before this batch closed.
        opts.write({ kind, outcome: 'accepted', editedAfterAccept: false })
      }
      // 'decided' kinds already wrote their event in edited() below.
    }
    batch = null
  }

  return {
    shown() {
      finalize()
      batch = new Map(KINDS.map(k => [k, 'pending']))
    },
    accepted(kind) {
      if (batch?.get(kind) === 'pending') batch.set(kind, 'accepted')
    },
    edited(kind) {
      if (batch?.get(kind) !== 'accepted') return
      batch.set(kind, 'decided')
      opts.write({ kind, outcome: 'accepted', editedAfterAccept: true })
    },
    reset() {
      finalize()
    },
  }
}
