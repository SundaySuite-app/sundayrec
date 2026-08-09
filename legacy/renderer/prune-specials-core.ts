/**
 * Renderer-side mirror of the scheduler's specials prune (R3-D, interim).
 *
 * The Rust scheduler prunes one-off recordings whose stop time ended more than
 * 7 days ago from its sqlite settings (`sundayrec_core::schedule::
 * prune_specials`, scheduler/mod.rs supervisor loop). But the renderer's
 * settings live in localStorage, and every save sends the UNPRUNED
 * `specialRecordings` list back through `settings_save` — silently
 * resurrecting what the backend just pruned. Until R4 makes sqlite the single
 * owner of the list, the honest interim fix is to apply the SAME rule where
 * localStorage is read (`loadSettings` in api-shim.ts), so a save can no
 * longer resurrect an expired special.
 *
 * The rule mirrors the Rust one field for field:
 *   - keep when `date` (YYYY-MM-DD) + `stop` (HH:MM, fallback 12:00 —
 *     `prune_specials` passes `(12, 0)`) is >= now − 7 days;
 *   - keep MALFORMED entries (unparseable date) — pruning is housekeeping,
 *     never data loss;
 *   - the 7-day threshold is `Duration::days(7)` in schedule.rs — keep the two
 *     in lockstep.
 */

/** Mirrors `Duration::days(7)` in `sundayrec_core::schedule::prune_specials`. */
export const SPECIALS_PRUNE_DAYS = 7

/** `HH:MM` → minutes, with the same range checks as schedule.rs `parse_hm`. */
function parseHm(s: unknown, fallback: number): number {
  if (typeof s !== 'string' || !s.trim()) return fallback
  const [hRaw, mRaw] = s.split(':')
  const h = Number.parseInt((hRaw ?? '').trim(), 10)
  const m = Number.parseInt((mRaw ?? '').trim(), 10)
  if (Number.isInteger(h) && Number.isInteger(m) && h >= 0 && h < 24 && m >= 0 && m < 60) {
    return h * 60 + m
  }
  return fallback
}

/** The special's stop as epoch ms (local wall clock), or null when malformed. */
function stopMs(entry: Record<string, unknown>): number | null {
  const date = entry.date
  if (typeof date !== 'string') return null
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date.trim())
  if (!m) return null
  const [, y, mo, d] = m
  const minutes = parseHm(entry.stop, 12 * 60)
  const dt = new Date(Number(y), Number(mo) - 1, Number(d), 0, minutes)
  // new Date() normalises out-of-range components (2026-02-31 → March 3rd);
  // Rust's chrono refuses them (→ kept as malformed). Match chrono.
  if (dt.getFullYear() !== Number(y) || dt.getMonth() !== Number(mo) - 1 || dt.getDate() !== Number(d)) {
    return null
  }
  return dt.getTime()
}

/**
 * Drop specials that stopped more than [`SPECIALS_PRUNE_DAYS`] before `nowMs`;
 * keep everything else, malformed entries included. Returns the input array
 * itself when nothing is pruned (cheap steady state for every settings load).
 */
export function pruneEndedSpecials(specials: unknown, nowMs: number): unknown {
  if (!Array.isArray(specials)) return specials
  const threshold = nowMs - SPECIALS_PRUNE_DAYS * 24 * 60 * 60 * 1000
  const kept = specials.filter((entry) => {
    if (!entry || typeof entry !== 'object') return true
    const stop = stopMs(entry as Record<string, unknown>)
    return stop === null || stop >= threshold
  })
  return kept.length === specials.length ? specials : kept
}
