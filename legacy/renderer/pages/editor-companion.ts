/**
 * AI sermon companion panel (R8).
 *
 * Turns a finished whisper transcript into:
 *   - chapter markers (deterministic, on-device — scripture refs + topic shifts),
 *   - 2–4 quotable highlight passages (pure heuristic ranking),
 *   - a Norwegian summary + title.
 *
 * The summary/title go through an OPTIONAL Anthropic Messages seam in the Rust
 * shell when a key is configured (keychain or ANTHROPIC_API_KEY). With NO key —
 * the default — the backend ships a fully-local extractive summary instead, and
 * `summarySource` tells us which. The panel NEVER blocks the editor: on any
 * failure it shows a calm "ikke tilgjengelig" state.
 *
 * The "send to SundayEdit/Stage" hand-off reuses the EXISTING integration
 * pipeline: companion output is pushed into the editor metadata (E.meta — title,
 * description, chapters), which flows into the export and the SundayEdit/Stage
 * hand-offs already wired in the transcript + metadata panels. We do not invent a
 * new IPC channel for the hand-off; we feed the channel that already exists.
 */

import { t } from '../i18n'
import type { TranscriptData, SermonCompanion } from '../../types'
import { E } from './editor/state'
import { renderChapterList, renderMetaPanel } from './editor/metadata'
import { drawWaveform } from './editor/waveform'

const $ = (id: string) => document.getElementById(id)

let getTranscript: (() => TranscriptData | null) = () => null

/** Wire the companion section. `transcriptGetter` lets us read the panel's
 *  current transcript without coupling to its module internals. */
export function setupCompanionPanel(transcriptGetter: () => TranscriptData | null): void {
  getTranscript = transcriptGetter
}

/** Reset on file change / transcript delete. */
export function clearCompanion(): void {
  const host = $('editor-companion-body')
  if (host) host.innerHTML = ''
}

function fmtTime(sec: number): string {
  const m = Math.floor(sec / 60)
  const s = Math.floor(sec % 60)
  return `${m}:${String(s).padStart(2, '0')}`
}

/** Build (or rebuild) the companion. `useLlm` lets the caller force the offline
 *  path even when a key is configured. */
async function build(useLlm: boolean): Promise<void> {
  const transcript = getTranscript()
  if (!transcript || transcript.segments.length === 0) return
  const host = $('editor-companion-body')
  const btn = $('btn-companion-build') as HTMLButtonElement | null
  if (btn) { btn.disabled = true; btn.textContent = t('companion.working', 'Lager prekenhjelp…') }
  if (host) host.innerHTML = `<div class="editor-companion-status">${t('companion.working', 'Lager prekenhjelp…')}</div>`

  let result: SermonCompanion | null = null
  try {
    result = await window.api.companionBuild(transcript, useLlm)
  } catch {
    result = null
  }

  if (btn) { btn.disabled = false; btn.textContent = t('companion.build', '✦ Lag prekenhjelp') }

  if (!result) {
    if (host) {
      host.innerHTML = `<div class="editor-companion-status editor-companion-status--muted">${t('companion.unavailable', 'AI-prekenhjelp er ikke tilgjengelig akkurat nå. Transkripsjon og kapitler virker fortsatt.')}</div>`
    }
    return
  }
  render(result)
}

function render(c: SermonCompanion): void {
  const host = $('editor-companion-body')
  if (!host) return

  const sourceBadge = c.summarySource === 'llm'
    ? `<span class="editor-companion-badge editor-companion-badge--ai" title="${escapeAttr(t('companion.sourceAiHint', 'Oppsummeringen er laget av en språkmodell på serveren'))}">${t('companion.sourceAi', 'AI-oppsummering')}</span>`
    : `<span class="editor-companion-badge" title="${escapeAttr(t('companion.sourceLocalHint', 'Lokal oppsummering (ingen API-nøkkel) — helt på enheten'))}">${t('companion.sourceLocal', 'Lokal oppsummering')}</span>`

  const highlightsHtml = c.highlights.length === 0
    ? `<div class="editor-companion-status editor-companion-status--muted">${t('companion.noHighlights', 'Ingen tydelige høydepunkter funnet')}</div>`
    : c.highlights.map(h => `
        <div class="editor-companion-highlight" data-time="${h.time}">
          <button class="editor-companion-hl-time btn-ghost btn-sm" data-seek="${h.time}">${fmtTime(h.time)}</button>
          <span class="editor-companion-hl-text"></span>
        </div>`).join('')

  const chapterCount = c.chapters.length

  host.innerHTML = `
    <div class="editor-companion-row">
      <strong class="editor-companion-suggested-title"></strong>
      ${sourceBadge}
    </div>
    <p class="editor-companion-summary"></p>
    <div class="editor-companion-actions">
      <button class="btn-ghost btn-sm" id="btn-companion-use-meta" title="${escapeAttr(t('companion.useMetaHint', 'Legg tittel + oppsummering i metadata — brukes ved eksport og av SundayEdit/Stage'))}">${t('companion.useMeta', '→ Bruk i metadata')}</button>
      <button class="btn-ghost btn-sm" id="btn-companion-use-chapters">${t('companion.useChapters', '→ Legg til {n} kapitler').replace('{n}', String(chapterCount))}</button>
      <button class="btn-ghost btn-sm" id="btn-companion-copy">${t('companion.copy', '⧉ Kopier')}</button>
      <span id="companion-action-hint" class="editor-companion-action-hint"></span>
    </div>
    <div class="editor-companion-highlights-title">${t('companion.highlightsTitle', 'Sitater')}</div>
    <div class="editor-companion-highlights">${highlightsHtml}</div>
  `

  // Use textContent for model/transcript-derived text (never innerHTML — these
  // strings come from a transcript or a language model, so treat them as data).
  const titleEl = host.querySelector('.editor-companion-suggested-title') as HTMLElement | null
  if (titleEl) titleEl.textContent = c.title
  const summaryEl = host.querySelector('.editor-companion-summary') as HTMLElement | null
  if (summaryEl) summaryEl.textContent = c.summary
  host.querySelectorAll('.editor-companion-hl-text').forEach((el, i) => {
    el.textContent = c.highlights[i]?.text ?? ''
  })

  $('btn-companion-use-meta')?.addEventListener('click', () => useInMetadata(c))
  $('btn-companion-use-chapters')?.addEventListener('click', () => useChapters(c))
  $('btn-companion-copy')?.addEventListener('click', () => copyToClipboard(c))
  host.querySelectorAll('[data-seek]').forEach(el => {
    el.addEventListener('click', () => {
      const sec = Number((el as HTMLElement).dataset.seek)
      if (Number.isFinite(sec)) onSeek?.(sec)
    })
  })
}

let onSeek: ((sec: number) => void) | null = null
export function setCompanionSeek(cb: (sec: number) => void): void { onSeek = cb }

/** Push the suggested title + summary into editor metadata. Title only fills an
 *  empty field (never clobbers a title the user typed); the summary is appended
 *  to the description, also non-destructively. This metadata is exactly what the
 *  export embeds and what the SundayEdit/Stage hand-offs read. */
function useInMetadata(c: SermonCompanion): void {
  let changed = false
  if (!E.meta.title.trim() && c.title.trim()) {
    E.meta.title = c.title.trim()
    changed = true
  }
  if (c.summary.trim()) {
    const existing = E.meta.description.trim()
    if (!existing) {
      E.meta.description = c.summary.trim()
      changed = true
    } else if (!existing.includes(c.summary.trim())) {
      E.meta.description = `${existing}\n\n${c.summary.trim()}`
      changed = true
    }
  }
  if (changed) {
    E.metaDirty = true
    renderMetaPanel()
  }
  flashHint(changed ? t('companion.metaApplied', 'Lagt i metadata') : t('companion.metaAlready', 'Allerede i metadata'))
}

/** Merge companion chapters into E.meta.chapters, de-duping against existing
 *  markers (same title within 2 s, or any marker within 1 s). Same merge policy
 *  as the transcript panel's "detect chapters" so the two are consistent. */
function useChapters(c: SermonCompanion): void {
  const existing = E.meta.chapters
  let added = 0
  for (const ch of c.chapters) {
    const dup = existing.some(
      e => (e.title === ch.title && Math.abs(e.time - ch.time) < 2) || Math.abs(e.time - ch.time) < 1,
    )
    if (!dup) { existing.push({ time: ch.time, title: ch.title }); added++ }
  }
  if (added > 0) {
    existing.sort((a, b) => a.time - b.time)
    E.metaDirty = true
    renderChapterList()
    drawWaveform()
  }
  flashHint(added > 0
    ? `${added} ${t('transcript.chaptersAdded', 'kapitler lagt til')}`
    : t('transcript.chaptersAllPresent', 'Alle funne kapitler finnes allerede'))
}

async function copyToClipboard(c: SermonCompanion): Promise<void> {
  const lines = [
    c.title,
    '',
    c.summary,
    '',
    t('companion.highlightsTitle', 'Sitater') + ':',
    ...c.highlights.map(h => `• [${fmtTime(h.time)}] ${h.text}`),
  ]
  try {
    await navigator.clipboard.writeText(lines.join('\n'))
    flashHint(t('companion.copied', 'Kopiert'))
  } catch {
    flashHint(t('companion.copyFailed', 'Kunne ikke kopiere'))
  }
}

function flashHint(msg: string): void {
  const hint = $('companion-action-hint')
  if (!hint) return
  hint.textContent = msg
  setTimeout(() => { if (hint.textContent === msg) hint.textContent = '' }, 2500)
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** Render the companion header controls into the host (called when a transcript
 *  is present). Shows the build button + an offline indicator if no LLM key. */
export async function renderCompanionControls(): Promise<void> {
  const header = $('editor-companion-header-controls')
  if (!header) return
  let configured = false
  try { configured = await window.api.companionLlmConfigured() } catch { configured = false }

  header.innerHTML = `
    <button class="btn-primary btn-sm" id="btn-companion-build">${t('companion.build', '✦ Lag prekenhjelp')}</button>
    <span class="editor-companion-mode" title="${escapeAttr(configured
      ? t('companion.modeAiHint', 'En API-nøkkel er konfigurert — oppsummeringen lages av en språkmodell')
      : t('companion.modeLocalHint', 'Ingen API-nøkkel — oppsummeringen lages lokalt på enheten'))}">${configured
        ? t('companion.modeAi', 'AI-modus')
        : t('companion.modeLocal', 'Lokal modus')}</span>
  `
  // Default to the LLM when a key is present; the backend falls back on its own.
  $('btn-companion-build')?.addEventListener('click', () => void build(configured))
}
