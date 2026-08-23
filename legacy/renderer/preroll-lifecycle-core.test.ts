import { describe, expect, it } from 'vitest'

import {
  decidePreroll,
  liveFromRecordingState,
  planReconcile,
  RESTART_SETTLE_MS,
  type PrerollConditions,
  type PrerollDecision,
  type ReconcilePlan,
} from './preroll-lifecycle-core'

const ready: PrerollConditions = {
  enabled: true,
  seconds: 30,
  deviceKnown: true,
  isRecording: false,
}

describe('decidePreroll', () => {
  it('runs only when every condition holds', () => {
    expect(decidePreroll(ready)).toBe('run')
  })

  it('never runs without the advanced opt-in', () => {
    expect(decidePreroll({ ...ready, enabled: false })).toBe('stop')
  })

  it('treats pre-roll = 0 seconds as off', () => {
    expect(decidePreroll({ ...ready, seconds: 0 })).toBe('stop')
    expect(decidePreroll({ ...ready, seconds: -5 })).toBe('stop')
  })

  it('will not open an unknown device', () => {
    expect(decidePreroll({ ...ready, deviceKnown: false })).toBe('stop')
  })

  it('yields the microphone to a running recording — the one-owner invariant', () => {
    expect(decidePreroll({ ...ready, isRecording: true })).toBe('stop')
  })

  it('stops on ANY failing condition, not just the first', () => {
    // Every single-condition failure, and the all-off case.
    const failures: Array<Partial<PrerollConditions>> = [
      { enabled: false },
      { seconds: 0 },
      { deviceKnown: false },
      { isRecording: true },
      { enabled: false, seconds: 0, deviceKnown: false, isRecording: true },
    ]
    for (const f of failures) {
      expect(decidePreroll({ ...ready, ...f })).toBe('stop')
    }
  })

  it('is pure — the same input always gives the same answer', () => {
    const input = { ...ready }
    const first = decidePreroll(input)
    expect(decidePreroll(input)).toBe(first)
    expect(input).toEqual(ready)
  })
})

describe('planReconcile', () => {
  // previous, decision, force → action, applied
  const CASES: Array<
    [PrerollDecision | null, PrerollDecision, boolean, ReconcilePlan, string]
  > = [
    [
      'run',
      'run',
      false,
      { action: 'none', applied: 'run' },
      'nothing changed — no IPC, and `applied` is left exactly as it was',
    ],
    [
      'stop',
      'stop',
      false,
      { action: 'none', applied: 'stop' },
      'nothing changed',
    ],
    [
      'run',
      'stop',
      false,
      { action: 'apply', applied: 'stop' },
      'a STOP is always immediate — yielding the microphone never waits',
    ],
    [
      'stop',
      'run',
      false,
      { action: 'defer-restart', applied: 'run' },
      'coming back up after a stop waits for the driver to let go',
    ],
    [
      null,
      'run',
      false,
      { action: 'apply', applied: 'run' },
      'no previous decision is NOT the same as a previous stop — nothing to wait for',
    ],
    [
      null,
      'stop',
      false,
      { action: 'apply', applied: 'stop' },
      'app start with the feature off still issues the stop',
    ],
    [
      'run',
      'run',
      true,
      { action: 'apply', applied: 'run' },
      'force re-issues an unchanged decision (app start, device change)',
    ],
    [
      'stop',
      'stop',
      true,
      { action: 'apply', applied: 'stop' },
      'force re-issues an unchanged stop too',
    ],
    [
      'stop',
      'run',
      true,
      { action: 'defer-restart', applied: 'run' },
      'force defeats the no-op check ONLY — a forced restart still settles first',
    ],
  ]

  for (const [previous, decision, force, expected, why] of CASES) {
    it(`${previous ?? 'null'} → ${decision}${force ? ' (forced)' : ''}: ${why}`, () => {
      expect(planReconcile({ previous, decision, force })).toEqual(expected)
    })
  }

  it('claims the restart in `applied` up front, so a second reconcile cannot queue a second timer', () => {
    const first = planReconcile({ previous: 'stop', decision: 'run', force: false })
    expect(first.applied).toBe('run')
    // The shell writes `applied` before arming the timer; the next reconcile
    // therefore sees run → run and does nothing rather than arming another.
    expect(
      planReconcile({ previous: first.applied, decision: 'run', force: false }).action,
    ).toBe('none')
  })

  it('settles for 3 s — the same figure the overlay meter restart uses', () => {
    expect(RESTART_SETTLE_MS).toBe(3000)
  })
})

describe('liveFromRecordingState', () => {
  // `recording://state` fires on EVERY transition, so this mapping is what
  // decides whether the buffer may hold the microphone.
  const LIVE = ['preparing', 'recording', 'reconnecting', 'stopping']
  const DONE = ['stopped', 'failed', 'idle']

  for (const st of LIVE) {
    it(`${st} means a recording owns the device`, () => {
      expect(liveFromRecordingState(st)).toBe(true)
    })
  }

  for (const st of DONE) {
    it(`${st} means the device is free again`, () => {
      expect(liveFromRecordingState(st)).toBe(false)
    })
  }

  it('has NO opinion about an unknown or missing state', () => {
    // Not `false`: guessing "not recording" would release the buffer's
    // restraint mid-service. Not `true` either: that would strand it down.
    expect(liveFromRecordingState(undefined)).toBeNull()
    expect(liveFromRecordingState('')).toBeNull()
    expect(liveFromRecordingState('paused')).toBeNull()
    expect(liveFromRecordingState('RECORDING')).toBeNull()
  })

  it('covers every state the backend can send — a new one lands as null, not as a guess', () => {
    const known = [...LIVE, ...DONE]
    expect(known.every(s => liveFromRecordingState(s) !== null)).toBe(true)
  })
})
