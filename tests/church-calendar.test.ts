import { computeEaster, getChurchHolidays, churchCalendarName, adventStart } from '../src/shared/church-calendar'

describe('computeEaster', () => {
  const cases: [number, string][] = [
    [2024, '2024-03-31'],
    [2025, '2025-04-20'],
    [2026, '2026-04-05'],
    [2022, '2022-04-17'],
    [2000, '2000-04-23']
  ]
  test.each(cases)('Easter %i = %s', (year, expected) => {
    const e = computeEaster(year)
    const iso = `${e.getFullYear()}-${String(e.getMonth()+1).padStart(2,'0')}-${String(e.getDate()).padStart(2,'0')}`
    expect(iso).toBe(expected)
  })
})

describe('getChurchHolidays', () => {
  it('includes Christmas 2025', () => {
    const h = getChurchHolidays(2025)
    expect(h['2025-12-24']).toBe('Julaften')
    expect(h['2025-12-25']).toBe('Første juledag')
  })
  it('includes Advent 2025', () => {
    const h = getChurchHolidays(2025)
    const adv = adventStart(2025)
    const iso = `${adv.getFullYear()}-${String(adv.getMonth()+1).padStart(2,'0')}-${String(adv.getDate()).padStart(2,'0')}`
    expect(h[iso]).toBe('1. søndag i advent')
  })
  it('returns easter-relative holidays', () => {
    const h = getChurchHolidays(2025)
    expect(h['2025-04-20']).toBe('Første påskedag')
    expect(h['2025-04-18']).toBe('Langfredag')
    expect(h['2025-06-08']).toBe('Første pinsedag')
  })
})

describe('churchCalendarName', () => {
  it('names Christmas', () => {
    expect(churchCalendarName(new Date(2025, 11, 24))).toBe('julaften')
    expect(churchCalendarName(new Date(2025, 11, 25))).toBe('1-juledag')
  })
  it('names Easter 2025', () => {
    expect(churchCalendarName(new Date(2025, 3, 20))).toBe('1-paaskedag')
  })
  it('falls back to gudstjeneste on regular Sunday', () => {
    expect(churchCalendarName(new Date(2025, 0, 12))).toBe('gudstjeneste')
  })
})
