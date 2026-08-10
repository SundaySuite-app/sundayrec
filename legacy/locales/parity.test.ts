// Locale key parity — every language must carry the exact key set of no.json
// (the primary locale). A missing key doesn't crash at runtime: `t()` silently
// falls back to its hardcoded (Norwegian) second argument, so a non-Norwegian
// user quietly gets Norwegian text. This test makes that regression loud.
//
// ## The plural-group amendment (2026-08)
//
// Count-dependent keys are no longer flat strings but CLDR plural GROUPS:
//
//     "trash.moved": { "one": "…", "other": "…" }            // no/en/sv/da/de/fr
//     "trash.moved": { "one": …, "few": …, "many": …, "other": … }   // pl
//
// A naive flatten would call `trash.moved.few` an "extra key in pl.json" and
// fail — but Polish genuinely needs a form the others do not (2–4 and 22–24
// take their own noun form). So the parity CONTRACT changed in one specific
// way: a plural group counts as ONE logical key, and the per-language category
// set is checked separately, against `Intl.PluralRules` rather than a guess.
import { describe, expect, it } from 'vitest'
import no from './no.json'
import en from './en.json'
import sv from './sv.json'
import da from './da.json'
import de from './de.json'
import fr from './fr.json'
import pl from './pl.json'

type Tree = Record<string, unknown>

const CLDR_CATEGORIES = new Set(['zero', 'one', 'two', 'few', 'many', 'other'])

/** A plural group: an object keyed only by CLDR categories, carrying `other`
 *  (which is `tn()`'s universal fallback and therefore mandatory). No ordinary
 *  nested section can look like this — none of them has an `other` leaf. */
export function isPluralGroup(value: unknown): value is Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const keys = Object.keys(value as Tree)
  return (
    keys.length > 0 &&
    keys.every(k => CLDR_CATEGORIES.has(k)) &&
    typeof (value as Tree).other === 'string'
  )
}

/** Logical keys: a plural group is one key, not one key per form. */
function flattenKeys(obj: Tree, prefix = ''): string[] {
  return Object.entries(obj).flatMap(([key, value]) =>
    typeof value === 'object' && value !== null && !Array.isArray(value) && !isPluralGroup(value)
      ? flattenKeys(value as Tree, prefix + key + '.')
      : [prefix + key],
  )
}

function pluralGroupKeys(obj: Tree, prefix = ''): string[] {
  return Object.entries(obj).flatMap(([key, value]) => {
    if (isPluralGroup(value)) return [prefix + key]
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return pluralGroupKeys(value as Tree, prefix + key + '.')
    }
    return []
  })
}

const lookup = (tree: Tree, key: string): unknown =>
  key.split('.').reduce<unknown>((o, k) => (o as Tree)?.[k], tree)

/**
 * The categories a language must actually supply.
 *
 * Not "every category CLDR defines for the language" — French declares `many`
 * and Polish declares `other`, but French `many` needs n ≥ 1 000 000 and Polish
 * `other` needs a fraction, and no count this app renders is either. So: every
 * category reachable from an integer count, plus `other` as the fallback.
 * Computed from Intl, never hand-listed, so a CLDR data update moves the gate
 * rather than silently disagreeing with it.
 */
export function requiredCategories(locale: string): Set<string> {
  const rules = new Intl.PluralRules(locale)
  const cats = new Set<string>(['other'])
  for (let n = 0; n <= 1000; n++) cats.add(rules.select(n))
  return cats
}

const reference = flattenKeys(no as Tree).sort()
const referenceGroups = pluralGroupKeys(no as Tree).sort()

const locales: Array<[string, Tree, string]> = [
  ['en', en as Tree, 'en'],
  ['sv', sv as Tree, 'sv'],
  ['da', da as Tree, 'da'],
  ['de', de as Tree, 'de'],
  ['fr', fr as Tree, 'fr'],
  ['pl', pl as Tree, 'pl'],
]

describe('locale key parity with no.json', () => {
  for (const [lang, tree] of locales) {
    it(`${lang}.json has exactly the no.json key set`, () => {
      const keys = new Set(flattenKeys(tree))
      const missing = reference.filter(k => !keys.has(k))
      const extra = [...keys].filter(k => !reference.includes(k)).sort()
      expect(missing, `keys missing from ${lang}.json`).toEqual([])
      expect(extra, `keys in ${lang}.json that no.json lacks`).toEqual([])
    })
  }
})

describe('plural groups carry exactly the forms their language needs', () => {
  it('no.json declares at least one plural group', () => {
    // A guard on the guard: if the detection ever stops recognising groups,
    // every assertion below would pass vacuously.
    expect(referenceGroups.length).toBeGreaterThan(0)
  })

  for (const [lang, tree, tag] of [['no', no as Tree, 'nb-NO'], ...locales] as Array<
    [string, Tree, string]
  >) {
    it(`${lang}.json`, () => {
      const want = [...requiredCategories(tag)].sort()
      for (const key of referenceGroups) {
        const node = lookup(tree, key)
        expect(isPluralGroup(node), `${lang}.json: ${key} must be a plural group`).toBe(true)
        expect(
          Object.keys(node as Tree).sort(),
          `${lang}.json: ${key} must carry exactly [${want}]`,
        ).toEqual(want)
      }
    })
  }
})
