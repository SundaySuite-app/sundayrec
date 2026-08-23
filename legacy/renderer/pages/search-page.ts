/**
 * «Historikk» — the recording history with a search box over it.
 *
 * One search box searches the recording metadata (filename / date / note);
 * below it sits the full recording history (the list relocated from the home
 * page). An empty query shows the whole history.
 *
 * (Until v0.15 the same box also searched whisper transcripts and rendered
 * hit-snippets under each sermon. Transcription left SundayRec with the
 * content cluster — «Frivilligen først» R2 — and the transcript index, the
 * «↻ Oppdater indeks» button and the «Med transkript» chip went with it.)
 *
 * The history list + its tools live in `history.ts`; this module owns the
 * query that drives the render.
 */

import { t } from '../i18n'
import { escHtml } from '../helpers'
import {
  applyHistoryView,
  closeTrashView,
  loadHistory,
  getFullHistory,
  renderHistoryRows,
  updateHistoryStats,
  setupHistoryTools,
} from './history'

let pendingQuery = ''

const $ = (id: string) => document.getElementById(id)

export function setupSearchPage(): void {
  const input = $('search-query') as HTMLInputElement | null
  input?.addEventListener('input', () => {
    pendingQuery = (input.value ?? '').trim()
    runSearch()
  })
  // History maintenance tools (clear / prune / delete-errors / "⋯") re-run the
  // current query so the list + stats refresh in place after a mutation.
  setupHistoryTools(runSearch)
}

/** Called from showPage('search'): refresh the history (cheap — picks up new
 *  recordings), then render. */
export function activateSearchPage(): void {
  // The tab is called Historikk — open it on the recordings, never on a
  // papirkurv left behind by a visit three pages ago.
  closeTrashView()
  void (async () => {
    await loadHistory()
    runSearch()
  })()
}

function runSearch(): void {
  const tbody = $('history-tbody')
  if (!tbody) return

  const all = getFullHistory()
  showEmptyState(all.length === 0)
  if (all.length === 0) { setStatus(''); return }

  const q = pendingQuery
  if (q.length < 2) {
    // The chip filter + column sort are the last step before rendering, so the
    // table and the stats line always describe the same set of rows.
    const view = applyHistoryView(all)
    renderHistoryRows(tbody, view, true)
    updateHistoryStats(view)
    setStatus('')
    return
  }

  const needle = q.toLowerCase()
  const matches = applyHistoryView(all.filter(r =>
    (r.filename ?? '').toLowerCase().includes(needle) ||
    (r.date ?? '').includes(q) ||
    (r.note ?? '').toLowerCase().includes(needle)))

  renderHistoryRows(tbody, matches, true)
  updateHistoryStats(matches)

  setStatus(
    matches.length === 0
      ? `${t('search.noHits', 'Ingen treff for')} «${escHtml(q)}»`
      : `${matches.length} ${t('search.recordings', 'opptak')}`,
  )
}

function setStatus(s: string): void {
  const el = $('search-index-status')
  if (!el) return
  el.textContent = s
  el.style.display = s ? '' : 'none'
}

function showEmptyState(show: boolean): void {
  const empty = $('search-empty')
  const tableWrap = $('search-history-wrap')
  if (empty) empty.style.display = show ? '' : 'none'
  if (tableWrap) tableWrap.style.display = show ? 'none' : ''
}
