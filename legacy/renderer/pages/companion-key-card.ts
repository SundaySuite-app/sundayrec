/**
 * The AI sermon-companion API-key card — the one card left under the Avansert
 * disclosure at the bottom of System now that the Sunday-suite integrations
 * are gone (R1 of «Frivilligen først»; the companion itself is R2's call).
 *
 * The key is stored keychain-only via the dedicated companion IPC, never in
 * settings, and it only upgrades the summary prose — chapters/highlights
 * always run locally. Explicit save: a key committed halfway is a key that
 * silently does not work.
 */

import { t } from '../i18n'
import { showSavedChip } from '../ui/bind-setting'
import { setFieldError } from '../ui/field-error'

/** Human-readable message from a rejected `window.api` call (Tauri serializes
 *  AppError to `{ code, message }`, not an `Error`). */
function errText(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  const o = (e ?? {}) as { message?: unknown; code?: unknown }
  if (typeof o.message === 'string' && o.message) return o.message
  if (typeof o.code === 'string' && o.code) return o.code
  return String(e)
}

const $ = (id: string) => document.getElementById(id)

export async function setupCompanionKeyCard(): Promise<void> {
  try {
    const companionStatus = $('companion-apikey-status')
    if (companionStatus) {
      const configured = await window.api.companionLlmConfigured()
      companionStatus.textContent = configured
        ? t('companion.keyStored', '✓ API-nøkkel lagret (nøkkelring)')
        : t('companion.keyNone', 'Ingen nøkkel — lokal oppsummering brukes')
    }
  } catch { /* leave status blank */ }

  $('btn-companion-apikey-save')?.addEventListener('click', async () => {
    const inp = $('companion-apikey') as HTMLInputElement | null
    const statusEl = $('companion-apikey-status')
    if (!inp) return
    const key = inp.value.trim()
    if (!key) {
      // Used to return silently — a click on Lagre that did nothing at all.
      setFieldError(inp, t('companion.errKeyEmpty', 'Lim inn nøkkelen først'))
      return
    }
    setFieldError(inp, null)
    try {
      await window.api.companionSetLlmKey(key)
    } catch (err) {
      setFieldError(inp, `${t('companion.errKeySaveFailed', 'Kunne ikke lagre nøkkelen')}: ${errText(err)}`)
      return
    }
    inp.value = ''
    if (statusEl) statusEl.textContent = t('companion.keyStored', '✓ API-nøkkel lagret (nøkkelring)')
    showSavedChip($('btn-companion-apikey-save')?.parentElement ?? null)
  })

  $('btn-companion-apikey-clear')?.addEventListener('click', async () => {
    const inp = $('companion-apikey') as HTMLInputElement | null
    const statusEl = $('companion-apikey-status')
    try {
      await window.api.companionClearLlmKey()
    } catch (err) {
      // «Ingen nøkkel» under er et LØFTE om at nøkkelen er borte — det får
      // ikke stå der hvis slettingen faktisk feilet.
      setFieldError(inp, `${t('companion.errKeyClearFailed', 'Kunne ikke fjerne nøkkelen')}: ${errText(err)}`)
      return
    }
    setFieldError(inp, null)
    if (inp) inp.value = ''
    if (statusEl) statusEl.textContent = t('companion.keyNone', 'Ingen nøkkel — lokal oppsummering brukes')
  })
}
