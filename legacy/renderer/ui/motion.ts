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
 * Reveal an element by adding `cls` (default `is-open`) and clearing any inline
 * `display:none` left behind by the old style.display idiom.
 */
export function showEl(el: HTMLElement | null, cls = 'is-open'): void {
  if (!el) return
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
    finish()
    return
  }
  el.classList.add('is-leaving')
  let done = false
  const once = (): void => {
    if (done) return
    done = true
    el.removeEventListener('transitionend', onEnd)
    finish()
  }
  const onEnd = (e: TransitionEvent): void => {
    if (e.target === el) once()
  }
  el.addEventListener('transitionend', onEnd)
  setTimeout(once, 220)
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
