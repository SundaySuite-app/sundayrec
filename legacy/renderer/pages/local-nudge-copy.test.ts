// What the «Hva appen har justert» card says, and — more importantly — what it
// refuses to say (E10).
//
// The card is the entire justification for letting the app change what happens
// on a Sunday: an adjustment nobody can see is the failure this feature is
// built to avoid, so "the sentence is wrong" and "the sentence is missing" are
// both bugs of the same severity here. Split from
// `learning-summary-core.test.ts` because it tests a different question about
// the same module — that one asks what the app has NOTICED, this one what it
// has DONE.
import { describe, expect, it } from 'vitest'
import {
  applyParams,
  buildLocalNudgeView,
  MAX_BOUNDARY_NUDGE_SEC,
  MIN_CORRECTIONS_FOR_NUDGE,
  type CopyLine,
} from './learning-summary-core'
import type { LocalNudge } from '../../bindings/LocalNudge'
import no from '../../locales/no.json'

const SHIPPED: LocalNudge = {
  sermonStartSec: { value: 0, atLimit: false, samples: 0 },
  sermonEndSec: { value: 0, atLimit: false, samples: 0 },
}

const nudge = (start: Partial<LocalNudge['sermonStartSec']>, end: Partial<LocalNudge['sermonEndSec']> = {}): LocalNudge => ({
  sermonStartSec: { ...SHIPPED.sermonStartSec, ...start },
  sermonEndSec: { ...SHIPPED.sermonEndSec, ...end },
})

describe('the switch is off', () => {
  it('says the app is not adjusting anything, whatever is stored', () => {
    // The command returns the shipped value when adaptivity is off, but a
    // stored value could still arrive from a build that returned it — and the
    // card must describe what will HAPPEN, not what is on disk.
    const view = buildLocalNudgeView(nudge({ value: 40, samples: 20 }), false)
    expect(view.headline.key).toBe('general.localNudgeOff')
    expect(view.start).toBeNull()
    expect(view.end).toBeNull()
    expect(view.canReset).toBe(false)
  })
})

describe('the switch is on but nothing has been earned yet', () => {
  it('says so, and says how far off the bar the operator is', () => {
    const view = buildLocalNudgeView(nudge({ value: 0, samples: 5 }), true)
    expect(view.headline.key).toBe('general.localNudgeWaiting')
    // The bar and the progress toward it are two counts, so two units: one
    // plural group cannot inflect «12 opptak» and «5 opptak» at once, and in
    // Polish those are two different noun forms (nagrań / nagrania).
    expect(view.headline.count).toBe(MIN_CORRECTIONS_FOR_NUDGE)
    expect(view.headline.extra).toEqual([
      {
        key: 'general.localNudgeWaitingSoFar',
        fallback: 'Så langt har den {n} opptak.',
        count: 5,
      },
    ])
    expect(view.canReset).toBe(false)
  })

  it('reads the larger of the two boundaries — the one that will cross first', () => {
    const view = buildLocalNudgeView(nudge({ samples: 3 }, { samples: 9 }), true)
    expect(view.headline.extra?.[0].count).toBe(9)
  })

  it('never shows a reset button when there is nothing to reset', () => {
    expect(buildLocalNudgeView(SHIPPED, true).canReset).toBe(false)
  })
})

describe('an adjustment is in force', () => {
  it('names the direction in the word, not in a minus sign', () => {
    const later = buildLocalNudgeView(nudge({ value: 40, samples: 14 }), true)
    expect(later.start?.key).toBe('general.localNudgeStartLater')
    expect(later.start?.count).toBe(40)

    const earlier = buildLocalNudgeView(nudge({ value: -40, samples: 14 }), true)
    expect(earlier.start?.key).toBe('general.localNudgeStartEarlier')
    // The sentence supplies "tidligere"; the number must not also be negative,
    // or it reads as «40 sekunder tidligere» with a minus sign in front of it.
    expect(earlier.start?.count).toBe(40)
  })

  it('carries the seconds and the corrections as separately inflected units', () => {
    // The whole point of the split: `count` on the sentence is the SECOND
    // count, `extra[0].count` is the CORRECTION count, and no single plural
    // group is asked to be right about both.
    const view = buildLocalNudgeView(nudge({ value: 40, samples: 14 }), true)
    expect(view.start?.count).toBe(40)
    expect(view.start?.extra).toEqual([
      {
        key: 'general.localNudgeEvidence',
        fallback: 'Det bygger på {n} rettelser fra deg.',
        count: 14,
      },
    ])
  })

  it('rounds the seconds before choosing a form, so 1.4 s is never «1 sekunder»', () => {
    // `count` governs the noun; it has to be the number the sentence prints,
    // not the raw float behind it.
    expect(buildLocalNudgeView(nudge({ value: 1.4, samples: 14 }), true).start?.count).toBe(1)
    expect(buildLocalNudgeView(nudge({ value: -1.6, samples: 14 }), true).start?.count).toBe(2)
  })

  it('judges the end boundary independently of the start', () => {
    const view = buildLocalNudgeView(nudge({ value: 40, samples: 14 }, { value: -25, samples: 14 }), true)
    expect(view.start?.key).toBe('general.localNudgeStartLater')
    expect(view.end?.key).toBe('general.localNudgeEndEarlier')
    expect(view.end?.count).toBe(25)
    expect(view.end?.extra?.[0].count).toBe(14)
  })

  it('shows only the boundary that moved', () => {
    const view = buildLocalNudgeView(nudge({ value: 40, samples: 14 }, { value: 0, samples: 14 }), true)
    expect(view.start).not.toBeNull()
    expect(view.end).toBeNull()
  })

  it('offers the reset', () => {
    expect(buildLocalNudgeView(nudge({ value: 40, samples: 14 }), true).canReset).toBe(true)
  })
})

describe('a clamp that has been reached', () => {
  it('gets a line of its own rather than being folded into the number', () => {
    // A detector that wanted four minutes and was allowed one is a reason to
    // look at the recordings; a saturated clamp that reads like a working one
    // is how that goes unnoticed for a year.
    const view = buildLocalNudgeView(nudge({ value: 60, atLimit: true, samples: 14 }), true)
    expect(view.atLimit).not.toBeNull()
    expect(view.atLimit?.key).toBe('general.localNudgeAtLimit')
    expect(view.atLimit?.params).toEqual({ limit: String(MAX_BOUNDARY_NUDGE_SEC) })
  })

  it('stays silent when neither boundary hit its ceiling', () => {
    expect(buildLocalNudgeView(nudge({ value: 59, samples: 14 }), true).atLimit).toBeNull()
  })

  it('speaks up when EITHER boundary hit its ceiling', () => {
    const endOnly = buildLocalNudgeView(
      nudge({ value: 5, samples: 14 }, { value: -60, atLimit: true, samples: 14 }),
      true,
    )
    expect(endOnly.atLimit).not.toBeNull()
  })
})

describe('the clamp and the evidence bar match the Rust constants', () => {
  // These two numbers exist twice — here and in
  // `crates/sundayrec-core/src/local_adaptivity.rs` — because a rule that
  // arrived over IPC could disagree with the one the detector applies. This is
  // the check that keeps the copy honest, and it fails loudly rather than
  // showing the operator a limit the app does not enforce.
  it('60 seconds and 12 corrections', () => {
    expect(MAX_BOUNDARY_NUDGE_SEC).toBe(60)
    expect(MIN_CORRECTIONS_FOR_NUDGE).toBe(12)
  })
})

/** A line and every unit it composes: an `extra` is a catalogue key in its own
 *  right, so it has to be checked like one. */
function units(line: CopyLine): CopyLine[] {
  return [line, ...(line.extra ?? [])]
}

describe('every fallback matches legacy/locales/no.json, key for key', () => {
  const general = no.general as Record<string, unknown>
  const lines: CopyLine[] = [
    buildLocalNudgeView(SHIPPED, false).headline,
    buildLocalNudgeView(SHIPPED, true).headline,
    buildLocalNudgeView(nudge({ value: 40, samples: 14 }), true).headline,
    buildLocalNudgeView(nudge({ value: 40, samples: 14 }), true).start as CopyLine,
    buildLocalNudgeView(nudge({ value: -40, samples: 14 }), true).start as CopyLine,
    buildLocalNudgeView(nudge({}, { value: 40, samples: 14 }), true).end as CopyLine,
    buildLocalNudgeView(nudge({}, { value: -40, samples: 14 }), true).end as CopyLine,
    buildLocalNudgeView(nudge({ value: 60, atLimit: true, samples: 14 }), true).atLimit as CopyLine,
  ].flatMap(units)
  const seen = new Set<string>()
  for (const line of lines) {
    if (seen.has(line.key)) continue
    seen.add(line.key)
    it(line.key, () => {
      const node = general[line.key.replace('general.', '')]
      // A counted unit is a plural GROUP in the catalogue, and its inline
      // fallback is the `other` form — the convention every `tn()` call site
      // in the app already follows.
      expect(line.count === undefined ? node : (node as Record<string, string>).other).toBe(
        line.fallback,
      )
    })
  }

  // The strings the DOM layer holds rather than this module — the toggle hint,
  // its two counted clauses, and the two outcomes of pressing the reset.
  // Nothing else watches them.
  it('general.localNudgeToggleHint', () => {
    expect(general.localNudgeToggleHint).toBe(
      'Når dette er på, flytter appen sitt eget forslag til preken-start og -slutt etter hvordan du pleier å rette det.',
    )
  })
  it('general.localNudgeLimitSeconds', () => {
    expect((general.localNudgeLimitSeconds as Record<string, string>).other).toBe(
      'Aldri mer enn {n} sekunder.',
    )
  })
  it('general.localNudgeLimitRecordings', () => {
    expect((general.localNudgeLimitRecordings as Record<string, string>).other).toBe(
      'Aldri før du har rettet minst {n} opptak.',
    )
  })
  it('general.localNudgeResetDone', () => {
    expect(general.localNudgeResetDone).toBe(
      'Nullstilt. Appen gjetter nå på nøyaktig samme måte som en nyinstallert app, og lærer ikke mer før du slår det på igjen.',
    )
  })
  it('general.localNudgeLoadFailed', () => {
    expect(general.localNudgeLoadFailed).toBe(
      'Kunne ikke hente hva appen har justert. Prøv igjen.',
    )
  })
})

describe('the interpolated sentences read as Norwegian', () => {
  /** What `general-page.ts` paints, minus the catalogue: every unit filled from
   *  its own fallback and its own count, joined by a space. `{n}` comes from
   *  `count` exactly as `tn()` binds it. */
  const render = (line: CopyLine): string =>
    units(line)
      .map(part =>
        applyParams(part.fallback, {
          ...(part.count === undefined ? {} : { n: String(part.count) }),
          ...part.params,
        }),
      )
      .join(' ')

  it('fills every placeholder — a stray {n} on screen is the visible failure', () => {
    const view = buildLocalNudgeView(nudge({ value: 40, samples: 14 }, { value: -60, atLimit: true, samples: 14 }), true)
    for (const line of [view.headline, view.start, view.end, view.atLimit]) {
      if (!line) continue
      expect(render(line)).not.toMatch(/\{[a-z]+\}/i)
    }
    expect(render(view.start!)).toBe(
      'Appen foreslår nå at prekenen starter 40 sekunder senere enn den ellers ville gjort. Det bygger på 14 rettelser fra deg.',
    )
  })

  it('still reads as one paragraph when the app is waiting', () => {
    expect(render(buildLocalNudgeView(nudge({ samples: 5 }), true).headline)).toBe(
      'Appen har ikke justert noe ennå. Den venter til du har rettet minst 12 opptak. Så langt har den 5 opptak.',
    )
  })
})
