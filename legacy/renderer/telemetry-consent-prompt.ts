/**
 * The one-time telemetry consent prompt for installs that already finished
 * onboarding.
 *
 * SundayRec has shipped since v0.1 without telemetry, so most installs have
 * `settings.onboardingDone === true` and will never see the onboarding
 * wizard's own consent step (pages/onboarding.ts, step 5) again. Those
 * installs still deserve to be asked — once — so this runs at startup and
 * shows a small card instead of the wizard's full page.
 *
 * NON-BLOCKING BY DESIGN. SundayRec is launched on Sunday mornings by a
 * volunteer who wants to press one button and walk away. A confirmDialog()
 * here would sit between them and «Start opptak» the one morning it matters
 * most — so this is a corner card (`#telemetry-consent-toast` in index.html,
 * the same non-modal shape as the update-available toast), which can be
 * ignored, answered, or closed without blocking a single other click in the
 * app. See the module's CSS comment in styles.css for why it sits in its own
 * corner rather than sharing the update toast's.
 *
 * The question asked is the CONSENT_VERSION scope, not merely "never
 * answered": `needsPrompt` from telemetry_consent_get is true both for a
 * never-asked install and for a stale grant/denial whose scope has since
 * widened (crates/sundayrec-core/telemetry/consent.rs) — both are genuinely
 * "the UI should ask", and a plain, CURRENT-version "no" is deliberately NOT
 * one of them, so declining here (or in onboarding) is never re-litigated.
 *
 * Those two cases get DIFFERENT WORDS — see telemetry-consent-copy-core.ts,
 * which owns the choice and is where the reasoning lives. The card carries the
 * first-ask wording statically in index.html; `applyCopy()` swaps in the
 * scope-widened wording for anyone who has already answered.
 */

import { settings } from './state'
import { t } from './i18n'
import { promptCopyFor, type PromptCopy } from './telemetry-consent-copy-core'

let wired = false
let shown = false

/** Call once at startup, after settings have loaded. Wires the card's buttons
 *  (idempotent) and shows it if — and only if — this install is due to be
 *  asked. */
export function initTelemetryConsentPrompt(): void {
  wireCard()
  void maybeShow()
}

function wireCard(): void {
  if (wired) return
  wired = true

  document.getElementById('telemetry-consent-toast-yes')?.addEventListener('click', () => void answer(true))
  document.getElementById('telemetry-consent-toast-no')?.addEventListener('click', () => void answer(false))
  // The × close resolves the SAME way as «Nei takk». SundayRec never collects
  // anything without an explicit "yes", so dismissing without answering costs
  // the user nothing beyond staying at the default-off state they were
  // already in — and it is the only way to honour "ask once" for someone who
  // closes the card instead of clicking one of the two buttons: the backend
  // has exactly two answered states (granted/denied) plus "never asked", and
  // leaving it at "never asked" would just show this card again next launch.
  document.getElementById('telemetry-consent-toast-close')?.addEventListener('click', () => void answer(false))
  document.getElementById('telemetry-consent-toast-privacy-link')?.addEventListener('click', e => {
    e.preventDefault()
    void window.api.openPrivacyPolicy()
  })
}

async function maybeShow(): Promise<void> {
  if (shown) return
  // A fresh install answers this as onboarding's own step 5 — asking again
  // here the moment that wizard closes (it also sets onboardingDone) would be
  // exactly the double-ask this module exists to avoid.
  if (!settings.onboardingDone) return

  const consent = await window.api.telemetryConsentGet().catch(() => null)
  if (!consent || !consent.needsPrompt) return

  applyCopy(promptCopyFor(consent.status))

  shown = true
  const card = document.getElementById('telemetry-consent-toast')
  if (card) card.style.display = 'flex'
}

/** Point the card's title and body at the chosen wording.
 *
 *  The `data-i18n` ATTRIBUTE is rewritten, not just the text: `applyTranslations()`
 *  re-reads it on every language change and would otherwise put index.html's
 *  first-ask wording back under a user who is mid-re-ask. */
function applyCopy(copy: PromptCopy): void {
  retarget('telemetry-consent-toast-title', copy.titleKey, copy.titleFallback)
  retarget('telemetry-consent-toast-text', copy.descKey, copy.descFallback)
}

function retarget(id: string, key: string, fallback: string): void {
  const el = document.getElementById(id)
  if (!el) return
  el.dataset.i18n = key
  el.textContent = t(key, fallback)
}

async function answer(granted: boolean): Promise<void> {
  hide()
  // Best-effort: on a real failure the backend stays at NeverAsked, which
  // means this card would legitimately be due again next launch — the honest
  // outcome for an answer that was never actually recorded.
  await window.api.telemetryConsentSet(granted).catch(() => null)
}

function hide(): void {
  const card = document.getElementById('telemetry-consent-toast')
  if (card) card.style.display = 'none'
}
