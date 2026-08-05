import { t } from '../../i18n'
import { E, $, markDirty, type Cut } from './state'
import { pushSnapshot, undoSnapshot, redoSnapshot } from './cut-history'
import { createDraftScheduler } from './draft-scheduler'
import { computeKeepSegs } from './keep-segments'
import { addCutToList } from './cut-ops'
import { gainFactor } from './peaks'
import { formatTime, formatDuration } from './format'
import { drawWaveform, drawMinimap, updateMinimapViewport } from './waveform'
import { startPlay, stopPlay, updateTimecode } from './playback'
import { firstMount, resetMount } from '../../ui/motion'
import { updateHeaderSummary } from '../editor-page'

// Cut-region model. (Mutations + rendering land here in a later phase; for now
// just the read-only predicates the waveform renderer needs.)

export function isInCut(sec: number): boolean {
  return E.cuts.some(c => sec >= c.start && sec <= c.end)
}

export function isInDrag(sec: number): boolean {
  if (!E.isDragging) return false
  const s = Math.min(E.dragStartSec, E.dragEndSec)
  const e = Math.max(E.dragStartSec, E.dragEndSec)
  return sec >= s && sec <= e
}

// ── Cut helpers ───────────────────────────────────────────────────────────
// Undo/redo: history stores snapshots; cutHistoryIdx points to the current
// live state. Index -1 means "no history yet" (initial empty state).
// pushCutHistory() is called AFTER a mutation to record the new state.
export function pushCutHistory(): void {
  // Pure state machine (unit-tested in cut-history.test.ts) owns the snapshot/
  // cap/redo-discard invariants; here we just store the result + persist.
  const next = pushSnapshot({ history: E.cutHistory, idx: E.cutHistoryIdx }, E.cuts)
  E.cutHistory = next.history
  E.cutHistoryIdx = next.idx
  // Persist cuts to a draft sidecar so a crash mid-edit doesn't lose the work
  scheduleDraftSave()
}

/** Debounce window — long enough that a drag doesn't spam IPC, short enough
 *  that a crash costs at most a couple of seconds of editing. */
const DRAFT_SAVE_DEBOUNCE_MS = 2000

// The pending write captures its file path AND its cut list up front (see
// draft-scheduler.ts for why both). A timer armed for file A can therefore only
// ever write A's cuts to A's sidecar, even if it fires in the middle of B's load.
const draftSaver = createDraftScheduler<Cut[]>({
  delayMs: DRAFT_SAVE_DEBOUNCE_MS,
  save: (fp, cuts) => { window.api.editorSaveCutsDraft(fp, cuts).catch(() => {}) },
})

export function scheduleDraftSave(): void {
  if (!E.filePath) return
  // Snapshot: the live array keeps being mutated (drags edit cuts in place), and
  // the write must describe the state at the moment it was requested.
  draftSaver.schedule(E.filePath, E.cuts.map(c => ({ ...c })))
}

/** Disarm any pending draft write. Called at the top of `loadFile` and from
 *  `teardownPlayback` — once we start tearing the current file down, a queued
 *  write for it is at best pointless and at worst lands on the NEXT file. */
export function cancelDraftSave(): void {
  draftSaver.cancel()
}

/** Called after a successful save/export to remove the draft. */
export function clearEditorDraft(): void {
  draftSaver.cancel()
  if (E.filePath) window.api.editorDeleteCutsDraft(E.filePath).catch(() => {})
}

export function addCut(s: number, e: number): void {
  // Pure add+clamp+merge (unit-tested in cut-ops.test.ts). Clamps to [0,duration]
  // in case the drag started/ended in an intro/outro slot; null = too short to keep.
  const next = addCutToList(E.cuts, s, e, E.duration)
  if (!next) return
  E.cuts = next
  pushCutHistory()
  markDirty()
  updateRemainingDisplay()
}

export function deleteCut(i: number): void {
  E.cuts.splice(i, 1)
  pushCutHistory()
  markDirty()
  renderCutList()
  updateRemainingDisplay()
  drawWaveform()
  drawMinimap()
}

export function undoCut(): void {
  // Never swap the cut array out from under an in-flight drag — that orphans the
  // drag's in-place edits and corrupts history on mouse-up. Let the drag finish.
  if (E.handleDrag || E.isDragging) return
  const r = undoSnapshot({ history: E.cutHistory, idx: E.cutHistoryIdx }, E.cuts.length)
  if (!r) return
  E.cutHistoryIdx = r.idx
  E.cuts = r.cuts
  renderCutList()
  updateRemainingDisplay()
  drawWaveform()
  drawMinimap()
}

export function redoCut(): void {
  if (E.handleDrag || E.isDragging) return // see undoCut: don't disrupt a live drag
  const r = redoSnapshot({ history: E.cutHistory, idx: E.cutHistoryIdx })
  if (!r) return
  E.cutHistoryIdx = r.idx
  E.cuts = r.cuts
  renderCutList()
  updateRemainingDisplay()
  drawWaveform()
  drawMinimap()
}

export function getKeepSegs(): { start: number; end: number }[] {
  // Pure computation (unit-tested in keep-segments.test.ts) — drives preview
  // playback + the export cut-plan.
  return computeKeepSegs(E.cuts, E.duration)
}

export function getRemainingDuration(): number {
  return getKeepSegs().reduce((sum, s) => sum + (s.end - s.start), 0)
}

export function updateRemainingDisplay(): void {
  const el  = $('editor-remaining')
  const dur = $('editor-remaining-dur')
  // Update header summary regardless — duration / normalize state may have
  // changed even if no cuts exist yet.
  updateHeaderSummary()
  if (!el || !E.duration) return

  if (E.cuts.length === 0) {
    el.style.display = 'none'
    return
  }

  const rem = getRemainingDuration()
  const cut = E.duration - rem
  el.style.display = 'flex'
  el.classList.toggle('has-cuts', E.cuts.length > 0)
  if (dur) dur.textContent = `${formatDuration(rem)} (fjerner ${formatDuration(cut)})`
}

export function renderCutList(): void {
  const panel = $('editor-cuts-panel')
  const list  = $('editor-cuts-list')
  const undo  = $('btn-editor-undo-all')
  if (!panel || !list || !undo) return

  if (E.cuts.length === 0) {
    panel.style.display = 'none'
    undo.style.display  = 'none'
    // Emptied (undo-all, or a new file loaded) — the next cut is an arrival
    // again and gets to slide in.
    resetMount(list)
    return
  }

  panel.style.display = ''
  undo.style.display  = ''

  list.innerHTML = ''
  // The list re-renders on EVERY cut add, delete, drag-resize and undo. Sliding
  // all of the rows in again each time turned a two-cut edit into a small
  // parade; the entrance now plays once, when the panel first appears.
  const animate = firstMount(list)

  E.cuts.forEach((c, i) => {
    const dur = c.end - c.start
    const row = document.createElement('div')
    row.className = animate ? 'editor-cut-row row-in' : 'editor-cut-row'
    if (animate) row.style.animationDelay = `${i * 0.05}s`

    // Thumbnail
    const thumb = document.createElement('div')
    thumb.className = 'editor-cut-thumb'
    if (E.peaks) {
      thumb.innerHTML = makeCutThumbnailSvg(c)
    }

    // Info
    const info = document.createElement('div')
    info.className = 'editor-cut-info'
    info.innerHTML = `<div class="editor-cut-range">${formatTime(c.start)} – ${formatTime(c.end)}</div>
      <div class="editor-cut-dur">${formatDuration(dur)}</div>`

    // Preview button (pre/post-roll)
    const prevBtn = document.createElement('button')
    prevBtn.className = 'editor-cut-prev'
    prevBtn.title = 'Spill 3s rundt kuttet'
    prevBtn.innerHTML = '<svg viewBox="0 0 20 20" fill="currentColor" width="12" height="12"><path d="M6.3 4.6a1 1 0 011.4 0l6 5a1 1 0 010 1.6l-6 5A1 1 0 016 15.4V4.6z"/></svg>'
    prevBtn.addEventListener('click', () => previewCut(c))

    // Delete button
    const delBtn = document.createElement('button')
    delBtn.className = 'editor-cut-del'
    delBtn.title = t('editor.deleteCut') || 'Fjern kutt'
    delBtn.textContent = '✕'
    delBtn.addEventListener('click', () => deleteCut(i))

    row.appendChild(thumb)
    row.appendChild(info)
    row.appendChild(prevBtn)
    row.appendChild(delBtn)
    list.appendChild(row)
  })
}

export function makeCutThumbnailSvg(cut: Cut): string {
  if (!E.peaks) return ''
  const W = 72, H = 24
  const startIdx = Math.floor(cut.start * 100)
  const endIdx   = Math.ceil(cut.end * 100)
  const count    = Math.max(1, endIdx - startIdx)
  const midY     = H / 2
  const maxH     = midY - 2
  const gFac     = gainFactor()
  const rects: string[] = []
  for (let px = 0; px < W; px++) {
    const pi = startIdx + Math.floor(px / W * count)
    if (pi >= E.peaks.length) break
    const h = Math.min(maxH, E.peaks[pi] * gFac * maxH)
    rects.push(`<rect x="${px}" y="${(midY - h).toFixed(1)}" width="1" height="${(h * 2).toFixed(1)}"/>`)
  }
  return `<svg width="${W}" height="${H}" xmlns="http://www.w3.org/2000/svg" style="display:block">${rects.join('')}</svg>`
}

export function previewCut(cut: Cut): void {
  stopPlay()
  const PRE_ROLL = 3
  E.playStartSec = Math.max(0, cut.start - PRE_ROLL)
  updateTimecode(E.playStartSec)
  if (E.playStartSec < E.vpStart || E.playStartSec > E.vpEnd) {
    const half = (E.vpEnd - E.vpStart) / 2
    E.vpStart = Math.max(0, E.playStartSec - half * 0.3)
    E.vpEnd   = Math.min(E.duration, E.vpStart + half * 2)
    updateMinimapViewport()
  }
  startPlay(false)
}
