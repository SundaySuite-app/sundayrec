/**
 * The toolbar's «hva vises på tidslinjen» popover.
 *
 * Tale / Musikk / Stillhet decide what the waveform DRAWS. They were three
 * checkboxes inside the «Analyser opptak» card, which put a way of looking at
 * the recording inside a job you run on it — and kept a permanent block of
 * chrome on screen for a choice most operators make once, if ever.
 *
 * They now live behind a layers button next to the zoom controls, where the
 * other view controls are. Deliberately a small LOCAL popover rather than
 * anything reusable: it is one anchored panel with three checkboxes, and the
 * app already has a modal manager for the cases that need a focus trap.
 */

const $ = (id: string): HTMLElement | null => document.getElementById(id)

let open = false

function apply(next: boolean): void {
  const btn = $('btn-editor-view-menu')
  const pop = $('editor-view-popover')
  if (!btn || !pop) return
  open = next
  pop.hidden = !next
  btn.setAttribute('aria-expanded', String(next))
  btn.classList.toggle('active', next)
}

/** Close the popover. `refocus` returns focus to the button — right when the
 *  user dismissed it themselves, wrong when they clicked elsewhere on purpose. */
export function closeViewMenu(refocus = false): void {
  if (!open) return
  apply(false)
  if (refocus) $('btn-editor-view-menu')?.focus()
}

/** Wire the layers button + its popover. Called once from `setupEditorPage`. */
export function setupViewMenu(): void {
  const btn = $('btn-editor-view-menu')
  const pop = $('editor-view-popover')
  const wrap = $('editor-view-menu')
  if (!btn || !pop || !wrap) return

  btn.addEventListener('click', e => {
    e.stopPropagation()
    apply(!open)
    if (open) {
      // Land on the first control the user can actually operate — which, before
      // the recording has been analysed, is none of them.
      pop.querySelector<HTMLInputElement>('input[type="checkbox"]')?.focus()
    }
  })

  // Escape closes and hands focus back. Stopped here so it never doubles as the
  // editor's global "Escape stops playback".
  wrap.addEventListener('keydown', e => {
    if (e.key !== 'Escape' || !open) return
    e.stopPropagation()
    e.preventDefault()
    closeViewMenu(true)
  })

  // Click anywhere else — including on the waveform the popover describes.
  document.addEventListener('click', e => {
    if (!open) return
    if (wrap.contains(e.target as Node)) return
    closeViewMenu(false)
  })

  // Leaving the editor entirely should not leave a popover hanging over the
  // page you came back to.
  window.addEventListener('blur', () => closeViewMenu(false))
}
