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

/**
 * ## Pauset paritet — «Frivilligen først» (S1a, 2026-08)
 *
 * Redesignet river og bygger UI-teksten i `app/` om igjen, skjerm for skjerm,
 * gjennom seks faser. Å oversette hver nye nøkkel til sju språk mens teksten
 * fortsatt flytter seg er å oversette det samme fire ganger — og en oversetter
 * som får det samme til gjennomsyn fire ganger slutter å lese nøye.
 *
 * Så: `app/` er norsk + engelsk (`ACTIVE_LOCALES` i `app/i18n/index.ts`) fram
 * til fase B, og de fem andre språkene er PAUSET for de nøklene redesignet
 * legger til — og BARE for dem.
 *
 * Det er hele poenget med `PAUSED_KEYS`. Den er en eksplisitt, innsjekket
 * liste: en nøkkel som fantes FØR redesignet har nøyaktig de kravene den
 * alltid har hatt, og en glemt oversettelse av gammel tekst er fortsatt en
 * feilende test. Bare det som står i lista slipper unna, og bare i de fem
 * pausete språkene — «ingen EKSTRA nøkler» gjelder fortsatt overalt, så et
 * språk kan aldri få tekst no.json ikke har.
 *
 * Fase B tømmer lista. Den går derfor bare én vei som skrallen: å legge noe
 * til her er en beslutning noen må skrive ned, ikke noe som siger inn.
 */
export const PAUSED_LOCALES = ['sv', 'da', 'de', 'fr', 'pl']

/** Nøkler `app/` har lagt til under redesignet. Tømmes i fase B. */
export const PAUSED_KEYS = new Set([
  // S1a: skallets sidenavn — også `tDyn('app.page', route.page)`-subtreet.
  'app.page.record',
  'app.page.library',
  'app.page.setup',
  // S1b: komponentbiblioteket, skinnen og de tre tomtilstandene. Alt som
  // faktisk RENDRES i det nye skallet i dag; ingenting på forskudd.
  'app.rail.label',
  'app.rail.churchUnset',
  'app.status.ready',
  'app.status.nosound',
  'app.status.rec',
  'app.status.next',
  'app.status.lowdisk',
  'app.receipt.saving',
  'app.receipt.saved',
  'app.receipt.failed',
  'app.common.recommended',
  'app.common.dismiss',
  'app.dialog.confirm',
  'app.dialog.cancel',
  'app.vu.hear',
  'app.vu.nothing',
  'app.vu.loud',
  'app.library.empty',
  'app.library.emptyDesc',
  'app.library.goRecord',
  'app.library.stored',
  'app.record.noSource',
  'app.record.noSourceDesc',
  'app.record.chooseSound',
  'app.setup.lede',
  'app.setup.q1',
  'app.setup.q2',
  'app.setup.q3',
  'app.setup.q4',
  'app.setup.q5',
  'app.setup.notSetUp',
  'app.setup.nobodyYet',
  'app.setup.quality.mp3',
  'app.setup.quality.flac',
  'app.setup.quality.wav',
  // P1a: OPPSETT nivå 1, de fem undersidene og de to tilleggene.
  'app.setup.change',
  'app.setup.setUp',
  'app.setup.back',
  'app.setup.save',
  'app.setup.cancel',
  'app.setup.addons',
  'app.language.no',
  'app.language.en',
  'app.setup.sound.lede',
  'app.setup.sound.deviceWithPair',
  'app.setup.sound.gone',
  'app.setup.sound.goneDesc',
  'app.setup.sound.noneDesc',
  'app.setup.sound.builtIn',
  'app.setup.sound.builtInDesc',
  'app.setup.sound.external',
  'app.setup.sound.mixer',
  'app.setup.sound.whichChannels',
  'app.setup.sound.pairLabel',
  'app.setup.sound.channelHint',
  'app.setup.sound.testHint',
  'app.setup.sound.useThis',
  'app.setup.sound.noChange',
  'app.setup.sound.empty',
  'app.setup.sound.emptyDesc',
  'app.setup.sound.retry',
  'app.setup.folder.lede',
  'app.setup.folder.label',
  'app.setup.folder.pick',
  'app.setup.folder.free',
  'app.setup.folder.space',
  'app.setup.folder.unknownSpace',
  'app.setup.folder.none',
  'app.setup.folder.noneDesc',
  'app.setup.qualityLede',
  'app.setup.qDesc.mp3',
  'app.setup.qDesc.flac',
  'app.setup.qDesc.wav',
  'app.setup.qualityCustom',
  'app.setup.qualityCustomDesc',
  'app.setup.church.lede',
  'app.setup.church.name',
  'app.setup.church.nameDesc',
  'app.setup.church.placeholder',
  'app.setup.church.languageLabel',
  'app.setup.church.languageDesc',
  'app.setup.church.language',
  'app.setup.notify.lede',
  'app.setup.notify.os',
  'app.setup.notify.osDesc',
  'app.setup.notify.mail',
  'app.setup.notify.mailDesc',
  'app.setup.notify.nobodyDesc',
  'app.setup.notify.emailDesc',
  'app.setup.notify.gateChip',
  'app.setup.notify.gateSmtp',
  'app.setup.notify.gateChipUnavailable',
  'app.setup.notify.gateNoFeature',
  'app.setup.notify.to',
  'app.setup.notify.toDesc',
  'app.setup.notify.toPlaceholder',
  'app.setup.notify.invalid',
  'app.setup.notify.nothingToSave',
  'app.setup.notify.test',
  'app.setup.notify.testSent',
  'app.setup.notify.testFailed',
  'app.setup.notify.testNoRecipient',
  'app.setup.notify.remind',
  'app.setup.notify.remindDesc',
  'app.setup.notify.remindChip',
  'app.setup.notify.remindGate',
  'app.setup.notify.remindOff',
  'app.setup.notify.minutes',
  'app.setup.camera.title',
  'app.setup.camera.desc',
  'app.setup.camera.noneChosen',
  'app.setup.camera.pick',
  'app.setup.camera.keepAudio',
  'app.setup.camera.keepAudioDesc',
  'app.setup.camera.delivers',
  'app.setup.camera.probeFailed',
  'app.setup.camera.none',
  'app.setup.camera.noneDesc',
  'app.setup.auto.title',
  'app.setup.auto.desc',
  'app.setup.auto.summary',
  'app.setup.auto.day',
  'app.setup.auto.start',
  'app.setup.auto.duration',
  'app.setup.auto.durationDesc',
  'app.setup.auto.minutes',
  'app.setup.auto.nothingToSave',
  'app.setup.auto.launch',
  'app.setup.auto.launchDesc',
  'app.setup.auto.more',
  'app.setup.auto.offTitle',
  'app.setup.auto.offBody',
  'app.setup.auto.offConfirm',
  'app.setup.days.0',
  'app.setup.days.1',
  'app.setup.days.2',
  'app.setup.days.3',
  'app.setup.days.4',
  'app.setup.days.5',
  'app.setup.days.6',
])

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
      const paused = PAUSED_LOCALES.includes(lang)
      // A paused language may lag ONLY on the redesign's own new keys; every
      // key that existed before is still required, in every language.
      const missing = reference
        .filter(k => !keys.has(k))
        .filter(k => !(paused && PAUSED_KEYS.has(k)))
      // «Extra» is never paused: a language must never carry text no.json
      // does not have — that is how a string ends up impossible to review.
      const extra = [...keys].filter(k => !reference.includes(k)).sort()
      expect(missing, `keys missing from ${lang}.json`).toEqual([])
      expect(extra, `keys in ${lang}.json that no.json lacks`).toEqual([])
    })
  }

  it('every paused key really is missing from at least one paused locale', () => {
    // A guard on the pause: once a key HAS been translated everywhere, its
    // entry here is dead weight that quietly excuses the next key someone
    // adds next to it. Fase B empties the list; this keeps it shrinking.
    const stale = [...PAUSED_KEYS].filter(key =>
      locales
        .filter(([lang]) => PAUSED_LOCALES.includes(lang))
        .every(([, tree]) => lookup(tree, key) !== undefined),
    )
    expect(stale, 'PAUSED_KEYS entries that are no longer missing anywhere').toEqual([])
  })

  it('no.json actually has every paused key', () => {
    // The pause excuses the OTHER languages, never Norwegian: a key nobody
    // has written at all would otherwise pass silently.
    const missingFromNo = [...PAUSED_KEYS].filter(k => !reference.includes(k))
    expect(missingFromNo, 'PAUSED_KEYS entries missing from no.json').toEqual([])
  })
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
        // Same pause as the key-set test above: a plural group added for the
        // redesign is not yet expected in the five paused languages.
        if (PAUSED_LOCALES.includes(lang) && PAUSED_KEYS.has(key)) continue
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
