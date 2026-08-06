import { describe, expect, it } from 'vitest'
import {
  TRASH_KEEP_DAYS,
  ageText,
  toTrashRows,
  trashedPaths,
  withoutTrashed,
  type TrashEntry,
} from './trash-core'

const DAY = 24 * 60 * 60 * 1000
const NOW = Date.UTC(2026, 7, 6, 12, 0, 0)

function entry(over: Partial<TrashEntry> = {}): TrashEntry {
  return {
    id: 'e1',
    originalPath: '/rec/2026-08-02.mp3',
    trashedPath: '/rec/.sundayrec-trash/1-2026-08-02.mp3',
    name: '2026-08-02.mp3',
    deletedAt: NOW,
    related: [],
    byteSize: 4096,
    ...over,
  }
}

describe('withoutTrashed', () => {
  it('drops exactly the rows whose file is in the trash', () => {
    const rows = [{ path: '/rec/a.mp3' }, { path: '/rec/b.mp3' }, { path: '/rec/a.mp4' }]
    const trashed = trashedPaths([
      entry({ originalPath: '/rec/a.mp3' }),
      entry({ id: 'e2', originalPath: '/rec/a.mp4' }),
    ])
    expect(withoutTrashed(rows, trashed)).toEqual([{ path: '/rec/b.mp3' }])
  })

  it('keeps rows that have no path — a failed run is still yours to tidy', () => {
    const rows = [{ path: undefined }, { path: '/rec/a.mp3' }]
    expect(withoutTrashed(rows, trashedPaths([entry({ originalPath: '/rec/a.mp3' })])))
      .toEqual([{ path: undefined }])
  })

  it('returns the list untouched when the trash is empty', () => {
    const rows = [{ path: '/rec/a.mp3' }]
    expect(withoutTrashed(rows, new Set())).toBe(rows)
  })
})

describe('toTrashRows', () => {
  it('is newest first', () => {
    const rows = toTrashRows(
      [
        entry({ id: 'old', deletedAt: NOW - 5 * DAY }),
        entry({ id: 'new', deletedAt: NOW - 1 * DAY }),
      ],
      NOW,
    )
    expect(rows.map(r => r.id)).toEqual(['new', 'old'])
  })

  it('counts whole days and the days left before the sweep', () => {
    const [row] = toTrashRows([entry({ deletedAt: NOW - 3 * DAY - 1000 })], NOW)
    expect(row.ageDays).toBe(3)
    expect(row.daysLeft).toBe(TRASH_KEEP_DAYS - 3)
  })

  it('never reports a negative age or a negative reprieve', () => {
    // A clock that moved backwards, and an entry the sweep has not reached yet.
    const [future] = toTrashRows([entry({ deletedAt: NOW + DAY })], NOW)
    expect(future.ageDays).toBe(0)
    const [ancient] = toTrashRows([entry({ deletedAt: NOW - 400 * DAY })], NOW)
    expect(ancient.daysLeft).toBe(0)
  })

  it('carries the companion count so the row can say what else went with it', () => {
    const [row] = toTrashRows(
      [
        entry({
          related: [
            { originalPath: '/rec/a.meta.json', trashedPath: '/t/1-a.meta.json' },
            { originalPath: '/rec/a.peaks.json', trashedPath: '/t/1-a.peaks.json' },
          ],
        }),
      ],
      NOW,
    )
    expect(row.relatedCount).toBe(2)
  })
})

describe('ageText', () => {
  const words = { today: 'i dag', yesterday: 'i går', daysAgo: '{n} dager siden' }

  it('reads as a person would say it', () => {
    expect(ageText(0, words)).toBe('i dag')
    expect(ageText(1, words)).toBe('i går')
    expect(ageText(9, words)).toBe('9 dager siden')
  })
})
