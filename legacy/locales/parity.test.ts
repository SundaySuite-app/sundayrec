// Locale key parity — every language must carry the exact key set of no.json
// (the primary locale). A missing key doesn't crash at runtime: `t()` silently
// falls back to its hardcoded (Norwegian) second argument, so a non-Norwegian
// user quietly gets Norwegian text. This test makes that regression loud.
import { describe, expect, it } from 'vitest'
import no from './no.json'
import en from './en.json'
import sv from './sv.json'
import da from './da.json'
import de from './de.json'
import fr from './fr.json'
import pl from './pl.json'

type Tree = Record<string, unknown>

function flattenKeys(obj: Tree, prefix = ''): string[] {
  return Object.entries(obj).flatMap(([key, value]) =>
    typeof value === 'object' && value !== null && !Array.isArray(value)
      ? flattenKeys(value as Tree, prefix + key + '.')
      : [prefix + key],
  )
}

const reference = flattenKeys(no as Tree).sort()
const locales: Array<[string, Tree]> = [
  ['en', en as Tree],
  ['sv', sv as Tree],
  ['da', da as Tree],
  ['de', de as Tree],
  ['fr', fr as Tree],
  ['pl', pl as Tree],
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
