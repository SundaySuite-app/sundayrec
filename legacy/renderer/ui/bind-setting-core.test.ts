import { describe, expect, it } from 'vitest'
import {
  coerceValue,
  controlKindOf,
  guardReasonFor,
  isRealChange,
  minutesUntil,
  planCommit,
  SAVE_COALESCE_MS,
  TEXT_DEBOUNCE_MS,
  validateNumber,
} from './bind-setting-core'

describe('controlKindOf', () => {
  it('classifies the shapes the settings pages actually use', () => {
    expect(controlKindOf({ tag: 'SELECT' })).toBe('select')
    expect(controlKindOf({ tag: 'textarea' })).toBe('textarea')
    expect(controlKindOf({ tag: 'INPUT', type: 'checkbox' })).toBe('toggle')
    expect(controlKindOf({ tag: 'input', type: 'radio' })).toBe('radio')
    expect(controlKindOf({ tag: 'input', type: 'range' })).toBe('slider')
    expect(controlKindOf({ tag: 'input', type: 'number' })).toBe('number')
    expect(controlKindOf({ tag: 'input', type: 'email' })).toBe('text')
  })

  it('treats a typeless input as text', () => {
    expect(controlKindOf({ tag: 'input' })).toBe('text')
    expect(controlKindOf({ tag: 'input', type: null })).toBe('text')
  })
})

describe('planCommit', () => {
  it('commits discrete controls on change with no delay', () => {
    for (const kind of ['toggle', 'radio', 'select'] as const) {
      expect(planCommit(kind)).toEqual({ debounceMs: 0, events: ['change'] })
    }
  })

  it('commits a slider on release, not during the drag', () => {
    // `change` (not `input`) is the release event for a range input.
    expect(planCommit('slider')).toEqual({ debounceMs: 0, events: ['change'] })
  })

  it('debounces free text and listens on input so an unblurred edit still saves', () => {
    expect(planCommit('text')).toEqual({
      debounceMs: TEXT_DEBOUNCE_MS,
      events: ['input', 'change'],
    })
    expect(planCommit('textarea').debounceMs).toBe(TEXT_DEBOUNCE_MS)
    expect(planCommit('number').debounceMs).toBe(TEXT_DEBOUNCE_MS)
  })

  it('honours immediate and explicit overrides', () => {
    expect(planCommit('text', { immediate: true }).debounceMs).toBe(0)
    expect(planCommit('text', { debounceMs: 900 }).debounceMs).toBe(900)
    expect(planCommit('text', { debounceMs: -5 }).debounceMs).toBe(0)
    // immediate wins over an explicit debounce
    expect(planCommit('text', { immediate: true, debounceMs: 900 }).debounceMs).toBe(0)
  })

  it('keeps the save-coalescing window shorter than the typing debounce', () => {
    expect(SAVE_COALESCE_MS).toBeLessThan(TEXT_DEBOUNCE_MS)
  })
})

describe('coerceValue', () => {
  it('reads a toggle as a boolean regardless of its value attribute', () => {
    expect(coerceValue('toggle', { value: 'on', checked: true })).toBe(true)
    expect(coerceValue('toggle', { value: 'on', checked: false })).toBe(false)
    expect(coerceValue('toggle', { value: '' })).toBe(false)
  })

  it('parses numeric controls and reports an empty field as null, not 0', () => {
    expect(coerceValue('number', { value: '90' })).toBe(90)
    expect(coerceValue('slider', { value: '-12.5' })).toBe(-12.5)
    expect(coerceValue('number', { value: '  ' })).toBeNull()
    expect(coerceValue('number', { value: 'abc' })).toBeNull()
    // 0 is a real value the old `|| fallback` idiom used to swallow.
    expect(coerceValue('number', { value: '0' })).toBe(0)
  })

  it('passes text and select values through untouched', () => {
    expect(coerceValue('text', { value: ' Alta Frikirke ' })).toBe(' Alta Frikirke ')
    expect(coerceValue('select', { value: 'church' })).toBe('church')
    expect(coerceValue('textarea', { value: '' })).toBe('')
  })
})

describe('validateNumber', () => {
  it('rejects non-numbers', () => {
    expect(validateNumber(null)).toEqual({ ok: false, value: null, issue: 'nan' })
    expect(validateNumber('12' as unknown as number)).toEqual({
      ok: false,
      value: null,
      issue: 'nan',
    })
  })

  it('rejects out-of-range values by default and reports the bound', () => {
    expect(validateNumber(2, { min: 15 })).toEqual({
      ok: false,
      value: null,
      issue: 'below',
      bound: 15,
    })
    expect(validateNumber(9000, { max: 3650 })).toEqual({
      ok: false,
      value: null,
      issue: 'above',
      bound: 3650,
    })
  })

  it('clamps instead when the rule asks for it', () => {
    expect(validateNumber(100, { min: 500, max: 50000, clamp: true })).toEqual({
      ok: true,
      value: 500,
      bound: 500,
    })
    expect(validateNumber(99999, { min: 500, max: 50000, clamp: true })).toEqual({
      ok: true,
      value: 50000,
      bound: 50000,
    })
  })

  it('accepts in-range values and enforces integers when asked', () => {
    expect(validateNumber(90, { min: 1, max: 3650 })).toEqual({ ok: true, value: 90 })
    expect(validateNumber(90.5, { integer: true }).issue).toBe('notInteger')
  })
})

describe('isRealChange', () => {
  it('ignores a re-fire of the same value', () => {
    expect(isRealChange('mp3', 'mp3')).toBe(false)
    expect(isRealChange(false, false)).toBe(false)
    expect(isRealChange('mp3', 'flac')).toBe(true)
    expect(isRealChange(null, 0)).toBe(true)
  })
})

describe('guardReasonFor', () => {
  const now = Date.UTC(2026, 7, 5, 10, 0, 0)

  it('guards while a recording is running', () => {
    expect(guardReasonFor({ isRecording: true, nextAtMs: null }, now)).toBe('recording')
  })

  it('guards inside the wake-lead window before the next start', () => {
    expect(guardReasonFor({ isRecording: false, nextAtMs: now + 4 * 60_000 }, now)).toBe(
      'imminent',
    )
    // Exactly at the lead boundary still counts — the machine is already waking.
    expect(guardReasonFor({ isRecording: false, nextAtMs: now + 10 * 60_000 }, now)).toBe(
      'imminent',
    )
  })

  it('stays quiet when the next recording is far away, absent or already past', () => {
    expect(guardReasonFor({ isRecording: false, nextAtMs: now + 11 * 60_000 }, now)).toBeNull()
    expect(guardReasonFor({ isRecording: false, nextAtMs: null }, now)).toBeNull()
    expect(guardReasonFor({ isRecording: false, nextAtMs: now - 60_000 }, now)).toBeNull()
  })

  it('takes a custom lead window', () => {
    expect(guardReasonFor({ isRecording: false, nextAtMs: now + 20 * 60_000 }, now, 30)).toBe(
      'imminent',
    )
  })
})

describe('minutesUntil', () => {
  it('floors to whole minutes and never goes negative', () => {
    const now = 1_000_000
    expect(minutesUntil(now + 119_000, now)).toBe(1)
    expect(minutesUntil(now + 60_000, now)).toBe(1)
    expect(minutesUntil(now - 5_000, now)).toBe(0)
  })
})
