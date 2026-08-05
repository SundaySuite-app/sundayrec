import { E } from './state'
import { xToSec, xToMainSec, secToX, clampMain, clampPlayable, maxPlayableSec } from './geometry'
import { addCut, deleteCut, pushCutHistory, renderCutList, updateRemainingDisplay } from './cuts'
import { drawWaveform, scheduleDrawWaveform, scheduleViewportDraw, drawMinimap } from './waveform'
import { stopPlay, updateTimecode, seekMediaTo } from './playback'
import { shouldShowSegment } from './detection'
import { panBy } from './viewport'

// ── Canvas mouse/wheel input + minimap drag ─────────────────────────────────

// Every handler here needs the canvas' position on screen, and every one of
// them used to ask the layout engine for it — getBoundingClientRect forces a
// style+layout flush, and mousemove fires as fast as the mouse reports (125 Hz
// on plenty of hardware) with wheel events on a trackpad faster still. The rect
// only changes when the canvas moves or resizes, so it is cached and
// invalidated by exactly those events (ResizeObserver on the canvas — the same
// prior art as audio/waveform.ts — plus scroll/resize on the window).
let cachedRect: DOMRect | null = null
let rectWatchTarget: HTMLElement | null = null
let rectObserver: ResizeObserver | null = null

export function invalidateCanvasRect(): void {
  cachedRect = null
}

function watchCanvasRect(canvas: HTMLElement): void {
  if (rectWatchTarget === canvas) return
  rectWatchTarget = canvas
  rectObserver?.disconnect()
  // Not every environment implements ResizeObserver (jsdom in tests, ancient
  // WebKit); the scroll/resize listeners below still keep the cache honest.
  if (typeof ResizeObserver === 'function') {
    rectObserver = new ResizeObserver(invalidateCanvasRect)
    rectObserver.observe(canvas)
  }
}

let rectListenersBound = false
function bindRectInvalidation(): void {
  if (rectListenersBound) return
  rectListenersBound = true
  window.addEventListener('resize', invalidateCanvasRect)
  // Capture phase: the editor lives inside #main, which is the element that
  // actually scrolls — a listener on window alone would never hear it.
  document.addEventListener('scroll', invalidateCanvasRect, { capture: true, passive: true })
}

/** The canvas' viewport rect, measured at most once per layout change. */
function canvasRect(): DOMRect {
  if (rectWatchTarget !== E.canvas) {
    // The editor page can be torn down and rebuilt with a fresh canvas.
    cachedRect = null
    watchCanvasRect(E.canvas)
  }
  if (cachedRect) return cachedRect
  bindRectInvalidation()
  cachedRect = E.canvas.getBoundingClientRect()
  return cachedRect
}

export function onCanvasDown(e: MouseEvent): void {
  if (!E.peaks || e.button !== 0) return
  // One fresh measurement per interaction: a drag that starts from a stale rect
  // would seek to the wrong second, and that is not a cost worth saving.
  invalidateCanvasRect()
  const rect = canvasRect()
  const extSec  = xToSec(e.clientX - rect.left, rect.width)
  const mainSec = xToMainSec(e.clientX - rect.left, rect.width)

  // Check if clicking near a cut boundary → start handle drag. Cut handles
  // only live in main coords, so this uses mainSec.
  const threshold = (E.vpEnd - E.vpStart) / rect.width * 10
  for (let i = 0; i < E.cuts.length; i++) {
    if (Math.abs(mainSec - E.cuts[i].start) < threshold) {
      E.handleDrag = { cutIdx: i, side: 'start' }
      return
    }
    if (Math.abs(mainSec - E.cuts[i].end) < threshold) {
      E.handleDrag = { cutIdx: i, side: 'end' }
      return
    }
  }

  // Check if clicking near playhead in the ruler area → playhead drag
  const yInCanvas = e.clientY - rect.top
  const playX = secToX(E.playStartSec, rect.width)
  if (Math.abs(e.clientX - rect.left - playX) < 12 && yInCanvas < 28) {
    E.playheadDragging = true
    stopPlay()
    return
  }

  // Normal drag to create cut — drag coords are clamped to main, since cuts
  // can only exist inside the recording.
  E.dragStartSec = clampMain(extSec)
  E.dragEndSec   = E.dragStartSec
  E.isDragging   = true
}

export function onCanvasMove(e: MouseEvent): void {
  if (!E.peaks) return
  const rect = canvasRect()
  const extSec  = xToSec(e.clientX - rect.left, rect.width)
  const mainSec = xToMainSec(e.clientX - rect.left, rect.width)

  // Handle drag: resize cut boundary. Snap to nearby segment boundaries when
  // shift is NOT held — gives precise lock-in to detected speech/music edges.
  // Repaints are rAF-coalesced so 60+ mousemoves/sec only redraw ~60 times.
  if (E.handleDrag) {
    const c = E.cuts[E.handleDrag.cutIdx]
    const snapped = e.shiftKey ? mainSec : snapToSegmentBoundary(mainSec, rect.width)
    if (E.handleDrag.side === 'start') {
      c.start = Math.max(0, Math.min(c.end - 0.1, snapped))
    } else {
      c.end   = Math.min(E.duration, Math.max(c.start + 0.1, snapped))
    }
    updateRemainingDisplay()
    scheduleDrawWaveform()
    return
  }

  // Playhead drag — covers full extended timeline (intro/main/outro)
  if (E.playheadDragging) {
    E.playStartSec = clampPlayable(extSec)
    updateTimecode(E.playStartSec)
    seekMediaTo(clampMain(E.playStartSec))
    scheduleDrawWaveform()
    return
  }

  E.hoverSec = extSec

  // Cursor feedback
  const threshold = (E.vpEnd - E.vpStart) / rect.width * 10
  const nearBoundary = E.cuts.some(c =>
    Math.abs(mainSec - c.start) < threshold || Math.abs(mainSec - c.end) < threshold
  )
  const overCut = E.cuts.some(c => mainSec >= c.start && mainSec <= c.end)
  const nearPlayhead = Math.abs(e.clientX - rect.left - secToX(E.playStartSec, rect.width)) < 12
    && (e.clientY - rect.top) < 28

  E.canvas.style.cursor = nearBoundary ? 'ew-resize'
    : nearPlayhead    ? 'col-resize'
    : overCut         ? 'pointer'
    : 'crosshair'

  if (E.isDragging) E.dragEndSec = clampMain(extSec)

  scheduleDrawWaveform()
}

export function onCanvasUp(e: MouseEvent): void {
  if (!E.peaks) return
  const rect  = canvasRect()
  const extSec = xToSec(e.clientX - rect.left, rect.width)
  const upMainSec = xToMainSec(e.clientX - rect.left, rect.width)

  if (E.handleDrag) {
    E.handleDrag = null
    E.cuts.sort((a, b) => a.start - b.start)
    pushCutHistory()
    renderCutList()
    updateRemainingDisplay()
    drawWaveform()
    drawMinimap()
    return
  }

  if (E.playheadDragging) {
    E.playheadDragging = false
    // Snap playhead out of any cut region the user dragged into — cuts are
    // "skip me" zones, so resting the playhead inside one is meaningless.
    E.playStartSec = snapOutOfCut(E.playStartSec)
    updateTimecode(E.playStartSec)
    seekMediaTo(clampMain(E.playStartSec))
    drawWaveform()
    return
  }

  if (!E.isDragging) return
  E.isDragging = false

  // Cut-creation drag: hold shift to disable snap, otherwise snap both edges
  // to nearby detected segment boundaries.
  if (Math.abs(upMainSec - E.dragStartSec) > 0.1) {
    const s = e.shiftKey ? E.dragStartSec : snapToSegmentBoundary(E.dragStartSec, rect.width)
    const eSec = e.shiftKey ? upMainSec : snapToSegmentBoundary(upMainSec, rect.width)
    addCut(s, eSec)
    renderCutList()
  } else {
    // Tap to seek — covers full extended timeline so users can click into
    // intro/outro slots. If the click lands inside a cut, snap to the cut's
    // end (the nearest keep-region start) so playback always begins at a
    // position that will actually produce audio.
    stopPlay()
    E.playStartSec = snapOutOfCut(clampPlayable(extSec))
    updateTimecode(E.playStartSec)
    seekMediaTo(clampMain(E.playStartSec))
  }

  E.dragStartSec = -1
  E.dragEndSec   = -1
  drawWaveform()
  drawMinimap()
}

export function onCanvasLeave(): void {
  E.hoverSec = -99999

  if (E.handleDrag) {
    E.handleDrag = null
    E.cuts.sort((a, b) => a.start - b.start)
    pushCutHistory()
    renderCutList()
    updateRemainingDisplay()
    drawWaveform(); drawMinimap()
    return
  }

  if (E.playheadDragging) {
    E.playheadDragging = false
    drawWaveform()
    return
  }

  if (E.isDragging) {
    E.isDragging = false
    if (Math.abs(E.dragEndSec - E.dragStartSec) > 0.1) {
      addCut(E.dragStartSec, E.dragEndSec)
      renderCutList()
    }
    E.dragStartSec = -1; E.dragEndSec = -1
    drawWaveform(); drawMinimap()
  } else {
    drawWaveform()
  }
}

export function onCanvasContextMenu(e: MouseEvent): void {
  e.preventDefault()
  if (!E.peaks) return
  const rect = canvasRect()
  const mainSec = xToMainSec(e.clientX - rect.left, rect.width)
  const idx  = E.cuts.findIndex(c => mainSec >= c.start && mainSec <= c.end)
  if (idx >= 0) deleteCut(idx)
}

/**
 * Wheel zoom / pan.
 *
 * A trackpad emits wheel events well above the display refresh rate, and this
 * used to run a full synchronous canvas redraw plus a minimap update for every
 * one of them — on a 90-minute file, most of those frames were painted and
 * thrown away before anything reached the screen, which is precisely why a
 * two-finger flick felt like it was fighting back.
 *
 * The fix is NOT to accumulate the deltas and apply them later: the zoom is
 * anchored to the time under the cursor, and deferring the maths would apply
 * later events against a viewport that hasn't moved yet, drifting the anchor.
 * The viewport is updated synchronously per event — exactly as before, so the
 * anchoring is bit-for-bit the old behaviour — and only the two repaints are
 * coalesced into one rAF.
 */
export function onCanvasWheel(e: WheelEvent): void {
  e.preventDefault()
  if (e.ctrlKey || e.metaKey) {
    // Zoom centered on mouse position (main coords only — intro/outro slots
    // have their own fixed scale).
    const rect = canvasRect()
    const mouseSec = xToMainSec(e.clientX - rect.left, rect.width)
    const factor   = e.deltaY > 0 ? 1.25 : 0.75
    const span     = (E.vpEnd - E.vpStart) * factor
    const frac     = (mouseSec - E.vpStart) / (E.vpEnd - E.vpStart)
    E.vpStart = Math.max(0, mouseSec - frac * span)
    E.vpEnd   = Math.min(E.duration, E.vpStart + span)
    if (E.vpEnd - E.vpStart < 0.5) { E.vpEnd = E.vpStart + 0.5 }
    scheduleViewportDraw()
  } else {
    panBy(e.deltaY * (E.vpEnd - E.vpStart) / 800)
  }
}

/** If `sec` falls inside a cut region, return the cut's end (the nearest
 *  keep-region start). Cuts are skip-zones — the playhead resting inside
 *  one is meaningless because no audio plays there. Out-of-range or
 *  already-outside-cut input is returned unchanged. */
export function snapOutOfCut(sec: number): number {
  for (const c of E.cuts) {
    if (sec >= c.start && sec < c.end) {
      // Snap to the cut's end, clamped to the playable range so we never
      // overshoot duration when a trailing cut runs to the file end.
      return Math.min(maxPlayableSec(), c.end)
    }
  }
  return sec
}

/** Snaps a main-coords second to the nearest detected segment boundary within
 *  threshold (default ~0.4 sec). Falls through to input unchanged when no
 *  suggestions are loaded or no boundary is close enough. */
export function snapToSegmentBoundary(sec: number, W: number): number {
  if (!E.suggestions.length) return sec
  // Threshold scales with zoom level (~8 px) — tight at high zoom, lenient
  // when zoomed out so coarse drags still find the boundary.
  const threshold = Math.max(0.15, ((E.vpEnd - E.vpStart) / Math.max(1, W)) * 8)
  let closest = sec
  let minDist = threshold
  for (const seg of E.suggestions) {
    if (!shouldShowSegment(seg.type)) continue
    for (const t of [seg.start, seg.end]) {
      const d = Math.abs(sec - t)
      if (d < minDist) { minDist = d; closest = t }
    }
  }
  return closest
}

// Module-scoped listener refs so repeated setupEditorPage() calls (renderer
// reload, page-switch) don't keep adding new window-level listeners. Each
// re-invocation removes the previous pair before attaching new ones.
let minimapWindowMoveHandler: ((e: MouseEvent) => void) | null = null
let minimapWindowUpHandler:   (() => void) | null = null

export function setupMinimapInteraction(): void {
  E.minimap?.addEventListener('mousedown', (e: MouseEvent) => {
    E.minimapDragging = true
    jumpViewportToMouse(e)
  })
  if (minimapWindowMoveHandler) window.removeEventListener('mousemove', minimapWindowMoveHandler)
  if (minimapWindowUpHandler)   window.removeEventListener('mouseup',   minimapWindowUpHandler)
  minimapWindowMoveHandler = (e: MouseEvent) => {
    if (E.minimapDragging) jumpViewportToMouse(e)
  }
  minimapWindowUpHandler = () => { E.minimapDragging = false }
  window.addEventListener('mousemove', minimapWindowMoveHandler)
  window.addEventListener('mouseup',   minimapWindowUpHandler)
}

export function jumpViewportToMouse(e: MouseEvent): void {
  if (!E.duration || !E.minimap) return
  const rect   = E.minimap.getBoundingClientRect()
  const frac   = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width))
  const center = frac * E.duration
  const half   = (E.vpEnd - E.vpStart) / 2
  E.vpStart = Math.max(0, Math.min(E.duration - half * 2, center - half))
  E.vpEnd   = E.vpStart + half * 2
  // Fires from a window-level mousemove for the whole minimap drag — same
  // coalescing as the wheel path.
  scheduleViewportDraw()
}
