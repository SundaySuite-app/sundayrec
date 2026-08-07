import { describe, expect, it } from 'vitest'
import {
  applyParams,
  buildLearningSummaryView,
  isLearningSummaryEmpty,
  type CopyLine,
} from './learning-summary-core'
import type { LearningSummary } from '../../bindings/LearningSummary'
import no from '../../locales/no.json'

/** A summary with nothing recorded — the state a fresh install starts in. */
const EMPTY: LearningSummary = {
  sermonPickCorrections: 0,
  trimAdjustments: 0,
  startTendency: 'unclear',
  startAvgAbsDeltaSec: 0,
  endTendency: 'unclear',
  endAvgAbsDeltaSec: 0,
  companionTotal: 0,
  companionAccepted: 0,
  companionLeftAlone: 0,
  companionRejected: 0,
  companionEditedAfterAccept: 0,
}

describe('isLearningSummaryEmpty', () => {
  it('is empty when all three signals are zero', () => {
    expect(isLearningSummaryEmpty(EMPTY)).toBe(true)
  })

  it('is not empty when only the sermon pick count is nonzero', () => {
    expect(isLearningSummaryEmpty({ ...EMPTY, sermonPickCorrections: 1 })).toBe(false)
  })

  it('is not empty when only trim adjustments are nonzero', () => {
    expect(isLearningSummaryEmpty({ ...EMPTY, trimAdjustments: 2 })).toBe(false)
  })

  it('is not empty when only companion suggestions are nonzero', () => {
    expect(isLearningSummaryEmpty({ ...EMPTY, companionTotal: 1 })).toBe(false)
  })
})

describe('buildLearningSummaryView — the empty state', () => {
  it('flags isEmpty on a fresh-install summary', () => {
    expect(buildLearningSummaryView(EMPTY).isEmpty).toBe(true)
  })
})

describe('buildLearningSummaryView — sermon-pick line', () => {
  it('reads as a plain "not corrected yet" at zero', () => {
    const v = buildLearningSummaryView({ ...EMPTY, trimAdjustments: 1 })
    expect(v.sermonPick.key).toBe('general.learningSermonPickZero')
    expect(v.sermonPick.params).toBeUndefined()
  })

  it('uses the singular fallback at exactly one', () => {
    const v = buildLearningSummaryView({ ...EMPTY, sermonPickCorrections: 1 })
    expect(v.sermonPick.key).toBe('general.learningSermonPickOne')
  })

  it('uses the plural fallback with the count above one', () => {
    const v = buildLearningSummaryView({ ...EMPTY, sermonPickCorrections: 4 })
    expect(v.sermonPick.key).toBe('general.learningSermonPickMany')
    expect(v.sermonPick.params).toEqual({ n: '4' })
  })
})

describe('buildLearningSummaryView — the trim-tendency sentence', () => {
  it('is absent for both boundaries when the tendency is unclear', () => {
    const v = buildLearningSummaryView({ ...EMPTY, trimAdjustments: 1 })
    expect(v.startTendency).toBeNull()
    expect(v.endTendency).toBeNull()
  })

  it('reports the start landing too early, with the average rounded', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      startTendency: 'too_early',
      startAvgAbsDeltaSec: 39.6,
    })
    expect(v.startTendency).toEqual<CopyLine>({
      key: 'general.learningStartTooEarly',
      fallback:
        'Den automatiske starten treffer ofte for tidlig — i snitt har du flyttet den {n} sekunder senere.',
      params: { n: '40' },
    })
  })

  it('reports the start landing too late', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      startTendency: 'too_late',
      startAvgAbsDeltaSec: 12.4,
    })
    expect(v.startTendency?.key).toBe('general.learningStartTooLate')
    expect(v.startTendency?.params).toEqual({ n: '12' })
  })

  it('reports the end landing too early — a DIFFERENT key from the start', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      endTendency: 'too_early',
      endAvgAbsDeltaSec: 8.0,
    })
    expect(v.endTendency?.key).toBe('general.learningEndTooEarly')
    expect(v.startTendency).toBeNull()
  })

  it('reports the end landing too late', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      endTendency: 'too_late',
      endAvgAbsDeltaSec: 5.2,
    })
    expect(v.endTendency?.key).toBe('general.learningEndTooLate')
    expect(v.endTendency?.params).toEqual({ n: '5' })
  })

  it('judges the two boundaries independently', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      startTendency: 'too_early',
      startAvgAbsDeltaSec: 30,
      endTendency: 'too_late',
      endAvgAbsDeltaSec: 20,
    })
    expect(v.startTendency?.key).toBe('general.learningStartTooEarly')
    expect(v.endTendency?.key).toBe('general.learningEndTooLate')
  })
})

describe('buildLearningSummaryView — companion line', () => {
  it('is absent when nothing has been built yet', () => {
    const v = buildLearningSummaryView({ ...EMPTY, sermonPickCorrections: 1 })
    expect(v.companion).toBeNull()
  })

  it('carries the three outcome counts, never the suggested text', () => {
    const v = buildLearningSummaryView({
      ...EMPTY,
      companionTotal: 5,
      companionAccepted: 2,
      companionLeftAlone: 3,
      companionRejected: 0,
      companionEditedAfterAccept: 1,
    })
    expect(v.companion).toEqual<CopyLine>({
      key: 'general.learningCompanionSummary',
      fallback:
        'Av AI-forslagene er {accepted} tatt i bruk ({edited} skrevet om etterpå), og {leftAlone} latt være urørt.',
      params: { accepted: '2', edited: '1', leftAlone: '3' },
    })
  })
})

describe('applyParams', () => {
  it('substitutes every token', () => {
    expect(applyParams('{a} og {b}', { a: '1', b: '2' })).toBe('1 og 2')
  })

  it('returns the template unchanged with no params', () => {
    expect(applyParams('rettet {n} ganger')).toBe('rettet {n} ganger')
  })

  it('substitutes a repeated token everywhere it appears', () => {
    expect(applyParams('{n} og igjen {n}', { n: '3' })).toBe('3 og igjen 3')
  })
})

describe('every fallback matches legacy/locales/no.json, key for key', () => {
  // Same discipline telemetry-consent-copy-core.test.ts applies: the fallback
  // baked into this module is a THIRD copy of the Norwegian sentence (the
  // other two are no.json itself and, for the static card text, index.html),
  // and nothing else watches this one for drift.
  const general = no.general as Record<string, string>

  const allLines: CopyLine[] = [
    { key: 'general.learningSermonPickZero', fallback: 'Det automatiske prekenvalget er ikke rettet ennå.' },
    { key: 'general.learningSermonPickOne', fallback: 'Det automatiske prekenvalget er rettet 1 gang.' },
    buildLearningSummaryView({ ...EMPTY, sermonPickCorrections: 4 }).sermonPick,
    buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      startTendency: 'too_early',
      startAvgAbsDeltaSec: 10,
    }).startTendency as CopyLine,
    buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      startTendency: 'too_late',
      startAvgAbsDeltaSec: 10,
    }).startTendency as CopyLine,
    buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      endTendency: 'too_early',
      endAvgAbsDeltaSec: 10,
    }).endTendency as CopyLine,
    buildLearningSummaryView({
      ...EMPTY,
      trimAdjustments: 2,
      endTendency: 'too_late',
      endAvgAbsDeltaSec: 10,
    }).endTendency as CopyLine,
    buildLearningSummaryView({ ...EMPTY, companionTotal: 1, companionAccepted: 1 })
      .companion as CopyLine,
  ]

  for (const line of allLines) {
    it(line.key, () => {
      const leaf = line.key.replace('general.', '')
      expect(general[leaf]).toBe(line.fallback)
    })
  }
})
