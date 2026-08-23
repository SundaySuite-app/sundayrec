import { E, $, playbackMediaEl, currentPlaybackSec } from './state'
import { clampMain, clampPlayable, effIntroDur, effOutroDur } from './geometry'
import { sharedAudioCtx } from './audio-ctx'
import { nextRegion, regionPosToTimeline, resolvePosition, type RegionTimeline } from './play-regions'
import { formatTime } from './format'
import { drawWaveform, updateMinimapViewport } from './waveform'
import { DRAW_INTERVAL_MS, nextDrawGate } from '../../ui/frame-gate'
import { snapOutOfCut } from './canvas-input'

// ── Playback engine, seek/scroll + timecode display ─────────────────────────
//
// The MAIN recording always plays through a media element streaming from disk
// (`state.playerEl` for audio, `E.videoEl` for video) — never a decoded buffer.
// The intro/outro jingles are AudioBuffers on the shared context, scheduled
// around it. So a play spanning all three regions is a small state machine:
//
//   intro buffer  --onended-->  element.play()  --'ended'-->  outro buffer
//
// The region math itself is pure and lives in play-regions.ts; this file owns
// only the transitions and the DOM.
//
// What this does NOT do any more: apply `audioGainDb` to playback. The old
// Web-Audio path had a gain node, the element path silently had none, so
// Normalize was audible for small files and inaudible for large ones. Now it is
// consistently an EXPORT-time filter and the UI says so (editor.normalizeExportHint).

/** The current extended timeline: recording length + whichever jingles are
 *  actually in play (excluded ones have zero length and therefore don't exist). */
function timeline(): RegionTimeline {
  return { duration: E.duration, introDur: effIntroDur(), outroDur: effOutroDur() }
}

/**
 * Move a media element's position, tolerating a not-yet-open resource. WebKit
 * throws `InvalidStateError` when `currentTime` is written while readyState is
 * HAVE_NOTHING — harmless in the old design (the element only ever held a video
 * that was already open), but now every seek in the editor lands on a streamed
 * <audio> that may still be opening. An exception here would abort whatever
 * handler made the seek, taking the playhead redraw down with it.
 */
export function seekMediaEl(el: HTMLMediaElement, sec: number): void {
  try { el.currentTime = sec } catch { /* resource not open yet — playStartSec still holds the target */ }
}

/** Point the active transport at `mainSec`, if there is one. `E.playStartSec`
 *  remains the source of truth, so a seek that can't land yet is not lost. */
export function seekMediaTo(mainSec: number): void {
  const el = playbackMediaEl()
  if (el) seekMediaEl(el, mainSec)
}

/** Move playhead to an absolute extended-timeline second, stopping any active
 *  playback. Centralises the seek-and-redraw logic used by keyboard shortcuts.
 *  Snaps out of cuts so keyboard navigation always lands on playable audio. */
export function seekTo(sec: number): void {
  stopPlay()
  E.playStartSec = snapOutOfCut(clampPlayable(sec))
  updateTimecode(E.playStartSec)
  seekMediaTo(clampMain(E.playStartSec))
  const mainPlayhead = clampMain(E.playStartSec)
  if (mainPlayhead < E.vpStart || mainPlayhead > E.vpEnd) {
    const span = E.vpEnd - E.vpStart
    E.vpStart = Math.max(0, mainPlayhead - span * 0.3)
    E.vpEnd   = Math.min(E.duration, E.vpStart + span)
    updateMinimapViewport()
  }
  drawWaveform()
}

/** Jump playhead to the next/previous cut boundary. Direction = +1 forward,
 *  -1 backward. Considers both cut start and end so each cut counts as two
 *  navigation stops. */
export function jumpToCutBoundary(dir: 1 | -1): void {
  if (E.cuts.length === 0) return
  const ph = clampMain(E.playStartSec)
  const points: number[] = []
  for (const c of E.cuts) { points.push(c.start, c.end) }
  points.sort((a, b) => a - b)
  let target: number | null = null
  if (dir > 0) {
    target = points.find(p => p > ph + 0.05) ?? null
  } else {
    for (let i = points.length - 1; i >= 0; i--) {
      if (points[i] < ph - 0.05) { target = points[i]; break }
    }
  }
  if (target == null) return
  seekTo(target)
}

export function seekBy(secs: number): void {
  stopPlay()
  E.playStartSec = clampPlayable(E.playStartSec + secs)
  updateTimecode(E.playStartSec)
  seekMediaTo(clampMain(E.playStartSec))
  // Pan viewport when playhead drops out of view. Viewport itself stays in
  // main coords — intro/outro live in their own slots and always remain
  // visible when at the recording edge.
  const mainPlayhead = clampMain(E.playStartSec)
  if (mainPlayhead < E.vpStart || mainPlayhead > E.vpEnd) {
    const half = (E.vpEnd - E.vpStart) / 2
    E.vpStart = Math.max(0, mainPlayhead - half)
    E.vpEnd   = Math.min(E.duration, E.vpStart + half * 2)
    updateMinimapViewport()
  }
  drawWaveform()
}

export function autoScrollToPlayhead(curSec: number): void {
  // Auto-scroll only operates inside the main recording. Intro/outro slots
  // are fixed and always visible at their respective edges.
  if (curSec < 0 || curSec > E.duration) return
  const span = E.vpEnd - E.vpStart
  if (curSec > E.vpEnd - span * 0.1) {
    E.vpStart = curSec - span * 0.05
    E.vpEnd   = E.vpStart + span
    if (E.vpEnd > E.duration) { E.vpEnd = E.duration; E.vpStart = Math.max(0, E.duration - span) }
    updateMinimapViewport()
  }
}

export function togglePlay(preview: boolean): void {
  if (E.isPlaying && E.isPreview === preview) { stopPlay(); return }
  stopPlay()
  startPlay(preview)
}

// Track the current onEnded listener so we can remove it on manual stopPlay.
// Without removal, repeated start/stop accumulates dead listeners on the media
// element (the once:true flag fires-and-removes, but only when the event
// actually fires — manual stop never fires it). Covers both transports: the
// video element and the streamed audio proxy.
let mediaEndedHandler: (() => void) | null = null

export function attachMediaEndedHandler(onEnded: () => void): void {
  const el = playbackMediaEl()
  if (!el) return
  if (mediaEndedHandler) el.removeEventListener('ended', mediaEndedHandler)
  mediaEndedHandler = onEnded
  el.addEventListener('ended', onEnded, { once: true })
}

export function detachMediaEndedHandler(): void {
  const el = playbackMediaEl()
  if (el && mediaEndedHandler) {
    el.removeEventListener('ended', mediaEndedHandler)
    mediaEndedHandler = null
  }
}

// Every start/stop bumps this. Region transitions are asynchronous — a jingle's
// `onended`, an element's `canplay` — and a callback from a SUPERSEDED playback
// must not act: pressing stop then play within the same second used to leave the
// old file's pending `canplay` running, which then started the element on top of
// the new intro jingle. Callbacks capture their generation and check it.
let playGen = 0

/** Stop and release a sounding jingle, if any. Idempotent. */
function stopJingle(): void {
  const node = E.jingleSource
  E.jingleSource = null
  E.jingleRegion = null
  if (node) {
    node.onended = null
    try { node.stop() } catch { /* already stopped */ }
    try { node.disconnect() } catch {}
  }
}

/**
 * Sound one jingle from `offset` seconds in, calling `onDone` when it reaches
 * its natural end (never when we stop it ourselves — `stopJingle` clears the
 * handler first). Returns false when there is nothing left to play, so the
 * caller can move straight on to the next region instead of hanging.
 */
function playJingle(region: 'intro' | 'outro', offset: number, gen: number, onDone: () => void): boolean {
  const buffer = region === 'intro' ? E.introBuffer : E.outroBuffer
  if (!buffer) return false
  const playDur = buffer.duration - offset
  if (playDur <= 0.01) return false

  const ctx = sharedAudioCtx()
  const node = ctx.createBufferSource()
  node.buffer = buffer
  node.connect(ctx.destination)
  node.onended = () => {
    // A stop we initiated clears jingleSource first, so this only fires on a
    // genuine end-of-jingle.
    if (gen !== playGen || E.jingleSource !== node) return
    E.jingleSource = null
    E.jingleRegion = null
    onDone()
  }
  E.jingleSource      = node
  E.jingleRegion      = region
  E.playStartCtxTime  = ctx.currentTime
  E.playStartSec      = regionPosToTimeline(region, offset, timeline())
  node.start(0, offset)
  return true
}

/**
 * Start the element at `mainSec`. The element may still be opening the file
 * (readyState 0 on a cold external drive, or right after `src` was assigned), in
 * which case we seek+play on `canplay` instead of dropping the request — the
 * play button used to look dead when pressed within a second of opening a file.
 */
function playMainFrom(mainSec: number, gen: number): void {
  const el = playbackMediaEl()
  // No transport at all — no element, or one whose source could never be
  // prepared (the proxy transcode failed too). End cleanly rather than spinning
  // the animation loop against an element that will never advance: the play icon
  // would sit on "pause" while nothing happened.
  if (!el || !el.getAttribute('src')) { finishPlayback(gen); return }

  E.playStartSec = regionPosToTimeline('main', clampMain(mainSec), timeline())
  E.mainPlayPending = true

  const start = () => {
    if (gen !== playGen || !E.isPlaying) return
    E.mainPlayPending = false
    seekMediaEl(el, clampMain(E.playStartSec))
    el.play().catch(() => {})
  }

  // On natural end, hand over to the outro (or loop / stop).
  attachMediaEndedHandler(() => {
    mediaEndedHandler = null
    if (gen !== playGen || !E.isPlaying) return
    if (nextRegion('main', timeline()) === 'outro' && playJingle('outro', 0, gen, () => finishPlayback(gen))) return
    finishPlayback(gen)
  })

  if (el.readyState >= 1) { start(); return }
  el.addEventListener('canplay', start, { once: true })
}

/** End-of-playback: loop back to where the user pressed play, or stop cleanly. */
function finishPlayback(gen: number): void {
  if (gen !== playGen || !E.isPlaying) return
  if (E.isLooping) {
    const preview = E.isPreview
    const from = E.loopStartSec
    stopPlay()
    E.playStartSec = from
    startPlay(preview)
    return
  }
  stopJingle()
  E.mainPlayPending = false
  E.isPlaying = false
  cancelAnimationFrame(E.rafId)
  updatePlayIcon()
  drawWaveform()
}

export function startPlay(preview: boolean): void {
  // Never leave a previous region sounding underneath a new one.
  const gen = ++playGen
  stopJingle()
  cancelDuck()

  // If the playhead has ended up inside a cut (e.g. an arrow-key seek landed
  // there), snap it to the cut's end first so audio and playhead agree from the
  // very first frame.
  E.playStartSec = snapOutOfCut(clampPlayable(E.playStartSec))

  E.isPreview       = preview
  E.loopStartSec    = E.playStartSec
  E.isPlaying       = true
  E.mainPlayPending = false

  const pos = resolvePosition(E.playStartSec, timeline())

  if (pos.region === 'intro') {
    // Intro first, then the recording from its very start.
    if (!playJingle('intro', pos.offset, gen, () => playMainFrom(0, gen))) playMainFrom(0, gen)
  } else if (pos.region === 'outro') {
    if (!playJingle('outro', pos.offset, gen, () => finishPlayback(gen))) { finishPlayback(gen); return }
  } else {
    playMainFrom(pos.offset, gen)
  }

  if (!E.isPlaying) return
  updatePlayIcon()
  resetWaveformDrawGate()
  animate()
}

export function stopPlay(): void {
  playGen++                 // invalidate every pending region transition
  detachMediaEndedHandler()
  cancelDuck()
  // Read the position BEFORE tearing anything down — and only when playback was
  // actually running. A pending element reports currentTime 0, and writing that
  // back to playStartSec silently threw the user's seek away.
  const wasPlaying = E.isPlaying
  const pos = wasPlaying ? currentPlaybackSec() : E.playStartSec
  stopJingle()
  E.mainPlayPending = false
  const mediaEl = playbackMediaEl()
  if (mediaEl) {
    try { mediaEl.pause() } catch {}
    mediaEl.volume = 1
  }
  if (wasPlaying) E.playStartSec = clampPlayable(pos)
  E.isPlaying = false
  cancelAnimationFrame(E.rafId)
  updatePlayIcon()
  drawWaveform()
}

// Cap the per-frame full-canvas redraw to ~30 fps during playback. The redraw
// is the expensive part of the loop; the playhead is imperceptible at 30 vs
// 60 fps, but halving it frees the main thread (smoother editor, esp. on long
// files). Only the playback loop is gated — direct drawWaveform() calls
// (seek, edits, stop) still draw immediately.
//
// The gate is the ACCUMULATOR from ui/frame-gate.ts, not the boundary check
// this used to be. `if (now - last >= 33) last = now` re-phases on the jitter of
// whichever frame happens to cross the line, so with a 16.7 ms rAF it admits
// draws 33 ms / 50 ms / 33 ms / 50 ms apart — and a playhead moving at constant
// speed drawn on an alternating cadence is exactly what "not smooth" looks
// like. The accumulator advances by one whole interval instead.
let waveformDrawGate = 0
function drawWaveformThrottled(): void {
  const next = nextDrawGate(waveformDrawGate, performance.now())
  if (next === waveformDrawGate) return
  waveformDrawGate = next
  drawWaveform()
}

/** Re-phase the draw gate to now — called when playback starts so the first
 *  frame of a take always paints instead of waiting out a stale gate. */
function resetWaveformDrawGate(): void {
  waveformDrawGate = performance.now() - DRAW_INTERVAL_MS
}

// ── Cut-preview de-click ────────────────────────────────────────────────────
//
// Preview playback jumps the element over each cut. A hard `currentTime` jump
// lands mid-waveform on both sides, and the resulting step in the signal is an
// audible click on every single cut — enough to make a clean edit sound broken.
// Fading out over a few frames, jumping, then fading back in turns the step into
// a ~50 ms dip nobody hears as a defect.
//
// `volume` is the only knob available: media elements allow ATTENUATION only
// (0..1), which is exactly what a de-click needs. Frame-counted rather than
// time-based so it costs nothing but the rAF we are already running.
const DUCK_FRAMES = 3
let duckToken = 0
let ducking = false

/** Abandon any in-flight ramp and restore full volume. Called from stop/start so
 *  a ramp can never outlive the playback it belongs to (and leave the element
 *  silent for the next play). */
function cancelDuck(): void {
  duckToken++
  ducking = false
  const el = playbackMediaEl()
  if (el) el.volume = 1
}

/** Fade out → jump to `targetSec` → fade back in. */
function previewSkipTo(el: HTMLMediaElement, targetSec: number): void {
  const token = ++duckToken
  ducking = true
  let frame = 0

  const fadeIn = () => {
    if (token !== duckToken) return          // superseded: cancelDuck already restored volume
    frame++
    el.volume = Math.min(1, frame / DUCK_FRAMES)
    if (frame < DUCK_FRAMES) { requestAnimationFrame(fadeIn); return }
    el.volume = 1
    ducking = false
  }

  const fadeOut = () => {
    if (token !== duckToken) return
    frame++
    el.volume = Math.max(0, 1 - frame / DUCK_FRAMES)
    if (frame < DUCK_FRAMES) { requestAnimationFrame(fadeOut); return }
    try { el.currentTime = targetSec } catch {}
    E.playStartSec = targetSec
    frame = 0
    requestAnimationFrame(fadeIn)
  }

  requestAnimationFrame(fadeOut)
}

export function animate(): void {
  if (!E.isPlaying) return

  const curSec = currentPlaybackSec()

  // Preview mode: skip over cut regions. Only while the RECORDING is playing —
  // cuts live in main coords and the jingles have none. Suppressed during a ramp
  // so the fade-out isn't restarted every frame by the cut it is escaping.
  const mediaEl = playbackMediaEl()
  if (E.isPreview && mediaEl && !E.jingleRegion && !E.mainPlayPending && !ducking) {
    const insideCut = E.cuts.find(c => curSec >= c.start && curSec < c.end)
    if (insideCut) previewSkipTo(mediaEl, insideCut.end)
  }

  updateTimecode(curSec)
  autoScrollToPlayhead(curSec)
  drawWaveformThrottled()
  E.rafId = requestAnimationFrame(animate)
}

export function updatePlayIcon(): void {
  const icon       = $('editor-play-icon')
  const previewBtn = $('btn-editor-preview')
  const canvasWrap = $('editor-canvas-wrap')
  if (!icon) return
  if (E.isPlaying && !E.isPreview) {
    icon.innerHTML = '<rect x="5" y="4" width="4" height="12" rx="1"/><rect x="11" y="4" width="4" height="12" rx="1"/>'
    previewBtn?.classList.remove('is-playing')
    canvasWrap?.classList.add('is-playing')
  } else if (E.isPlaying && E.isPreview) {
    icon.innerHTML = '<path d="M6.3 4.6a1 1 0 011.4 0l6 5a1 1 0 010 1.6l-6 5A1 1 0 016 15.4V4.6z"/>'
    previewBtn?.classList.add('is-playing')
    canvasWrap?.classList.add('is-playing')
  } else {
    icon.innerHTML = '<path d="M6.3 4.6a1 1 0 011.4 0l6 5a1 1 0 010 1.6l-6 5A1 1 0 016 15.4V4.6z"/>'
    canvasWrap?.classList.remove('is-playing')
    previewBtn?.classList.remove('is-playing')
  }
}

export function updateTimecode(sec: number): void {
  const el = $('editor-time-cur')
  if (!el) return
  // Show "Intro 0:12" / "Outro 0:05" prefix when playhead is in those slots
  // so the user can see at a glance where they are on the extended timeline.
  if (sec < 0 && effIntroDur() > 0) {
    el.textContent = `Intro ${formatTime(sec + effIntroDur())}`
  } else if (sec > E.duration && effOutroDur() > 0) {
    el.textContent = `Outro ${formatTime(sec - E.duration)}`
  } else {
    el.textContent = formatTime(Math.max(0, Math.min(sec, E.duration)))
  }
}

export function updateTotalTime(): void {
  const el = $('editor-time-tot')
  if (el) el.textContent = formatTime(E.duration)
}
