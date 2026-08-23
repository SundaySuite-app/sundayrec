/**
 * Honest surfaces.
 *
 * `applyFeatureGate(section, {...})` puts a status chip + one line of
 * explanation at the top of a section and makes its controls `inert` when the
 * backend behind them is missing. `inert` is the load-bearing part: a `disabled`
 * attribute has to be applied per element and forgotten once, while `inert`
 * removes a whole subtree from clicks, focus and the accessibility tree.
 *
 * A section that WORKS gets nothing at all — no chip, no banner, no dimming.
 * A gate that is always on is wallpaper.
 *
 * See `feature-gate-core.ts` for the status mapping (unit-tested).
 */

import { t } from '../i18n'
import { mapGate, type GateInput, type GateStatus } from './feature-gate-core'

export type { GateStatus } from './feature-gate-core'

const BANNER_CLASS = 'feature-gate'
const OFF_CLASS = 'gate-off'

/** Sections are usually cards; `.card-title` should stay above the banner so the
 *  user still sees WHAT is gated before reading WHY. */
function insertionPoint(section: HTMLElement): Element | null {
  const title = section.querySelector<HTMLElement>(':scope > .card-title')
  return title ? title.nextElementSibling : section.firstElementChild
}

/**
 * Gate (or un-gate) a section.
 *
 * Idempotent: calling it again with a new status updates the banner in place
 * and re-enables whatever it previously turned off, so an async status check
 * that resolves late (or a re-check after the user connects an account) does
 * the right thing.
 */
export function applyFeatureGate(
  sectionEl: HTMLElement | string | null,
  opts: GateInput,
): void {
  const section = typeof sectionEl === 'string' ? document.getElementById(sectionEl) : sectionEl
  if (!section) return

  const view = mapGate(opts)
  let banner = section.querySelector<HTMLElement>(`:scope > .${BANNER_CLASS}`)

  // Always clear the previous gate first — a status can improve.
  section.querySelectorAll<HTMLElement>(`:scope > .${OFF_CLASS}`).forEach(child => {
    child.classList.remove(OFF_CLASS)
    child.removeAttribute('inert')
  })

  if (!view.showBanner) {
    banner?.remove()
    section.classList.remove('is-gated')
    return
  }

  if (!banner) {
    banner = document.createElement('div')
    banner.className = BANNER_CLASS
    banner.setAttribute('role', 'note')
    const at = insertionPoint(section)
    if (at) section.insertBefore(banner, at)
    else section.appendChild(banner)
  }
  banner.classList.remove('is-unconfigured', 'is-unavailable')
  banner.classList.add(`is-${view.variant}`)
  banner.innerHTML = ''

  const chip = document.createElement('span')
  chip.className = 'feature-gate-chip'
  chip.textContent = view.chipText
  banner.appendChild(chip)

  const text = document.createElement('span')
  text.className = 'feature-gate-text'
  text.textContent = view.explanation
  if (view.docsHint) {
    const hint = document.createElement('span')
    hint.className = 'feature-gate-hint'
    hint.textContent = view.docsHint
    text.appendChild(hint)
  }
  banner.appendChild(text)

  section.classList.add('is-gated')
  for (const child of Array.from(section.children)) {
    if (child === banner) continue
    // Keep the title readable — it is the label of what is gated.
    if (child.classList.contains('card-title')) continue
    child.classList.add(OFF_CLASS)
    child.setAttribute('inert', '')
  }
}

// `applyComingSoonGate` lived here — the «Kommer» chip, worn by the three
// (since-removed) thumbnail panels and by nothing else. All three had the same
// cause (no Rust side at all) and all three lost it in Fase 6, so the helper
// had no callers left. Keeping it would have been an invitation to reach for a
// chip instead of writing the missing half; the locale keys went with it.

/** Convenience for the common "no backend in this build" case. */
export function unavailableGate(
  sectionEl: HTMLElement | string | null,
  explanation: string,
  docsHint?: string,
): void {
  applyFeatureGate(sectionEl, {
    status: 'unavailable',
    chipText: t('gate.chipUnavailable', 'Ikke tilgjengelig'),
    explanation,
    docsHint,
  })
}

/** Convenience for "exists, but nobody has set it up". */
export function unconfiguredGate(
  sectionEl: HTMLElement | string | null,
  explanation: string,
  docsHint?: string,
): void {
  applyFeatureGate(sectionEl, {
    status: 'unconfigured',
    chipText: t('gate.chipUnconfigured', 'Ikke konfigurert'),
    explanation,
    docsHint,
  })
}

/** Re-export so callers can type a status without importing the core too. */
export function statusOf(ok: boolean, whenFalse: GateStatus = 'unconfigured'): GateStatus {
  return ok ? 'ok' : whenFalse
}
