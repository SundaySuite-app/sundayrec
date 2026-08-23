// Plural rules + interpolation for the hand-built i18n.
//
// Two production defects are pinned here.
//
// 1. INTERPOLATION. Every count string used to be rendered with
//    `t(key, fallback).replace('{n}', String(n))` — `String.replace` with a
//    string pattern substitutes only the FIRST occurrence, so any sentence
//    naming the same placeholder twice shipped a raw `{n}` to the operator.
//
// 2. PLURALS. The catalogue carried ONE form per count string, picked in
//    caller code with `=== 1`. That is enough for Norwegian, English, Swedish,
//    Danish and German (two forms) and nearly enough for French — and simply
//    wrong for Polish, which has four categories: «2 nagrania» (few) is not
//    «5 nagrań» (many), and 22–24 go back to `few`. Polish users read the
//    wrong noun form for 2–4, 22–24, 32–34 … in every count string the app has.
//
// The per-language expectations below are written out in full rather than
// derived, so changing a Polish form to the wrong one turns a test red.
import { describe, expect, it } from 'vitest'
import { interpolate, pluralCategory, selectPluralForm, t, tf, tn } from './i18n'
import no from '../locales/no.json'
import en from '../locales/en.json'
import sv from '../locales/sv.json'
import da from '../locales/da.json'
import de from '../locales/de.json'
import fr from '../locales/fr.json'
import pl from '../locales/pl.json'

type Tree = Record<string, unknown>

const TREES: Record<string, Tree> = {
  no: no as Tree,
  en: en as Tree,
  sv: sv as Tree,
  da: da as Tree,
  de: de as Tree,
  fr: fr as Tree,
  pl: pl as Tree,
}

const lookup = (tree: Tree, key: string): unknown =>
  key.split('.').reduce<unknown>((o, k) => (o as Tree)?.[k], tree)

/** What the operator would actually see: form picked for `count`, then filled. */
function render(lang: string, key: string, count: number, params: Record<string, number | string> = {}): string {
  const form = selectPluralForm(lookup(TREES[lang], key), count, lang)
  expect(form, `${lang}: ${key} has no form for ${count}`).toBeTypeOf('string')
  return interpolate(form!, { n: count, ...params })
}

describe('interpolate', () => {
  it('replaces EVERY occurrence, not just the first', () => {
    // The whole reason `tf` exists. `.replace('{n}', …)` returned "3 av {n}".
    expect(interpolate('{n} av {n}', { n: 3 })).toBe('3 av 3')
  })

  it('substitutes several distinct placeholders', () => {
    expect(interpolate('{h} t {m} min', { h: 1, m: 30 })).toBe('1 t 30 min')
  })

  it('leaves a placeholder with no param VISIBLE', () => {
    // Policy, deliberately chosen and pinned here: an unsupplied placeholder
    // stays as `{n}`. Substituting '' would read as finished copy — «opptak
    // ligger i papirkurven» — and hide the bug from everyone including the
    // person reading a screenshot. A visible `{n}` is ugly on purpose.
    expect(interpolate('{n} opptak ligger i {where}', { n: 4 })).toBe('4 opptak ligger i {where}')
  })

  it('is a no-op without params, and never touches non-placeholder braces', () => {
    expect(interpolate('ingen plassholdere', {})).toBe('ingen plassholdere')
    expect(interpolate('{n} {}', { n: 1 })).toBe('1 {}')
  })
})

describe('pluralCategory', () => {
  it('maps "no" to nb-NO, so the tag is the one helpers.localeTag uses', () => {
    expect(pluralCategory(1, 'no')).toBe('one')
    expect(pluralCategory(2, 'no')).toBe('other')
  })

  const cases: Array<[string, number, string]> = [
    ['pl', 0, 'many'],
    ['pl', 1, 'one'],
    ['pl', 2, 'few'],
    ['pl', 4, 'few'],
    ['pl', 5, 'many'],
    ['pl', 21, 'many'],
    ['pl', 22, 'few'],
    ['pl', 24, 'few'],
    ['pl', 25, 'many'],
    ['fr', 0, 'one'],
    ['fr', 1, 'one'],
    ['fr', 2, 'other'],
    ['de', 0, 'other'],
    ['de', 1, 'one'],
    ['en', 0, 'other'],
  ]
  for (const [lang, n, want] of cases) {
    it(`${lang}: ${n} → ${want}`, () => expect(pluralCategory(n, lang)).toBe(want))
  }
})

describe('selectPluralForm — fallback chain', () => {
  const group = { one: 'ett', other: 'flere' }

  it('picks the exact category', () => {
    expect(selectPluralForm(group, 1, 'no')).toBe('ett')
    expect(selectPluralForm(group, 7, 'no')).toBe('flere')
  })

  it('falls back to `other` for a category the catalogue omits', () => {
    // French `many` needs n ≥ 1e6 and Polish `other` needs a fraction; neither
    // is worth a hand-written string, so `other` covers them.
    expect(selectPluralForm({ one: 'un', other: 'des' }, 1_000_000, 'fr')).toBe('des')
    expect(selectPluralForm({ one: 'a', few: 'b', many: 'c', other: 'd' }, 1.5, 'pl')).toBe('d')
  })

  it('accepts a flat string — a key that was never pluralized still works', () => {
    expect(selectPluralForm('flat', 5, 'pl')).toBe('flat')
  })

  it('returns undefined for anything unusable, so the caller can fall back', () => {
    expect(selectPluralForm(undefined, 1, 'no')).toBeUndefined()
    expect(selectPluralForm({}, 1, 'no')).toBeUndefined()
    expect(selectPluralForm(['a'], 1, 'no')).toBeUndefined()
    expect(selectPluralForm({ one: 'ett' }, 3, 'no')).toBeUndefined()
  })
})

describe('the Polish repair — trash.moved across the category boundaries', () => {
  // Before: ONE string, «{n} nagrań jest w koszu», for every count. Right for
  // 5+, wrong for 1 and wrong for 2–4 and 22–24.
  const cases: Array<[number, string]> = [
    [0, '0 nagrań jest w koszu'],
    [1, '1 nagranie jest w koszu'],
    [2, '2 nagrania są w koszu'],
    [3, '3 nagrania są w koszu'],
    [4, '4 nagrania są w koszu'],
    [5, '5 nagrań jest w koszu'],
    [21, '21 nagrań jest w koszu'],
    [22, '22 nagrania są w koszu'],
    [24, '24 nagrania są w koszu'],
    [25, '25 nagrań jest w koszu'],
  ]
  for (const [n, want] of cases) {
    it(`n=${n}`, () => expect(render('pl', 'trash.moved', n)).toBe(want))
  }
})

describe('the Polish repair — the learning nudges said «{n} sekund» for every count', () => {
  const cases: Array<[number, string]> = [
    [1, 'o 1 sekundę później.'],
    [2, 'o 2 sekundy później.'],
    [4, 'o 4 sekundy później.'],
    [5, 'o 5 sekund później.'],
    [22, 'o 22 sekundy później.'],
  ]
  for (const [n, tail] of cases) {
    it(`n=${n}`, () => {
      expect(render('pl', 'general.learningStartTooEarly', n)).toBe(
        'Automatyczny początek często wypada za wcześnie — średnio przesuwasz go o ' + tail.slice(2),
      )
    })
  }
})

describe('the Polish repair — two counts in one sentence need two forms', () => {
  // The six localNudge sentences named a SECOND count and a CORRECTION count in
  // one string. A plural group inflects for one number, so whichever count the
  // group was keyed on, the other noun took its form: «40 sekund … 2 poprawek»
  // where Polish wants «2 poprawki». The fix splits each sentence into units
  // that are pluralized separately and joined by the caller — this is that
  // composition, done here the way general-page.ts does it.
  const paragraph = (lang: string, key: string, seconds: number, samples: number): string =>
    render(lang, key, seconds) + ' ' + render(lang, 'general.localNudgeEvidence', samples)

  it('inflects the seconds and the corrections independently', () => {
    expect(paragraph('pl', 'general.localNudgeStartLater', 1, 2)).toBe(
      'Aplikacja proponuje teraz, że kazanie zaczyna się o 1 sekundę później, ' +
        'niż zaproponowałaby w przeciwnym razie. Podstawą są 2 poprawki od Ciebie.',
    )
    expect(paragraph('pl', 'general.localNudgeEndEarlier', 22, 25)).toBe(
      'Aplikacja proponuje teraz, że kazanie kończy się o 22 sekundy wcześniej, ' +
        'niż zaproponowałaby w przeciwnym razie. Podstawą jest 25 poprawek od Ciebie.',
    )
  })

  it('gives the corrections all three Polish forms', () => {
    const evidence = (n: number) => render('pl', 'general.localNudgeEvidence', n)
    expect(evidence(1)).toContain('1 poprawka')
    expect(evidence(2)).toContain('2 poprawki')
    expect(evidence(5)).toContain('5 poprawek')
    // 22–24 fall back to `few`, which is the boundary a naive `n === 1` misses.
    expect(evidence(22)).toContain('22 poprawki')
    expect(evidence(25)).toContain('25 poprawek')
  })

  it('gives the waiting bar and the progress toward it separate forms', () => {
    // 12 recordings required, 2 corrected: «12 nagrań» and «2 nagrania» in the
    // same paragraph — the pair the single string could not render.
    expect(render('pl', 'general.localNudgeWaiting', 12)).toContain('12 nagrań')
    expect(render('pl', 'general.localNudgeWaitingSoFar', 2)).toBe('Na razie ma 2 nagrania.')
    expect(render('pl', 'general.localNudgeWaitingSoFar', 1)).toBe('Na razie ma 1 nagranie.')
    expect(render('pl', 'general.localNudgeWaitingSoFar', 7)).toBe('Na razie ma 7 nagrań.')
  })

  it('keeps the toggle hint’s two clauses independent too', () => {
    expect(render('pl', 'general.localNudgeLimitSeconds', 60)).toBe('Nigdy o więcej niż 60 sekund.')
    expect(render('pl', 'general.localNudgeLimitSeconds', 2)).toBe('Nigdy o więcej niż 2 sekundy.')
    expect(render('pl', 'general.localNudgeLimitRecordings', 12)).toBe(
      'I nigdy, zanim poprawisz co najmniej 12 nagrań.',
    )
  })
})

describe('every language gets its own singular', () => {
  const expected: Record<string, [string, string]> = {
    no: ['1 dag siden', '5 dager siden'],
    en: ['1 day ago', '5 days ago'],
    sv: ['1 dag sedan', '5 dagar sedan'],
    da: ['1 dag siden', '5 dage siden'],
    de: ['vor 1 Tag', 'vor 5 Tagen'],
    fr: ['il y a 1 jour', 'il y a 5 jours'],
    pl: ['1 dzień temu', '5 dni temu'],
  }
  for (const [lang, [one, many]] of Object.entries(expected)) {
    it(lang, () => {
      expect(render(lang, 'trash.daysAgo', 1)).toBe(one)
      expect(render(lang, 'trash.daysAgo', 5)).toBe(many)
    })
  }

  it('French treats zero as singular, which is the whole point of not using === 1', () => {
    // `n === 1 ? one : other` gets «0 jours»; CLDR French wants «0 jour».
    expect(render('fr', 'trash.daysAgo', 0)).toBe('il y a 0 jour')
    // …and Norwegian genuinely wants the plural at zero.
    expect(render('no', 'trash.daysAgo', 0)).toBe('0 dager siden')
  })
})

describe('every plural group renders in every language, at every boundary', () => {
  // A sweep rather than a spot check: any group whose forms were left half
  // written (a `{n}` that no longer matches, a missing category) shows up here
  // as an un-substituted placeholder or a thrown lookup.
  function groupKeys(tree: Tree, prefix = ''): string[] {
    return Object.entries(tree).flatMap(([k, v]) => {
      if (v && typeof v === 'object' && !Array.isArray(v)) {
        const keys = Object.keys(v as Tree)
        if (keys.includes('other') && keys.every(x => ['one', 'two', 'few', 'many', 'other', 'zero'].includes(x))) {
          return [prefix + k]
        }
        return groupKeys(v as Tree, prefix + k + '.')
      }
      return []
    })
  }
  const keys = groupKeys(no as Tree)

  it('finds the groups at all', () => expect(keys.length).toBeGreaterThan(20))

  for (const lang of Object.keys(TREES)) {
    it(lang, () => {
      for (const key of keys) {
        for (const n of [0, 1, 2, 5, 22, 101]) {
          const form = selectPluralForm(lookup(TREES[lang], key), n, lang)
          expect(form, `${lang}: ${key} @ ${n}`).toBeTypeOf('string')
          // `n` is the count placeholder in all but four groups (which name
          // theirs `d`, `days`, `when`/`n`, `list`); those keep their own
          // placeholder, so only assert that `{n}` itself is consumed.
          expect(interpolate(form!, { n }), `${lang}: ${key} @ ${n}`).not.toContain('{n}')
        }
      }
    })
  }
})

describe('tf / tn against the live catalogue (default locale: no)', () => {
  it('tf fills a real key', () => {
    expect(tf('trash.confirmPurgeOne', { name: 'Gudstjeneste' })).toBe(
      'Slett «Gudstjeneste» for godt?',
    )
  })

  it('tf falls back to the literal when the key is unknown', () => {
    expect(tf('nope.not.here', { n: 2 }, '{n} ting')).toBe('2 ting')
  })

  it('tn binds {n} to the count without being asked', () => {
    expect(tn('trash.daysLeft', 1)).toBe('1 dag igjen')
    expect(tn('trash.daysLeft', 9)).toBe('9 dager igjen')
  })

  it('tn takes extra params, and lets them override n', () => {
    expect(tn('trash.clearHistoryBody', 30, { d: 30 })).toContain('30 dager')
    // The form follows `count` (singular); an explicit `n` still wins the slot.
    expect(tn('trash.daysLeft', 1, { n: 4 })).toBe('4 dag igjen')
  })

  it('tn falls back to the literal for an unknown key', () => {
    expect(tn('nope.not.here', 3, {}, '{n} ting')).toBe('3 ting')
  })

  it('t refuses to stringify a plural group', () => {
    // Before the guard, `t('trash.moved')` rendered "[object Object]".
    expect(t('trash.moved', 'reserve')).toBe('reserve')
  })
})
