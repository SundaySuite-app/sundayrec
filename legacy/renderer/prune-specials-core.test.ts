// R3-D: the renderer half of the specials prune — the rule that stops a
// settings save from resurrecting one-off recordings the backend scheduler
// already pruned from sqlite (`sundayrec_core::schedule::prune_specials`).

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { pruneEndedSpecials, SPECIALS_PRUNE_DAYS } from './prune-specials-core'

const HERE = dirname(fileURLToPath(import.meta.url))

// A fixed "now": 2026-08-09 10:00 local.
const NOW = new Date(2026, 7, 9, 10, 0).getTime()
const DAY = 24 * 60 * 60 * 1000

function special(date: string, stop: string): Record<string, unknown> {
  return { id: 'x', date, name: 'Dåp', start: '10:00', stop }
}

describe('pruneEndedSpecials', () => {
  it('drops a special that stopped more than 7 days ago (the resurrection bug)', () => {
    // Stop 2026-07-20 12:00 — long gone. Before R3-D, loadSettings returned
    // this entry untouched and the next settings_save re-sent it to the
    // backend the scheduler had just pruned it from.
    const out = pruneEndedSpecials([special('2026-07-20', '12:00')], NOW)
    expect(out).toEqual([])
  })

  it('keeps recent, future and boundary specials — same 7-day rule as Rust', () => {
    const recent = special('2026-08-05', '12:00') // 4 days ago → keep
    const future = special('2026-08-20', '12:00')
    const kept = pruneEndedSpecials([recent, future], NOW)
    expect(kept).toEqual([recent, future])
    // Exactly at the threshold (stop == now − 7d) is KEPT (Rust: `stop >= threshold`).
    const atThreshold = special('2026-08-02', '10:00')
    expect(new Date(2026, 7, 2, 10, 0).getTime()).toBe(NOW - SPECIALS_PRUNE_DAYS * DAY)
    expect(pruneEndedSpecials([atThreshold], NOW)).toEqual([atThreshold])
  })

  it('keeps malformed entries and applies the 12:00 stop fallback, like Rust', () => {
    // Unparseable dates are housekeeping-exempt (prune must never eat data)…
    const malformed = [special('not-a-date', '12:00'), special('2026-02-31', '10:00'), { id: 'y' }]
    expect(pruneEndedSpecials([...malformed], NOW)).toEqual(malformed)
    // …and a garbage stop time falls back to 12:00 (`parse_hm` fallback (12,0)):
    // 2026-08-02 with stop 12:00 is INSIDE the window at NOW (threshold 02.08 10:00).
    expect(pruneEndedSpecials([special('2026-08-02', 'zz:zz')], NOW)).toHaveLength(1)
    // A non-array (corrupt blob) passes through untouched.
    expect(pruneEndedSpecials('junk', NOW)).toBe('junk')
  })

  it('is wired into loadSettings, so a save cannot resurrect pruned specials', () => {
    // api-shim.ts cannot be imported here (module-scope localStorage access),
    // so pin the wiring the display_ratchet way: the prune must run inside
    // loadSettings — the single place every getSettings/save round-trip reads
    // localStorage through.
    const source = readFileSync(join(HERE, 'api-shim.ts'), 'utf8')
    const fnStart = source.indexOf('function loadSettings')
    expect(fnStart, 'loadSettings not found in api-shim.ts').toBeGreaterThan(-1)
    const fnEnd = source.indexOf('\n}', fnStart)
    const body = source.slice(fnStart, fnEnd)
    expect(body).toContain('pruneEndedSpecials(')
  })
})
