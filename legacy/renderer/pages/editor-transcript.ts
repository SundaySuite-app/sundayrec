/**
 * Editor transcription panel.
 *
 * Flow:
 *   1. User clicks "▶ Transkriber".
 *   2. We open the model+language modal.
 *   3. If the chosen model isn't downloaded, we download it (progress UI).
 *   4. Then we kick off whisperTranscribe and show the progress modal.
 *   5. On success we render the segments below the timeline and save a
 *      <recording>.transcript.json sidecar.
 *
 *   Future: clicking a segment seeks the playhead. The current-segment
 *   highlight updates from the editor's animation loop via setCurrentTranscriptTime().
 */

import { t } from '../i18n'
import type { TranscriptData, RecordingMetadata } from '../../types'
import { closeModal, openModal } from '../ui/modal-manager'
import { alertDialog, confirmDialog } from '../ui/dialog'
import { toast } from '../ui/toast'
import { attachProgress, type ProgressHandle } from '../ui/progress'
import { E } from './editor/state'
import { renderChapterList } from './editor/metadata'
import { drawWaveform } from './editor/waveform'
import {
  setupCompanionPanel,
  clearCompanion,
  renderCompanionControls,
  setCompanionSeek,
} from './editor-companion'

interface ModelStatus {
  id:             string
  label:          string
  description:    string
  sizeBytes:      number
  quality:        string
  realtimeFactor: number
  installed:      boolean
  sizeOk:         boolean
}

let currentFilePath: string | null = null
let currentTranscript: TranscriptData | null = null
let selectedModelId: string = 'ggml-large-v3-turbo-q5_0'
let activeJobId: string | null = null
let modelStatuses: ModelStatus[] = []
let onSeekCallback: ((sec: number) => void) | null = null

const $ = (id: string) => document.getElementById(id)

// Extensions that route to the SundayEdit hand-off (video only — SundayEdit is a
// video-captioning tool). Mirrors the editor's video set, kept local so this
// panel has no dependency on editor-page internals.
const SUNDAYEDIT_VIDEO_EXTS = new Set([
  '.mp4', '.mov', '.m4v', '.avi', '.wmv', '.mkv', '.webm', '.flv', '.ts', '.mts', '.m2ts', '.3gp',
])
function isVideoPath(p: string): boolean {
  const ext = ('.' + (p.split('.').pop()?.toLowerCase() ?? ''))
  return SUNDAYEDIT_VIDEO_EXTS.has(ext)
}

export function setupTranscriptPanel(onSeek: (sec: number) => void): void {
  onSeekCallback = onSeek
  // R8: wire the AI sermon companion. It reads the current transcript through a
  // getter (no module-coupling) and seeks via the same callback as the segments.
  setupCompanionPanel(() => currentTranscript)
  setCompanionSeek(onSeek)
  $('btn-transcribe')?.addEventListener('click', openTranscribeModal)
  $('btn-transcribe-cancel')?.addEventListener('click', closeTranscribeModal)
  $('btn-transcribe-start')?.addEventListener('click', startTranscription)
  $('btn-transcribe-progress-cancel')?.addEventListener('click', cancelActiveJob)
  $('btn-transcript-export')?.addEventListener('click', () => { void exportSubtitleFile('srt') })
  $('btn-transcript-export-vtt')?.addEventListener('click', () => { void exportSubtitleFile('vtt') })
  $('btn-transcript-export-txt')?.addEventListener('click', () => { void exportSubtitleFile('txt') })
  $('btn-transcript-delete')?.addEventListener('click', deleteTranscript)
  $('btn-transcript-sundayedit')?.addEventListener('click', sendToSundayEdit)

  // Probe availability once at startup so the button can be disabled with
  // an inline explanation if the binary didn't ship (CI build issue,
  // unsupported platform, missing dependency on Linux build).
  void checkBinaryAvailabilityOnce()

  // Listen for progress events from main process
  window.api.on?.('whisper-progress', (payload: unknown) => {
    if (!payload || typeof payload !== 'object') return
    const p = payload as { jobId: string; percent: number; processedSec?: number; totalSec?: number }
    if (p.jobId !== activeJobId) return
    updateProgressUI(transcribeFraction(p), t('transcript.progressTitle', 'Transkriberer…'))
  })

  window.api.on?.('whisper-model-progress', (payload: unknown) => {
    if (!payload || typeof payload !== 'object') return
    const p = payload as { id: string; bytesDownloaded: number; bytesTotal: number; fraction: number | null }
    updateDownloadUI(p)
  })
}

async function checkBinaryAvailabilityOnce(): Promise<void> {
  try {
    const status = await window.api.whisperStatus()
    const btn = $('btn-transcribe') as HTMLButtonElement | null
    if (!btn) return
    if (!status.binaryAvailable) {
      btn.disabled = true
      btn.title = t('transcript.unavailableHint',
        'Transkribering er ikke tilgjengelig i denne versjonen av appen på denne maskinen.')
      btn.textContent = t('transcript.unavailable', '✕ Ikke tilgjengelig')
    }
  } catch {
    // If status check itself fails, leave the button enabled — user can
    // click and see the actual error then.
  }
}

// ── SundayEdit hand-off (Sunday-suite integration) ───────────────────────────
// Shows the "→ SundayEdit" button only when the integration is enabled AND the
// open file is a video. Reads the opt-in settings each load so toggling them
// in Settings reflects on the next file open.
async function updateSundayEditButton(): Promise<void> {
  const btn = $('btn-transcript-sundayedit') as HTMLElement | null
  if (!btn) return
  let show = false
  try {
    if (currentFilePath && isVideoPath(currentFilePath)) {
      const s = await window.api.getIntegrationSettings()
      show = !!s.enabled && !!s.sundayedit?.enabled
    }
  } catch { show = false }
  btn.style.display = show ? '' : 'none'
}

// Sends the open video to SundayEdit, primed with sermon context + the speaker
// name as a glossary term (improves recognition of the name). Fire-and-forget
// from the user's perspective; SundayEdit returns captions out-of-band.
async function sendToSundayEdit(): Promise<void> {
  if (!currentFilePath) return
  let context = 'Preken'
  const glossary: string[] = []
  try {
    const meta = await window.api.editorReadMeta?.(currentFilePath) as RecordingMetadata | null
    if (meta?.speaker) { context = `Preken. Taler: ${meta.speaker}`; glossary.push(meta.speaker) }
  } catch { /* no metadata — generic context */ }

  const btn = $('btn-transcript-sundayedit') as HTMLButtonElement | null
  try {
    const res = await window.api.sundayEditSend({ videoPath: currentFilePath, context, glossary })
    if (!res.ok && btn) {
      btn.textContent = res.error === 'sundayedit_not_installed'
        ? t('integrations.sundayEditMissing', 'SundayEdit ikke funnet')
        : t('integrations.sundayEditFailed', 'Kunne ikke åpne')
      setTimeout(() => { btn.textContent = '→ SundayEdit' }, 2500)
    }
  } catch {
    if (btn) { btn.textContent = t('integrations.sundayEditFailed', 'Kunne ikke åpne'); setTimeout(() => { btn.textContent = '→ SundayEdit' }, 2500) }
  }
}

/** Called by editor when a file loads — clears state and loads existing sidecar if any. */
export async function loadTranscriptForFile(filePath: string): Promise<void> {
  currentFilePath = filePath
  currentTranscript = null
  renderPanel()
  void updateSundayEditButton()
  // Try to load sidecar
  try {
    const sidecar = await window.api.editorReadTranscript?.(filePath) as TranscriptData | null
    if (sidecar && sidecar.version === 1) {
      currentTranscript = sidecar
      renderPanel()
    }
  } catch {}
}

export function clearTranscript(): void {
  currentFilePath = null
  currentTranscript = null
  clearCompanion()
  renderPanel()
}

/** Called from editor animate-loop on each frame so we can highlight which
 *  segment is currently playing. Cheap binary-search on segments. */
export function setCurrentTranscriptTime(sec: number): void {
  if (!currentTranscript) return
  const segs = currentTranscript.segments
  // Find segment containing sec
  let idx = -1
  for (let i = 0; i < segs.length; i++) {
    if (sec >= segs[i].start && sec < segs[i].end) { idx = i; break }
  }
  highlightSegment(idx)
}

// Reading the transcript while it plays is a legitimate thing to do, and an
// auto-scroll that yanks the panel back every few seconds makes it impossible.
// After a manual scroll the follow behaviour steps aside for this long.
const FOLLOW_PAUSE_AFTER_USER_SCROLL_MS = 3000
// scrollIntoView({behavior:'smooth'}) emits scroll events of its own for the
// length of the animation. Without this window every auto-scroll would look
// like a user scroll and permanently disable the next one.
const PROGRAMMATIC_SCROLL_WINDOW_MS = 800

let lastUserScrollMs = 0
let programmaticScrollUntil = 0
let scrollTrackedContainer: HTMLElement | null = null

/** Passive scroll listener on the segment list, bound once per container (the
 *  panel is rebuilt from innerHTML on every render, so the element changes). */
function trackUserScroll(container: HTMLElement): void {
  if (scrollTrackedContainer === container) return
  scrollTrackedContainer = container
  container.addEventListener(
    'scroll',
    () => {
      if (performance.now() < programmaticScrollUntil) return
      lastUserScrollMs = performance.now()
    },
    { passive: true },
  )
}

let lastHighlightedIdx = -1
let highlightScrollRaf = 0

function highlightSegment(idx: number): void {
  if (idx === lastHighlightedIdx) return
  lastHighlightedIdx = idx
  const container = $('editor-transcript-segments')
  if (!container) return
  trackUserScroll(container)
  container.querySelectorAll('.editor-transcript-segment').forEach((el, i) => {
    el.classList.toggle('is-current', i === idx)
  })

  if (idx < 0) return
  // The two getBoundingClientRect reads below are layout FLUSHES, and this runs
  // from the playback rAF — right after the class writes above dirtied style.
  // Deferring them to their own frame keeps the read/write phases apart instead
  // of forcing a synchronous relayout in the middle of the playback loop.
  if (highlightScrollRaf) cancelAnimationFrame(highlightScrollRaf)
  highlightScrollRaf = requestAnimationFrame(() => {
    highlightScrollRaf = 0
    if (performance.now() - lastUserScrollMs < FOLLOW_PAUSE_AFTER_USER_SCROLL_MS) return
    const el = container.children[idx] as HTMLElement | undefined
    if (!el) return
    const rect = el.getBoundingClientRect()
    const cRect = container.getBoundingClientRect()
    if (rect.top >= cRect.top && rect.bottom <= cRect.bottom) return
    programmaticScrollUntil = performance.now() + PROGRAMMATIC_SCROLL_WINDOW_MS
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })
}

// ─── Rendering ──────────────────────────────────────────────────────────────

function renderPanel(): void {
  const body = $('editor-transcript-body')
  if (!body) return
  const exportBtn    = $('btn-transcript-export')     as HTMLElement | null
  const exportVttBtn = $('btn-transcript-export-vtt') as HTMLElement | null
  const exportTxtBtn = $('btn-transcript-export-txt') as HTMLElement | null
  const deleteBtn    = $('btn-transcript-delete')     as HTMLElement | null

  if (!currentTranscript || currentTranscript.segments.length === 0) {
    body.innerHTML = `<div class="editor-transcript-empty">${t('transcript.empty', 'Ingen transkripsjon ennå. Klikk «Transkriber» for å lage søkbar tekst av talen.')}</div>`
    if (exportBtn)    exportBtn.style.display    = 'none'
    if (exportVttBtn) exportVttBtn.style.display = 'none'
    if (exportTxtBtn) exportTxtBtn.style.display = 'none'
    if (deleteBtn)    deleteBtn.style.display    = 'none'
    const companionSection = $('editor-companion-section')
    if (companionSection) companionSection.style.display = 'none'
    clearCompanion()
    return
  }

  const meta = currentTranscript
  const d = new Date(meta.createdAt)
  const dateStr = `${d.getDate()}.${d.getMonth() + 1}.${d.getFullYear()} ${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}`
  const langLabel = meta.language === 'auto' ? t('transcript.langAuto', 'Auto') : meta.language.toUpperCase()
  const segCount = meta.segments.length

  body.innerHTML = `
    <div class="editor-transcript-meta">
      ${dateStr} · ${meta.model} · ${langLabel} · ${segCount} ${t('transcript.segments', 'segmenter')}
    </div>
    <div class="editor-transcript-meta" style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">
      <button class="btn-ghost btn-sm" id="btn-detect-chapters">${t('transcript.detectChapters', '✦ Generer kapitler fra tema')}</button>
      <span id="detect-chapters-hint" style="opacity:.75"></span>
    </div>
    <div class="editor-transcript-segments" id="editor-transcript-segments"></div>
  `

  const detectBtn = $('btn-detect-chapters') as HTMLButtonElement | null
  if (detectBtn) detectBtn.addEventListener('click', generateChaptersFromTranscript)

  const container = $('editor-transcript-segments')!
  for (const seg of meta.segments) {
    const row = document.createElement('div')
    row.className = 'editor-transcript-segment'
    const time = document.createElement('span')
    time.className = 'editor-transcript-segment-time'
    time.textContent = formatTime(seg.start)
    const text = document.createElement('span')
    text.className = 'editor-transcript-segment-text'
    text.textContent = seg.text
    row.append(time, text)
    row.addEventListener('click', () => onSeekCallback?.(seg.start))
    container.appendChild(row)
  }

  if (exportBtn)    exportBtn.style.display    = ''
  if (exportVttBtn) exportVttBtn.style.display = ''
  if (exportTxtBtn) exportTxtBtn.style.display = ''
  if (deleteBtn)    deleteBtn.style.display    = ''

  // R8: reveal the companion section and (re)render its header controls. We
  // reset its body so a previous file's companion doesn't linger; the user
  // clicks "Lag prekenhjelp" to build for this transcript.
  const companionSection = $('editor-companion-section')
  if (companionSection) companionSection.style.display = ''
  clearCompanion()
  void renderCompanionControls()
}

function formatTime(sec: number): string {
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = Math.floor(sec % 60)
  return h > 0
    ? `${h}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`
    : `${m}:${String(s).padStart(2,'0')}`
}

/**
 * Generate topic chapters from the transcript. The Rust detector scans each
 * line for Bible references ("Johannes 3:16") and enumeration points ("for det
 * første", "punkt 2") and returns { time, title } markers on the recording's
 * timeline. We MERGE into E.meta.chapters — keeping any manual chapters and
 * skipping detected ones that duplicate an existing marker — then re-render the
 * chapter list + waveform dots. The export embeds these as ID3 CHAP/CTOC.
 */
async function generateChaptersFromTranscript(): Promise<void> {
  if (!currentTranscript || currentTranscript.segments.length === 0) return
  const btn  = $('btn-detect-chapters') as HTMLButtonElement | null
  const hint = $('detect-chapters-hint')
  if (btn) { btn.disabled = true; btn.textContent = t('transcript.detecting', 'Analyserer tema…') }

  const lines = currentTranscript.segments.map(s => ({ start: s.start, text: s.text }))
  // Use the transcript's detected language so English sermons get English
  // Bible-name + point detection (Norwegian otherwise).
  const lang = currentTranscript.language || 'no'
  let detected: Array<{ time: number; title: string }> = []
  try {
    detected = (await window.api.editorDetectChapters(lines, lang)) as Array<{ time: number; title: string }>
  } catch {
    detected = []
  }

  // Merge into the existing chapters, de-duping against markers already present
  // (same title within 2 s, or any chapter within 1 s — manual or re-detected).
  const existing = E.meta.chapters
  let added = 0
  for (const ch of detected) {
    const dup = existing.some(
      e => (e.title === ch.title && Math.abs(e.time - ch.time) < 2) || Math.abs(e.time - ch.time) < 1,
    )
    if (!dup) { existing.push({ time: ch.time, title: ch.title }); added++ }
  }
  existing.sort((a, b) => a.time - b.time)
  E.metaDirty = true
  renderChapterList()
  drawWaveform()

  if (btn) { btn.disabled = false; btn.textContent = t('transcript.detectChapters', '✦ Generer kapitler fra tema') }
  if (hint) {
    hint.textContent =
      added > 0
        ? `${added} ${t('transcript.chaptersAdded', 'kapitler lagt til')}`
        : detected.length > 0
          ? t('transcript.chaptersAllPresent', 'Alle funne kapitler finnes allerede')
          : t('transcript.chaptersNoneFound', 'Fant ingen tema-kapitler i talen')
  }
}

// ─── Modal: choose model + language ─────────────────────────────────────────

async function openTranscribeModal(): Promise<void> {
  if (!currentFilePath) return

  // Load fresh model statuses every time the modal opens — user may have
  // downloaded one in a different session and we want to show "Installed".
  try {
    const status = await window.api.whisperStatus()
    if (!status.binaryAvailable) {
      await alertDialog({
        title: t('transcript.errNoBinary', 'Whisper er ikke tilgjengelig på denne plattformen. Kontakt support.'),
        tone:  'error',
      })
      return
    }
    modelStatuses = status.models
    renderModelList()
  } catch (err) {
    await alertDialog({
      title:   t('transcript.errStatusFailed', 'Kunne ikke sjekke Whisper-status'),
      message: (err as Error).message,
      tone:    'error',
    })
    return
  }

  openModal('transcribe-modal')
}

function closeTranscribeModal(): void {
  closeModal('transcribe-modal')
}

function renderModelList(): void {
  const list = $('transcribe-model-list')
  if (!list) return
  list.innerHTML = ''

  // Default-select the "best"-quality model that's installed, otherwise the
  // best-quality one regardless of install status (user will be prompted to
  // download).
  const installed = modelStatuses.find(m => m.installed && m.sizeOk && m.quality === 'best')
  if (installed) selectedModelId = installed.id
  else {
    const best = modelStatuses.find(m => m.quality === 'best')
    if (best) selectedModelId = best.id
  }

  for (const m of modelStatuses) {
    const card = document.createElement('label')
    card.className = 'transcribe-model-card'
    if (m.id === selectedModelId) card.classList.add('is-selected')

    const radio = document.createElement('input')
    radio.type = 'radio'
    radio.name = 'transcribe-model'
    radio.value = m.id
    radio.checked = m.id === selectedModelId
    radio.className = 'transcribe-model-card-radio'
    radio.addEventListener('change', () => {
      selectedModelId = m.id
      list.querySelectorAll('.transcribe-model-card').forEach(c => c.classList.remove('is-selected'))
      card.classList.add('is-selected')
    })

    const body = document.createElement('div')
    body.className = 'transcribe-model-card-body'

    const title = document.createElement('div')
    title.className = 'transcribe-model-card-title'
    const titleText = document.createElement('span')
    titleText.textContent = m.label
    title.appendChild(titleText)
    if (m.quality === 'best') {
      const badge = document.createElement('span')
      badge.className = 'transcribe-model-card-badge'
      badge.textContent = t('transcript.recommended', 'Anbefalt')
      title.appendChild(badge)
    }
    const statusEl = document.createElement('span')
    statusEl.className = m.installed
      ? 'transcribe-model-card-status transcribe-model-card-status-installed'
      : 'transcribe-model-card-status transcribe-model-card-status-missing'
    statusEl.textContent = m.installed
      ? t('transcript.modelInstalled', '✓ Lastet ned')
      : `${formatSize(m.sizeBytes)}`
    title.appendChild(statusEl)
    body.appendChild(title)

    const desc = document.createElement('div')
    desc.className = 'transcribe-model-card-desc'
    desc.textContent = m.description
    body.appendChild(desc)

    const meta = document.createElement('div')
    meta.className = 'transcribe-model-card-meta'
    meta.innerHTML = `
      <span>${t('transcript.speed', 'Hastighet')}: ~${m.realtimeFactor}x sanntid</span>
      <span>${t('transcript.size', 'Størrelse')}: ${formatSize(m.sizeBytes)}</span>
    `
    body.appendChild(meta)

    card.appendChild(radio)
    card.appendChild(body)
    list.appendChild(card)
  }
}

function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(0)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

// ─── Run transcription ──────────────────────────────────────────────────────

async function startTranscription(): Promise<void> {
  if (!currentFilePath) return
  closeTranscribeModal()

  const language = ($('transcribe-language') as HTMLSelectElement | null)?.value ?? 'auto'
  const translate = ($('transcribe-translate') as HTMLInputElement | null)?.checked ?? false

  // Open the progress modal so the user has feedback while we work
  showProgressModal(
    t('transcript.progressTitle', 'Transkriberer…'),
    t('progress.preparing', 'Forbereder …'),
  )

  // 1. If model not installed, download first
  const modelStatus = modelStatuses.find(m => m.id === selectedModelId)
  if (!modelStatus) {
    closeProgressModal()
    await alertDialog({
      title:   t('transcript.errUnknownModel', 'Ukjent modell'),
      message: selectedModelId,
      tone:    'error',
    })
    return
  }
  if (!modelStatus.installed || !modelStatus.sizeOk) {
    setProgressTitle(t('transcript.downloadTitle', 'Laster ned modell…'))
    updateProgressUI(null, formatSize(modelStatus.sizeBytes))
    const dl = await window.api.whisperDownloadModel(selectedModelId)
    if (!dl.ok) {
      closeProgressModal()
      if (dl.error !== 'cancelled') {
        await alertDialog({
          title:   t('transcript.errDownload', 'Modell-nedlasting feilet'),
          message: dl.error,
          tone:    'error',
        })
      }
      return
    }
  }

  // 2. Transcribe
  setProgressTitle(t('transcript.progressTitle', 'Transkriberer…'))
  // The stripe covers the run-up that genuinely has no denominator: the ffmpeg
  // convert to 16 kHz mono and loading a 1.5 GB model into Metal. The first
  // `whisper://progress` tick swaps it for a real bar and a remaining-time line.
  updateProgressUI(null, t('transcript.preparingModel', 'Klargjør modell…'))
  activeJobId = 'whisper-' + Date.now()
  try {
    const res = await window.api.whisperTranscribe({
      filePath:  currentFilePath,
      modelId:   selectedModelId,
      language,
      translate,
      jobId:     activeJobId,
    })
    closeProgressModal()
    if (!res.ok || !res.transcript) {
      if (res.error !== 'cancelled') {
        await alertDialog({
          title:   t('transcript.errFailed', 'Transkribering feilet'),
          message: res.error ?? undefined,
          tone:    'error',
        })
      }
      return
    }
    currentTranscript = res.transcript
    renderPanel()
    // Save sidecar
    try {
      await window.api.editorWriteTranscript?.(currentFilePath, res.transcript)
    } catch (err) {
      console.warn('[transcript] sidecar save failed', err)
    }
  } finally {
    activeJobId = null
  }
}

function cancelActiveJob(): void {
  // Cancel calls are fire-and-forget (IPC return values aren't actionable
  // for us — main process does the actual abort). But the IPC bridge can
  // throw if main is mid-restart (e.g. renderer crashed and reloaded
  // before main re-registered handlers), so wrap both .catch'es to keep
  // the modal close path running even when cancel IPC fails.
  if (activeJobId) {
    void window.api.whisperCancelTranscribe(activeJobId).catch((err: unknown) => {
      console.warn('[transcript] whisperCancelTranscribe failed:', err)
    })
  }
  if (selectedModelId) {
    void window.api.whisperCancelDownload(selectedModelId).catch((err: unknown) => {
      console.warn('[transcript] whisperCancelDownload failed:', err)
    })
  }
  // The modal closes on the COMPLETION event, not on a timer: cancelling
  // whisper raises a flag its abort callback polls between decoder steps, so
  // the run ends a step or two later and the awaited call resolves with
  // `cancelled` — which is what runs closeProgressModal. Saying «Avbryter…»
  // for that moment is honest; snapping the dialog shut while inference is
  // still winding down is not (and it used to hide a cancel that failed).
  // The timer that remains is a FALLBACK for a backend that never answers.
  updateProgressUI(null, t('transcript.cancelling', 'Avbryter…'))
  if (cancelFallbackTimer) clearTimeout(cancelFallbackTimer)
  cancelFallbackTimer = window.setTimeout(closeProgressModal, CANCEL_FALLBACK_MS)
}

/**
 * How far along a `whisper://progress` tick says we are, as a 0..1 fraction.
 *
 * The backend sends two clocks (see `TranscribeTick` in src-tauri): whisper's
 * own percentage, which only moves in 5 % steps — on a 90-minute service that
 * is roughly twenty updates for the whole run, far too coarse to estimate a
 * rate from — and `processedSec`, the decoder's real position in the audio,
 * which the segment callback advances every few seconds.
 *
 * Both are truthful LOWER BOUNDS on where inference has reached, so the max of
 * the two is the best answer available: the position clock carries the run, and
 * the percentage covers the moment before the first segment lands (and any
 * build where the segment callback gives us nothing).
 */
function transcribeFraction(p: { percent?: number; processedSec?: number; totalSec?: number }): number {
  const total = typeof p.totalSec === 'number' && p.totalSec > 0 ? p.totalSec : 0
  const processed = typeof p.processedSec === 'number' && p.processedSec > 0 ? p.processedSec : 0
  const byPosition = total > 0 ? processed / total : 0
  const byPercent = typeof p.percent === 'number' && isFinite(p.percent) ? p.percent / 100 : 0
  return Math.max(0, Math.min(1, Math.max(byPosition, byPercent)))
}

/** The shared bar+ETA widget for the whole modal, alive only while it is open. */
let progressUi: ProgressHandle | null = null
/**
 * A cancel the backend never answers must not strand the modal. It normally
 * closes on the COMPLETION path (the awaited call resolves with `cancelled`),
 * which is why this is a fallback and not the mechanism.
 */
const CANCEL_FALLBACK_MS = 5000
let cancelFallbackTimer = 0

function showProgressModal(title: string, label: string): void {
  setProgressTitle(title)
  const host = $('transcribe-progress-host')
  progressUi = host ? attachProgress(host, { label }) : null
  // Nothing has been measured yet — the stripe says "starting", not "0 %".
  progressUi?.update(null)
  openModal('transcribe-progress-modal')
}

function setProgressTitle(title: string): void {
  const el = $('transcribe-progress-title')
  if (el) el.textContent = title
}

/** `null` = running with no denominator (the ffmpeg convert + model load ahead
 *  of inference report nothing at all, and a bar pinned at 0 % reads as hung). */
function updateProgressUI(fraction: number | null, label?: string): void {
  progressUi?.update(fraction, label)
}

function updateDownloadUI(p: { id: string; bytesDownloaded: number; bytesTotal: number; fraction: number | null }): void {
  // Bytes make an excellent ETA source: a download's rate is far steadier than
  // inference's, so «ca. 2 min igjen» settles within seconds of the first chunk.
  updateProgressUI(
    p.fraction,
    `${formatSize(p.bytesDownloaded)} / ${formatSize(p.bytesTotal)}`,
  )
}

function closeProgressModal(): void {
  if (cancelFallbackTimer) { clearTimeout(cancelFallbackTimer); cancelFallbackTimer = 0 }
  progressUi?.destroy()
  progressUi = null
  closeModal('transcribe-progress-modal')
}

// ─── Sidecar helpers ────────────────────────────────────────────────────────

async function deleteTranscript(): Promise<void> {
  if (!currentFilePath || !currentTranscript) return
  const ok = await confirmDialog({
    title:        t('transcript.confirmDelete', 'Slett transkripsjonen?'),
    message:      t('dialog.deleteTranscriptBody', 'Selve opptaket beholdes. Du kan transkribere på nytt senere.'),
    confirmLabel: t('dialog.delete', 'Slett'),
    danger:       true,
  })
  if (!ok) return
  try {
    await window.api.editorDeleteTranscript?.(currentFilePath)
  } catch {}
  currentTranscript = null
  renderPanel()
}

/**
 * Export the transcript as SRT / VTT / plain text.
 *
 * This used to build the file in the renderer and hand it to a synthetic
 * `<a download>` — a *browser* download inside a desktop app: the file landed
 * in whatever the webview considered its download dir (on macOS often nowhere
 * the user could find it), with no say in the name or folder. Now the user
 * picks the destination in the native save dialog and the backend
 * (`whisper_export_transcript`, pure formatting + one fs write) renders it, so
 * SRT/VTT/TXT come out byte-identical to every other place we emit them.
 */
async function exportSubtitleFile(fmt: 'srt' | 'vtt' | 'txt'): Promise<void> {
  if (!currentTranscript || !currentFilePath) return
  const baseName = currentFilePath.split(/[/\\]/).pop()?.replace(/\.[^.]+$/, '') ?? 'transcript'
  const path = await window.api.pickSavePath({
    defaultPath: `${currentFilePath.replace(/\.[^./\\]+$/, '')}.${fmt}`,
    name:        fmt.toUpperCase(),
    extensions:  [fmt],
  })
  if (!path) return
  const res = await window.api.whisperExportTranscript(currentTranscript, fmt, path)
  if (res.ok) {
    toast('success', t('transcript.exportDone', 'Transkripsjon lagret').replace('{name}', baseName), {
      action: {
        label:   t('general.showInFolder', 'Vis i mappe'),
        onClick: () => { void window.api.revealFile(path) },
      },
    })
  } else {
    await alertDialog({
      title:   t('transcript.exportFailed', 'Kunne ikke lagre transkripsjonen'),
      message: res.error,
      tone:    'error',
    })
  }
}
