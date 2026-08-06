import { t } from '../i18n'
import { settings, patchSettings } from '../state'
import { escHtml as escapeHtml } from '../helpers'
import type { RecordingMetadata } from '../../types'
import { setupTranscriptPanel, clearTranscript } from './editor-transcript'
import { navigateTo } from '../ui/navigate'
import { alertDialog, confirmDialog } from '../ui/dialog'
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
import { PICK, jinglePathFor, jingleValueFor } from './editor/review-jingles'
import { toast } from '../ui/toast'
import { setupMasteringPanel } from './editor/mastering'
import { setupStageUi } from './editor/stage-ui'
import { setupEditorTabs, flagEditorTab } from './editor/tabs'
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

  // Empty-state review queue link
  $('editor-empty-review-link')?.addEventListener('click', (e) => {
    e.preventDefault()
    window.showPage('home')
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
  $('btn-export-confirm')?.addEventListener('click', () => { E.publishAfterExport = false; runExport() })
  $('btn-export-and-publish')?.addEventListener('click', () => { E.publishAfterExport = true; runExport() })
  $('export-publish-configure')?.addEventListener('click', (e) => {
    e.preventDefault()
    closeExportModal()
    // Publisering is a SECTION of the Deling tab since Fase 3 — navigateTo
    // hands the tab switch to the page's own handler, so its side effects
    // still run, and the anchor lands on the section rather than the tab top.
    navigateTo('settings', { tab: 'settings-sharing', anchor: '#settings-publish' })
  })

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
  setupReviewBanner()

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
  reviewPrepId = null
  loadAndUpdateReviewBanner()
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

// ── Review mode state (prep-and-review v5.0) ──────────────────────────────
// When non-null, the editor is in "review mode" — it pre-applies suggested
// cuts/preset/jingles from the queue entry and shows the green publish banner.
// Exported (read-only) so loader.ts can skip auto-detection in review mode.
export let reviewPrepId: string | null = null
let reviewPrep: import('../../types').EpisodePrep | null = null

/**
 * Entry point from the home-page review queue. Opens the editor, loads the
 * recording, pre-applies the suggested sermon trim as cuts (cut before
 * suggestedTrim.startSec + cut after suggestedTrim.endSec), pre-selects the
 * mastering preset, and surfaces a "Klargjort for publisering" banner with
 * the three big action buttons.
 */
export async function openEditorReviewMode(prepId: string, filePath: string): Promise<void> {
  reviewPrepId = prepId
  try {
    const entry = await window.api.reviewQueueGet(prepId)
    reviewPrep = entry?.prep ?? null
  } catch {
    reviewPrep = null
  }
  loadAndUpdateReviewBanner()
  window.showPage('editor')
  await loadFile(filePath)
  // After loadFile sets `duration`, apply the suggested trim as cuts. We have
  // to wait one tick for showState('workspace') and the canvas to settle.
  requestAnimationFrame(() => applyReviewModeDefaults())
}

function applyReviewModeDefaults(): void {
  if (!reviewPrep || !E.duration) return
  const trim = reviewPrep.suggestedTrim
  // Pre-apply the suggested trim as cuts (everything before + after).
  if (trim && trim.endSec > trim.startSec) {
    E.cuts = []
    if (trim.startSec > 0.5) {
      E.cuts.push({ start: 0, end: Math.min(trim.startSec, E.duration) })
    }
    if (trim.endSec < E.duration - 0.5) {
      E.cuts.push({ start: Math.max(0, trim.endSec), end: E.duration })
    }
    pushCutHistory()
    renderCutList()
    updateRemainingDisplay()
    drawWaveform()
    drawMinimap()
  }
  // Pre-select the mastering preset the review prep recommends.
  //
  // This used to poke `#editor-master-preset`, an id that has never existed in
  // index.html — so the preselect was a silent no-op and a review-mode export
  // shipped unmastered. The real control is the export modal's
  // `#enhance-master-preset`, and `setupEnhanceSection` syncs it FROM `E` every
  // time the modal opens: setting `E.masterPreset` is what actually takes
  // effect, both in the select and in the modal's level row (which reads `E`).
  // The select is also updated directly, for the case where the modal is
  // already open when review mode loads a file into it.
  if (reviewPrep.masterPreset) {
    E.masterPreset = reviewPrep.masterPreset
    const presetSel = $('enhance-master-preset') as HTMLSelectElement | null
    if (presetSel) presetSel.value = E.masterPreset
  }
}

function loadAndUpdateReviewBanner(): void {
  const banner = $('editor-review-banner')
  if (!banner) return
  banner.style.display = reviewPrepId ? '' : 'none'

  if (!reviewPrep || !reviewPrepId) return

  // Attention reasons block
  const attention = $('editor-review-attention')
  if (attention) {
    const reasons = reviewPrep.attentionReasons ?? []
    if (reasons.length > 0) {
      attention.innerHTML = '<strong>⚠ Trenger oppmerksomhet:</strong><ul style="margin:4px 0 0 18px;padding:0">' +
        reasons.map(r => `<li>${escapeHtml(r)}</li>`).join('') + '</ul>'
      attention.style.display = ''
    } else {
      attention.style.display = 'none'
    }
  }

  // Banner detail
  const detail = $('editor-review-banner-detail')
  if (detail) {
    if (reviewPrep.suggestedTrim) {
      const lenMin = Math.round((reviewPrep.suggestedTrim.endSec - reviewPrep.suggestedTrim.startSec) / 60)
      detail.textContent = t('review.detectedSermon', 'Vi har detektert {min} min preken og foreslått trim. Gå over og trykk publiser.').replace('{min}', String(lenMin))
    } else {
      detail.textContent = t('review.noSermonFound', 'Vi fant ingen klar preken-blokk. Sjekk filen før publisering.')
    }
  }

  // Jingle dropdowns reflect current prep state
  const introSel = $('editor-review-intro-select') as HTMLSelectElement | null
  const outroSel = $('editor-review-outro-select') as HTMLSelectElement | null
  const introLbl = $('editor-review-intro-label')
  const outroLbl = $('editor-review-outro-label')
  if (introSel) {
    const ip = reviewPrep.introPath
    introSel.value = jingleValueFor(ip, settings.editorIntroPath)
    if (introLbl) introLbl.textContent = ip ? ip.split(/[/\\]/).pop() ?? '' : ''
  }
  if (outroSel) {
    const op = reviewPrep.outroPath
    outroSel.value = jingleValueFor(op, settings.editorOutroPath)
    if (outroLbl) outroLbl.textContent = op ? op.split(/[/\\]/).pop() ?? '' : ''
  }
}

/** Say that the episode has left the queue, and put the banner's controls back
 *  where the (unchanged) prep says they belong.
 *
 *  Every push can come back `false`: the queue is a shared blob, and 14 days of
 *  neglect auto-discards an entry while an editor window sits open on it. Before
 *  the pushes were real this could not happen, because nothing was ever saved. */
function reviewEntryVanished(): void {
  toast('warn', t('review.errNotFound',
    'Episoden ligger ikke lenger i gjennomgangs-køen — den kan allerede være publisert eller forkastet.'))
  loadAndUpdateReviewBanner()
}

/** Turn a review-queue error code into a sentence. The one code the backend
 *  produces on its own is "the id is no longer in the queue" — which reads as
 *  a mystery unless we say what it means. */
function describeReviewError(code: string | undefined): string | undefined {
  if (!code) return undefined
  if (code === 'review_entry_not_found') {
    return t('review.errNotFound', 'Episoden ligger ikke lenger i gjennomgangs-køen — den kan allerede være publisert eller forkastet.')
  }
  return code
}

function setupReviewBanner(): void {
  $('btn-review-publish')?.addEventListener('click', async () => {
    if (!reviewPrepId) return
    const btn = $('btn-review-publish') as HTMLButtonElement | null
    if (!btn) return
    // Idempotency: disable immediately on first click
    if (btn.disabled) return
    btn.disabled = true
    const orig = btn.textContent
    btn.textContent = 'Publiserer…'
    try {
      const r = await window.api.reviewQueuePublish(reviewPrepId)
      if (r.ok) {
        btn.textContent = '✓ Publisert!'
        setTimeout(() => {
          reviewPrepId = null
          reviewPrep = null
          loadAndUpdateReviewBanner()
          window.showPage('home')
        }, 1500)
      } else {
        btn.disabled = false
        btn.textContent = orig
        await alertDialog({
          title:   t('dialog.publishFailedTitle', 'Kunne ikke publisere'),
          message: describeReviewError(r.error),
          tone:    'error',
        })
      }
    } catch (err) {
      btn.disabled = false
      btn.textContent = orig
      await alertDialog({
        title:   t('dialog.publishFailedTitle', 'Kunne ikke publisere'),
        message: (err as Error).message,
        tone:    'error',
      })
    }
  })

  $('btn-review-edit')?.addEventListener('click', () => {
    // Drop the review banner so the user gets the normal editor — the file
    // and cuts stay loaded, they just lose the publish-button shortcut.
    reviewPrepId = null
    loadAndUpdateReviewBanner()
  })

  $('btn-review-discard')?.addEventListener('click', async () => {
    if (!reviewPrepId) return
    const ok = await confirmDialog({
      title:        t('dialog.discardEpisodeTitle', 'Forkast denne episoden?'),
      message:      t('dialog.discardEpisodeBody', 'Selve opptaket beholdes, men det blir ikke publisert.'),
      confirmLabel: t('dialog.discardEpisode', 'Forkast'),
      danger:       true,
    })
    if (!ok) return
    // The backend answers with a bool: false means the entry was already gone
    // from the queue. Silently pretending it worked would send the user home
    // believing they had made a decision that never landed.
    const done = await window.api.reviewQueueDiscard(reviewPrepId)
    if (!done) {
      await alertDialog({
        title:   t('dialog.discardFailedTitle', 'Kunne ikke forkaste episoden'),
        message: describeReviewError('review_entry_not_found'),
        tone:    'error',
      })
      return
    }
    reviewPrepId = null
    reviewPrep = null
    loadAndUpdateReviewBanner()
    window.showPage('home')
  })

  // Jingle selectors. Both dropdowns run the same three steps — resolve the
  // value to a path (opening the picker for «Egen fil»), push ONLY that field,
  // and mirror it locally if and only if the push landed.
  const wireJingle = (
    selectId: string,
    field: 'introPath' | 'outroPath',
    defaultPath: () => string | undefined,
  ): void => {
    $(selectId)?.addEventListener('change', async () => {
      if (!reviewPrepId) return
      const sel = $(selectId) as HTMLSelectElement
      const stored = reviewPrep?.[field]
      let path = jinglePathFor(sel.value, defaultPath())
      if (path === PICK) {
        const fp = await window.api.pickAudioFile()
        if (!fp) {
          // Cancelled: back to what is actually stored (which may be «Ingen» —
          // the old restore assumed «Standard» and quietly changed the choice).
          sel.value = jingleValueFor(stored, defaultPath())
          return
        }
        path = fp
      }
      // One key per call: the backend leaves the OTHER jingle untouched
      // precisely because it is absent from this patch.
      const ok = await window.api.reviewQueueUpdateJingles(reviewPrepId, { [field]: path })
      if (!ok) { reviewEntryVanished(); return }
      if (reviewPrep) reviewPrep[field] = path ?? undefined
      loadAndUpdateReviewBanner()
    })
  }
  wireJingle('editor-review-intro-select', 'introPath', () => settings.editorIntroPath)
  wireJingle('editor-review-outro-select', 'outroPath', () => settings.editorOutroPath)

  // Mastering preset → the queue entry.
  //
  // `applyReviewModeDefaults` seeds the export modal's select FROM the prep;
  // this is the return leg, which never existed — an operator who reviewed the
  // episode and changed the preset had that choice live only in `E`, so it was
  // gone the moment the editor closed and the next open re-seeded the old one.
  //
  // Only a deliberate human pick pushes: the seeding above and the export
  // modal's one-click auto-enhance both assign `.value` programmatically, which
  // does not fire `change`. That is the wanted split — auto-enhance clears
  // mastering for THIS export rather than proposing a new default for the
  // episode. The listener is attached once here (the select is static markup);
  // export.ts keeps its own listener for `E`, and both run.
  $('enhance-master-preset')?.addEventListener('change', async () => {
    if (!reviewPrepId) return
    const presetId = ($('enhance-master-preset') as HTMLSelectElement).value
    const ok = await window.api.reviewQueueUpdateMasterPreset(reviewPrepId, presetId)
    // No banner repaint on failure: the preset the user just picked still
    // governs the export they are about to run — only the queue entry (which no
    // longer exists) missed it.
    if (!ok) { toast('warn', t('review.errNotFound',
      'Episoden ligger ikke lenger i gjennomgangs-køen — den kan allerede være publisert eller forkastet.')); return }
    if (reviewPrep) reviewPrep.masterPreset = presetId
  })

  // NOTE: the suggested TRIM is deliberately NOT pushed back. The editor's cuts
  // and `prep.suggestedTrim` are different models — cuts are a list of removed
  // ranges anywhere in the file, the trim is one kept span — and collapsing the
  // former into the latter would silently discard every cut but the outer two.
  // `reviewQueueUpdateTrim` exists and is tested for the caller that will do the
  // conversion honestly; wiring it to `E.cuts` here would not be that caller.
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
  // suggestions / clipTimes / meta / isVideoFile / audioGainDb / reviewPrepId.
  // Those are owned by the open-file lifecycle, not the tab-visibility one.
  reviewPrep = null
  loadAndUpdateReviewBanner()
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
 * 5 entries from settings.recordingHistory that have a valid `path`, and
 * makes each item clickable to load via openEditorWithFile.
 *
 * Also shows the "Gjennomgangs-kø" link when there are pending review-queue
 * entries. The link navigates to home (where the prep queue lives).
 */
function renderRecentFiles(): void {
  // The review-queue link is decided on its own — it used to be skipped
  // entirely by the early return below, so a fresh install with a queued
  // episode but no recent-file history never saw it.
  void refreshEmptyStateReviewLink()

  const wrap = $('editor-empty-recents')
  const list = $('editor-empty-recents-list')
  if (!wrap || !list) return
  const hist = settings.recordingHistory ?? []
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
 * Show the "Gjennomgangs-kø →" link only when there is something in the queue.
 *
 * It used to be shown unconditionally — a link promising a queue that, until
 * this branch wired `review_queue_list` up, was always empty. Now the queue is
 * real, so ask it: an empty queue gets no link, and a non-empty one says how
 * many are waiting.
 */
async function refreshEmptyStateReviewLink(): Promise<void> {
  const wrap = $('editor-empty-review')
  const link = $('editor-empty-review-link')
  if (!wrap) return
  wrap.style.display = 'none'
  try {
    const entries = await window.api.reviewQueueList()
    const pending = entries.filter(e =>
      e.prep.status !== 'published' && e.prep.status !== 'discarded')
    if (pending.length === 0) return
    if (link) {
      link.textContent = pending.length === 1
        ? t('editor.gotoReviewOne', 'Gjennomgangs-kø (1 episode) →')
        : t('editor.gotoReviewN', 'Gjennomgangs-kø ({n} episoder) →').replace('{n}', String(pending.length))
    }
    wrap.style.display = ''
  } catch (err) {
    // A queue we cannot read is not a queue we should advertise.
    console.warn('[editor] review queue lookup failed:', err)
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
  reviewPrepId = null
  reviewPrep = null
  loadAndUpdateReviewBanner()
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
