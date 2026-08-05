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

/** Switch the inner tab of a page without going through a synthetic click. */
export function selectInnerTab(pageId: string, tabId: string): void {
  const btn = document.querySelector<HTMLElement>(
    `#page-${pageId} .inner-tab[data-tab="${tabId}"]`,
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

  if (!opts.anchor) return
  const anchor = opts.anchor
  const shouldHighlight = opts.highlight !== false

  requestAnimationFrame(() => {
    const el = resolve(anchor)
    if (!el) return
    // A card is what the user recognises as "the setting"; fall back to the
    // element itself when it is not inside one.
    const card = (el.closest('.card') as HTMLElement | null) ?? el
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
    if (shouldHighlight) highlightEl(card)
  })
}
