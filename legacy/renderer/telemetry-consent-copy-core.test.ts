import { describe, expect, it } from 'vitest'
import { FIRST_ASK, RE_ASK, promptCopyFor } from './telemetry-consent-copy-core'
import no from '../locales/no.json'

describe('consent prompt copy', () => {
  it('asks a never-asked install the plain first-time question', () => {
    expect(promptCopyFor('never-asked')).toBe(FIRST_ASK)
  })

  it('explains itself to anyone who has answered before — yes OR no', () => {
    // Both, deliberately. A stale DENIAL is re-asked too (consent.rs: "'no'
    // answered a different question"), and that person has even more reason to
    // read a repeated prompt as the app ignoring them.
    expect(promptCopyFor('granted')).toBe(RE_ASK)
    expect(promptCopyFor('denied')).toBe(RE_ASK)
  })

  it('never reuses the first-ask wording for a re-ask', () => {
    expect(RE_ASK.titleKey).not.toBe(FIRST_ASK.titleKey)
    expect(RE_ASK.descKey).not.toBe(FIRST_ASK.descKey)
    expect(RE_ASK.titleFallback).not.toBe(FIRST_ASK.titleFallback)
    expect(RE_ASK.descFallback).not.toBe(FIRST_ASK.descFallback)
  })

  it('says it is asking again, names what was added, and disclaims the old answer', () => {
    // The three things the re-ask has to do. Asserted on the Norwegian source
    // of truth; the other six locales are held to the same key set by
    // legacy/locales/parity.test.ts, and translated from this text.
    const d = RE_ASK.descFallback
    expect(d, 'must acknowledge the earlier answer').toMatch(/svart på dette før/)
    expect(d, 'must say the scope was widened').toMatch(/utvidet/)
    expect(d, 'must name the addition concretely').toMatch(/30–60 sekunder/)
    // v2 widened twice before shipping — the banded corrections and then the
    // companion outcomes — and the card has to name BOTH. Someone re-reading a
    // prompt that mentions one of two additions has been under-told.
    expect(d, 'must name the companion addition too').toMatch(/kapittelmerker/)
    expect(d, 'must say the suggestion text itself is not sent').toMatch(
      /[Ss]elve teksten sendes aldri/,
    )
    expect(d, 'must disclaim carrying the old answer over').toMatch(
      /ikke lagt til grunn, verken som ja eller som nei/,
    )
    expect(d, 'must say nothing moves before they answer').toMatch(/ingenting sendes/)
  })

  it('carries the same words as no.json, key for key', () => {
    // Closes the last gap in the chain. check-i18n-fallbacks.mjs already pins
    // index.html against no.json, and parity.test.ts pins the six other locales
    // against it — but these fallbacks are a THIRD copy of the same sentences,
    // and the one nothing was watching. A drift here means the card shows one
    // thing in the first frame and another once translations apply.
    const ob = no.onboarding as Record<string, string>
    for (const copy of [FIRST_ASK, RE_ASK]) {
      expect(ob[copy.titleKey.replace('onboarding.', '')]).toBe(copy.titleFallback)
      expect(ob[copy.descKey.replace('onboarding.', '')]).toBe(copy.descFallback)
    }
  })

  it('leans on neither answer', () => {
    // The copy half of the equal-weight rule the CSS enforces visually: no
    // recommendation, no "anbefalt", no pre-selection in words. NB: «helst» is
    // not in the pattern — «når som helst» ("at any time") is the innocent
    // phrase both texts legitimately use.
    for (const copy of [FIRST_ASK, RE_ASK]) {
      expect(copy.descFallback.toLowerCase()).not.toMatch(/anbefal|vi håper|vennligst|du bør/)
    }
  })
})
