// The VU feed's SHELL — refcounting, device switching, packet fan-out, and the
// one invariant this whole phase exists to protect: a `stop_vu` must never
// overtake an in-flight `start_vu` and leave a cpal stream open on the device
// with nobody left to close it.
//
// No DOM: the shell touches `window.api` and `window.__isRecording` and nothing
// else, so a stub object is enough. The module holds singleton state, so every
// test re-imports it through vi.resetModules().
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { VuLevels } from '../../../legacy/bindings/VuLevels'
import type { VuFeedState, VuFeedSubscriber } from './vu-feed'

interface Harness {
  acquireVuFeed: (sub: VuFeedSubscriber) => () => void
  vuFeedSubscriberCount: () => number
  /** Ordered log of backend calls, e.g. ['start:Qu-5', 'stop']. */
  calls: string[]
  /** Push one vu://levels packet to the (single) registered listener. */
  emit: (levels: VuLevels) => void
  /** Resolve the pending start_vu with a channel count. */
  settleStart: (channels: number) => void
  /** Reject the pending start_vu. */
  failStart: () => void
  /** Everything queued on the feed's serial queue has run. */
  drain: () => Promise<void>
}

async function makeHarness(opts: { autoStart?: number | 'manual' } = {}): Promise<Harness> {
  vi.resetModules()
  const calls: string[] = []
  let listener: ((p: unknown) => void) | null = null
  let pending: { resolve: (n: number) => void; reject: () => void } | null = null

  const api = {
    startVu: (deviceName: string | null) => {
      calls.push(`start:${deviceName ?? 'default'}`)
      if (opts.autoStart === 'manual') {
        return new Promise<number>((resolve, reject) => {
          pending = { resolve, reject: () => reject(new Error('no device')) }
        })
      }
      return Promise.resolve(opts.autoStart ?? 2)
    },
    stopVu: () => {
      calls.push('stop')
      return Promise.resolve()
    },
    on: (channel: string, fn: (p: unknown) => void) => {
      if (channel !== 'vu-levels') return () => {}
      listener = fn
      return () => {
        listener = null
      }
    },
  }
  ;(globalThis as unknown as { window: unknown }).window = { api, __isRecording: false }

  const mod = await import('./vu-feed')
  return {
    acquireVuFeed: mod.acquireVuFeed,
    vuFeedSubscriberCount: mod.vuFeedSubscriberCount,
    calls,
    emit: (levels) => listener?.(levels),
    settleStart: (n) => pending?.resolve(n),
    failStart: () => pending?.reject(),
    // Two macrotask turns is enough to flush the promise queue the feed uses.
    drain: async () => {
      for (let i = 0; i < 6; i++) await Promise.resolve()
    },
  }
}

const packet = (peak: number[], rms: number[] = peak): VuLevels => ({
  peak_dbfs: peak,
  rms_dbfs: rms,
})

beforeEach(() => {
  vi.useRealTimers()
})

describe('refcounting', () => {
  it('the first acquire starts the engine, the second is free', async () => {
    const h = await makeHarness()
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])

    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])
    expect(h.vuFeedSubscriberCount()).toBe(2)
  })

  it('only the LAST release stops the engine', async () => {
    const h = await makeHarness()
    const a = h.acquireVuFeed({ deviceName: 'Qu-5' })
    const b = h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()

    a()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])

    b()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5', 'stop'])
    expect(h.vuFeedSubscriberCount()).toBe(0)
  })

  it('a double release neither stops twice nor corrupts the count', async () => {
    const h = await makeHarness()
    const a = h.acquireVuFeed({ deviceName: 'Qu-5' })
    const b = h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    a()
    a()
    a()
    await h.drain()
    expect(h.vuFeedSubscriberCount()).toBe(1)
    expect(h.calls).toEqual(['start:Qu-5'])

    b()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5', 'stop'])
  })
})

describe('device switching', () => {
  it('a newer subscriber on another device restarts, without a stop between', async () => {
    const h = await makeHarness()
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    h.acquireVuFeed({ deviceName: 'Scarlett' })
    await h.drain()
    // start_vu is stop-first-then-start on the Rust side, so a restart is ONE
    // call — an explicit stop in between would only widen the window in which
    // the device is unowned.
    expect(h.calls).toEqual(['start:Qu-5', 'start:Scarlett'])
  })

  it('a subscriber on the same device does not restart', async () => {
    const h = await makeHarness()
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    h.acquireVuFeed({ deviceName: ' Qu-5 ' })
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])
  })

  it('releasing the subscriber that chose the device re-points the feed', async () => {
    const h = await makeHarness()
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    const b = h.acquireVuFeed({ deviceName: 'Scarlett' })
    await h.drain()
    b()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5', 'start:Scarlett', 'start:Qu-5'])
  })

  it('two acquires in the same turn cost ONE device open, not two', async () => {
    // The audio page renders its device list and starts the grid in the same
    // tick the home meter is still subscribed; the superseded request must drop
    // out of the queue rather than open a device only to have it replaced.
    const h = await makeHarness()
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    h.acquireVuFeed({ deviceName: 'Scarlett' })
    await h.drain()
    expect(h.calls).toEqual(['start:Scarlett'])
  })
})

describe('start/stop ordering', () => {
  it('a stop never overtakes an in-flight start (the device-left-open bug)', async () => {
    const h = await makeHarness({ autoStart: 'manual' })
    const release = h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])

    // The last meter closes while start_vu is STILL open. Unserialised, the
    // stop would be issued now and the start would land after it — a cpal
    // stream on the device with nobody left to close it.
    release()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5'])

    h.settleStart(32)
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5', 'stop'])
  })

  it('a start that fails twice reports failed and leaves nothing running', async () => {
    const h = await makeHarness({ autoStart: 'manual' })
    const states: VuFeedState[] = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onState: (s) => states.push(s) })
    await h.drain()
    h.failStart()
    // The retry re-enters start_vu after a 400 ms wait.
    await new Promise((r) => setTimeout(r, 450))
    h.failStart()
    await h.drain()
    expect(h.calls).toEqual(['start:Qu-5', 'start:Qu-5'])
    expect(states).toContain('failed')
  })
})

describe('packet fan-out', () => {
  it('hands every subscriber its own picked pair plus the raw payload', async () => {
    const h = await makeHarness()
    const stereo: Array<[number, number]> = []
    const mono: Array<[number, number]> = []
    let raw: VuLevels | null = null

    h.acquireVuFeed({
      deviceName: 'Qu-5',
      pick: () => ({ mode: 'stereo', chL: 2, chR: 3 }),
      onLevels: (l, r, p) => {
        stereo.push([l, r])
        raw = p
      },
    })
    h.acquireVuFeed({
      deviceName: 'Qu-5',
      pick: () => ({ mode: 'monoL', chL: 5, chR: 0 }),
      onLevels: (l, r) => mono.push([l, r]),
    })
    await h.drain()

    h.emit(packet([-40, -35, -20, -10, -6, -3]))
    expect(stereo).toEqual([[-20, -10]])
    expect(mono).toEqual([[-3, -3]])
    expect(raw).not.toBeNull()
    expect(raw!.peak_dbfs).toHaveLength(6)
  })

  it('a subscriber without a pick gets plain stereo 0/1', async () => {
    const h = await makeHarness()
    const got: Array<[number, number]> = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onLevels: (l, r) => got.push([l, r]) })
    await h.drain()
    h.emit(packet([-12, -18, -3]))
    expect(got).toEqual([[-12, -18]])
  })

  it('the bar reads rms_dbfs, not peak_dbfs', async () => {
    const h = await makeHarness()
    const got: Array<[number, number]> = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onLevels: (l, r) => got.push([l, r]) })
    await h.drain()
    h.emit(packet([-3, -3], [-20, -24]))
    expect(got).toEqual([[-20, -24]])
  })

  it('a released subscriber stops receiving packets', async () => {
    const h = await makeHarness()
    const got: number[] = []
    const a = h.acquireVuFeed({ deviceName: 'Qu-5', onLevels: (l) => got.push(l) })
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    h.emit(packet([-10, -10]))
    a()
    h.emit(packet([-20, -20]))
    expect(got).toEqual([-10])
  })

  it('one subscriber throwing does not starve the others', async () => {
    const h = await makeHarness()
    const got: number[] = []
    h.acquireVuFeed({
      deviceName: 'Qu-5',
      onLevels: () => {
        throw new Error('render blew up')
      },
    })
    h.acquireVuFeed({ deviceName: 'Qu-5', onLevels: (l) => got.push(l) })
    await h.drain()
    expect(() => h.emit(packet([-9, -9]))).not.toThrow()
    expect(got).toEqual([-9])
  })

  it('levels are delivered even when start_vu never resolved (alternate emitter)', async () => {
    // Phase 5's native pre-roll buffer emits on the very same channel. Liveness
    // is "packets are arriving", not "our own start_vu returned".
    const h = await makeHarness({ autoStart: 'manual' })
    const states: VuFeedState[] = []
    const got: number[] = []
    h.acquireVuFeed({
      deviceName: 'Qu-5',
      onLevels: (l) => got.push(l),
      onState: (s) => states.push(s),
    })
    await h.drain()
    expect(states).not.toContain('live')

    h.emit(packet([-14, -14, -14, -14]))
    expect(got).toEqual([-14])
    expect(states).toContain('live')
  })
})

describe('state reporting', () => {
  it('publishes the negotiated channel count', async () => {
    const h = await makeHarness({ autoStart: 32 })
    const seen: Array<[VuFeedState, number]> = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onState: (s, n) => seen.push([s, n]) })
    await h.drain()
    expect(seen).toContainEqual(['live', 32])
  })

  it('a late joiner is told the current state immediately', async () => {
    const h = await makeHarness({ autoStart: 8 })
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()

    const seen: Array<[VuFeedState, number]> = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onState: (s, n) => seen.push([s, n]) })
    expect(seen[0]).toEqual(['live', 8])
  })

  it('does not re-announce an unchanged state on every packet', async () => {
    const h = await makeHarness({ autoStart: 4 })
    const seen: VuFeedState[] = []
    h.acquireVuFeed({ deviceName: 'Qu-5', onState: (s) => seen.push(s) })
    await h.drain()
    const before = seen.length
    h.emit(packet([-10, -10, -10, -10]))
    h.emit(packet([-11, -11, -11, -11]))
    h.emit(packet([-12, -12, -12, -12]))
    expect(seen.length).toBe(before)
  })
})

describe('recording guard', () => {
  it('does not ask for a metering session while a recording owns the device', async () => {
    const h = await makeHarness()
    ;(globalThis as unknown as { window: { __isRecording: boolean } }).window.__isRecording = true
    h.acquireVuFeed({ deviceName: 'Qu-5' })
    await h.drain()
    expect(h.calls).toEqual([])
  })
})
