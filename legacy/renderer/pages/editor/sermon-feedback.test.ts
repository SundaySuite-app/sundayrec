import { describe, it, expect } from 'vitest'
import type { Suggestion } from './state'
import { autoSermonIndex, buildSermonPickRequest } from './sermon-feedback'

const seg = (start: number, end: number, type: string): Suggestion => ({
  start,
  end,
  duration: end - start,
  label: type,
  type,
})

/** A service: prelude music, a short reading the detector mistook for the
 *  sermon, songs, then the real 25-minute message. */
const service = (): Suggestion[] => [
  seg(0, 300, 'music'),
  seg(300, 480, 'sermon'),
  seg(480, 700, 'music'),
  seg(700, 2200, 'speech'),
  seg(2200, 2400, 'silence'),
]

describe('autoSermonIndex', () => {
  it('finds the block the detector promoted', () => {
    expect(autoSermonIndex(service())).toBe(1)
  })

  it('is null when the detector found no sermon at all', () => {
    expect(autoSermonIndex([seg(0, 300, 'music'), seg(300, 400, 'speech')])).toBeNull()
  })
})

describe('buildSermonPickRequest', () => {
  it('carries both picks and the whole segment list', () => {
    const segments = service()
    const req = buildSermonPickRequest(segments, 1, 3, 2400)
    expect(req.autoIndex).toBe(1)
    expect(req.chosenIndex).toBe(3)
    expect(req.durationSec).toBe(2400)
    // The music/silence blocks travel too — the attention heuristics read them.
    expect(req.segments).toHaveLength(5)
  })

  it('offers exactly what the picker offered', () => {
    // Only speech-like blocks of a minute or more — the same rule the dropdown
    // builds its <option> list from.
    expect(buildSermonPickRequest(service(), 1, 3, 2400).candidateIndices).toEqual([1, 3])
  })

  // ── The trap this module exists for ─────────────────────────────────────────
  //
  // `setSermonSegment` promotes by MUTATING the segment objects. A payload built
  // after that mutation shows the chosen block already labelled `sermon` and the
  // detector's block demoted to `speech` — i.e. it says the detector was right,
  // which is the exact opposite of what just happened.
  it('describes the world as it was BEFORE the promotion', () => {
    const segments = service()
    const req = buildSermonPickRequest(segments, 1, 3, 2400)

    // Now promote exactly as `setSermonSegment` does — in place, on the very
    // objects the request was built from. The IPC call leaves AFTER this.
    segments[1].type = 'speech'
    segments[3].type = 'sermon'

    expect(req.segments[1].type).toBe('sermon')
    expect(req.segments[3].type).toBe('speech')
  })

  it('reports a missing auto-pick as null rather than as an index', () => {
    const segments = [seg(0, 300, 'music'), seg(300, 1800, 'speech')]
    expect(buildSermonPickRequest(segments, null, 1, 2100).autoIndex).toBeNull()
    // -1 is what `findIndex` returns for "not found"; it must never reach the
    // backend, where it would be a `usize` cast of nonsense.
    expect(buildSermonPickRequest(segments, -1, 1, 2100).autoIndex).toBeNull()
  })
})
