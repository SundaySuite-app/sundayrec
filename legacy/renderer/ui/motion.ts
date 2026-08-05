/**
 * Motion primitives.
 *
 * Two jobs: (1) make show/hide a class change instead of a `style.display`
 * assignment, so things can animate at all; (2) make every animation in the app
 * agree on what "reduced motion" means — instant, not merely faster.
 *
 * The stylesheet already clamps animation-duration under
 * `prefers-reduced-motion`, but a clamped duration still fires transitionend
 * late and still moves. Code that needs to *skip* the animation asks here.
 */

const REDUCED = '(prefers-reduced-motion: reduce)'

/** True when the OS asks for reduced motion. Read live — users toggle it. */
export function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia(REDUCED).matches
}

/**
 * Exits that have started but not finished, keyed by element. `showEl` calls
 * the stored canceller so a re-show inside the exit window doesn't get undone a
 * moment later by the previous hide's `display:none`.
 */
const pendingHides = new WeakMap<HTMLElement, () => void>()

/**
 * Reveal an element by adding `cls` (default `is-open`) and clearing any inline
 * `display:none` left behind by the old style.display idiom.
 */
export function showEl(el: HTMLElement | null, cls = 'is-open'): void {
  if (!el) return
  // Abandon an exit that is still in flight. Without this, hide→show inside the
  // exit window leaves the OLD hide's finish handler armed, and it fires a
  // moment later and hides an element that is now supposed to be on screen.
  // The recording overlay makes this concrete: the engine can emit a terminal
  // state and a fresh `recording` state within the same few hundred ms (split
  // restart, resync after a transient error), and an invisible overlay over a
  // live take means a running recording with no stop button.
  pendingHides.get(el)?.()
  el.style.display = ''
  el.classList.remove('is-leaving')
  // Force a reflow so a hide→show inside the same frame still transitions.
  void el.offsetWidth
  el.classList.add(cls)
}

/**
 * Hide an element: mark it leaving, wait for the CSS transition to finish, then
 * take it out of the layout. Falls back to a timeout because `transitionend`
 * never fires when nothing actually transitions (element already hidden, zero
 * duration, reduced motion) — a leak that silently strands elements half-open.
 */
export function hideEl(el: HTMLElement | null, cls = 'is-open'): void {
  if (!el) return
  const finish = (): void => {
    el.classList.remove('is-leaving')
    el.style.display = 'none'
  }
  el.classList.remove(cls)
  if (prefersReducedMotion()) {
    pendingHides.delete(el)
    finish()
    return
  }
  el.classList.add('is-leaving')
  let done = false
  const onEnd = (e: TransitionEvent): void => {
    if (e.target === el) once()
  }
  const timer = setTimeout(() => once(), 220)
  const detach = (): void => {
    done = true
    clearTimeout(timer)
    el.removeEventListener('transitionend', onEnd)
    pendingHides.delete(el)
  }
  const once = (): void => {
    if (done) return
    detach()
    finish()
  }
  // Abandon the exit WITHOUT hiding — showEl's escape hatch.
  pendingHides.set(el, () => {
    if (done) return
    detach()
    el.classList.remove('is-leaving')
  })
  el.addEventListener('transitionend', onEnd)
}

/**
 * Cross-fade between pages.
 *
 * `swap` performs the actual page switch and must be synchronous. The outgoing
 * page fades for 120ms first so the two pages are never both at full opacity,
 * which is what made navigation feel like a jump cut.
 *
 * Also resets `#main`'s scroll — the old showPage never did, so arriving on a
 * short page after scrolling a long one left you looking at blank space.
 */
export function applyPageTransition(outgoing: HTMLElement | null, swap: () => void): void {
  const main = document.getElementById('main')
  const done = (): void => {
    swap()
    if (main) main.scrollTop = 0
  }
  if (!outgoing || prefersReducedMotion()) {
    done()
    return
  }
  outgoing.classList.add('page-leaving')
  setTimeout(() => {
    outgoing.classList.remove('page-leaving')
    done()
  }, 120)
}

/**
 * Cross-fade between INNER tabs (the settings tab strip).
 *
 * Same shape and same 120 ms as `applyPageTransition`, minus the scroll reset:
 * an inner tab is a section of the page you are already on, so throwing away
 * the scroll position would be a second, unasked-for change. `swap` must be
 * synchronous.
 */
export function applyInnerTabTransition(outgoing: HTMLElement | null, swap: () => void): void {
  if (!outgoing || prefersReducedMotion()) {
    swap()
    return
  }
  outgoing.classList.add('tab-leaving')
  setTimeout(() => {
    outgoing.classList.remove('tab-leaving')
    swap()
  }, 120)
}

/**
 * True the FIRST time a list container is populated, false on every refresh
 * after that — so entrance animations play on arrival and never again.
 *
 * A list that re-staggers on every refresh is the UI equivalent of forgetting
 * what it just showed you: finish a recording and the four rows that were
 * already on screen fly in again alongside the new one.
 *
 * Renderers that clear the list (empty state, a new file in the editor) call
 * `resetMount` so the next population counts as an arrival again.
 */
export function firstMount(container: Element | null): boolean {
  if (!(container instanceof HTMLElement)) return false
  if (container.dataset.mounted === '1') return false
  container.dataset.mounted = '1'
  return true
}

/** Re-arm `firstMount` for a container that has been emptied. */
export function resetMount(container: Element | null): void {
  if (container instanceof HTMLElement) delete container.dataset.mounted
}

/**
 * Mark a page as having been shown at least once.
 *
 * `.card { animation: cardIn }` is a nice first impression and an irritant on
 * every subsequent visit — nine cards restaging themselves each time you tab
 * back to Innstillinger. The `.entered` flag turns the animation off from the
 * second activation onward (see ui.css).
 */
export function markPageEntered(page: HTMLElement | null): void {
  if (!page) return
  if (page.dataset.entered === '1') {
    page.classList.add('entered')
    return
  }
  page.dataset.entered = '1'
  // Let the entrance play out once, then latch it off.
  setTimeout(() => page.classList.add('entered'), 400)
}
