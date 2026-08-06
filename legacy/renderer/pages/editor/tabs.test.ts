import { describe, expect, it } from 'vitest'
import { EDITOR_TABS, resolveTabId } from './tabs'

describe('resolveTabId', () => {
  it('accepts every id the strip actually has', () => {
    for (const id of EDITOR_TABS) expect(resolveTabId(id)).toBe(id)
  })

  it('opens Lyd when nothing is stored', () => {
    expect(resolveTabId(null)).toBe('audio')
    expect(resolveTabId(undefined)).toBe('audio')
    expect(resolveTabId('')).toBe('audio')
  })

  it('opens Lyd for a value an older build (or a hand edit) left behind', () => {
    expect(resolveTabId('mastering')).toBe('audio')
    expect(resolveTabId('AUDIO')).toBe('audio')
  })
})
