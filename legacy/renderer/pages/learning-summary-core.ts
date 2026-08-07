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

export interface CopyLine {
  key: string
  fallback: string
  params?: Record<string, string>
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
  if (boundary === 'start') {
    return tooEarly
      ? {
          key: 'general.learningStartTooEarly',
          fallback:
            'Den automatiske starten treffer ofte for tidlig — i snitt har du flyttet den {n} sekunder senere.',
          params: { n },
        }
      : {
          key: 'general.learningStartTooLate',
          fallback:
            'Den automatiske starten treffer ofte for sent — i snitt har du flyttet den {n} sekunder tidligere.',
          params: { n },
        }
  }
  return tooEarly
    ? {
        key: 'general.learningEndTooEarly',
        fallback:
          'Den automatiske slutten kuttes ofte for tidlig — i snitt har du flyttet den {n} sekunder senere.',
        params: { n },
      }
    : {
        key: 'general.learningEndTooLate',
        fallback:
          'Den automatiske slutten treffer ofte for sent — i snitt har du flyttet den {n} sekunder tidligere.',
        params: { n },
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
      : s.sermonPickCorrections === 1
        ? {
            key: 'general.learningSermonPickOne',
            fallback: 'Det automatiske prekenvalget er rettet 1 gang.',
          }
        : {
            key: 'general.learningSermonPickMany',
            fallback: 'Det automatiske prekenvalget er rettet {n} ganger.',
            params: { n: String(s.sermonPickCorrections) },
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

/** Substitute `{token}` placeholders. Split out from the DOM layer because it
 *  is pure, and from `buildLearningSummaryView` because more than one caller
 *  (any future card with a translated, parameterised line) can reuse it
 *  instead of hand-rolling another `.replace()` chain. */
export function applyParams(template: string, params?: Record<string, string>): string {
  if (!params) return template
  return Object.entries(params).reduce((s, [key, value]) => s.split(`{${key}}`).join(value), template)
}
