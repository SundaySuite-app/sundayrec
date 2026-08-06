import { describe, expect, it } from 'vitest'
import { PICK, jinglePathFor, jingleValueFor } from './review-jingles'

describe('dropdown value → stored path', () => {
  it('maps «Ingen» to an explicit null, not to "unchanged"', () => {
    // The backend tells absent from null: this null is what clears the jingle.
    expect(jinglePathFor('none', '/j/intro.mp3')).toBeNull()
  })

  it('maps «Standard» to the configured default', () => {
    expect(jinglePathFor('default', '/j/intro.mp3')).toBe('/j/intro.mp3')
  })

  it('maps «Standard» to null when nothing is configured', () => {
    // Choosing "the default" when there is no default is a real "no jingle" —
    // not a reason to leave the entry pointing at a file that was removed.
    expect(jinglePathFor('default', undefined)).toBeNull()
    expect(jinglePathFor('default', null)).toBeNull()
  })

  it('asks for a picker on anything else', () => {
    expect(jinglePathFor('custom', '/j/intro.mp3')).toBe(PICK)
    // A value the markup grows later must not silently become a path.
    expect(jinglePathFor('', '/j/intro.mp3')).toBe(PICK)
  })
})

describe('stored path → dropdown value', () => {
  it('shows «Ingen» for a missing path', () => {
    expect(jingleValueFor(null, '/j/intro.mp3')).toBe('none')
    expect(jingleValueFor(undefined, '/j/intro.mp3')).toBe('none')
    // A blank string reaches here from an old prep written by the Electron
    // build; it means the same thing as absent and must not read as «Egen fil».
    expect(jingleValueFor('', '/j/intro.mp3')).toBe('none')
  })

  it('shows «Standard» only when the path IS the configured default', () => {
    expect(jingleValueFor('/j/intro.mp3', '/j/intro.mp3')).toBe('default')
    expect(jingleValueFor('/other/x.mp3', '/j/intro.mp3')).toBe('custom')
    // No default configured: a real path is always the user's own file.
    expect(jingleValueFor('/other/x.mp3', undefined)).toBe('custom')
  })

  it('round-trips every dropdown value that does not need a picker', () => {
    const def = '/j/intro.mp3'
    for (const value of ['none', 'default'] as const) {
      const path = jinglePathFor(value, def)
      expect(path).not.toBe(PICK)
      expect(jingleValueFor(path as string | null, def)).toBe(value)
    }
  })
})
