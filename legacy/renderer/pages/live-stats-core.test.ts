import { describe, it, expect } from 'vitest'
import {
  classifyStartError,
  emptyStreamStatus,
  formatBitrate,
  formatDropped,
  formatFps,
  formatUptime,
  isQualityReduced,
  liveDestinationCount,
  livePillState,
  mapDestinationStates,
  NO_VALUE,
  type StreamStatusView,
} from './live-stats-core'

function live(over: Partial<StreamStatusView> = {}): StreamStatusView {
  return {
    ...emptyStreamStatus(),
    active: true,
    startedAt: 1_000_000,
    bitrateKbps: 2500,
    fps: 30,
    ...over,
  }
}

describe('the four numbers', () => {
  it('shows a dash while idle instead of a confident zero', () => {
    // The whole reason the card was gated: four zeros are indistinguishable
    // from "your stream is sending nothing".
    const idle = emptyStreamStatus()
    expect(formatBitrate(idle)).toBe(NO_VALUE)
    expect(formatFps(idle)).toBe(NO_VALUE)
    expect(formatDropped(idle)).toBe(NO_VALUE)
  })

  it('shows the measurement while live', () => {
    const s = live({ bitrateKbps: 2480, fps: 29.6, dropped: 4 })
    expect(formatBitrate(s)).toBe('2480 kbps')
    expect(formatFps(s)).toBe('30')
    expect(formatDropped(s)).toBe('4')
  })

  it('keeps the dropped-frame count after the stream ends', () => {
    // "How many frames did we lose?" is asked after the service too.
    const after = { ...emptyStreamStatus(), dropped: 12 }
    expect(formatDropped(after)).toBe('12')
  })

  it('never renders a negative reading', () => {
    const s = live({ bitrateKbps: -5, fps: -1, dropped: -3 })
    expect(formatBitrate(s)).toBe('0 kbps')
    expect(formatFps(s)).toBe('0')
    expect(formatDropped(s)).toBe('0')
  })
})

describe('uptime', () => {
  it('counts mm:ss from startedAt', () => {
    const s = live({ startedAt: 0 })
    expect(formatUptime(s, 0)).toBe('00:00')
    expect(formatUptime(s, 9_000)).toBe('00:09')
    expect(formatUptime(s, 65_000)).toBe('01:05')
    expect(formatUptime(s, 59 * 60_000 + 59_000)).toBe('59:59')
  })

  it('grows an hours field rather than reading 94:12 for a long service', () => {
    const s = live({ startedAt: 0 })
    expect(formatUptime(s, 60 * 60_000)).toBe('1:00:00')
    expect(formatUptime(s, 94 * 60_000 + 12_000)).toBe('1:34:12')
  })

  it('is 00:00 when nothing is running', () => {
    expect(formatUptime(emptyStreamStatus(), 999_999)).toBe('00:00')
    expect(formatUptime(live({ startedAt: null }), 999_999)).toBe('00:00')
  })

  it('does not run backwards on a clock skew', () => {
    expect(formatUptime(live({ startedAt: 5_000 }), 1_000)).toBe('00:00')
  })
})

describe('the status pill', () => {
  it('is live while streaming', () => {
    expect(livePillState(live())).toBe('is-live')
  })

  it('returns to idle on a clean stop', () => {
    // The old subscriber only touched the pill when active, so a clean stop
    // left «🔴 Live» on screen indefinitely.
    expect(livePillState(emptyStreamStatus())).toBe('is-idle')
  })

  it('shows an error state when the backend stopped with something to say', () => {
    const died = {
      ...emptyStreamStatus(),
      lastLine: 'Mistet forbindelsen — klarte ikke å koble til igjen.',
    }
    expect(livePillState(died)).toBe('is-error')
  })

  it('ignores a whitespace-only line', () => {
    expect(livePillState({ ...emptyStreamStatus(), lastLine: '   ' })).toBe('is-idle')
  })
})

describe('reduced quality', () => {
  it('is announced only while live and only once stepped down', () => {
    expect(isQualityReduced(live({ bitrateStep: 0 }))).toBe(false)
    expect(isQualityReduced(live({ bitrateStep: 1 }))).toBe(true)
    expect(isQualityReduced({ ...emptyStreamStatus(), bitrateStep: 2 })).toBe(false)
  })
})

describe('per-destination health', () => {
  const rows = [
    { id: 'yt', name: 'YouTube', enabled: true },
    { id: 'fb', name: 'Facebook', enabled: true },
  ]

  it('paints the survivor green and the dropped one red', () => {
    const s = live({
      destinations: [
        { name: 'YouTube', ok: true },
        { name: 'Facebook', ok: false },
      ],
    })
    const m = mapDestinationStates(rows, s)
    expect(m.get('yt')).toBe('live')
    expect(m.get('fb')).toBe('failed')
  })

  it('leaves a toggled-off row disabled regardless of the stream', () => {
    const s = live({ destinations: [{ name: 'Facebook', ok: true }] })
    const m = mapDestinationStates([{ id: 'fb', name: 'Facebook', enabled: false }], s)
    expect(m.get('fb')).toBe('disabled')
  })

  it('keeps an enabled row the backend never pushed to on idle, not live', () => {
    // A destination whose stream key is missing from the keychain is dropped
    // by the backend before the tee is built — it is not broadcasting, and it
    // is not broken either.
    const s = live({ destinations: [{ name: 'YouTube', ok: true }] })
    const m = mapDestinationStates(rows, s)
    expect(m.get('yt')).toBe('live')
    expect(m.get('fb')).toBe('idle')
  })

  it('resets every row to idle when the stream is not running', () => {
    const m = mapDestinationStates(rows, emptyStreamStatus())
    expect(m.get('yt')).toBe('idle')
    expect(m.get('fb')).toBe('idle')
  })

  it('maps duplicate names in order rather than collapsing them', () => {
    const dupes = [
      { id: 'a', name: 'Kirkens server', enabled: true },
      { id: 'b', name: 'Kirkens server', enabled: true },
    ]
    const s = live({
      destinations: [
        { name: 'Kirkens server', ok: false },
        { name: 'Kirkens server', ok: true },
      ],
    })
    const m = mapDestinationStates(dupes, s)
    expect(m.get('a')).toBe('failed')
    expect(m.get('b')).toBe('live')
  })

  it('counts the destinations still receiving', () => {
    const s = live({
      destinations: [
        { name: 'YouTube', ok: true },
        { name: 'Facebook', ok: false },
      ],
    })
    expect(liveDestinationCount(s)).toBe(1)
    expect(liveDestinationCount(emptyStreamStatus())).toBe(0)
  })
})

describe('start failures', () => {
  it('recognises the build-without-streaming case', () => {
    // The exact string a --no-default-features build returns, which used to be
    // printed verbatim under the START button.
    expect(
      classifyStartError(
        'feature_disabled: streaming.start requires a build with `--features streaming`',
      ),
    ).toBe('featureDisabled')
  })

  it('recognises the other backend codes', () => {
    expect(classifyStartError('no_camera')).toBe('noCamera')
    expect(classifyStartError('stream_already_active')).toBe('alreadyActive')
    expect(classifyStartError('invalid_destination:yt:TooShort')).toBe('invalidDestination')
    expect(classifyStartError('stream_args:MissingDestination')).toBe('invalidDestination')
    expect(classifyStartError('stream ffmpeg spawn: No such file or directory')).toBe(
      'spawnFailed',
    )
  })

  it('falls through to unknown for anything unrecognised', () => {
    expect(classifyStartError('something nobody predicted')).toBe('unknown')
    expect(classifyStartError('')).toBe('unknown')
  })
})
