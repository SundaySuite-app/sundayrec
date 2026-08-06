/**
 * The editor workspace's three tabs — Lyd · Innhold · Klipp-verktøy.
 *
 * Everything below the waveform used to be one vertical stack of cards:
 * normalize, intro/outro, metadata, analyse, transcript, sermon helper,
 * mastering, cover art. Eight cards, most of them idle most of the time, and
 * the one you wanted was reliably three scrolls away. The owner's word for it
 * was «rotete».
 *
 * Two rules make the tabs safe to use in an editor:
 *
 *  1. **Nothing unmounts.** Switching flips the `hidden` attribute on a panel
 *     that is already in the DOM. No panel is created, re-initialised or torn
 *     down, so the transcript you just generated, a running mastering job, the
 *     cover-art preview and every event listener survive a switch. The waveform
 *     lives OUTSIDE the tabs and is never touched by one.
 *
 *  2. **Nothing hides behind a tab.** Work that finishes while its panel is not
 *     on screen has to announce itself: {@link flagEditorTab} lights a dot on
 *     the tab label, and the suggestion banner that the detection pass produces
 *     sits above the tabs, permanently visible.
 */

/** The tab ids, in tab order. */
export const EDITOR_TABS = ['audio', 'content', 'clip'] as const
export type EditorTabId = (typeof EDITOR_TABS)[number]

/** localStorage key for the remembered tab. */
const TAB_KEY = 'sundayrec.editor.tab'

/**
 * Which tab a stored value means. Anything we don't recognise — a key written
 * by an older build, a hand-edited value, a `null` from private mode — opens
 * Lyd, because that is the tab whose controls apply to every file.
 */
export function resolveTabId(stored: string | null | undefined): EditorTabId {
  return (EDITOR_TABS as readonly string[]).includes(stored ?? '')
    ? (stored as EditorTabId)
    : 'audio'
}

let active: EditorTabId = 'audio'

const tabButton = (id: EditorTabId): HTMLButtonElement | null =>
  document.querySelector<HTMLButtonElement>(`#editor-tabs .editor-tab[data-tab="${id}"]`)

const tabPanel = (id: EditorTabId): HTMLElement | null =>
  document.getElementById(`editor-tabpanel-${id}`)

/** The tab currently on screen. */
export function activeEditorTab(): EditorTabId {
  return active
}

/**
 * Show one tab. Idempotent, and safe to call before the page is wired — a
 * missing panel is simply skipped rather than throwing inside a click handler.
 */
export function showEditorTab(id: EditorTabId, opts: { persist?: boolean } = {}): void {
  active = id
  for (const t of EDITOR_TABS) {
    const btn = tabButton(t)
    const panel = tabPanel(t)
    const on = t === id
    if (btn) {
      btn.classList.toggle('active', on)
      btn.setAttribute('aria-selected', String(on))
      // Roving tabindex: one stop for the whole strip, arrows move inside it.
      btn.tabIndex = on ? 0 : -1
    }
    if (panel) panel.hidden = !on
  }
  // Arriving on a tab is reading its news, so the dot goes out.
  flagEditorTab(id, false)
  if (opts.persist !== false) {
    try { localStorage.setItem(TAB_KEY, id) } catch { /* private mode */ }
  }
}

/**
 * Light (or clear) the attention dot on a tab label.
 *
 * Lighting the tab you are already looking at would be noise, so a request to
 * flag the active tab is dropped — the user can see whatever happened.
 */
export function flagEditorTab(id: EditorTabId, on: boolean): void {
  const badge = document.getElementById(`editor-tab-badge-${id}`)
  if (!badge) return
  const lit = on && id !== active
  badge.hidden = !lit
  const btn = tabButton(id)
  btn?.classList.toggle('has-badge', lit)
}

/** Wire the tab strip. Called once from `setupEditorPage`. */
export function setupEditorTabs(): void {
  const strip = document.getElementById('editor-tabs')
  if (!strip) return

  for (const id of EDITOR_TABS) {
    tabButton(id)?.addEventListener('click', () => showEditorTab(id))
  }

  // ArrowLeft/Right (plus Home/End) move between tabs and take focus with
  // them, which is what a `role="tablist"` promises a keyboard user.
  strip.addEventListener('keydown', (e: KeyboardEvent) => {
    const idx = EDITOR_TABS.indexOf(active)
    let next: EditorTabId | null = null
    if (e.key === 'ArrowRight') next = EDITOR_TABS[(idx + 1) % EDITOR_TABS.length]
    else if (e.key === 'ArrowLeft') next = EDITOR_TABS[(idx - 1 + EDITOR_TABS.length) % EDITOR_TABS.length]
    else if (e.key === 'Home') next = EDITOR_TABS[0]
    else if (e.key === 'End') next = EDITOR_TABS[EDITOR_TABS.length - 1]
    if (!next) return
    e.preventDefault()
    showEditorTab(next)
    tabButton(next)?.focus()
  })

  let stored: string | null = null
  try { stored = localStorage.getItem(TAB_KEY) } catch { /* private mode */ }
  // Restoring is not a choice the user just made, so it doesn't re-persist.
  showEditorTab(resolveTabId(stored), { persist: false })
}
