jest.mock('electron-store')

import { importProfile, getHistory, addHistory, deleteHistoryEntry, clearHistory } from '../src/main/store'

describe('importProfile', () => {
  test('accepts valid minimal profile', () => {
    expect(importProfile(JSON.stringify({ format: 'mp3' }))).toBe(true)
  })

  test('accepts full valid profile', () => {
    const profile = {
      language: 'no',
      format: 'flac',
      saveFolder: '/tmp/recordings',
      emailSmtp: 'smtp.example.com',
      emailAddress: 'test@example.com',
      slots: [],
      specialRecordings: [],
    }
    expect(importProfile(JSON.stringify(profile))).toBe(true)
  })

  test('rejects invalid JSON', () => {
    expect(importProfile('not json {')).toBe(false)
    expect(importProfile('')).toBe(false)
  })

  test('rejects non-object JSON', () => {
    expect(importProfile(JSON.stringify([1, 2, 3]))).toBe(false)
    expect(importProfile(JSON.stringify(42))).toBe(false)
    expect(importProfile(JSON.stringify('string'))).toBe(false)
    expect(importProfile(JSON.stringify(null))).toBe(false)
  })

  test('rejects saveFolder of wrong type', () => {
    expect(importProfile(JSON.stringify({ saveFolder: 123 }))).toBe(false)
    expect(importProfile(JSON.stringify({ saveFolder: true }))).toBe(false)
  })

  test('accepts null saveFolder (reset to default)', () => {
    expect(importProfile(JSON.stringify({ saveFolder: null }))).toBe(true)
  })

  test('rejects emailSmtp of wrong type', () => {
    expect(importProfile(JSON.stringify({ emailSmtp: 587 }))).toBe(false)
  })

  test('rejects emailAddress of wrong type', () => {
    expect(importProfile(JSON.stringify({ emailAddress: false }))).toBe(false)
  })

  test('rejects slots of wrong type', () => {
    expect(importProfile(JSON.stringify({ slots: 'not-an-array' }))).toBe(false)
    expect(importProfile(JSON.stringify({ slots: {} }))).toBe(false)
  })

  test('rejects specialRecordings of wrong type', () => {
    expect(importProfile(JSON.stringify({ specialRecordings: 'bad' }))).toBe(false)
  })

  test('rejects language of wrong type', () => {
    expect(importProfile(JSON.stringify({ language: 42 }))).toBe(false)
  })

  test('accepts null language', () => {
    expect(importProfile(JSON.stringify({ language: null }))).toBe(true)
  })

  test('strips recordingHistory from import', () => {
    // Should not throw — history is silently dropped
    const result = importProfile(JSON.stringify({
      format: 'mp3',
      recordingHistory: [{ date: '2025-01-01', status: 'ok' }]
    }))
    expect(result).toBe(true)
  })
})

describe('history', () => {
  beforeEach(() => clearHistory())

  test('starts empty', () => {
    expect(getHistory()).toEqual([])
  })

  test('addHistory prepends entries', () => {
    addHistory({ date: '2025-01-01', startTime: '11:00', duration: '60m', filename: 'a.mp3', status: 'ok' })
    addHistory({ date: '2025-01-08', startTime: '11:00', duration: '60m', filename: 'b.mp3', status: 'ok' })
    const h = getHistory()
    expect(h.length).toBe(2)
    expect(h[0].filename).toBe('b.mp3')
    expect(h[1].filename).toBe('a.mp3')
  })

  test('addHistory sets timestamp', () => {
    const before = Date.now()
    addHistory({ date: '2025-01-01', startTime: '11:00', duration: '60m', filename: 'a.mp3', status: 'ok' })
    const after = Date.now()
    const ts = getHistory()[0].timestamp!
    expect(ts).toBeGreaterThanOrEqual(before)
    expect(ts).toBeLessThanOrEqual(after)
  })

  test('deleteHistoryEntry removes by timestamp', () => {
    addHistory({ date: '2025-01-01', startTime: '11:00', duration: '60m', filename: 'a.mp3', status: 'ok' })
    const ts = getHistory()[0].timestamp!
    deleteHistoryEntry(ts)
    expect(getHistory()).toEqual([])
  })

  test('clearHistory empties the list', () => {
    addHistory({ date: '2025-01-01', startTime: '11:00', duration: '60m', filename: 'a.mp3', status: 'ok' })
    clearHistory()
    expect(getHistory()).toEqual([])
  })

  test('addHistory caps at 200 entries', () => {
    for (let i = 0; i < 210; i++) {
      addHistory({ date: '2025-01-01', startTime: '11:00', duration: '1m', filename: `f${i}.mp3`, status: 'ok' })
    }
    expect(getHistory().length).toBe(200)
  })
})
