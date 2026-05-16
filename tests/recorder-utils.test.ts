import { localDateStr, formatDuration, codecFor, buildFilename } from '../src/main/recorder-utils'

describe('localDateStr', () => {
  test('formats a normal date', () => {
    expect(localDateStr(new Date(2025, 4, 16))).toBe('2025-05-16')
  })

  test('pads month and day with leading zero', () => {
    expect(localDateStr(new Date(2025, 0, 1))).toBe('2025-01-01')
    expect(localDateStr(new Date(2025, 8, 9))).toBe('2025-09-09')
  })

  test('uses LOCAL date — not UTC (key regression test)', () => {
    // Simulate midnight in UTC+2: 2025-05-16T22:00:00Z = 2025-05-17T00:00:00+02:00
    // localDateStr must return the local date (2025-05-17 in +2), not the UTC date (2025-05-16)
    // We can't control TZ in tests, but we verify it matches new Date() local date
    const d = new Date(2025, 4, 17, 0, 0, 0)   // local midnight
    expect(localDateStr(d)).toBe('2025-05-17')
  })
})

describe('formatDuration', () => {
  test('minutes only', () => {
    expect(formatDuration(0)).toBe('0m')
    expect(formatDuration(60)).toBe('1m')
    expect(formatDuration(90)).toBe('1m')
    expect(formatDuration(3599)).toBe('59m')
  })

  test('hours and minutes', () => {
    expect(formatDuration(3600)).toBe('1t 0m')
    expect(formatDuration(3660)).toBe('1t 1m')
    expect(formatDuration(5400)).toBe('1t 30m')
    expect(formatDuration(7200)).toBe('2t 0m')
    expect(formatDuration(7261)).toBe('2t 1m')
  })
})

describe('codecFor', () => {
  test('known formats', () => {
    expect(codecFor('mp3')).toBe('libmp3lame')
    expect(codecFor('flac')).toBe('flac')
    expect(codecFor('aac')).toBe('aac')
    expect(codecFor('wav')).toBe('pcm_s16le')
  })

  test('unknown format falls back to mp3 codec', () => {
    expect(codecFor('ogg')).toBe('libmp3lame')
    expect(codecFor('')).toBe('libmp3lame')
  })
})

describe('buildFilename', () => {
  const fixedDate = new Date(2025, 4, 16, 11, 0, 0)   // 2025-05-16 11:00

  test('default pattern (date)', () => {
    expect(buildFilename({ format: 'mp3' }, fixedDate)).toBe('2025-05-16.mp3')
  })

  test('plain pattern', () => {
    expect(buildFilename({ format: 'mp3', filenamePattern: 'plain' }, fixedDate))
      .toBe('gudstjeneste_2025-05-16.mp3')
  })

  test('datetime pattern', () => {
    expect(buildFilename({ format: 'mp3', filenamePattern: 'datetime' }, fixedDate))
      .toBe('2025-05-16_1100.mp3')
  })

  test('church pattern on easter', () => {
    const easter2025 = new Date(2025, 3, 20, 11, 0, 0)
    expect(buildFilename({ format: 'mp3', filenamePattern: 'church' }, easter2025))
      .toBe('1-paaskedag_2025-04-20.mp3')
  })

  test('church pattern on ordinary sunday', () => {
    const ordinary = new Date(2025, 1, 9, 11, 0, 0)
    expect(buildFilename({ format: 'mp3', filenamePattern: 'church' }, ordinary))
      .toBe('gudstjeneste_2025-02-09.mp3')
  })

  test('customName overrides pattern', () => {
    const result = buildFilename(
      { format: 'mp3', filenamePattern: 'church', customName: 'Bededag 2025' },
      fixedDate
    )
    expect(result).toBe('Bededag 2025_2025-05-16.mp3')
  })

  test('customName sanitizes illegal characters', () => {
    const result = buildFilename(
      { format: 'mp3', customName: 'Min/Fil:Test*' },
      fixedDate
    )
    expect(result).toBe('Min_Fil_Test__2025-05-16.mp3')
    expect(result).not.toMatch(/[/\\:*?"<>|]/)
  })

  test('customName trims whitespace', () => {
    const result = buildFilename({ format: 'mp3', customName: '  Gudstjeneste  ' }, fixedDate)
    expect(result.startsWith('Gudstjeneste_')).toBe(true)
  })

  test('splitTimestamp is included', () => {
    const result = buildFilename(
      { format: 'mp3', filenamePattern: 'plain', splitTimestamp: '1100' },
      fixedDate
    )
    expect(result).toBe('gudstjeneste_1100_2025-05-16.mp3')
  })

  test('format defaults to mp3', () => {
    expect(buildFilename({}, fixedDate)).toMatch(/\.mp3$/)
  })

  test('flac extension', () => {
    expect(buildFilename({ format: 'flac' }, fixedDate)).toMatch(/\.flac$/)
  })

  test('wav extension', () => {
    expect(buildFilename({ format: 'wav' }, fixedDate)).toMatch(/\.wav$/)
  })
})
