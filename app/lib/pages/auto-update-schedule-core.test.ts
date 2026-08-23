import { describe, expect, it } from 'vitest'
import {
  AUTO_UPDATE_INTERVAL_MS,
  autoUpdateEnabled,
  planAutoUpdateSchedule,
} from './auto-update-schedule-core'

describe('autoUpdateEnabled', () => {
  it('is off only for an explicit false', () => {
    expect(autoUpdateEnabled(false)).toBe(false)
  })

  it('is on when the operator has switched it on', () => {
    expect(autoUpdateEnabled(true)).toBe(true)
  })

  it('is on before the persisted settings have arrived', () => {
    // `settings` starts as `{}`; an unanswered setting must not read as "off",
    // or a fresh install would silently stop receiving security fixes.
    expect(autoUpdateEnabled(undefined)).toBe(true)
    expect(autoUpdateEnabled(null)).toBe(true)
  })
})

describe('planAutoUpdateSchedule', () => {
  it('arms nothing that is already armed', () => {
    expect(planAutoUpdateSchedule(true, true)).toEqual({ start: false, stop: false })
  })

  it('arms when enabled and no timer is running', () => {
    expect(planAutoUpdateSchedule(false, true)).toEqual({ start: true, stop: false })
  })

  it('stops the running timer when the setting goes off', () => {
    expect(planAutoUpdateSchedule(true, false)).toEqual({ start: false, stop: true })
  })

  it('has nothing to stop when disabled and idle', () => {
    expect(planAutoUpdateSchedule(false, false)).toEqual({ start: false, stop: false })
  })
})

/**
 * The defect this module exists for: the gate used to be read once at wire-up
 * and the interval handle thrown away, so «off» never stopped anything and
 * «on» could stack timers. Both halves are replayed here against a stand-in for
 * the DOM layer's timer state.
 */
describe('toggling repeatedly', () => {
  /** Mirrors general-page.ts's `applyAutoUpdateSchedule`, minus the DOM/IPC. */
  function drive(): { toggle: (enabled: boolean) => void; timers: () => number; checks: () => number } {
    let live = 0
    let checks = 0
    return {
      toggle: (enabled: boolean) => {
        const action = planAutoUpdateSchedule(live > 0, enabled)
        if (action.stop) live -= 1
        if (action.start) { checks += 1; live += 1 }
      },
      timers: () => live,
      checks: () => checks,
    }
  }

  it('never accumulates timers across many flips', () => {
    const d = drive()
    for (let i = 0; i < 8; i++) {
      d.toggle(true)
      expect(d.timers()).toBe(1)
      d.toggle(false)
      expect(d.timers()).toBe(0)
    }
  })

  it('does not stack a second timer when told "on" while already on', () => {
    const d = drive()
    d.toggle(true)
    d.toggle(true)
    d.toggle(true)
    expect(d.timers()).toBe(1)
    // …and re-affirming "on" is not a reason to contact the server again.
    expect(d.checks()).toBe(1)
  })

  it('leaves nothing running after the operator switches it off', () => {
    const d = drive()
    d.toggle(true)
    d.toggle(false)
    d.toggle(false)
    expect(d.timers()).toBe(0)
  })
})

describe('AUTO_UPDATE_INTERVAL_MS', () => {
  it('is the hour PRIVACY.md promises the operator', () => {
    expect(AUTO_UPDATE_INTERVAL_MS).toBe(60 * 60 * 1000)
  })
})
