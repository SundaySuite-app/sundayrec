import { churchCalendarName } from '../src/shared/church-calendar'

describe('filename pattern — church calendar name', () => {
  it('uses correct name for Palm Sunday 2025 (April 13)', () => {
    expect(churchCalendarName(new Date(2025, 3, 13))).toBe('palmesondag')
  })
  it('uses correct name for Ascension 2025 (May 29)', () => {
    expect(churchCalendarName(new Date(2025, 4, 29))).toBe('kristi-himmelfartsdag')
  })
  it('handles New Year', () => {
    expect(churchCalendarName(new Date(2025, 0, 1))).toBe('nyttarsdag')
  })
  it('returns gudstjeneste for a normal date', () => {
    expect(churchCalendarName(new Date(2025, 5, 15))).toBe('gudstjeneste')
  })
})
