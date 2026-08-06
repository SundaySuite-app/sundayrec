import { describe, expect, it } from 'vitest'
import {
  buildNext,
  computeWake,
  emptyState,
  formatCountdown,
  formatMissed,
  formatMissedBanner,
  formatNextDate,
  formatNextTitle,
  formatPreflightHeadline,
  formatSchedulePreview,
  formatSidebarStatus,
  formatWakeHint,
  intlParts,
  parseLocalIso,
  WAKE_LEAD_MINUTES,
  type DateParts,
  type FormatCtx,
  type NextRecordingState,
} from './next-recording-core'

// Near-identity translator: `key::fallback`. The key makes assertions
// language-independent; keeping the fallback means the `{n}` / `{time}`
// placeholders are still there to be substituted, so a formatter that forgets
// to replace one is caught. The real `t()` returns the locale string, which
// carries the same placeholders.
const t = (key: string, fallback = ''): string => (fallback ? `${key}::${fallback}` : key)

// Deterministic date rendering — no ICU, no timezone, no locale data.
const parts = (ms: number): DateParts => ({
  weekdayLong: `LONG@${ms}`,
  weekdayShort: `SHORT@${ms}`,
  time: `TIME@${ms}`,
  dateLong: `DATE@${ms}`,
})

const NOW = 1_000_000
const ctx = (nowMs = NOW): FormatCtx => ({ t, parts, nowMs })

/** Countdown duration stub — the app injects `fmtCountdown`. */
const duration = (ms: number): string => `DUR(${ms})`

function stateWith(patch: Partial<NextRecordingState>): NextRecordingState {
  return { ...emptyState(), ...patch }
}

const AT = '2026-06-07T11:00:00'
const AT_MS = new Date(AT).getTime()

/** A stored special, in the full shape the renderer keeps them in — `buildNext`
 *  only reads date/name/start, which is why it accepts a structural type. */
const special = (date: string, name: string, start: string) => ({
  id: 'sp-1',
  date,
  name,
  start,
  stop: '12:00',
  deviceId: null,
})

describe('parseLocalIso', () => {
  it('reads the backend zone-less ISO as LOCAL wall time', () => {
    const ms = parseLocalIso('2026-06-07T11:00:00')
    const d = new Date(ms)
    expect(d.getHours()).toBe(11)
    expect(d.getMinutes()).toBe(0)
    expect(d.getDate()).toBe(7)
  })
})

describe('buildNext', () => {
  it('returns null for a null/empty payload', () => {
    expect(buildNext(null)).toBeNull()
    expect(buildNext(undefined)).toBeNull()
    expect(buildNext('')).toBeNull()
  })

  it('returns null for an unparseable payload instead of NaN', () => {
    expect(buildNext('not-a-date')).toBeNull()
  })

  it('is an unnamed slot start when no special matches', () => {
    expect(buildNext(AT, [])).toEqual({ at: AT, atMs: AT_MS })
  })

  it('names the start when a one-off special produced it', () => {
    const next = buildNext(AT, [special('2026-06-07', 'Julaften', '11:00')])
    expect(next).toEqual({ at: AT, atMs: AT_MS, label: 'Julaften', isSpecial: true })
  })

  it('does not match a special on a different day or time', () => {
    const wrongDay = buildNext(AT, [special('2026-06-08', 'X', '11:00')])
    const wrongTime = buildNext(AT, [special('2026-06-07', 'X', '11:30')])
    expect(wrongDay?.isSpecial).toBeUndefined()
    expect(wrongTime?.isSpecial).toBeUndefined()
  })
})

describe('computeWake', () => {
  it('is null without a next recording', () => {
    expect(computeWake(null, true)).toBeNull()
  })

  it('uses the BACKEND lead time, not a UI guess', () => {
    const next = buildNext(AT)!
    const wake = computeWake(next, true)!
    expect(WAKE_LEAD_MINUTES).toBe(10) // mirrors crates/sundayrec-core/src/wake.rs
    expect(wake.leadMinutes).toBe(10)
    expect(next.atMs - wake.atMs).toBe(10 * 60_000)
  })

  it('carries the disabled flag rather than dropping the info', () => {
    const wake = computeWake(buildNext(AT)!, false)!
    expect(wake.enabled).toBe(false)
  })
})

describe('formatNextTitle', () => {
  const cases: Array<[string, NextRecordingState, string]> = [
    ['no schedule at all', stateWith({}), 'home.readyNoSchedule'],
    [
      'schedule exists but nothing ahead',
      stateWith({ hasAnySchedule: true }),
      'home.readyTitle',
    ],
    [
      'a next start',
      stateWith({ hasAnySchedule: true, next: buildNext(AT) }),
      `home.readyTitleDay`,
    ],
  ]

  for (const [name, state, expected] of cases) {
    it(name, () => {
      const out = formatNextTitle(state, ctx())
      expect(out).toContain(expected)
      if (expected === 'home.readyTitleDay') {
        // Placeholders are substituted, so none may survive into the UI.
        expect(out).toContain(`LONG@${AT_MS}`)
        expect(out).toContain(`TIME@${AT_MS}`)
        expect(out).not.toContain('{day}')
        expect(out).not.toContain('{time}')
      }
    })
  }
})

describe('formatNextDate', () => {
  it('is an em-dash when nothing is scheduled', () => {
    expect(formatNextDate(stateWith({}), ctx())).toBe('—')
  })

  it('is the long date for a slot start', () => {
    expect(formatNextDate(stateWith({ next: buildNext(AT) }), ctx())).toBe(`DATE@${AT_MS}`)
  })

  it('appends the special name when there is one', () => {
    const next = buildNext(AT, [special('2026-06-07', 'Julaften', '11:00')])
    expect(formatNextDate(stateWith({ next }), ctx())).toBe(`DATE@${AT_MS} · Julaften`)
  })
})

describe('formatCountdown', () => {
  it('is empty when nothing is scheduled', () => {
    expect(formatCountdown(stateWith({}), ctx(), duration)).toBe('')
  })

  it('is empty once the start has passed', () => {
    const state = stateWith({ next: buildNext(AT) })
    expect(formatCountdown(state, ctx(AT_MS), duration)).toBe('')
    expect(formatCountdown(state, ctx(AT_MS + 1), duration)).toBe('')
  })

  it('counts down while idle', () => {
    const state = stateWith({ next: buildNext(AT) })
    const out = formatCountdown(state, ctx(AT_MS - 60_000), duration)
    expect(out).toContain('DUR(60000)')
    expect(out).toContain('home.untilStart')
    expect(out).not.toContain('status.recording')
  })

  it('KEEPS TICKING during a recording, prefixed with the take', () => {
    const state = stateWith({ next: buildNext(AT), isRecording: true })
    const out = formatCountdown(state, ctx(AT_MS - 60_000), duration)
    expect(out).toContain('status.recording')
    expect(out).toContain('home.nextShort')
    expect(out).toContain('DUR(60000)')
  })
})

describe('formatSidebarStatus', () => {
  it('a running take wins over everything', () => {
    const state = stateWith({ isRecording: true, next: buildNext(AT) })
    const out = formatSidebarStatus(state, ctx(), { connected: false, name: 'Qu-5' })
    expect(out.dot).toBe('recording')
    expect(out.text).toContain('status.recording')
  })

  it('a missing device names itself', () => {
    const out = formatSidebarStatus(stateWith({ next: buildNext(AT) }), ctx(), {
      connected: false,
      name: 'Qu-5',
    })
    expect(out.dot).toBe('warn')
    expect(out.text).toContain('status.warning')
    expect(out.text.endsWith(': Qu-5')).toBe(true)
  })

  it('falls back to the plain warning when the device has no name', () => {
    const out = formatSidebarStatus(stateWith({}), ctx(), { connected: false })
    expect(out.dot).toBe('warn')
    // Exactly the warning, with no ": <device>" tail appended.
    expect(out.text).toBe(t('status.warning', 'Trenger oppmerksomhet'))
  })

  it('shows the short weekday + time when idle with a next start', () => {
    const out = formatSidebarStatus(stateWith({ next: buildNext(AT) }), ctx())
    expect(out).toEqual({ text: `SHORT@${AT_MS} TIME@${AT_MS}`, dot: '' })
  })

  it('says so when nothing is planned', () => {
    const out = formatSidebarStatus(stateWith({}), ctx())
    expect(out.dot).toBe('')
    expect(out.text).toContain('status.noSchedule')
  })
})

describe('formatWakeHint', () => {
  it('is null without a schedule', () => {
    expect(formatWakeHint(stateWith({}), ctx())).toBeNull()
  })

  it('is null when wake-from-sleep is off', () => {
    const next = buildNext(AT)
    const state = stateWith({ next, wake: computeWake(next, false) })
    expect(formatWakeHint(state, ctx())).toBeNull()
  })

  it('reports the wake POINT and the real lead time', () => {
    const next = buildNext(AT)
    const state = stateWith({ next, wake: computeWake(next, true) })
    const out = formatWakeHint(state, ctx())!
    expect(out).toContain(`TIME@${AT_MS - 10 * 60_000}`)
    expect(out).toContain('10')
    expect(out).not.toContain('{time}')
    expect(out).not.toContain('{min}')
  })
})

describe('formatSchedulePreview', () => {
  it('is empty when nothing is scheduled', () => {
    expect(formatSchedulePreview(stateWith({}), ctx())).toBe('')
  })

  it('includes a SPECIAL — the old hand-rolled preview could not see them', () => {
    const next = buildNext(AT, [special('2026-06-07', 'Julaften', '11:00')])
    const out = formatSchedulePreview(stateWith({ next }), ctx())
    expect(out).toContain(`DATE@${AT_MS}`)
    expect(out).toContain(`TIME@${AT_MS}`)
    expect(out).toContain('Julaften')
  })
})

describe('missed + preflight formatting', () => {
  it('has no banner when nothing was missed', () => {
    expect(formatMissedBanner(stateWith({}), ctx())).toBeNull()
  })

  it('uses singular vs plural copy', () => {
    const one = stateWith({ missed: [{ at: AT, label: 'Gudstjeneste' }] })
    const two = stateWith({
      missed: [
        { at: AT, label: 'Gudstjeneste' },
        { at: AT, label: 'Kveldsmøte' },
      ],
    })
    expect(formatMissedBanner(one, ctx())).toContain('missed.bannerOne')
    const many = formatMissedBanner(two, ctx())!
    expect(many).toContain('missed.bannerMany')
    expect(many).toContain('2')
    expect(many).not.toContain('{n}')
  })

  it('renders a missed row with its time and label', () => {
    const out = formatMissed({ at: AT, label: 'Gudstjeneste' }, ctx())
    expect(out).toContain(`DATE@${AT_MS}`)
    expect(out).toContain(`TIME@${AT_MS}`)
    expect(out).toContain('Gudstjeneste')
  })

  it('degrades to the bare label when the timestamp is unusable', () => {
    expect(formatMissed({ at: 'garbage', label: 'Gudstjeneste' }, ctx())).toBe('Gudstjeneste')
  })

  it('has no preflight headline for zero findings', () => {
    expect(formatPreflightHeadline([], ctx())).toBeNull()
  })

  it('lets an error outrank warnings', () => {
    const out = formatPreflightHeadline(
      [
        { severity: 'warn', category: 'disk', message: 'a' },
        { severity: 'error', category: 'device', message: 'b' },
      ],
      ctx(),
    )!
    expect(out.severity).toBe('error')
    expect(out.text).toContain('1')
  })

  it('counts warnings when there is no error', () => {
    const out = formatPreflightHeadline(
      [
        { severity: 'warn', category: 'disk', message: 'a' },
        { severity: 'warn', category: 'cloud', message: 'b' },
      ],
      ctx(),
    )!
    expect(out.severity).toBe('warn')
    expect(out.text).toContain('2')
  })
})

describe('intlParts', () => {
  it('produces the four shapes the UI needs', () => {
    const p = intlParts('en-GB')(AT_MS)
    expect(p.weekdayLong.length).toBeGreaterThan(0)
    expect(p.weekdayShort.length).toBeGreaterThan(0)
    expect(p.time).toMatch(/\d{1,2}[.:]\d{2}/)
    expect(p.dateLong.length).toBeGreaterThan(0)
  })
})
