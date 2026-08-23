import { PLAYER_EXTS } from './state'

// ── Extended-timeline region math (PURE — no DOM, no state) ─────────────────
//
// The editor plays three pieces of media back to back: the intro jingle, the
// recording itself, and the outro jingle. They live on ONE extended timeline
// whose origin is the start of the recording:
//
//   -introDur ............ 0 ................... duration ..... +outroDur
//   |<---- intro ------->|<------ main ------->|<--- outro --->|
//
// `playStartSec` and everything the user sees (playhead, timecode, ruler) are
// in these extended seconds; each MEDIA source, however, only knows its own
// 0-based offset. Getting that mapping wrong is how the playhead and the audio
// drift apart, so the mapping lives here as pure functions with tests instead
// of being re-derived inline in the play/seek/stop paths.

export type PlaybackRegion = 'intro' | 'main' | 'outro'

export interface RegionTimeline {
  /** Length of the main recording, in seconds. */
  duration: number
  /** Intro jingle length — 0 when there is none, or it is excluded from export. */
  introDur: number
  /** Outro jingle length — 0 when there is none, or it is excluded from export. */
  outroDur: number
}

export interface RegionPos {
  region: PlaybackRegion
  /** Offset into that region's OWN media, from its start. Always ≥ 0. */
  offset: number
}

/** Earliest playable extended second (start of the intro, or 0). */
export function timelineStart(tl: RegionTimeline): number {
  const introDur = Math.max(0, tl.introDur)
  // `-0` would compare equal to 0 but survives into `Object.is` checks and
  // formatted timecodes as "-0:00". Keep the no-intro case a plain zero.
  return introDur > 0 ? -introDur : 0
}

/** Last playable extended second (end of the outro, or the recording's end). */
export function timelineEnd(tl: RegionTimeline): number {
  return tl.duration + Math.max(0, tl.outroDur)
}

export function clampToTimeline(sec: number, tl: RegionTimeline): number {
  return Math.max(timelineStart(tl), Math.min(timelineEnd(tl), sec))
}

/**
 * Which region an extended-timeline second falls in, and how far into that
 * region's own media it is. A region with zero duration does not exist, so a
 * negative second with no intro resolves to the start of `main` rather than to
 * a region that has nothing to play.
 */
export function resolvePosition(sec: number, tl: RegionTimeline): RegionPos {
  const introDur = Math.max(0, tl.introDur)
  const outroDur = Math.max(0, tl.outroDur)
  const duration = Math.max(0, tl.duration)

  if (introDur > 0 && sec < 0) {
    // -introDur → offset 0 (jingle start); just-below-0 → nearly its end.
    return { region: 'intro', offset: Math.max(0, Math.min(introDur, introDur + sec)) }
  }
  if (outroDur > 0 && sec > duration) {
    return { region: 'outro', offset: Math.max(0, Math.min(outroDur, sec - duration)) }
  }
  return { region: 'main', offset: Math.max(0, Math.min(duration, sec)) }
}

/** Inverse of [`resolvePosition`]: a region-local offset back to extended seconds. */
export function regionPosToTimeline(region: PlaybackRegion, offset: number, tl: RegionTimeline): number {
  if (region === 'intro') return timelineStart(tl) + offset
  if (region === 'outro') return Math.max(0, tl.duration) + offset
  return offset
}

/**
 * What plays when `region` reaches its natural end — `null` means "playback is
 * over, stop". Intro always hands over to the recording (a jingle without a
 * recording is not a thing the editor can open); the recording hands over to the
 * outro only when there is one.
 */
export function nextRegion(region: PlaybackRegion, tl: RegionTimeline): PlaybackRegion | null {
  if (region === 'intro') return 'main'
  if (region === 'main') return Math.max(0, tl.outroDur) > 0 ? 'outro' : null
  return null
}

/**
 * How the MAIN recording is played back: straight from the original file
 * (`element` — the webview streams it via `asset://`, full fidelity, instant),
 * or through a transcoded AAC proxy (`proxy` — costs a transcode up front, so
 * only for containers the webview has no decoder for).
 *
 * Accepts `flac`, `.flac`, `.FLAC` alike: extensions reach us from filenames,
 * drag-and-drop payloads and settings, and the casing is not ours to trust.
 */
export function routePlayback(ext: string): 'element' | 'proxy' {
  const norm = ext.trim().toLowerCase()
  const dotted = norm.startsWith('.') ? norm : `.${norm}`
  return PLAYER_EXTS.has(dotted) ? 'element' : 'proxy'
}
