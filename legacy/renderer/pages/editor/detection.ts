import { t } from '../../i18n'
import { E, $, markDirty, type Suggestion } from './state'
import { formatTime, formatDuration } from './format'
import { drawWaveform, drawMinimap } from './waveform'
import { pushCutHistory, renderCutList, updateRemainingDisplay } from './cuts'
import { flagEditorTab } from './tabs'
import { sermonCandidates } from './sermon-candidates'
import { autoSermonIndex, buildSermonPickRequest } from './sermon-feedback'
import { attachProgress, type ProgressHandle } from '../../ui/progress'

// Segment detection / analyze panel. (Full detection logic lands here in a
// later phase; for now just the display predicate the waveform renderer needs.)

export function shouldShowSegment(type: string): boolean {
  if (type === 'sermon') return true
  if (type === 'speech') return E.showSpeechSegments
  if (type === 'music')  return E.showMusicSegments
  if (type === 'silence') return E.showSilenceSegments
  // mixed / unknown → render only if speech is on (closest match)
  return E.showSpeechSegments
}


/** Per-type visibility filter for segments. Sermon (the highlighted
 *  suggested-keep range) is always visible — it's the most actionable
 *  outcome of analysis. Speech / music / silence honour the user's toggles. */
/** Runs segment detection. `auto` = true skips the button-disabled UI dance
 *  (used for auto-run after file load — we don't want to spook the user with
 *  a disabled button they didn't click).
 *
 *  `auto` also decides whether the backend may answer from its
 *  `<stem>.segments.json` cache: the automatic post-open run happily takes the
 *  cached answer (that's what makes a reopen instant), while a click on
 *  «Analyser opptak» FORCES a fresh pass — the user pressing that button is
 *  asking for the work to be done, not for last time's answer. */
/** One analysis at a time. The automatic post-open run does NOT disable the
 *  button, so a user could start a second pass on top of it — two full ffmpeg
 *  decodes of the same multi-gigabyte recording at once, and two writers
 *  fighting over one progress bar. The click is dropped instead: the run already
 *  in flight is about to produce the same answer. */
let detectionInFlight = false

export async function runDetection(auto = false): Promise<void> {
  if (!E.filePath) return
  if (detectionInFlight) return
  detectionInFlight = true
  const btn       = $('btn-detect-segments') as HTMLButtonElement | null
  const analyzing = $('editor-segments-analyzing')
  if (!auto && btn) { btn.disabled = true; btn.textContent = t('editor.analyzing', 'Analyserer…') }
  if (analyzing)   analyzing.style.display = ''

  E.suggestions = []
  E.autoSermonIndex = null
  flagEditorTab('clip', false)
  renderAnalyzePanel()
  hideSuggestionBanner()

  // A spinner is all this card had, for a pass that reads the WHOLE recording —
  // minutes on a service, and indistinguishable from a hang. The backend now
  // reports its decode position, so say how far along it is and roughly how much
  // longer. Indeterminate until the first tick: a cached answer returns before
  // any arrives, and the backend reports nothing for a container whose duration
  // it could not probe.
  const host = $('editor-analyze-progress')
  let progressUi: ProgressHandle | null = null
  if (host) {
    host.style.display = ''
    progressUi = attachProgress(host, { compact: true })
    progressUi.update(null)
  }
  const unsub = window.api.on?.('editor-analysis-progress', (payload: unknown) => {
    const f = (payload as { fraction?: number } | null)?.fraction
    if (typeof f !== 'number' || !isFinite(f)) return
    progressUi?.update(Math.max(0, Math.min(1, f)))
  })
  const stopProgress = (): void => {
    unsub?.()
    progressUi?.destroy()
    progressUi = null
    if (host) host.style.display = 'none'
  }

  const fpAtStart = E.filePath
  let raw: Suggestion[] = []
  try {
    raw = await window.api.editorDetectSegments(E.filePath, !auto)
  } catch {
    raw = []
  } finally {
    detectionInFlight = false
    stopProgress()
  }
  // Guard against the user closing/swapping the file mid-analysis: drop the
  // result if we're no longer on the same recording.
  if (fpAtStart !== E.filePath) return

  E.suggestions = raw
  E.lastAnalyzedAt = Date.now()
  // Remember the DETECTOR's own answer before anything is promoted on top of
  // it: it is the baseline every correction is recorded against, and once a
  // stored correction has been applied it is no longer visible in the list.
  E.autoSermonIndex = autoSermonIndex(raw)
  await applyStoredSermonPick()

  if (!auto && btn) { btn.disabled = false; btn.textContent = t('editor.analyzeRun', '▶ Analyser opptak') }
  if (analyzing)   analyzing.style.display = 'none'
  renderAnalyzePanel()
  drawWaveform()

  // Show the auto-trim suggestion banner whenever we have a meaningful trim
  // (silence/music head or tail bigger than 0.5 s). Don't show if the user
  // already has cuts — they're clearly editing manually.
  if (E.cuts.length === 0) showSuggestionBanner()

  // The auto-run finishes minutes after the file opened, quite possibly while
  // the operator is on another tab. The banner above is always visible, but
  // the sermon picker and «Marker preken automatisk» live in Klipp-verktøy —
  // so say that there is now something there. No-op when that tab is already
  // the one on screen.
  if (E.suggestions.length > 0) flagEditorTab('clip', true)
}

/**
 * Render the merged "Analyser opptak" panel — replaces the old
 * separate Kapittelmarkører + Analyser opptak sections. Shows a summary
 * line ("Sist analysert: 31.5 14:23 · 3 tale-segmenter funnet"), the
 * three on-timeline toggles (speech/music/silence), and the
 * "Marker preken automatisk" button.
 *
 * (v0.15: chapters left the editor entirely — a `.meta.json` that still
 * carries a `chapters` key passes through the loader/saver untouched, but
 * nothing draws or exports it.)
 */
export function renderAnalyzePanel(): void {
  const summary  = $('editor-analyze-summary')
  const controls = $('editor-analyze-controls')
  const markBtn  = $('btn-apply-auto-trim')
  const markHint = $('editor-auto-trim-hint')

  // Render summary line if we've ever analyzed this file.
  if (summary) {
    if (E.lastAnalyzedAt > 0) {
      const speechCount = E.suggestions.filter(s => s.type === 'speech' || s.type === 'sermon').length
      const d = new Date(E.lastAnalyzedAt)
      const date = `${d.getDate()}.${d.getMonth() + 1}`
      const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
      summary.textContent = `${t('editor.analyzedAt', 'Sist analysert')}: ${date} ${time} · ${speechCount} ${t('editor.speechSegments', 'tale-segmenter funnet')}`
      summary.style.display = ''
    } else {
      summary.style.display = 'none'
    }
  }

  // The three layer toggles now live in the toolbar's view popover. Before the
  // recording has been analysed there are no segments to draw, so the popover
  // shows a sentence saying where to get some instead of three dead checkboxes.
  const analyzed = E.lastAnalyzedAt > 0
  if (controls) controls.style.display = analyzed ? '' : 'none'
  const layersEmpty = $('editor-view-popover-empty')
  if (layersEmpty) layersEmpty.style.display = analyzed ? 'none' : ''

  // Show "Bruk forslag" / sermon-picker only when we have a sermon detected.
  const hasSermon = E.suggestions.some(s => s.type === 'sermon')
  if (markBtn)  (markBtn as HTMLElement).style.display  = hasSermon ? '' : 'none'
  if (markHint) (markHint as HTMLElement).style.display = hasSermon ? '' : 'none'
  renderSermonPicker()
}

/** Apply trim cuts to keep ONLY the sermon: drop everything before sermon.start
 *  and after sermon.end, AND any music that falls inside the sermon span (the
 *  auto-pick can span a song between two talk blocks — the user wants all music
 *  gone). Interior silence is kept (natural pauses; cutting them chops the talk).
 *  Mirrors `sundayrec_core::editor::sermon_cut_regions`. */
export function applySermonTrim(): void {
  const sermon = E.suggestions.find(s => s.type === 'sermon')
  if (!sermon || !E.duration) return
  const cuts: { start: number; end: number }[] = []
  if (sermon.start > 0.5) {
    cuts.push({ start: 0, end: Math.min(sermon.start, E.duration) })
  }
  if (sermon.end < E.duration - 0.5) {
    cuts.push({ start: Math.max(0, sermon.end), end: E.duration })
  }
  for (const s of E.suggestions) {
    if (s.type !== 'music') continue
    const start = Math.max(s.start, sermon.start)
    const end = Math.min(s.end, sermon.end)
    if (end > start + 0.5) cuts.push({ start, end })
  }
  cuts.sort((a, b) => a.start - b.start)
  E.cuts = cuts
  pushCutHistory()
  markDirty()
  renderCutList()
  updateRemainingDisplay()
  drawWaveform()
  drawMinimap()
}

/** Move the 'sermon' label onto `segIdx`, demoting the previous holder back to
 *  plain 'speech'. In-memory only, and deliberately silent about it — the
 *  restore path below promotes a block WITHOUT recording a correction, since
 *  replaying a stored answer is not a new answer. */
function promoteSermonSegment(segIdx: number): boolean {
  const target = E.suggestions[segIdx]
  if (!target) return false
  // Reset any current sermon → speech
  for (const s of E.suggestions) {
    if (s.type === 'sermon') { s.type = 'speech'; s.label = t('editor.speechLabel', 'Tale') }
  }
  target.type = 'sermon'
  target.label = 'Preken'
  renderAnalyzePanel()
  drawWaveform()
  return true
}

/** Promote a specific segment to be the "sermon" (overrides the auto-detected
 *  pick), and RECORD that the detector got it wrong.
 *
 *  `segIdx` is an index into `E.suggestions` itself — the value the picker's
 *  options carry. It used to be an index into a filtered copy that the picker
 *  did NOT build its options from, so any recording with a sub-minute speech
 *  block promoted the wrong segment (see `sermon-candidates.ts`).
 *
 *  The record is built BEFORE the promotion: afterwards the two `type` fields
 *  have already swapped, and a payload assembled from that says the detector
 *  picked what the human picked. Whether this counts as a correction at all —
 *  and what it replaces — is the backend's call (`sundayrec_core::feedback`);
 *  the renderer reports the event, it does not judge it. */
export function setSermonSegment(segIdx: number): void {
  if (!E.suggestions[segIdx]) return
  const request = E.filePath && E.duration > 0
    ? buildSermonPickRequest(E.suggestions, E.autoSermonIndex, segIdx, E.duration)
    : null
  const filePath = E.filePath
  if (!promoteSermonSegment(segIdx)) return
  // Fire-and-forget: a correction that fails to persist must never block the
  // edit the user is actually doing. The shim swallows the failure.
  if (request) void window.api.editorRecordSermonPick?.(filePath, request)
}

/** Put back the sermon block the human corrected us to last time, if any.
 *
 *  The gate this whole phase exists for: correct the pick, close the editor,
 *  reopen — and see YOUR block, not the detector's. Runs on every detection
 *  result, so it covers the automatic post-open run and an explicit
 *  «Analyser opptak» alike. The backend matches on offsets, so a re-analysis
 *  that renumbered the list still lands on the right block, and answers `null`
 *  when the recording no longer contains it. */
async function applyStoredSermonPick(): Promise<void> {
  if (!E.filePath) return
  const fpAtStart = E.filePath
  let stored: number | null = null
  try {
    stored = await window.api.editorSermonPick?.(E.filePath, E.suggestions) ?? null
  } catch {
    stored = null
  }
  // The user may have swapped recordings while we were asking.
  if (fpAtStart !== E.filePath) return
  if (stored === null || stored === E.autoSermonIndex) return
  promoteSermonSegment(stored)
}

/** Render the sermon-picker dropdown so the user can override the auto-pick.
 *  Shows when there's more than one speech segment that could plausibly be
 *  the sermon (≥ 1 min). Hidden otherwise — single-segment recordings have
 *  no alternative to offer. */
export function renderSermonPicker(): void {
  const picker = $('editor-sermon-picker') as HTMLSelectElement | null
  const wrap   = $('editor-sermon-picker-wrap')
  if (!picker || !wrap) return

  // Speech-like segments worth offering, in time order — each still carrying the
  // index of the segment it means.
  const candidates = sermonCandidates(E.suggestions)

  if (candidates.length < 2) {
    wrap.style.display = 'none'
    return
  }

  wrap.style.display = ''
  picker.innerHTML = ''
  for (let i = 0; i < candidates.length; i++) {
    const { index, segment: s } = candidates[i]
    const opt = document.createElement('option')
    // The SOURCE index, not the display position: filtering out a too-short
    // block used to shift these apart, so a correction silently promoted (and
    // trimmed to) a different segment than the one the user chose.
    opt.value = String(index)
    const startLbl = formatTime(s.start)
    const durLbl   = formatDuration(s.duration)
    const marker   = s.type === 'sermon' ? '★ ' : ''
    // The NUMBER stays the display position — "Tale-blokk 3" is the third one
    // in the list the user is looking at, which is the only meaning it can have.
    opt.textContent = `${marker}${t('editor.speechBlock', 'Tale-blokk')} ${i + 1} — ${startLbl} (${durLbl})`
    if (s.type === 'sermon') opt.selected = true
    picker.appendChild(opt)
  }
}

export function showSuggestionBanner(): void {
  const banner = $('editor-suggestion-banner')
  const detail = $('editor-suggestion-detail')
  const sermon = E.suggestions.find(s => s.type === 'sermon')
  if (!banner || !detail || !sermon || !E.duration) return
  const headDur = sermon.start
  const tailDur = E.duration - sermon.end
  if (headDur < 0.5 && tailDur < 0.5) { banner.style.display = 'none'; return }
  const parts: string[] = []
  if (headDur > 0.5) parts.push(`${formatDuration(headDur)} ${t('editor.beforeSermon', 'før prekenen')}`)
  if (tailDur > 0.5) parts.push(`${formatDuration(tailDur)} ${t('editor.afterSermon', 'etter prekenen')}`)
  const keep = formatDuration(sermon.end - sermon.start)
  detail.textContent = `${parts.join(' + ')} ${t('editor.willBeTrimmed', 'fjernes')} · ${keep} ${t('editor.willRemain', 'preken igjen')}`
  banner.style.display = ''
}

export function hideSuggestionBanner(): void {
  const banner = $('editor-suggestion-banner')
  if (banner) banner.style.display = 'none'
}
