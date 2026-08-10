/**
 * Turning a `LearningSummary` (raw counts + a trim-direction code from
 * `crates/sundayrec-core/src/learning_summary.rs`) into the sentences the
 * System-tab transparency card shows — the pure half, split out so it is
 * testable in the node-only vitest gate (see vitest.config.ts: no jsdom, by
 * decision) and so the DOM layer (general-page.ts) is the only place that
 * touches an element.
 *
 * Every returned line is a `{ key, fallback, params }` triple, never a
 * finished string: `key`/`fallback` go through `t()` in the DOM layer, the
 * same discipline `telemetry-consent-copy-core.ts` established, so a
 * mistranslation is a locale-file problem and never a rebuild.
 */

import type { LearningSummary } from '../../bindings/LearningSummary'
import type { LocalNudge } from '../../bindings/LocalNudge'
import type { Nudge } from '../../bindings/Nudge'

export interface CopyLine {
  key: string
  fallback: string
  params?: Record<string, string>
  /** Present when the line's grammar is governed by a count: the DOM layer
   *  then resolves `key` through `tn()` (CLDR plural form) instead of `t()`.
   *  Without it «rettet 2 ganger» / «{n} sekunder» were single strings, which
   *  is wrong in Polish for 2–4 and in every language for 1. */
  count?: number
  /**
   * Further sentences, appended after this one with a single space, each
   * carrying its OWN `count` and therefore its own plural group.
   *
   * A plural group inflects for exactly one number, so a sentence naming TWO
   * counts — «{n} sekunder … {samples} rettelser» — cannot be one group: Polish
   * wants «1 sekundę / 2 sekundy / 5 sekund» AND «1 poprawka / 2 poprawki /
   * 5 poprawek», independently, and picking one form for both is wrong for
   * every pair that straddles a category boundary. So each count gets its own
   * unit and the caller composes them, rather than the catalogue trying to
   * express a product of two grammars in one string.
   */
  extra?: CopyLine[]
}

export interface LearningSummaryView {
  /** Nothing has been corrected on any recording still on disk — the common
   *  case on a fresh install, and the case the empty state exists for. */
  isEmpty: boolean
  /** How many times the automatic sermon pick was corrected. Always present
   *  when `!isEmpty`, even when this ONE count is zero and the other two
   *  signals are not — see the zero-case fallback text. */
  sermonPick: CopyLine
  /** Whether the proposed sermon START tends to land too early or too late —
   *  `null` when there is not yet a clear pattern
   *  ([`TrimTendency.Unclear`]). */
  startTendency: CopyLine | null
  /** Same question, for the proposed sermon END. */
  endTendency: CopyLine | null
  /** What became of the AI companion's suggestions — `null` when none have
   *  been built yet. */
  companion: CopyLine | null
}

/** Whether every one of the three E8 signals is empty. Named as its own
 *  function (not just inlined into `buildLearningSummaryView`) so the DOM
 *  layer can ask the same question before deciding whether to render the
 *  empty-state card at all — one rule, not two copies of it. */
export function isLearningSummaryEmpty(s: LearningSummary): boolean {
  return s.sermonPickCorrections === 0 && s.trimAdjustments === 0 && s.companionTotal === 0
}

/** Seconds, rounded to the nearest whole second — this is a "roughly how
 *  much" figure for a human, not a value anything downstream recomputes
 *  from, so sub-second precision would only make the sentence harder to
 *  read for no benefit. */
function fmtSeconds(sec: number): string {
  return String(Math.round(sec))
}

function tendencyLine(
  boundary: 'start' | 'end',
  tendency: LearningSummary['startTendency'],
  avgAbsDeltaSec: number,
): CopyLine | null {
  if (tendency === 'unclear') return null
  const n = fmtSeconds(avgAbsDeltaSec)
  const tooEarly = tendency === 'too_early'
  const count = Math.round(avgAbsDeltaSec)
  if (boundary === 'start') {
    return tooEarly
      ? {
          key: 'general.learningStartTooEarly',
          fallback:
            'Den automatiske starten treffer ofte for tidlig — i snitt har du flyttet den {n} sekunder senere.',
          params: { n },
          count,
        }
      : {
          key: 'general.learningStartTooLate',
          fallback:
            'Den automatiske starten treffer ofte for sent — i snitt har du flyttet den {n} sekunder tidligere.',
          params: { n },
          count,
        }
  }
  return tooEarly
    ? {
        key: 'general.learningEndTooEarly',
        fallback:
          'Den automatiske slutten kuttes ofte for tidlig — i snitt har du flyttet den {n} sekunder senere.',
        params: { n },
        count,
      }
    : {
        key: 'general.learningEndTooLate',
        fallback:
          'Den automatiske slutten treffer ofte for sent — i snitt har du flyttet den {n} sekunder tidligere.',
        params: { n },
        count,
      }
}

/** Build the view from a raw summary. Returns `isEmpty: true` with the other
 *  fields left populated but unused — the DOM layer branches on `isEmpty`
 *  FIRST and renders only the empty-state text in that case, exactly as a
 *  fresh install should look: not broken, not accusatory, just quiet. */
export function buildLearningSummaryView(s: LearningSummary): LearningSummaryView {
  const sermonPick: CopyLine =
    s.sermonPickCorrections === 0
      ? {
          key: 'general.learningSermonPickZero',
          fallback: 'Det automatiske prekenvalget er ikke rettet ennå.',
        }
      : {
          key: 'general.learningSermonPick',
          fallback: 'Det automatiske prekenvalget er rettet {n} ganger.',
          params: { n: String(s.sermonPickCorrections) },
          count: s.sermonPickCorrections,
        }

  const companion: CopyLine | null =
    s.companionTotal === 0
      ? null
      : {
          key: 'general.learningCompanionSummary',
          fallback:
            'Av AI-forslagene er {accepted} tatt i bruk ({edited} skrevet om etterpå), og {leftAlone} latt være urørt.',
          params: {
            accepted: String(s.companionAccepted),
            edited: String(s.companionEditedAfterAccept),
            leftAlone: String(s.companionLeftAlone),
          },
        }

  return {
    isEmpty: isLearningSummaryEmpty(s),
    sermonPick,
    startTendency: tendencyLine('start', s.startTendency, s.startAvgAbsDeltaSec),
    endTendency: tendencyLine('end', s.endTendency, s.endAvgAbsDeltaSec),
    companion,
  }
}

// ── What the app has adjusted about ITSELF (E10) ─────────────────────────────

/**
 * The clamp and the evidence bar, restated for the copy.
 *
 * A second copy of two numbers that already exist in
 * `crates/sundayrec-core/src/local_adaptivity.rs` — and they are here rather
 * than on the wire because they are not observations, they are the RULE, and a
 * rule that arrived over IPC could disagree with the one the detector actually
 * applies without anything noticing. `local-nudge-copy.test.ts` asserts they
 * match the Rust constants, which is the check that keeps them honest.
 */
export const MAX_BOUNDARY_NUDGE_SEC = 60
export const MIN_CORRECTIONS_FOR_NUDGE = 12

export interface LocalNudgeView {
  /** The one line that is always shown — what the app is or is not doing. */
  headline: CopyLine
  /** How the sermon START has been moved, if it has been. */
  start: CopyLine | null
  /** How the sermon END has been moved, if it has been. */
  end: CopyLine | null
  /** Present only when a clamp decided one of the two numbers above.
   *  Deliberately its own line rather than a suffix on the boundary line: a
   *  detector that wanted four minutes and was allowed one is a reason to look
   *  at the recordings, and burying that inside a sentence about 60 seconds is
   *  how a saturated clamp reads as a working one. */
  atLimit: CopyLine | null
  /** Whether there is anything for the reset button to undo. */
  canReset: boolean
}

/** «Det bygger på {n} rettelser fra deg.» — the second sentence of every
 *  boundary line, and a unit of its own because it is inflected by the
 *  CORRECTION count while the sentence before it is inflected by the SECOND
 *  count. See `CopyLine.extra`. */
function evidenceLine(samples: number): CopyLine {
  return {
    key: 'general.localNudgeEvidence',
    fallback: 'Det bygger på {n} rettelser fra deg.',
    count: samples,
  }
}

/** One boundary's sentence, or `null` when the shipped constant still stands.
 *  Two keys per boundary rather than one with a signed number, for the same
 *  reason `tendencyLine` above needs four: Norwegian puts the direction in the
 *  word, not in a minus sign.
 *
 *  `count` is the ROUNDED second count, not the raw one: it has to be the
 *  number the sentence actually shows, or 1.4 s would render «1 sekunder». */
function nudgeLine(boundary: 'start' | 'end', nudge: Nudge): CopyLine | null {
  if (nudge.value === 0) return null
  const seconds = Math.round(Math.abs(nudge.value))
  const extra = [evidenceLine(nudge.samples)]
  const later = nudge.value > 0
  if (boundary === 'start') {
    return later
      ? {
          key: 'general.localNudgeStartLater',
          fallback:
            'Appen foreslår nå at prekenen starter {n} sekunder senere enn den ellers ville gjort.',
          count: seconds,
          extra,
        }
      : {
          key: 'general.localNudgeStartEarlier',
          fallback:
            'Appen foreslår nå at prekenen starter {n} sekunder tidligere enn den ellers ville gjort.',
          count: seconds,
          extra,
        }
  }
  return later
    ? {
        key: 'general.localNudgeEndLater',
        fallback:
          'Appen foreslår nå at prekenen slutter {n} sekunder senere enn den ellers ville gjort.',
        count: seconds,
        extra,
      }
    : {
        key: 'general.localNudgeEndEarlier',
        fallback:
          'Appen foreslår nå at prekenen slutter {n} sekunder tidligere enn den ellers ville gjort.',
        count: seconds,
        extra,
      }
}

/**
 * Build the «hva appen har justert» card from the nudge and the toggle.
 *
 * `enabled` is read from the checkbox rather than from `nudge`, because the
 * command returns the shipped value whenever adaptivity is off and the two
 * states have to read differently: "the app is not adjusting anything" and "the
 * app is allowed to adjust and has not found reason to yet" are different
 * answers to the question the operator is asking, and collapsing them into one
 * sentence is how a switch appears to do nothing.
 */
export function buildLocalNudgeView(nudge: LocalNudge, enabled: boolean): LocalNudgeView {
  if (!enabled) {
    return {
      headline: {
        key: 'general.localNudgeOff',
        fallback:
          'Appen justerer ikke sine egne forslag. Den gjetter på nøyaktig samme måte som en nyinstallert app.',
      },
      start: null,
      end: null,
      atLimit: null,
      canReset: false,
    }
  }

  const start = nudgeLine('start', nudge.sermonStartSec)
  const end = nudgeLine('end', nudge.sermonEndSec)
  if (!start && !end) {
    // The honest waiting state: below the evidence bar the nudge is exactly
    // zero, not a fraction, so there is nothing to show except how far off the
    // bar the operator is. `samples` is per boundary and they can differ; the
    // larger is the one that will cross first.
    const samples = Math.max(nudge.sermonStartSec.samples, nudge.sermonEndSec.samples)
    return {
      headline: {
        key: 'general.localNudgeWaiting',
        fallback:
          'Appen har ikke justert noe ennå. Den venter til du har rettet minst {n} opptak.',
        count: MIN_CORRECTIONS_FOR_NUDGE,
        // The bar and the progress toward it are two counts, so two units —
        // the same split as the boundary lines. See `CopyLine.extra`.
        extra: [
          {
            key: 'general.localNudgeWaitingSoFar',
            fallback: 'Så langt har den {n} opptak.',
            count: samples,
          },
        ],
      },
      start: null,
      end: null,
      atLimit: null,
      canReset: false,
    }
  }

  const atLimit = nudge.sermonStartSec.atLimit || nudge.sermonEndSec.atLimit
  return {
    headline: {
      key: 'general.localNudgeOn',
      fallback: 'Appen har justert sine egne forslag etter rettelsene dine:',
    },
    start,
    end,
    atLimit: atLimit
      ? {
          key: 'general.localNudgeAtLimit',
          fallback:
            'Rettelsene dine peker enda lenger, men appen flytter aldri et forslag mer enn {limit} sekunder. Treffer forslaget fortsatt dårlig, er det opptakene som bør ses på — ikke denne innstillingen.',
          params: { limit: String(MAX_BOUNDARY_NUDGE_SEC) },
        }
      : null,
    canReset: true,
  }
}

/** Substitute `{token}` placeholders. Split out from the DOM layer because it
 *  is pure, and from `buildLearningSummaryView` because more than one caller
 *  (any future card with a translated, parameterised line) can reuse it
 *  instead of hand-rolling another `.replace()` chain. */
export function applyParams(template: string, params?: Record<string, string>): string {
  if (!params) return template
  return Object.entries(params).reduce((s, [key, value]) => s.split(`{${key}}`).join(value), template)
}
