/**
 * Pure history logic — pairing, sorting, filtering, totals. No DOM, no IPC.
 *
 * Everything here used to be inline in the render loop, which meant it could
 * only be verified by looking at the table. Two of the four were wrong:
 *
 *  - **Pairing** matched an audio row with a video row by *adjacency* plus
 *    `date === date && startTime === startTime && note === 'Video'`. Under the
 *    Tauri shim `startTime` is always `''` and `date` is a full millisecond ISO
 *    stamp built from each row's own `created_at` — two rows of the same session
 *    are inserted separately, so their stamps differ and the audio/video pair
 *    never collapsed. It also depended on insertion order surviving every
 *    future sort. The real invariant is in the recorder: the separate-audio
 *    sidecar is written as `{stem}.{audio_ext}` next to `{stem}.{video_ext}` in
 *    the same directory (`extract_separate_audio`), so the pair share a base
 *    path. That is the key we use.
 *
 *  - **Total duration** was recovered by regex out of the *display string*
 *    (`"1t 30m"`), so it silently dropped any row whose label didn't match that
 *    exact shape and rounded every row to the minute before summing. The rows
 *    carry `durationSec`; use it.
 */

/** A recording as the renderer consumes it (adapted from the Rust RecordingRow
 *  in the api-shim). */
export interface RecordingEntry {
  date?: string
  startTime?: string
  duration?: string
  filename?: string
  path?: string
  status: string
  timestamp?: number
  note?: string
  fileSizeBytes?: number
  durationSec?: number
}

/** One rendered row: the primary recording plus the video file of the same
 *  session, when there is one. */
export interface PairedRecording {
  r: RecordingEntry
  videoEntry: RecordingEntry | null
}

export type HistorySortKey = 'time' | 'duration'
export type SortDir = 'asc' | 'desc'
export type HistoryFilter = 'all' | 'audio' | 'video'

/** Container extensions that mean "this row is the video half of a session".
 *  Mirrors the api-shim's VIDEO_EXT list. */
const VIDEO_EXTS = new Set([
  'mp4', 'mov', 'mkv', 'm4v', 'webm', 'avi', 'wmv', 'ts', 'mts', 'm2ts', 'flv',
  '3gp', 'asf', 'f4v',
])

/** A recording's base path without its extension — the join key for the
 *  audio/video pair. */
export function baseNoExt(p: string | undefined): string {
  if (!p) return ''
  return p.replace(/\.[^./\\]+$/, '')
}

function extOf(p: string | undefined): string {
  if (!p) return ''
  const m = /\.([^./\\]+)$/.exec(p)
  return m ? m[1].toLowerCase() : ''
}

/** Is this row the video file of a session? Decided by container extension —
 *  `note === 'Video'` is the legacy Electron marker and is kept as a fallback
 *  for rows imported from that history (a user-typed note never reaches here
 *  as an exact match by accident, and if it did the row is still shown). */
export function isVideoRow(r: RecordingEntry): boolean {
  const ext = extOf(r.path)
  if (ext) return VIDEO_EXTS.has(ext)
  return r.note === 'Video'
}

/** Sort value for a row: the numeric timestamp, falling back to a parsed date
 *  string, falling back to 0 (unknown sorts oldest). */
function timeOf(r: RecordingEntry): number {
  if (typeof r.timestamp === 'number' && isFinite(r.timestamp)) return r.timestamp
  const parsed = r.date ? Date.parse(r.date) : NaN
  return isFinite(parsed) ? parsed : 0
}

function durationOf(r: RecordingEntry): number {
  return typeof r.durationSec === 'number' && isFinite(r.durationSec) ? r.durationSec : 0
}

/**
 * Collapse the audio + video files of one recording session into a single row.
 *
 * Rows are keyed by base path (directory + stem, no extension). A group that
 * holds both a non-video and a video row becomes one paired row, anchored at
 * whichever of the two comes first in the incoming order — so the result keeps
 * the caller's sort. A group of video files only stays as ordinary rows, and a
 * group with more than one audio or more than one video pairs the first of
 * each and leaves the rest as their own rows: a recording is never swallowed.
 *
 * Rows without a path get a unique key and therefore never pair — there is
 * nothing to match them on, and guessing from a shared minute would merge two
 * genuinely different recordings.
 */
export function pairRecordings(rows: RecordingEntry[]): PairedRecording[] {
  const groups = new Map<string, number[]>()
  rows.forEach((r, i) => {
    const key = r.path ? `p:${baseNoExt(r.path)}` : `i:${i}`
    const g = groups.get(key)
    if (g) g.push(i)
    else groups.set(key, [i])
  })

  /** anchor row index → the pair rendered there. */
  const pairAt = new Map<number, { audio: number; video: number }>()
  /** rows folded into a pair rendered elsewhere. */
  const folded = new Set<number>()
  for (const idxs of groups.values()) {
    if (idxs.length < 2) continue
    const audio = idxs.find(i => !isVideoRow(rows[i]))
    const video = idxs.find(i => isVideoRow(rows[i]))
    if (audio === undefined || video === undefined) continue
    pairAt.set(Math.min(audio, video), { audio, video })
    folded.add(Math.max(audio, video))
  }

  const out: PairedRecording[] = []
  rows.forEach((r, i) => {
    if (folded.has(i)) return
    const p = pairAt.get(i)
    out.push(p ? { r: rows[p.audio], videoEntry: rows[p.video] } : { r, videoEntry: null })
  })
  return out
}

/** Rows matching the chip filter. Runs BEFORE pairing, so «Video» shows the
 *  video files themselves and «Lyd» the audio ones — a paired session appears
 *  under whichever half the filter asked for. */
export function filterRecordings(
  rows: RecordingEntry[],
  filter: HistoryFilter,
): RecordingEntry[] {
  switch (filter) {
    case 'audio': return rows.filter(r => !isVideoRow(r))
    case 'video': return rows.filter(r => isVideoRow(r))
    default:      return rows.slice()
  }
}

/** Sorted copy. Stable (ties keep their incoming order) and never mutates. */
export function sortRecordings(
  rows: RecordingEntry[],
  key: HistorySortKey,
  dir: SortDir,
): RecordingEntry[] {
  const value = key === 'duration' ? durationOf : timeOf
  const sign = dir === 'asc' ? 1 : -1
  return rows
    .map((r, i) => ({ r, i }))
    .sort((a, b) => {
      const d = value(a.r) - value(b.r)
      return d !== 0 ? d * sign : a.i - b.i
    })
    .map(x => x.r)
}

/** Summary of the rows the table is showing. Durations come from `durationSec`,
 *  never from the formatted `duration` label. */
export interface HistoryTotals {
  count: number
  totalSec: number
  /** Newest `date` among the counted rows, or undefined when none has one. */
  lastDate?: string
}

/** Totals over the successful recordings only — a failed run has no file and
 *  no meaningful duration, so counting it would inflate both numbers. */
export function historyTotals(rows: RecordingEntry[]): HistoryTotals {
  const ok = rows.filter(r => r.status === 'ok' || r.status === 'complete')
  let totalSec = 0
  let lastTime = -Infinity
  let lastDate: string | undefined
  for (const r of ok) {
    totalSec += durationOf(r)
    const ts = timeOf(r)
    if (r.date && ts > lastTime) { lastTime = ts; lastDate = r.date }
  }
  return { count: ok.length, totalSec, lastDate }
}
