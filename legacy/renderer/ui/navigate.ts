/**
 * One way to go somewhere in the app.
 *
 * Every "Endre" link on Home used to hand-roll the same four steps: call
 * showPage, synthesise a `.click()` on a settings tab button, wait a frame, then
 * scrollIntoView + pulse a card. Ten copies, each subtly different — some
 * highlighted, some didn't, some waited a frame, some didn't, and the ones that
 * clicked the tab did it through a selector that matched the wrong element on
 * pages with more than one tab strip.
 *
 *   navigateTo('settings', { tab: 'settings-audio', anchor: '#channel-grid-card' })
 */

import { markPageEntered } from './motion'

export interface NavigateOpts {
  /** Inner-tab id within the destination page, e.g. 'settings-audio'. */
  tab?: string
  /** CSS selector or element id of what to scroll to once the page is up. */
  anchor?: string
  /** Pulse the anchor's card so the eye finds it. Defaults to true when an
   *  anchor is given — arriving somewhere without knowing why is the failure
   *  mode this replaces. */
  highlight?: boolean
}

/** Pulse a card to say "this is the thing you came for". */
export function highlightEl(el: HTMLElement | null): void {
  if (!el) return
  el.classList.remove('setting-highlight')
  void el.offsetWidth // restart the animation if it is already running
  el.classList.add('setting-highlight')
  setTimeout(() => el.classList.remove('setting-highlight'), 4400)
}

/** Accepts '#id', '.selector' or a bare element id. */
function resolve(anchor: string): HTMLElement | null {
  if (anchor.startsWith('#') || anchor.startsWith('.') || anchor.includes(' ')) {
    return document.querySelector<HTMLElement>(anchor)
  }
  return document.getElementById(anchor)
}

/**
 * Where the retired settings tabs went.
 *
 * Fase 3 folded seven tabs into five. The blocks kept their ids and moved, so
 * every `navigateTo('settings', { tab: 'settings-publish' })` in the app — and
 * any we haven't found — still lands on the right thing: the alias resolves the
 * tab and supplies a default anchor pointing at the section itself.
 *
 * Keep this map. It is cheaper than hunting every call site every time the
 * information architecture moves, and a deep link that silently opens the wrong
 * tab is worse than one that fails loudly.
 */
export const TAB_ALIASES: Record<string, { tab: string; anchor: string }> = {
  'settings-publish':       { tab: 'settings-sharing', anchor: '#settings-publish' },
  'settings-notifications': { tab: 'settings-sharing', anchor: '#settings-notifications' },
  'settings-integrations':  { tab: 'settings-general', anchor: '#settings-integrations' },
}

/** Switch the inner tab of a page without going through a synthetic click. */
export function selectInnerTab(pageId: string, tabId: string): void {
  const resolved = TAB_ALIASES[tabId]?.tab ?? tabId
  const btn = document.querySelector<HTMLElement>(
    `#page-${pageId} .inner-tab[data-tab="${resolved}"]`,
  )
  // The tab buttons carry the page's own side effects (device-list refresh,
  // channel-grid teardown) in their click handlers, so clicking is the correct
  // way to switch — reproducing those effects here would just fork them.
  btn?.click()
}

/**
 * Go to a page, optionally to a specific tab, optionally to a specific control.
 * Everything after the page switch runs on the next frame, once the destination
 * is laid out and measurable.
 */
export function navigateTo(page: string, opts: NavigateOpts = {}): void {
  window.showPage(page)
  markPageEntered(document.getElementById(`page-${page}`))

  if (opts.tab) selectInnerTab(page, opts.tab)

  // A retired tab id with no anchor of its own still deserves to land on its
  // section rather than at the top of the tab it was merged into.
  const alias = opts.tab ? TAB_ALIASES[opts.tab] : undefined
  const anchor = opts.anchor ?? alias?.anchor
  if (!anchor) return
  const shouldHighlight = opts.highlight !== false

  requestAnimationFrame(() => {
    const el = resolve(anchor)
    if (!el) return
    // Something moved behind a disclosure (Sunday-suite) must be OPENED, not
    // just scrolled to — otherwise the deep link lands on a closed summary.
    const details = el.closest('details')
    if (details && !details.open) details.open = true
    // A card is what the user recognises as "the setting"; fall back to the
    // element itself when it is not inside one.
    const card = (el.closest('.card') as HTMLElement | null) ?? el
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
    if (shouldHighlight) highlightEl(card)
  })
}
