/**
 * Modal manager for the eight `.modal-backdrop` roots that live in index.html.
 *
 * They were opened and closed with `style.display = 'flex' | 'none'` from eight
 * different files. That gave us: no animation, no focus management (Tab walked
 * straight out of the modal and into the page behind it), no `inert` on the
 * background, and a global Escape handler that had to guess which button was
 * the cancel by matching id patterns.
 *
 * openModal/closeModal replace all of that. The `display` property is still
 * managed here (rather than purely by class) so that any code left reading
 * `style.display` keeps seeing the truth during the migration.
 */

import { nextFocusIndex } from './dialog-core'
import { prefersReducedMotion } from './motion'

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])'

interface OpenModal {
  el: HTMLElement
  restoreFocus: HTMLElement | null
}

const openStack: OpenModal[] = []

/** Elements we set `inert` on, so it can be removed from exactly those again. */
let inerted: HTMLElement[] = []

/**
 * Make everything except the open modal inert — no focus, no clicks, no screen
 * reader.
 *
 * Naively inerting #main and #sidebar is WRONG here: a modal root authored
 * inside `#main` (the transcribe modals were, until v0.15) would have been
 * made inert along with the page and left looking open but dead to every
 * click.
 *
 * Instead, walk from the modal up to <body> and inert each ancestor's other
 * children. At body level that resolves to exactly #main + #sidebar; for a
 * nested modal it spares the branch the modal lives on.
 */
function setBackgroundInert(modal: HTMLElement | null): void {
  for (const el of inerted) el.removeAttribute('inert')
  inerted = []
  if (!modal) return

  let node: HTMLElement = modal
  while (node.parentElement && node.parentElement !== document.documentElement) {
    for (const sibling of Array.from(node.parentElement.children)) {
      if (sibling === node) continue
      if (!(sibling instanceof HTMLElement)) continue
      // The dialog service owns its own layer and always sits on top.
      if (sibling.classList.contains('ui-dialog-root')) continue
      // Something already inert for its own reasons — leave it to its owner.
      if (sibling.hasAttribute('inert')) continue
      sibling.setAttribute('inert', '')
      inerted.push(sibling)
    }
    node = node.parentElement
  }
}

/** True when Escape and a backdrop click must NOT close this modal. */
function isDismissable(el: HTMLElement): boolean {
  return !el.hasAttribute('data-no-escape') && !el.hasAttribute('data-no-dismiss')
}

/** The modal on top of the stack, or null. */
export function topModal(): HTMLElement | null {
  return openStack.length ? openStack[openStack.length - 1].el : null
}

export function isModalOpen(id: string): boolean {
  return openStack.some(m => m.el.id === id)
}

/**
 * Open a modal by element id. Idempotent — opening an already-open modal just
 * re-focuses it rather than pushing a duplicate onto the stack.
 */
export function openModal(id: string): HTMLElement | null {
  const el = document.getElementById(id)
  if (!el) return null
  if (isModalOpen(id)) {
    focusFirst(el)
    return el
  }

  const restoreFocus = document.activeElement as HTMLElement | null
  openStack.push({ el, restoreFocus })

  el.style.display = 'flex'
  el.classList.remove('is-leaving')
  void el.offsetWidth // reflow so the entrance transition runs
  el.classList.add('open')
  setBackgroundInert(el)

  focusFirst(el)
  return el
}

/**
 * Close a modal by id. Safe to call on a modal that is already closed — the
 * old `style.display='none'` sites did that routinely.
 */
export function closeModal(id: string): void {
  const idx = openStack.findIndex(m => m.el.id === id)
  const el = document.getElementById(id)
  if (!el) return

  const entry = idx >= 0 ? openStack.splice(idx, 1)[0] : null
  // Re-scope the inert region to whatever modal is left, or clear it entirely.
  setBackgroundInert(topModal())

  el.classList.remove('open')

  const finish = (): void => {
    el.classList.remove('is-leaving')
    el.style.display = 'none'
  }
  if (prefersReducedMotion()) finish()
  else {
    el.classList.add('is-leaving')
    setTimeout(finish, 160)
  }

  // Focus returns to whatever opened this modal — or, if another modal is still
  // up underneath, into that one. Restoring is skipped if the opener has since
  // left the DOM (a re-rendered list row, typically).
  const next = topModal()
  if (next) focusFirst(next)
  else if (entry?.restoreFocus?.isConnected) entry.restoreFocus.focus()
}

/** Close every open modal — used when a page teardown invalidates them all. */
export function closeAllModals(): void {
  for (const m of [...openStack]) closeModal(m.el.id)
}

function focusFirst(el: HTMLElement): void {
  requestAnimationFrame(() => {
    const explicit = el.querySelector<HTMLElement>('[autofocus]')
    if (explicit) {
      explicit.focus()
      return
    }
    const first = el.querySelector<HTMLElement>(FOCUSABLE)
    first?.focus()
  })
}

/**
 * Find the control that means "close this modal".
 *
 * Order matters and is NOT the same as putting all three in one selector list:
 * `querySelector` resolves in document order, and the header ✕ comes before the
 * footer's Avbryt in the markup. The explicit cancel button carries the page's
 * own cleanup (aborting a job, stopping a preview), so it has to win.
 */
function cancelControl(el: HTMLElement): HTMLButtonElement | null {
  return (
    el.querySelector<HTMLButtonElement>('[data-modal-cancel]') ??
    el.querySelector<HTMLButtonElement>('[id$="-cancel"], [id^="btn-cancel-"]') ??
    el.querySelector<HTMLButtonElement>('.modal-close')
  )
}

/**
 * Dismiss the topmost modal the way its own cancel button would, so page-level
 * cleanup (stopping a preview, aborting a job) still runs. Falls back to a
 * plain close when the modal has no cancel control.
 */
export function dismissTopModal(): boolean {
  const el = topModal()
  if (!el || !isDismissable(el)) return false
  const cancel = cancelControl(el)
  if (cancel) cancel.click()
  else closeModal(el.id)
  return true
}

let wired = false

/**
 * Install the global Escape and backdrop-click handling. Called once from
 * main.ts; replaces the old setupGlobalEscape.
 */
export function setupModalManager(): void {
  if (wired) return
  wired = true

  document.addEventListener('keydown', e => {
    if (!openStack.length) return

    if (e.key === 'Escape') {
      // Only modals this manager knows about; the recording overlay and the
      // onboarding wizard are not Escape-dismissable by design.
      if (dismissTopModal()) {
        e.preventDefault()
        e.stopPropagation()
      }
      return
    }

    // Focus trap. `inert` on #main/#sidebar already keeps Tab out of the page,
    // but not out of the browser chrome — this closes the loop inside the card.
    if (e.key === 'Tab') {
      const el = topModal()
      if (!el) return
      const items = Array.from(el.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        item => item.offsetParent !== null,
      )
      if (!items.length) return
      e.preventDefault()
      const current = items.indexOf(document.activeElement as HTMLElement)
      const next = nextFocusIndex(items.length, current, e.shiftKey)
      if (next >= 0) items[next].focus()
    }
  })

  // The header ✕ is equivalent to the modal's own Avbryt, so it inherits that
  // button's cleanup instead of each modal wiring its own close handler.
  document.addEventListener('click', e => {
    const btn = (e.target as HTMLElement | null)?.closest<HTMLElement>('.modal-close')
    if (!btn) return
    const backdrop = btn.closest<HTMLElement>('.modal-backdrop')
    if (!backdrop?.id) return
    const own = cancelControl(backdrop)
    if (own && own !== btn) own.click()
    else closeModal(backdrop.id)
  })

  // Click on the backdrop itself (never on the modal card) closes.
  document.addEventListener('mousedown', e => {
    const target = e.target as HTMLElement | null
    if (!target || !target.classList.contains('modal-backdrop')) return
    if (topModal() !== target || !isDismissable(target)) return
    const cancel = cancelControl(target)
    if (cancel) cancel.click()
    else closeModal(target.id)
  })

  // Any modal already visible at boot (there should be none) is adopted so the
  // stack never lies about what is on screen.
  document.querySelectorAll<HTMLElement>('.modal-backdrop').forEach(el => {
    if (el.style.display && el.style.display !== 'none') openModal(el.id)
  })
}
