/**
 * Recording history — the full list + its tools (delete / note / prune / clear /
 * stats). This used to live on the home page; it now backs the merged
 * «Søk & historikk» tab (see search-page.ts), which composes this list with
 * transcript search. The home page keeps only a compact «Siste 5» of its own.
 *
 * The renderer can optionally interleave transcript hit-snippets under the
 * recordings that matched a text query (`hitsByBase`), so one unified list
 * serves both "browse all recordings" and "search inside sermons".
 */

import { t, tf, tn } from '../i18n'
import { fmtDate, flashMsg } from '../helpers'
import { closeModal, openModal } from '../ui/modal-manager'
import { confirmDialog } from '../ui/dialog'
import { firstMount, resetMount } from '../ui/motion'
import { toast } from '../ui/toast'
import { TRASH_KEEP_DAYS, ageText, toTrashRows, type TrashEntry } from './trash-core'
import {
  baseNoExt,
  filterRecordings,
  historyTotals,
  pairRecordings,
  sortRecordings,
  type HistoryFilter,
  type HistorySortKey,
  type RecordingEntry,
  type SortDir,
} from './history-core'

// Re-exported so the page modules keep importing history state from one place;
// the logic itself lives in the (DOM-free, unit-tested) core.
export { baseNoExt }
export type { RecordingEntry }

/** One transcript hit to render as a sub-row under its recording: the seek
 *  target (seconds) + the pre-highlighted snippet HTML. Built by search-page. */
export interface HistoryHit {
  start: number
  html:  string
}

let fullHistory: RecordingEntry[] = []

// ── View state (sort + filter) ──────────────────────────────────────────────
// Newest first is the default: the recording you just made is the one you want.
let sortKey: HistorySortKey = 'time'
let sortDir: SortDir        = 'desc'
let activeFilter: HistoryFilter = 'all'
/** Base paths (no extension) that have a transcript sidecar — fed by
 *  search-page once its index is loaded, so the «Med transkript» chip can
 *  answer without a second IPC round-trip. */
let transcriptBases = new Set<string>()

/** The current in-memory history (newest-first), for callers that need to
 *  filter it themselves (e.g. the unified search). */
export function getFullHistory(): RecordingEntry[] {
  return fullHistory
}

/** Tell the history which recordings have a transcript (from `transcripts_list`). */
export function setTranscriptBasePaths(bases: Set<string>): void {
  transcriptBases = bases
}

function hasTranscript(r: RecordingEntry): boolean {
  return transcriptBases.has(baseNoExt(r.path))
}

/** Apply the active chip filter + column sort. The caller has already narrowed
 *  by the search query; this is the last step before rendering, so the table
 *  and the stats line always describe the same set of rows. */
export function applyHistoryView(rows: RecordingEntry[]): RecordingEntry[] {
  return sortRecordings(filterRecordings(rows, activeFilter, hasTranscript), sortKey, sortDir)
}

/** Fetch the full history into the module cache. Rendering is driven by the
 *  caller (search-page's runSearch) so the active query survives a reload.
 *
 *  Trashed recordings are already gone from what `getHistory` returns — the
 *  api-shim filters them there so Historikk, the unified search and the home
 *  page's «Siste 5» can never disagree about whether a recording exists. */
export async function loadHistory(): Promise<void> {
  fullHistory = ((await window.api.getHistory()) ?? []) as RecordingEntry[]
  await refreshTrashButton()
}

// ── Papirkurv ───────────────────────────────────────────────────────────────
//
// Deleting a recording moves it — and its sidecars, and its video sibling — to
// `<saveFolder>/.sundayrec-trash`, where it stays for 30 days. Nothing about
// that is worth a modal: the whole point of a trash is that the decision is
// cheap, so the row goes and a toast offers «Angre».
//
// A history row whose FILE is trashed keeps existing in the database (that is
// what makes a restore give back the note and the duration);
// the shim hides it. A row whose file was already missing has nothing to
// recover, so those rows are dropped outright — which is also the only way
// «Slett alle» can actually empty the table.

/** Re-run the active view. Captured from `setupHistoryTools` so the trash
 *  actions can repaint the list after a restore. */
let rerenderHistory: () => void = () => {}

/** Whether the Papirkurv is the list currently on screen. */
let trashViewOpen = false

/** Repaint the history without letting it take the screen back from an open
 *  papirkurv — `runSearch` unhides the table unconditionally, which is right
 *  every other time it runs. */
function repaintHistory(): void {
  rerenderHistory()
  if (trashViewOpen) showTrashView(true)
}

/**
 * Trash the files of `rows`, and delete the history rows of any recording whose
 * file was already gone. Returns the entries that can be restored.
 */
async function trashRows(rows: RecordingEntry[]): Promise<TrashEntry[]> {
  const paths = rows.map(r => r.path).filter((p): p is string => !!p)
  const moved = paths.length ? await window.api.trashMove(paths) : []
  const movedPaths = new Set(moved.map(e => e.originalPath))
  for (const r of rows) {
    if (r.path && movedPaths.has(r.path)) continue
    if (r.timestamp) await window.api.deleteHistoryEntry(r.timestamp)
  }
  return moved
}

/** Restore everything one delete produced, then repaint. */
async function undoTrash(entries: TrashEntry[]): Promise<void> {
  let failed = 0
  for (const e of entries) {
    try { await window.api.trashRestore(e.id) } catch { failed++ }
  }
  await loadHistory()
  repaintHistory()
  if (failed > 0) {
    toast('warn', t('trash.undoFailed', 'Kunne ikke hente alt tilbake — se i papirkurven.'))
  }
}

/** The «… er flyttet til papirkurven» toast, with its one-click undo. */
function trashedToast(message: string, entries: TrashEntry[]): void {
  toast('info', message, {
    // Long enough to reconsider a mis-aimed click, short enough not to sit
    // over the list you are working in.
    durationMs: 9000,
    action: entries.length
      ? { label: t('trash.undo', 'Angre'), onClick: () => { void undoTrash(entries) } }
      : undefined,
  })
}

/** Show/hide the header's «Papirkurv (N)» link and keep its count honest. */
async function refreshTrashButton(): Promise<void> {
  const btn = document.getElementById('btn-trash-open')
  if (!btn) return
  let n = 0
  try { n = (await window.api.trashList()).length } catch { n = 0 }
  btn.textContent = `${t('trash.title', 'Papirkurv')} (${n})`
  // An empty trash is not a place worth offering to visit.
  btn.style.display = n > 0 ? '' : 'none'
  if (n === 0) showTrashView(false)
}

export function updateHistoryStats(history: RecordingEntry[]): void {
  const statsEl = document.getElementById('history-stats')
  if (!statsEl) return
  const totals = historyTotals(history)
  if (!totals.count) { statsEl.style.display = 'none'; return }
  statsEl.style.display = 'flex'
  const countEl    = document.getElementById('stat-count')
  const durationEl = document.getElementById('stat-duration')
  const lastEl     = document.getElementById('stat-last')
  if (countEl) countEl.textContent = `${totals.count} ${t('history.totalCount', 'opptak')}`
  const th = Math.floor(totals.totalSec / 3600)
  const tm = Math.round((totals.totalSec % 3600) / 60)
  if (durationEl) durationEl.textContent = th > 0
    ? `${th} t ${tm} min ${t('history.totalDuration', 'totalt')}`
    : `${tm} min ${t('history.totalDuration', 'totalt')}`
  if (lastEl) lastEl.textContent = totals.lastDate
    ? `${t('history.lastRecording', 'sist')} ${fmtDate(totals.lastDate)}`
    : ''
}

/**
 * Render the full history table into `tbody`. Audio+video pairs from the same
 * session collapse into one row. When `hitsByBase` is supplied, a recording with
 * transcript hits gets a snippet sub-row right under it (clicking a snippet opens
 * the editor at that timestamp).
 */
export function renderHistoryRows(
  tbody: HTMLElement | null,
  rows: RecordingEntry[],
  showReveal: boolean,
  hitsByBase?: Map<string, HistoryHit[]>,
): void {
  if (!tbody) return
  tbody.innerHTML = ''
  // Stagger the rows in on ARRIVAL only. A re-render (delete, note, filter
  // chip, a finished recording) must not replay the entrance for rows that
  // never left the screen.
  const animate = firstMount(tbody)
  if (!rows.length) {
    resetMount(tbody)
    const td = Object.assign(document.createElement('td'), {
      // The thead has FIVE columns (Tidspunkt · Varighet · Filnavn · Status ·
      // handlinger). This said 6, which stretched the empty-state cell past
      // the table's right edge.
      colSpan: 5,
      // "Ingen opptak ennå" is a lie when the archive is full and it is the
      // chip that emptied the table — say which of the two it is.
      textContent: activeFilter === 'all'
        ? t('history.empty', 'Ingen opptak ennå')
        : t('history.emptyFiltered', 'Ingen opptak i dette filteret')
    })
    td.style.cssText = 'color:var(--text3);text-align:center;padding:20px'
    const tr = document.createElement('tr')
    tr.appendChild(td); tbody.appendChild(tr)
    return
  }
  // Collapse the audio + video files of one session into a single row. Keyed on
  // the shared base path, not on row adjacency — see history-core for why the
  // old positional heuristic could not work under the Tauri shim.
  const grouped = pairRecordings(rows)
  grouped.forEach(({ r, videoEntry }, idx) => {
    const tr = document.createElement('tr')
    tr.className = animate ? 'hist-row row-in' : 'hist-row'
    if (animate) tr.style.animationDelay = `${idx * 0.04}s`
    const badgeCls = r.status === 'ok' || r.status === 'complete' ? 'ok' : r.status === 'error' ? 'error' : 'sched'
    tr.dataset.status = badgeCls
    const badge    = Object.assign(document.createElement('span'), { className: `badge badge-${badgeCls}`, textContent: t(`history.${r.status}`, r.status) })
    const tdStatus = document.createElement('td'); tdStatus.appendChild(badge)
    const tdActions = document.createElement('td'); tdActions.style.cssText = 'white-space:nowrap'

    if (showReveal && r.path) {
      const aReveal = document.createElement('a')
      aReveal.href = '#'; aReveal.className = 'hist-action'
      aReveal.title = t('history.revealFile', 'Vis i Finder / Utforsker')
      aReveal.innerHTML = '<svg viewBox="0 0 20 20"><path d="M11 3a1 1 0 100 2h2.586l-6.293 6.293a1 1 0 101.414 1.414L15 6.414V9a1 1 0 102 0V4a1 1 0 00-1-1h-5zM5 5a2 2 0 00-2 2v8a2 2 0 002 2h8a2 2 0 002-2v-3a1 1 0 10-2 0v3H5V7h3a1 1 0 000-2H5z"/></svg>'
      aReveal.addEventListener('click', e => { e.preventDefault(); window.api.revealFile(r.path!) })
      tdActions.appendChild(aReveal)

      const aEdit = document.createElement('a')
      aEdit.href = '#'; aEdit.className = 'hist-action'
      aEdit.title = t('editor.title', 'Rediger lydfil')
      aEdit.innerHTML = '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3 10h14M3 6h3m11 0h-3M3 14h3m11 0h-3" stroke-linecap="round"/><circle cx="7.5" cy="6" r="1.5" fill="currentColor" stroke="none"/><circle cx="12.5" cy="14" r="1.5" fill="currentColor" stroke="none"/></svg>'
      aEdit.addEventListener('click', e => { e.preventDefault(); window.openEditorWithFile(r.path!) })
      tdActions.appendChild(aEdit)
    }
    if (showReveal && videoEntry?.path) {
      const aRevealVid = document.createElement('a')
      aRevealVid.href = '#'; aRevealVid.className = 'hist-action'
      aRevealVid.title = t('history.revealVideo', 'Vis videofil i Finder')
      aRevealVid.innerHTML = '<svg viewBox="0 0 20 20"><path d="M2 6a2 2 0 012-2h6a2 2 0 012 2v8a2 2 0 01-2 2H4a2 2 0 01-2-2V6zm12.553-1.106A1 1 0 0115 5v10a1 1 0 01-1.553.832l-5-3.333a1 1 0 010-1.664l5-3.333a1 1 0 01.106-.072z"/></svg>'
      aRevealVid.addEventListener('click', e => { e.preventDefault(); window.api.revealFile(videoEntry.path!) })
      tdActions.appendChild(aRevealVid)
    }

    const aNote = document.createElement('a')
    aNote.href = '#'; aNote.className = 'hist-action'
    aNote.title = r.note ? t('history.editNote', 'Rediger notat') : t('history.addNote', 'Legg til notat')
    aNote.innerHTML = r.note
      ? '<svg viewBox="0 0 20 20"><path fill-rule="evenodd" d="M4 4a2 2 0 012-2h4.586A2 2 0 0112 2.586L15.414 6A2 2 0 0116 7.414V16a2 2 0 01-2 2H6a2 2 0 01-2-2V4zm2 6a1 1 0 011-1h6a1 1 0 110 2H7a1 1 0 01-1-1zm1 3a1 1 0 100 2h6a1 1 0 100-2H7z" clip-rule="evenodd"/></svg>'
      : '<svg viewBox="0 0 20 20"><path d="M13.586 3.586a2 2 0 112.828 2.828l-.793.793-2.828-2.828.793-.793zM11.379 5.793L3 14.172V17h2.828l8.38-8.379-2.83-2.828z"/></svg>'
    aNote.addEventListener('click', e => {
      e.preventDefault()
      showNoteModal(r.note ?? '', async (newNote: string) => {
        r.note = newNote.trim() || undefined
        await window.api.updateHistoryNote(r.timestamp!, newNote.trim())
        const fileCell = tr.cells[2]
        const existing = fileCell.querySelector('.hist-note')
        if (existing) existing.remove()
        if (r.note) {
          const noteEl = Object.assign(document.createElement('div'), { className: 'hist-note', textContent: r.note })
          fileCell.appendChild(noteEl)
        }
        aNote.title = r.note ? t('history.editNote', 'Rediger notat') : t('history.addNote', 'Legg til notat')
        aNote.innerHTML = r.note
          ? '<svg viewBox="0 0 20 20"><path fill-rule="evenodd" d="M4 4a2 2 0 012-2h4.586A2 2 0 0112 2.586L15.414 6A2 2 0 0116 7.414V16a2 2 0 01-2 2H6a2 2 0 01-2-2V4zm2 6a1 1 0 011-1h6a1 1 0 110 2H7a1 1 0 01-1-1zm1 3a1 1 0 100 2h6a1 1 0 100-2H7z" clip-rule="evenodd"/></svg>'
          : '<svg viewBox="0 0 20 20"><path d="M13.586 3.586a2 2 0 112.828 2.828l-.793.793-2.828-2.828.793-.793zM11.379 5.793L3 14.172V17h2.828l8.38-8.379-2.83-2.828z"/></svg>'
      })
    })
    tdActions.appendChild(aNote)

    const aDel = document.createElement('a')
    aDel.href = '#'; aDel.className = 'hist-action hist-del'
    aDel.title = t('trash.deleteEntry', 'Flytt til papirkurven')
    aDel.innerHTML = '<svg viewBox="0 0 20 20"><path fill-rule="evenodd" d="M9 2a1 1 0 00-.894.553L7.382 4H4a1 1 0 000 2v10a2 2 0 002 2h8a2 2 0 002-2V6a1 1 0 100-2h-3.382l-.724-1.447A1 1 0 0011 2H9zM7 8a1 1 0 012 0v6a1 1 0 11-2 0V8zm5-1a1 1 0 00-1 1v6a1 1 0 102 0V8a1 1 0 00-1-1z"/></svg>'
    // Deleting used to happen on the first click with no confirmation, then
    // behind a danger dialog that had to promise the file would survive —
    // because all it removed was the list entry.
    //
    // It is a real delete now: the recording, its sidecars and its video
    // sibling move to the papirkurv. That earns it the OPPOSITE treatment. A
    // decision you can take back does not deserve a modal in the way, so there
    // is no dialog — the row goes and a toast offers «Angre», which restores
    // everything the click moved. The row's actions still lock while the move
    // is in flight, so a double-click can't fire two rounds of IPC at a row
    // that is already leaving.
    let deleting = false
    aDel.addEventListener('click', async e => {
      e.preventDefault()
      if (deleting) return
      const name = r.filename ?? t('history.thisRecording', 'dette opptaket')
      deleting = true
      tr.classList.add('hist-row-busy')
      tr.setAttribute('aria-busy', 'true')
      let moved: TrashEntry[]
      try {
        moved = await trashRows(videoEntry ? [r, videoEntry] : [r])
      } catch (err) {
        deleting = false
        tr.classList.remove('hist-row-busy')
        tr.removeAttribute('aria-busy')
        flashMsg(aDel, t('history.deleteFailed', 'Kunne ikke slette'), true)
        console.warn('[history] trash failed:', err)
        return
      }
      trashedToast(
        tf('trash.movedNamed', { name }, '«{name}» ligger i papirkurven'),
        moved,
      )
      void refreshTrashButton()
      const delIdx = fullHistory.findIndex(h => h.timestamp === r.timestamp)
      if (delIdx >= 0) fullHistory.splice(delIdx, 1)
      if (videoEntry?.timestamp) {
        const vidIdx = fullHistory.findIndex(h => h.timestamp === videoEntry.timestamp)
        if (vidIdx >= 0) fullHistory.splice(vidIdx, 1)
      }
      // Drop the recording's row and any transcript sub-row that follows it.
      const sub = tr.nextElementSibling
      if (sub && sub.classList.contains('hist-transcript-hits')) sub.remove()
      tr.remove()
      if (!tbody.querySelector('tr')) renderHistoryRows(tbody, [], false)
      // Recompute from the rows THIS render is showing, minus the two just
      // deleted — passing the whole unfiltered history made the summary line
      // describe a different set of recordings than the table under it.
      updateHistoryStats(rows.filter(x =>
        x.timestamp !== r.timestamp &&
        (videoEntry?.timestamp === undefined || x.timestamp !== videoEntry.timestamp)))
    })
    tdActions.appendChild(aDel)
    tdActions.style.cssText = 'white-space:nowrap;display:flex;align-items:center;gap:3px'

    const timeStr  = r.startTime ? ` kl. ${r.startTime}` : ''
    const cells = [r.date ? `${fmtDate(r.date)}${timeStr}` : '—', r.duration ?? '—', r.filename ?? '—']
    cells.forEach((text, i) => {
      const td = document.createElement('td')
      td.textContent = text
      if (i === 2) {
        if (r.path) td.title = r.path
        if (r.note) {
          td.appendChild(Object.assign(document.createElement('div'), { className: 'hist-note', textContent: r.note }))
        }
        if (videoEntry?.filename) {
          const vidDiv = Object.assign(document.createElement('div'), { className: 'hist-note', textContent: `📹 ${videoEntry.filename}` })
          if (videoEntry.path) vidDiv.title = videoEntry.path
          td.appendChild(vidDiv)
        }
      }
      tr.appendChild(td)
    })
    tr.appendChild(tdStatus); tr.appendChild(tdActions)
    tbody.appendChild(tr)

    // Transcript hit-snippets for this recording (unified search): a sub-row
    // spanning all columns, each snippet seeking the editor to its timestamp.
    const hits = hitsByBase?.get(baseNoExt(r.path))
    if (hits && hits.length && r.path) {
      const hitTr = document.createElement('tr')
      hitTr.className = 'hist-transcript-hits'
      const hitTd = document.createElement('td')
      hitTd.colSpan = 5
      for (const h of hits) {
        const row = document.createElement('div')
        row.className = 'search-hit-row'
        row.innerHTML = `<span class="search-hit-time">${fmtClock(h.start)}</span><span class="search-hit-text">${h.html}</span>`
        row.addEventListener('click', () => window.openEditorWithFile(r.path!, h.start))
        hitTd.appendChild(row)
      }
      hitTr.appendChild(hitTd)
      tbody.appendChild(hitTr)
    }
  })
}

/** mm:ss / h:mm:ss for a seconds offset (snippet timestamps). */
function fmtClock(sec: number): string {
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = Math.floor(sec % 60)
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
    : `${m}:${String(s).padStart(2, '0')}`
}

/**
 * Wire the history maintenance tools (clear all / prune missing / delete errors
 * / the "⋯" panel toggle). `rerender` re-runs the active view so the list and
 * stats refresh after a mutation without losing the current search query.
 */
export function setupHistoryTools(rerender: () => void): void {
  rerenderHistory = rerender
  setupSortHeaders(rerender)
  setupFilterChips(rerender)
  setupTrashView(rerender)

  document.getElementById('btn-prune-history')?.addEventListener('click', async e => {
    e.preventDefault()
    const removed = await window.api.pruneHistory()
    await loadHistory()
    rerender()
    if (removed === 0) flashMsg(document.getElementById('btn-prune-history'), t('history.pruneNone', 'Ingen å rydde'), true)
  })

  document.getElementById('btn-clear-history')?.addEventListener('click', async e => {
    e.preventDefault()
    // Name the count: "Slett hele historikken?" gives no sense of scale, and
    // the difference between losing 3 entries and losing 300 is the whole
    // decision. It still asks — one click that moves 300 recordings is worth a
    // beat even when every one of them is recoverable — but it no longer has
    // to promise the files survive, because now they genuinely do, in the
    // papirkurv, for 30 days.
    const n = fullHistory.length
    if (!n) { flashMsg(document.getElementById('btn-clear-history'), t('history.clearNone', 'Listen er tom'), true); return }
    const ok = await confirmDialog({
      title:        tn('history.confirmClear', n, {}, 'Slett alle {n} oppføringene?'),
      message:      tn('trash.clearHistoryBody', TRASH_KEEP_DAYS, { d: TRASH_KEEP_DAYS },
        'Opptakene flyttes til papirkurven, der de ligger i {d} dager før de slettes for godt.'),
      confirmLabel: t('dialog.delete', 'Slett'),
      danger:       true,
    })
    if (!ok) return
    const moved = await trashRows(fullHistory.slice())
    await loadHistory()
    rerender()
    trashedToast(
      tn('trash.moved', moved.length, {}, '{n} opptak ligger i papirkurven'),
      moved,
    )
  })

  document.getElementById('btn-delete-errors')?.addEventListener('click', async e => {
    e.preventDefault()
    const errors = fullHistory.filter(r => r.status === 'error')
    if (!errors.length) {
      flashMsg(document.getElementById('btn-delete-errors'), t('history.errorsNone', 'Ingen feiloppføringer'), true)
      return
    }
    const ok = await confirmDialog({
      title:        tn('history.confirmDeleteErrors', errors.length, {}, 'Slett {n} feiloppføringer?'),
      message:      tn('trash.deleteErrorsBody', errors.length, {},
        '{n} oppføringer med feil fjernes. Filer som finnes flyttes til papirkurven.'),
      confirmLabel: t('dialog.delete', 'Slett'),
      danger:       true,
    })
    if (!ok) return
    const moved = await trashRows(errors)
    await loadHistory()
    rerender()
    if (moved.length) {
      trashedToast(
        tn('trash.moved', moved.length, {}, '{n} opptak ligger i papirkurven'),
        moved,
      )
    }
  })

  document.getElementById('btn-history-more')?.addEventListener('click', () => {
    const panel = document.getElementById('history-more-panel')
    const btn   = document.getElementById('btn-history-more')
    const open  = panel?.style.display !== 'none'
    if (panel) panel.style.display = open ? 'none' : 'flex'
    btn?.setAttribute('aria-expanded', String(!open))
  })
}

// ── The Papirkurv view ──────────────────────────────────────────────────────
//
// Takes the place of the history table rather than sitting under it: the search
// box above filters recordings, not trash, and a search field hovering over a
// list it cannot touch is a small lie told on every visit.

/** Which of the two lists «Søk & historikk» is currently showing.
 *
 *  The search FIELD goes with the history — it filters recordings, not trash,
 *  and a search box hovering over a list it cannot touch is a small lie told on
 *  every visit. The «Papirkurv» link beside it stays: it is the way back. */
function showTrashView(on: boolean): void {
  trashViewOpen = on
  const trash   = document.getElementById('trash-view')
  const history = document.getElementById('search-history-wrap')
  const empty   = document.getElementById('search-empty')
  const query   = document.getElementById('search-query')
  const reindex = document.getElementById('btn-search-reindex')
  const status  = document.getElementById('search-index-status')
  if (trash)   trash.style.display   = on ? '' : 'none'
  if (query)   query.style.display   = on ? 'none' : ''
  if (reindex) reindex.style.display = on ? 'none' : ''
  if (status && on) status.style.display = 'none'
  // Only the OPEN direction hides the history: closing hands the decision back
  // to `runSearch`, which knows whether the table or its empty state belongs on
  // screen. Unhiding both here would show an empty table next to «Ingen opptak
  // ennå».
  if (on) {
    if (history) history.style.display = 'none'
    if (empty)   empty.style.display   = 'none'
  }
}

/** Leave the papirkurv. Called when «Søk & historikk» is (re-)activated: the
 *  tab is called Historikk, so that is what opening it should show — a trash
 *  view left over from a visit three pages ago is a page that did not do what
 *  its name said. */
export function closeTrashView(): void {
  if (trashViewOpen) showTrashView(false)
}

function setupTrashView(rerender: () => void): void {
  document.getElementById('btn-trash-open')?.addEventListener('click', e => {
    e.preventDefault()
    showTrashView(true)
    void renderTrashList()
  })
  document.getElementById('btn-trash-close')?.addEventListener('click', e => {
    e.preventDefault()
    showTrashView(false)
    rerender()
  })
  document.getElementById('btn-trash-empty')?.addEventListener('click', async e => {
    e.preventDefault()
    const n = (await window.api.trashList()).length
    if (!n) { flashMsg(document.getElementById('btn-trash-empty'), t('trash.alreadyEmpty', 'Papirkurven er tom'), true); return }
    // The one irreversible step in the whole design, and the only one that
    // still gets a danger dialog.
    const ok = await confirmDialog({
      title:        tn('trash.confirmEmpty', n, {}, 'Slett {n} opptak for godt?'),
      message:      t('trash.confirmEmptyBody', 'Filene slettes fra disken. Dette kan ikke angres.'),
      confirmLabel: t('trash.deleteForever', 'Slett for godt'),
      danger:       true,
    })
    if (!ok) return
    await window.api.trashPurge([])
    await loadHistory()
    repaintHistory()
    await renderTrashList()
  })
}

/** Paint the trash list. Empty trash → back to the history, which is where the
 *  user wanted to be anyway. */
async function renderTrashList(): Promise<void> {
  const list = document.getElementById('trash-list')
  if (!list) return
  let entries: TrashEntry[] = []
  try { entries = await window.api.trashList() } catch { entries = [] }
  const rows = toTrashRows(entries, Date.now())
  list.innerHTML = ''

  if (rows.length === 0) {
    const empty = Object.assign(document.createElement('div'), {
      className: 'trash-empty',
      textContent: t('trash.empty', 'Papirkurven er tom.'),
    })
    list.appendChild(empty)
    // Emptying the last entry closes the view (`refreshTrashButton`), so the
    // history has to be told to come back — nothing else will put it there.
    await refreshTrashButton()
    rerenderHistory()
    return
  }

  const words = {
    today:     t('trash.today', 'i dag'),
    yesterday: t('trash.yesterday', 'i går'),
    daysAgo:   (n: number) => tn('trash.daysAgo', n, {}, '{n} dager siden'),
  }

  for (const row of rows) {
    const el = document.createElement('div')
    el.className = 'trash-row'

    const main = document.createElement('div')
    main.className = 'trash-row-main'
    const name = Object.assign(document.createElement('div'), {
      className: 'trash-row-name',
      textContent: row.name,
    })
    name.title = row.originalPath
    const meta = document.createElement('div')
    meta.className = 'trash-row-meta'
    const parts = [
      `${t('trash.deletedAt', 'Slettet')} ${ageText(row.ageDays, words)}`,
      row.daysLeft > 0
        ? tn('trash.daysLeft', row.daysLeft, {}, '{n} dager igjen')
        : t('trash.dueNow', 'slettes ved neste opprydding'),
    ]
    // Say that the companions travelled: a restore that silently also brings
    // back nine JSON files is fine, but it should not be a surprise.
    if (row.relatedCount > 0) {
      parts.push(tn('trash.related', row.relatedCount, {}, '+ {n} tilhørende filer'))
    }
    meta.textContent = parts.join(' · ')
    main.appendChild(name)
    main.appendChild(meta)

    const actions = document.createElement('div')
    actions.className = 'trash-row-actions'

    const restore = Object.assign(document.createElement('button'), {
      className: 'btn-secondary btn-sm',
      textContent: t('trash.restore', 'Gjenopprett'),
      type: 'button',
    })
    restore.addEventListener('click', async () => {
      restore.disabled = true
      try {
        await window.api.trashRestore(row.id)
      } catch (err) {
        restore.disabled = false
        flashMsg(restore, t('trash.restoreFailed', 'Kunne ikke gjenopprette'), true)
        console.warn('[trash] restore failed:', err)
        return
      }
      await loadHistory()
      repaintHistory()
      await renderTrashList()
    })

    const forever = Object.assign(document.createElement('button'), {
      className: 'btn-ghost btn-sm',
      textContent: t('trash.deleteForever', 'Slett for godt'),
      type: 'button',
    })
    forever.addEventListener('click', async () => {
      const ok = await confirmDialog({
        title:        tf('trash.confirmPurgeOne', { name: row.name }, 'Slett «{name}» for godt?'),
        message:      t('trash.confirmEmptyBody', 'Filene slettes fra disken. Dette kan ikke angres.'),
        confirmLabel: t('trash.deleteForever', 'Slett for godt'),
        danger:       true,
      })
      if (!ok) return
      await window.api.trashPurge([row.id])
      await loadHistory()
      repaintHistory()
      await renderTrashList()
    })

    actions.appendChild(restore)
    actions.appendChild(forever)
    el.appendChild(main)
    el.appendChild(actions)
    list.appendChild(el)
  }
}

// ── Sorting ─────────────────────────────────────────────────────────────────

/** The two sortable columns, by the `data-sort` value on their `<th>`. */
const SORTABLE: Record<string, HistorySortKey> = { time: 'time', duration: 'duration' }

/**
 * Make the Tidspunkt/Varighet headers clickable. Clicking the active column
 * flips direction; clicking the other switches to it at its natural default
 * (newest first / longest first) — going to "oldest first" should take a
 * deliberate second click, never be where a single click lands you.
 */
function setupSortHeaders(rerender: () => void): void {
  for (const th of document.querySelectorAll<HTMLElement>('#search-history-wrap th[data-sort]')) {
    const key = SORTABLE[th.dataset.sort ?? '']
    if (!key) continue
    th.classList.add('th-sortable')
    th.tabIndex = 0
    th.setAttribute('role', 'button')
    const activate = (): void => {
      if (sortKey === key) sortDir = sortDir === 'desc' ? 'asc' : 'desc'
      else { sortKey = key; sortDir = 'desc' }
      renderSortIndicators()
      rerender()
    }
    th.addEventListener('click', activate)
    th.addEventListener('keydown', e => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); activate() }
    })
  }
  renderSortIndicators()
}

function renderSortIndicators(): void {
  for (const th of document.querySelectorAll<HTMLElement>('#search-history-wrap th[data-sort]')) {
    const key = SORTABLE[th.dataset.sort ?? '']
    const active = key === sortKey
    th.classList.toggle('th-sort-active', active)
    th.dataset.dir = active ? sortDir : ''
    // aria-sort is what a screen reader announces; the arrow is what everyone
    // else reads. Both, or the column is only sorted for sighted users.
    th.setAttribute('aria-sort', active ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none')
    const arrow = th.querySelector('.th-sort-arrow')
    if (arrow) arrow.textContent = active ? (sortDir === 'asc' ? '↑' : '↓') : ''
  }
}

// ── Filter chips ────────────────────────────────────────────────────────────

const FILTERS: HistoryFilter[] = ['all', 'audio', 'video', 'transcript']

function setupFilterChips(rerender: () => void): void {
  for (const chip of document.querySelectorAll<HTMLElement>('#history-filter-chips .hist-chip')) {
    const value = chip.dataset.filter as HistoryFilter | undefined
    if (!value || !FILTERS.includes(value)) continue
    chip.addEventListener('click', () => {
      activeFilter = value
      renderFilterChips()
      rerender()
    })
  }
  renderFilterChips()
}

function renderFilterChips(): void {
  for (const chip of document.querySelectorAll<HTMLElement>('#history-filter-chips .hist-chip')) {
    const active = chip.dataset.filter === activeFilter
    chip.classList.toggle('hist-chip-active', active)
    chip.setAttribute('aria-pressed', String(active))
  }
}

function showNoteModal(currentNote: string, onSave: (note: string) => void): void {
  const textarea = document.getElementById('note-textarea') as HTMLTextAreaElement | null
  if (!textarea) return
  textarea.value = currentNote
  // Escape, backdrop-click, the focus trap and `inert` on the page behind are
  // the modal manager's job now — this function only owns save-vs-discard.
  openModal('modal-note')
  setTimeout(() => textarea.focus(), 50)

  const saveBtn   = document.getElementById('btn-note-save')
  const cancelBtn = document.getElementById('btn-note-cancel')

  const close = () => {
    closeModal('modal-note')
    saveBtn?.removeEventListener('click', handleSave)
    cancelBtn?.removeEventListener('click', handleCancel)
    textarea.removeEventListener('keydown', handleKey)
  }
  const handleSave   = () => { onSave(textarea.value); close() }
  const handleCancel = () => close()
  const handleKey    = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { onSave(textarea.value); close() }
  }
  saveBtn?.addEventListener('click', handleSave)
  cancelBtn?.addEventListener('click', handleCancel)
  textarea.addEventListener('keydown', handleKey)
}
