import { t } from '../i18n'
import { settings, patchSettings } from '../state'
import { escHtml as escapeHtml } from '../helpers'
import type { RecordingEntry, RecordingMetadata } from '../../types'
import { setupTranscriptPanel, clearTranscript } from './editor-transcript'
import { confirmDialog } from '../ui/dialog'
import { setupThumbPanel, panelElementsByPrefix } from './thumbnail-panel'
import { E, $, markDirty, clearDirty, setOnDirtyChange } from './editor/state'
import { formatDuration } from './editor/format'
import { computePeakGain, setNormalizeUI } from './editor/peaks'
import { minPlayableSec, maxPlayableSec, clampPlayable, clampMain, xToSec, getRegionAtX } from './editor/geometry'
import { deleteCut, undoCut, redoCut, getRemainingDuration, updateRemainingDisplay, renderCutList, pushCutHistory } from './editor/cuts'
import { runDetection, applySermonTrim, setSermonSegment, hideSuggestionBanner } from './editor/detection'
import { saveMetadata } from './editor/metadata'
import { syncCanvasSize, drawWaveform, drawMinimap, updateMinimapViewport } from './editor/waveform'
import { togglePlay, stopPlay, seekTo, seekBy, jumpToCutBoundary, updateTimecode, seekMediaTo } from './editor/playback'
import { fitAll, zoomBy } from './editor/viewport'
import { onCanvasDown, onCanvasMove, onCanvasUp, onCanvasLeave, onCanvasContextMenu, onCanvasWheel, setupMinimapInteraction, snapOutOfCut } from './editor/canvas-input'
import { openExportModal, closeExportModal, runExport, updateExportFormatUI } from './editor/export'
import { setupMasteringPanel } from './editor/mastering'
import { setupStageUi } from './editor/stage-ui'
import { setupEditorTabs, flagEditorTab } from './editor/tabs'
import { setupViewMenu } from './editor/view-menu'
import { pickAndLoad, loadFile, reloadIntroOutro, teardownPlayback, updateVideoIntroOutroDisplay, updateEditorIntroOutroDisplay } from './editor/loader'

// ── Setup ─────────────────────────────────────────────────────────────────
export function setupEditorPage(): void {
  setOnDirtyChange(updateHeaderSummary)
  E.canvas    = $('editor-canvas')  as HTMLCanvasElement
  E.minimap   = $('editor-minimap') as HTMLCanvasElement
  E.minimapVp = $('editor-minimap-vp') as HTMLElement
  E.videoEl   = $('editor-video') as HTMLVideoElement | null

  $('btn-editor-open')?.addEventListener('click',    () => pickAndLoad())
  $('btn-editor-change')?.addEventListener('click',  async () => {
    if (!(await confirmDiscardIfDirty('open'))) return
    pickAndLoad()
  })
  $('btn-editor-close')?.addEventListener('click', async () => {
    if (!(await confirmDiscardIfDirty('close'))) return
    closeCurrentFile()
  })

  // Empty-state click anywhere on the dropzone opens picker
  $('editor-empty-dropzone')?.addEventListener('click', (e) => {
    // Don't double-fire when the inner button is clicked
    if ((e.target as HTMLElement).closest('button')) return
    pickAndLoad()
  })

  // Intro/Outro panel header collapses on click of chevron
  $('editor-io-chevron')?.addEventListener('click', () => {
    document.getElementById('editor-io-panel')?.classList.toggle('editor-io-panel--collapsed')
  })
  // Keep clicks on the "Inkluder ved eksport" toggle from bubbling up to the
  // collapsible panel header (externalized from an inline onclick attribute,
  // which the strict CSP — script-src 'self' — would block).
  document.querySelector('.editor-io-include-label')?.addEventListener('click', (e) => e.stopPropagation())

  setupKbdHints()
  setupEditorTabs()
  setupViewMenu()
  setupMasteringCollapse()
  $('btn-editor-play')?.addEventListener('click',    () => togglePlay(false))
  $('btn-editor-preview')?.addEventListener('click', () => togglePlay(true))
  $('btn-zoom-in')?.addEventListener('click',   () => zoomBy(0.5))
  $('btn-zoom-out')?.addEventListener('click',  () => zoomBy(2))
  $('btn-zoom-fit')?.addEventListener('click',  () => fitAll())
  $('btn-editor-undo-all')?.addEventListener('click', () => {
    if (E.cuts.length === 0) return
    E.cuts = []
    pushCutHistory()
    renderCutList()
    updateRemainingDisplay()
    drawWaveform()
    drawMinimap()
  })

  $('btn-editor-save')?.addEventListener('click',    () => openExportModal())
  $('btn-export-cancel')?.addEventListener('click',  () => closeExportModal())
  $('btn-export-confirm')?.addEventListener('click', () => runExport())

  // Audio format picker pills. Scoped to #export-fmt-section so it doesn't fight
  // with the video format/codec and export-type pills (which share the
  // .export-fmt-btn style class but have their own group handlers in export.ts).
  document.querySelectorAll<HTMLElement>('#export-fmt-section .export-fmt-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('#export-fmt-section .export-fmt-btn').forEach(b => b.classList.remove('active'))
      btn.classList.add('active')
      updateExportFormatUI(btn.dataset.fmt ?? 'mp3')
    })
  })

  // Destination picker. Two pills: "Samme mappe" (default) leaves
  // E.exportOutputFolder EMPTY — the backend reads that as "beside the source
  // file" — and "Velg mappe…" sets an absolute path. There is no third
  // "Erstatt original" pill any more: replace-mode was never implemented in the
  // backend, so it silently behaved like "ny fil".
  document.querySelectorAll<HTMLElement>('.export-dest-btn').forEach(btn => {
    btn.addEventListener('click', async () => {
      document.querySelectorAll('.export-dest-btn').forEach(b => b.classList.remove('active'))
      btn.classList.add('active')
      if (btn.dataset.dest === 'folder') {
        const folder = await window.api.editorPickOutputFolder()
        if (folder) {
          const label = $('export-folder-path')
          if (label) { label.textContent = folder; label.style.display = '' }
          E.exportOutputFolder = folder
        } else {
          // Revert to same if cancelled
          document.querySelectorAll('.export-dest-btn').forEach(b => b.classList.remove('active'))
          document.querySelector<HTMLElement>('.export-dest-btn[data-dest="same"]')?.classList.add('active')
        }
      } else {
        // Back to the default → drop any previously picked folder, otherwise
        // the pill said "Samme mappe" while the export still went elsewhere.
        E.exportOutputFolder = ''
        const label = $('export-folder-path')
        if (label) { label.textContent = ''; label.style.display = 'none' }
      }
    })
  })

  // Peak normalization (Premiere-style "Normalize Max Peaks"): scans the
  // already-decoded peaks array, computes the gain needed to bring max peak
  // to -1 dBFS (1 dB safety headroom), and scales the waveform render +
  // export pipeline accordingly. Idempotent — clicking when already
  // normalized is a no-op.
  $('btn-normalize-peak')?.addEventListener('click', async () => {
    if (!E.peaks || E.peaks.length === 0) return
    if (E.audioGainDb !== 0) return     // already normalized — idempotent
    let gain = computePeakGain(E.peaks)
    // E.peaks ALWAYS comes from the backend's 8 kHz mono extract now (the
    // renderer no longer decodes the recording at all), and that downmix
    // UNDER-reads the true peak by several dB — while the export, where this
    // gain is applied, runs on the ORIGINAL file. Normalizing from the extract
    // peaks would push the export into clipping, so probe the original's true
    // peak (volumedetect) unconditionally; the peaks-derived gain is only the
    // fallback for when the probe itself fails.
    try {
      const maxDb = await window.api.editorProbePeak(E.filePath)
      if (typeof maxDb === 'number' && isFinite(maxDb)) {
        gain = maxDb >= -1 ? 0 : -1 - maxDb
      }
    } catch { /* keep peaks-derived gain */ }
    if (!isFinite(gain) || Math.abs(gain) < 0.05) {
      // Already at (or above) target — show that explicitly
      setNormalizeUI(0, /*alreadyAtTarget*/ true)
      return
    }
    E.audioGainDb = gain
    setNormalizeUI(gain, false)
    markDirty()
    updateHeaderSummary()
    drawWaveform()
    drawMinimap()
  })

  $('btn-normalize-reset')?.addEventListener('click', () => {
    if (E.audioGainDb === 0) return
    E.audioGainDb = 0
    setNormalizeUI(0, false)
    markDirty()
    updateHeaderSummary()
    drawWaveform()
    drawMinimap()
  })

  $('btn-editor-prompt-open')?.addEventListener('click', () => {
    const fp = ($('editor-prompt-toast') as HTMLElement).dataset.path ?? ''
    dismissEditorPrompt()
    if (fp) openEditorWithFile(fp)
  })
  $('btn-editor-prompt-dismiss')?.addEventListener('click', dismissEditorPrompt)

  // Intro/Outro controls
  const ioChk = $('editor-include-io') as HTMLInputElement | null
  if (ioChk) {
    ioChk.addEventListener('change', () => {
      E.includeIntroOutro = ioChk.checked
      markDirty()
      drawWaveform()
    })
  }

  $('btn-editor-pick-intro')?.addEventListener('click', async () => {
    const fp = await window.api.pickAudioFile()
    if (!fp) return
    patchSettings({ editorIntroPath: fp })
    await window.api.saveSettings(settings)
    await reloadIntroOutro()
    markDirty()
  })
  $('btn-editor-clear-intro')?.addEventListener('click', async () => {
    patchSettings({ editorIntroPath: undefined })
    await window.api.saveSettings(settings)
    await reloadIntroOutro()
    markDirty()
  })
  $('btn-editor-pick-outro')?.addEventListener('click', async () => {
    const fp = await window.api.pickAudioFile()
    if (!fp) return
    patchSettings({ editorOutroPath: fp })
    await window.api.saveSettings(settings)
    await reloadIntroOutro()
    markDirty()
  })
  $('btn-editor-clear-outro')?.addEventListener('click', async () => {
    patchSettings({ editorOutroPath: undefined })
    await window.api.saveSettings(settings)
    await reloadIntroOutro()
    markDirty()
  })

  // Video intro/outro buttons
  $('btn-editor-pick-video-intro')?.addEventListener('click', async () => {
    const fp = await window.api.editorPickVideoFile()
    if (!fp) return
    E.videoIntroPath = fp
    updateVideoIntroOutroDisplay()
  })
  $('btn-editor-clear-video-intro')?.addEventListener('click', () => {
    E.videoIntroPath = ''
    updateVideoIntroOutroDisplay()
  })
  $('btn-editor-pick-video-outro')?.addEventListener('click', async () => {
    const fp = await window.api.editorPickVideoFile()
    if (!fp) return
    E.videoOutroPath = fp
    updateVideoIntroOutroDisplay()
  })
  $('btn-editor-clear-video-outro')?.addEventListener('click', () => {
    E.videoOutroPath = ''
    updateVideoIntroOutroDisplay()
  })

  // Metadata panel toggle
  $('btn-meta-toggle')?.addEventListener('click', () => {
    const body = $('editor-meta-body')
    if (!body) return
    const open = body.style.display === 'none'
    body.style.display = open ? '' : 'none'
    $('editor-meta-chevron')?.classList.toggle('open', open)
  })

  // Metadata autosave on change
  const metaFields: [string, keyof RecordingMetadata][] = [
    ['meta-title', 'title'], ['meta-speaker', 'speaker'], ['meta-description', 'description']
  ]
  for (const [id, field] of metaFields) {
    $(id)?.addEventListener('input', () => {
      const el = $(id) as HTMLInputElement | HTMLTextAreaElement | null
      if (el) (E.meta as unknown as Record<string, unknown>)[field] = el.value
      E.metaDirty = true
      markDirty()
    })
  }

  // Analyse panel: run detection
  $('btn-detect-segments')?.addEventListener('click', () => runDetection())

  // Analyse panel: segment-type toggles
  $('editor-show-speech')?.addEventListener('change', () => {
    E.showSpeechSegments = ($('editor-show-speech') as HTMLInputElement).checked
    drawWaveform()
  })
  $('editor-show-music')?.addEventListener('change', () => {
    E.showMusicSegments = ($('editor-show-music') as HTMLInputElement).checked
    drawWaveform()
  })
  $('editor-show-silence')?.addEventListener('change', () => {
    E.showSilenceSegments = ($('editor-show-silence') as HTMLInputElement).checked
    drawWaveform()
  })

  // Analyse panel: "Marker preken automatisk"
  $('btn-apply-auto-trim')?.addEventListener('click', () => applySermonTrim())
  $('btn-suggestion-apply')?.addEventListener('click', () => {
    applySermonTrim()
    hideSuggestionBanner()
  })
  $('btn-suggestion-dismiss')?.addEventListener('click', () => hideSuggestionBanner())
  $('editor-sermon-picker')?.addEventListener('change', (e) => {
    const idx = parseInt((e.target as HTMLSelectElement).value, 10)
    if (!isNaN(idx)) setSermonSegment(idx)
  })

  // Meta save button
  $('btn-meta-save')?.addEventListener('click', saveMetadata)

  // Loop toggle
  $('btn-editor-loop')?.addEventListener('click', () => {
    E.isLooping = !E.isLooping
    $('btn-editor-loop')?.classList.toggle('active', E.isLooping)
  })

  // Clip badge — jump to first clip
  $('editor-clip-badge')?.addEventListener('click', () => {
    if (E.clipTimes.length === 0) return
    E.playStartSec = Math.max(0, E.clipTimes[0] - 1)
    updateTimecode(E.playStartSec)
    const half = (E.vpEnd - E.vpStart) / 2
    E.vpStart = Math.max(0, E.playStartSec - half * 0.3)
    E.vpEnd   = Math.min(E.duration, E.vpStart + half * 2)
    updateMinimapViewport()
    drawWaveform()
  })

  // NOTE: the export-progress listener lives in runExport() (export.ts), scoped
  // to one export and unsubscribed in its `finally`. It used to be registered
  // here for the whole page lifetime — and read a `percent` field the backend
  // never emitted, so the bar stayed frozen while the label printed NaN.

  // Mastering wiring
  setupMasteringPanel()

  // Stage integration button (opt-in; hidden until enabled in settings)
  setupStageUi()

  // Canvas interactions
  E.canvas?.addEventListener('mousedown',   onCanvasDown)
  E.canvas?.addEventListener('mousemove',   onCanvasMove)
  E.canvas?.addEventListener('mouseup',     onCanvasUp)
  E.canvas?.addEventListener('mouseleave',  onCanvasLeave)
  E.canvas?.addEventListener('contextmenu', onCanvasContextMenu)
  E.canvas?.addEventListener('wheel',       onCanvasWheel, { passive: false })
  // Double-click on the sermon segment → trim around the sermon. Forces a
  // deliberate double-tap so single-click stays as non-destructive tap-to-seek.
  E.canvas?.addEventListener('dblclick', (e: MouseEvent) => {
    if (!E.peaks) return
    const rect = E.canvas.getBoundingClientRect()
    const sec = xToSec(e.clientX - rect.left, rect.width)
    const sermon = E.suggestions.find(s => s.type === 'sermon' && sec >= s.start && sec <= s.end)
    if (sermon) applySermonTrim()
  })

  setupMinimapInteraction()
  setupKeyboardShortcuts()
  const seekToSec = (sec: number): void => {
    E.playStartSec = clampPlayable(snapOutOfCut(sec))
    updateTimecode(E.playStartSec)
    seekMediaTo(clampMain(E.playStartSec))
    drawWaveform()
  }
  setupTranscriptPanel(seekToSec)
  setupDragDrop()

  if (E.canvas && E.canvas.parentElement) {
    // Track the observer so repeated setupEditorPage() calls (after a renderer
    // reload, for example) don't leak observers. Single observer for app life.
    if (resizeObserver) resizeObserver.disconnect()
    resizeObserver = new ResizeObserver(() => { syncCanvasSize(); drawWaveform() })
    resizeObserver.observe(E.canvas.parentElement)
  }

  showState('empty')
  updateEditorIntroOutroDisplay()

  // Wire the per-episode thumbnail panel. Hidden until a file is loaded
  // (see loadFile completion). Reads window state via getRecordingPath().
  const thumbEls = panelElementsByPrefix('editor')
  if (thumbEls) {
    setupThumbPanel(thumbEls, { kind: 'episode', getRecordingPath: () => E.filePath })
  }
}

let resizeObserver: ResizeObserver | null = null

export function openEditorWithFile(fp: string, seekToSec?: number): void {
  window.showPage('editor')
  // Set the seek target applied once the file finishes loading. Consumed at the
  // tail of loadFile(). The CustomEvent path was racy because loadFile zeroes
  // playStartSec mid-flight; this gives us a deterministic "apply once decoded".
  E.pendingSeekSec = typeof seekToSec === 'number' ? seekToSec : null
  loadFile(fp)
}

// ── Editor chrome: keyboard hints + the secondary mastering panel ──────────

/** localStorage key for the keyboard-hint strip. Default closed. */
const KBD_HINTS_KEY = 'sundayrec.editor.kbdHints'

/**
 * Put the nine keyboard-shortcut chips behind the toolbar's "?".
 *
 * They used to sit above the waveform permanently, for the whole session: nine
 * chips of chrome between the operator and the thing they came to edit, useful
 * once and furniture forever after. Closed by default; the choice is
 * remembered, so someone who wants them keeps them.
 */
function setupKbdHints(): void {
  const btn   = $('btn-editor-kbd-toggle')
  const hints = $('editor-kbd-hints')
  if (!btn || !hints) return
  const apply = (open: boolean): void => {
    hints.hidden = !open
    btn.setAttribute('aria-expanded', String(open))
  }
  let open = false
  try { open = localStorage.getItem(KBD_HINTS_KEY) === 'open' } catch { /* private mode */ }
  apply(open)
  btn.addEventListener('click', () => {
    open = !open
    apply(open)
    try { localStorage.setItem(KBD_HINTS_KEY, open ? 'open' : 'closed') } catch { /* private mode */ }
  })
}

/**
 * The workspace mastering panel is the SECONDARY path — it writes a separate
 * `_mastert` file. Mastering that ends up in the exported file is chosen in the
 * export dialog's Lydforbedring section, which is where the operator already
 * is when they think about it. Two panels offering "mastering" with different
 * outcomes is one too many, so this one collapses and points at the other; it
 * stays fully functional for the separate-file flow.
 */
function setupMasteringCollapse(): void {
  const header  = $('editor-master-header')
  const section = $('editor-master-section')
  if (!header || !section) return
  const toggle = (): void => {
    const collapsed = section.classList.toggle('editor-master-section--collapsed')
    header.setAttribute('aria-expanded', String(!collapsed))
  }
  header.addEventListener('click', toggle)
  header.addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle() }
  })
}

/** Called when the user navigates BACK to the editor tab. Repaints the
 *  waveform if a file is still loaded — the canvas might have been resized
 *  or had its backing store cleared while away. Cheap no-op if no file. */
export function reactivateEditor(): void {
  if (!E.peaks) return
  // Re-sync canvas size first (could've changed if window resized while away)
  requestAnimationFrame(() => {
    syncCanvasSize()
    drawWaveform()
    drawMinimap()
    updateMinimapViewport()
  })
}

/** Called when the user navigates away from the editor tab.
 *
 *  IMPORTANT: We only PAUSE/STOP work that runs in the background — playback
 *  and video. We do NOT release peaks, the player's src, cuts, meta, or
 *  any of the editing state. Otherwise, returning to the editor with the same
 *  file open shows an empty waveform — the user has to close and re-open the
 *  file to see anything. (Reported bug, May 2026.)
 *
 *  Actual cleanup happens in closeCurrentFile() (explicit close-button or
 *  Cmd+W) and at loadFile() entry (replacing one file with another). */
export function deactivateEditor(): void {
  stopPlay()
  // Pause video element to release decode/GPU resources, but keep the src
  // so the frame is still there when the user returns.
  if (E.videoEl && !E.videoEl.paused) {
    E.videoEl.pause()
  }
  // Note: deliberately NOT touching peaks / playerEl / cuts / cutHistory /
  // suggestions / clipTimes / meta / isVideoFile / audioGainDb. Those are
  // owned by the open-file lifecycle, not the tab-visibility one.
}

// ── Keyboard shortcuts ────────────────────────────────────────────────────
//
// Shortcuts are only active while the editor tab is the visible page and
// the user isn't typing in an input/textarea. The mod key is Cmd on Mac
// and Ctrl elsewhere — we treat the two interchangeably (metaKey || ctrlKey).
function setupKeyboardShortcuts(): void {
  window.addEventListener('keydown', (e: KeyboardEvent) => {
    if (!document.getElementById('page-editor')?.classList.contains('active')) return
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return
    const mod = e.metaKey || e.ctrlKey

    // App-level shortcuts: Cmd+O / Cmd+W / Cmd+S / Cmd+E — these work even
    // when no file is open (e.g. Cmd+O on empty state). Also intentionally
    // skip the `peaks` guard so Cmd+O still works.
    if (mod && e.code === 'KeyO') {
      e.preventDefault()
      void confirmDiscardIfDirty('open').then(ok => { if (ok) pickAndLoad() })
      return
    }
    if (mod && e.code === 'KeyW') {
      e.preventDefault()
      if (!E.filePath) return
      void confirmDiscardIfDirty('close').then(ok => { if (ok) closeCurrentFile() })
      return
    }
    if (mod && (e.code === 'KeyS' || e.code === 'KeyE')) {
      e.preventDefault()
      if (E.filePath) openExportModal()
      return
    }

    // Per-file shortcuts (need an open file)
    if (!E.peaks) return
    if (e.target instanceof HTMLButtonElement) return

    switch (e.code) {
      case 'Space':
        e.preventDefault()
        togglePlay(E.isPlaying ? E.isPreview : false)
        break
      case 'ArrowLeft':
        e.preventDefault()
        seekBy(e.shiftKey ? -1 : -5)
        break
      case 'ArrowRight':
        e.preventDefault()
        seekBy(e.shiftKey ? 1 : 5)
        break
      case 'Equal':
      case 'NumpadAdd':
        if (!e.metaKey && !e.ctrlKey) { e.preventDefault(); zoomBy(0.55) }
        break
      case 'Minus':
      case 'NumpadSubtract':
        e.preventDefault()
        zoomBy(1.7)
        break
      case 'KeyZ':
        if ((e.metaKey || e.ctrlKey) && e.shiftKey) { e.preventDefault(); redoCut() }
        else if (e.metaKey || e.ctrlKey) { e.preventDefault(); undoCut() }
        break
      case 'KeyY':
        if (e.metaKey || e.ctrlKey) { e.preventDefault(); redoCut() }
        break
      case 'Escape':
        if (E.isPlaying) stopPlay()
        break
      case 'KeyF':
        e.preventDefault()
        fitAll()
        drawWaveform()
        updateMinimapViewport()
        break
      case 'KeyL':
        e.preventDefault()
        E.isLooping = !E.isLooping
        $('btn-editor-loop')?.classList.toggle('active', E.isLooping)
        break
      case 'Delete':
      case 'Backspace': {
        // Delete the cut under the playhead — the closest cut whose range
        // contains playStartSec, falling back to the most recently added.
        if (E.cuts.length === 0) break
        e.preventDefault()
        const idx = E.cuts.findIndex(c => E.playStartSec >= c.start && E.playStartSec <= c.end)
        if (idx >= 0) deleteCut(idx)
        else deleteCut(E.cuts.length - 1)
        break
      }
      case 'Home':
        // Jump to start of extended timeline (intro start if present, else 0)
        e.preventDefault()
        seekTo(minPlayableSec())
        break
      case 'End':
        // Jump to end of extended timeline (outro end if present, else duration)
        e.preventDefault()
        seekTo(maxPlayableSec())
        break
      case 'Tab':
        // Jump to next/previous cut boundary (works in main coords)
        e.preventDefault()
        jumpToCutBoundary(e.shiftKey ? -1 : 1)
        break
      case 'KeyP': {
        // Jump to the detected sermon start
        const sermon = E.suggestions.find(s => s.type === 'sermon')
        if (sermon) { e.preventDefault(); seekTo(sermon.start) }
        break
      }
    }
  })
}

// ── Drag and drop ─────────────────────────────────────────────────────────
//
// Two drop targets:
//   1. The whole editor page (when no file is open, OR when a video/audio
//      media file is dragged anywhere outside the timeline canvas) → loads
//      as main file.
//   2. The timeline canvas: drops on the LEFT third route to INTRO,
//      drops on the RIGHT third route to OUTRO. Middle third is ignored
//      (reserved for future cut/note drops).
const AUDIO_EXTS = new Set([
  'mp3', 'mp1', 'mp2', 'wav', 'flac', 'aac', 'm4a', 'm4b', 'm4r',
  'ogg', 'oga', 'opus', 'webm', 'aiff', 'aif', 'wma', 'mka',
  'ac3', 'eac3', 'dts', 'amr', '3ga', 'caf', 'ape', 'wv', 'tta',
  'mpc', 'au', 'snd', 'ra', 'ram', 'spx', 'gsm',
])
const VIDEO_DROP_EXTS = new Set([
  'mp4', 'mov', 'mkv', 'm4v', 'avi', 'wmv', 'ts', 'mts', 'm2ts', 'flv', '3gp', 'asf', 'f4v',
])

function setupDragDrop(): void {
  const page    = $('page-editor')
  const overlay = $('editor-drop-overlay')
  const canvasWrap = $('editor-canvas-wrap')
  if (!page) return

  // Page-wide drag (sets the main-file load overlay). Skip when the drag
  // hovers the canvas (which has its own zoned drop targets).
  page.addEventListener('dragover', (e: DragEvent) => {
    e.preventDefault()
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
    if (!canvasWrap?.contains(e.target as Node)) overlay?.classList.add('active')
  })

  page.addEventListener('dragleave', (e: DragEvent) => {
    if (!page.contains(e.relatedTarget as Node)) {
      overlay?.classList.remove('active')
    }
  })

  page.addEventListener('drop', async (e: DragEvent) => {
    // The canvas handler below claims its own drops via stopPropagation.
    e.preventDefault()
    overlay?.classList.remove('active')
    const file = e.dataTransfer?.files[0]
    if (!file) return
    const ext = file.name.split('.').pop()?.toLowerCase() ?? ''
    if (!AUDIO_EXTS.has(ext) && !VIDEO_DROP_EXTS.has(ext)) return
    const fp = (file as File & { path?: string }).path
    if (!fp) return
    if (!(await confirmDiscardIfDirty('open'))) return
    // Drag-and-drop is an explicit user action — trust the folder for this
    // session so path-defense doesn't silently refuse legitimate picks from
    // external drives or non-standard locations.
    try { await window.api.registerTrustedPath(fp) } catch {}
    loadFile(fp)
  })

  // Canvas-specific drop zones for intro/outro. The dragover handler
  // highlights the left or right third using CSS pseudo-elements; the
  // drop handler routes the file to the right intro/outro slot.
  if (canvasWrap) {
    canvasWrap.addEventListener('dragover', (e: DragEvent) => {
      e.preventDefault()
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
      const rect = canvasWrap.getBoundingClientRect()
      const x = e.clientX - rect.left
      const region = getRegionAtX(x, rect.width)
      canvasWrap.classList.toggle('is-dropzone-intro', region === 'intro')
      canvasWrap.classList.toggle('is-dropzone-outro', region === 'outro')
      // When we're highlighting an intro/outro zone, take precedence over
      // the page-wide overlay (which would otherwise show "load main file").
      if (region !== 'main') {
        overlay?.classList.remove('active')
        e.stopPropagation()
      }
    })

    canvasWrap.addEventListener('dragleave', () => {
      canvasWrap.classList.remove('is-dropzone-intro', 'is-dropzone-outro')
    })

    canvasWrap.addEventListener('drop', async (e: DragEvent) => {
      canvasWrap.classList.remove('is-dropzone-intro', 'is-dropzone-outro')
      const file = e.dataTransfer?.files[0]
      if (!file) return
      const ext = file.name.split('.').pop()?.toLowerCase() ?? ''
      if (!AUDIO_EXTS.has(ext)) return  // intro/outro must be audio (no video jingles yet)
      const fp = (file as File & { path?: string }).path
      if (!fp) return
      const rect = canvasWrap.getBoundingClientRect()
      const region = getRegionAtX(e.clientX - rect.left, rect.width)
      if (region === 'main') return  // ignore — reserved for cuts/notes
      // Claim this drop so the page-wide handler doesn't reload the main file
      e.preventDefault()
      e.stopPropagation()
      if (region === 'intro') {
        patchSettings({ editorIntroPath: fp })
      } else {
        patchSettings({ editorOutroPath: fp })
      }
      await window.api.saveSettings(settings)
      // Also turn on includeIntroOutro so the user immediately sees the result.
      if (!E.includeIntroOutro) {
        E.includeIntroOutro = true
        const chk = $('editor-include-io') as HTMLInputElement | null
        if (chk) chk.checked = true
      }
      await reloadIntroOutro()
      markDirty()
    })
  }
}

// ── Editor prompt toast ───────────────────────────────────────────────────
export function showEditorPrompt(fp: string): void {
  const toast = $('editor-prompt-toast')
  if (!toast) return
  toast.dataset.path    = fp
  toast.style.display   = 'flex'
  // Auto-dismiss after 12s
  setTimeout(() => { if (toast.style.display !== 'none') dismissEditorPrompt() }, 12000)
}

export function dismissEditorPrompt(): void {
  const toast = $('editor-prompt-toast')
  if (!toast) return
  toast.classList.add('toast-dismissing')
  setTimeout(() => {
    toast.style.display = 'none'
    toast.classList.remove('toast-dismissing')
    delete toast.dataset.path
  }, 250)
}

// ── Page state ────────────────────────────────────────────────────────────
export function showState(state: 'empty' | 'loading' | 'workspace'): void {
  const emptyEl     = $('editor-empty')
  const loadingEl   = $('editor-loading')
  const workspaceEl = $('editor-workspace')
  if (emptyEl)     emptyEl.style.display     = state === 'empty'     ? '' : 'none'
  if (loadingEl)   loadingEl.style.display   = state === 'loading'   ? '' : 'none'
  if (workspaceEl) workspaceEl.style.display = state === 'workspace' ? '' : 'none'
  if (state === 'empty') renderRecentFiles()
}

/**
 * Render the "Nylig brukte filer" list in the empty state. Pulls the last
 * 5 history entries with a valid `path` from the recordings table (the R4
 * home of history — `settings.recordingHistory` was the localStorage-era
 * shadow copy, which stopped being written when the sqlite `recording` table
 * became authoritative), and makes each item clickable via openEditorWithFile.
 */
function renderRecentFiles(): void {
  void renderRecentFilesFromHistory()
}

async function renderRecentFilesFromHistory(): Promise<void> {
  const wrap = $('editor-empty-recents')
  const list = $('editor-empty-recents-list')
  if (!wrap || !list) return
  // `getHistory` already excludes trashed recordings; a failed read answers []
  // (the E2.4 fallback discipline), which renders as "no recents", not a throw.
  const hist = (await window.api.getHistory()) as RecordingEntry[]
  const recent = hist
    .filter(e => e.path && e.status === 'ok')
    .slice(0, 5)
  if (recent.length === 0) { wrap.style.display = 'none'; return }
  wrap.style.display = ''
  list.innerHTML = ''
  for (const e of recent) {
    const item = document.createElement('div')
    item.className = 'editor-recent-item'
    const fname = (e.filename || (e.path?.split(/[/\\]/).pop() ?? '')).replace(/_redigert(_\d+)?/, '')
    item.innerHTML = `
      <svg viewBox="0 0 20 20" width="14" height="14" fill="currentColor"><path d="M4 4a2 2 0 012-2h4l2 2h4a2 2 0 012 2v9a2 2 0 01-2 2H6a2 2 0 01-2-2z"/></svg>
      <span class="editor-recent-name">${escapeHtml(fname)}</span>
      <span class="editor-recent-meta">${escapeHtml(e.date || '')} · ${escapeHtml(e.duration || '')}</span>
    `
    item.addEventListener('click', () => {
      if (e.path) openEditorWithFile(e.path)
    })
    list.appendChild(item)
  }
}

/**
 * Build a compact one-line summary for the header: duration · cut count ·
 * normalize state. Lives in the sticky editor header right next to the
 * filename so the user always knows what state the file is in without
 * scrolling. Updates lazily — only when something changes (dirty marker
 * flip, cut add/remove, normalize toggle).
 */
export function updateHeaderSummary(): void {
  const summaryEl = $('editor-header-summary')
  const dirtyEl   = $('editor-dirty-dot')
  if (dirtyEl) dirtyEl.style.display = E.editorDirty ? '' : 'none'
  if (!summaryEl) return
  if (!E.filePath || !E.duration) { summaryEl.textContent = ''; return }
  const remaining = getRemainingDuration()
  const parts = [formatDuration(remaining)]
  if (E.cuts.length > 0) parts.push(`${E.cuts.length} kutt`)
  if (E.audioGainDb !== 0) {
    const sign = E.audioGainDb >= 0 ? '+' : ''
    parts.push(`normalisert (${sign}${E.audioGainDb.toFixed(1)} dB)`)
  }
  summaryEl.textContent = parts.join(' · ')
}

/**
 * Ask before discarding unsaved edits. Resolves true when it is safe to
 * proceed (nothing dirty, or the user confirmed), false when they backed out.
 *
 * Async since the native confirm() went away — every caller is an event
 * handler, so awaiting costs nothing.
 */
async function confirmDiscardIfDirty(intent: 'open' | 'close'): Promise<boolean> {
  if (!E.editorDirty) return true
  return confirmDialog({
    title: intent === 'close'
      ? t('editor.confirmClose', 'Du har ulagrede endringer. Lukk likevel?')
      : t('editor.confirmOpenOther', 'Du har ulagrede endringer. Åpne ny fil likevel?'),
    message:      t('dialog.discardEditsBody', 'Kuttene du har gjort går tapt. Selve opptaksfilen røres ikke.'),
    confirmLabel: t('dialog.discardEdits', 'Forkast endringene'),
    danger:       true,
  })
}

/**
 * Tear down the current file and return to the empty state. Releases all
 * audio data, peaks, cuts, and metadata. The user confirmed any dirty-state
 * warning already (caller's responsibility).
 */
function closeCurrentFile(): void {
  stopPlay()
  clearTranscript()
  // The shared AudioContext is deliberately NOT closed — it is the app's only
  // one and the jingle buffers below belong to it (see audio-ctx.ts).
  E.introBuffer = null
  E.outroBuffer = null
  E.introPeaks = null
  E.outroPeaks = null
  E.peaks = null
  E.cuts = []
  E.cutHistory = []
  E.cutHistoryIdx = -1
  E.suggestions = []
  E.autoSermonIndex = null
  E.clipTimes = []
  E.lastAnalyzedAt = 0
  flagEditorTab('clip', false)
  E.meta = { title: '', speaker: '', description: '', chapters: [] }
  clearDirty()
  if (E.videoEl) {
    E.videoEl.pause()
    E.videoEl.src = ''
    E.videoEl.load()
  }
  // Release the streaming player too, or the open file handle (and any temp
  // proxy) would leak past an explicit close. Same teardown the loader runs when
  // one file replaces another.
  teardownPlayback()
  E.isVideoFile = false
  E.audioGainDb = 0
  setNormalizeUI(0, false)
  E.filePath = ''
  E.duration = 0
  showState('empty')
}

export function showEditorError(msg: string): void {
  const loadingEl   = $('editor-loading')
  const errorEl     = $('editor-loading-error') ?? (() => {
    const el = document.createElement('div')
    el.id        = 'editor-loading-error'
    el.className = 'editor-error-toast'
    el.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:#ef4444;color:#fff;padding:10px 20px;border-radius:8px;font-size:14px;z-index:9999'
    document.body.appendChild(el)
    return el
  })()
  errorEl.textContent = msg
  errorEl.style.display = ''
  if (loadingEl) loadingEl.style.display = 'none'
  setTimeout(() => { errorEl.style.display = 'none' }, 6000)
}
