/**
 * "Sunday-suite" — now the Avansert disclosure at the bottom of System.
 *
 * Opt-in controls for the sister-app integrations. State is persisted through
 * the dedicated integrations IPC (window.api.get/setIntegrationSettings), not
 * the main settings save flow — so each toggle takes effect immediately and
 * the recording-core settings are never touched. When the master toggle is
 * off, sub-toggles are disabled and no integration code runs anywhere.
 *
 * Save model: the TOGGLES auto-apply through `bindSetting` (with the same
 * «Lagret ✓» receipt as every other setting, via the `persist` override that
 * routes to the integrations IPC). The URL and API-key fields are the third
 * explicit-save exception — a church_id or an API URL is only meaningful once
 * fully typed, and a key committed halfway is a key that silently does not work.
 */

import { t } from '../i18n'
import { bindSetting, showSavedChip } from '../ui/bind-setting'
import { clearFieldErrors, setFieldError } from '../ui/field-error'

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

/** Accept an empty field or a well-formed http(s) URL — nothing in between. */
function urlProblem(value: string): string | null {
  const v = value.trim()
  if (!v) return null
  try {
    const u = new URL(v)
    if (u.protocol !== 'https:' && u.protocol !== 'http:') {
      return t('integrations.errUrlScheme', 'Adressen må begynne med https://')
    }
    return null
  } catch {
    return t('integrations.errUrl', 'Dette er ikke en gyldig adresse (f.eks. https://api.sundaysong.com)')
  }
}

export async function setupIntegrationsPage(): Promise<void> {
  const masterEl     = $('opt-integrations-enabled')  as HTMLInputElement | null
  const sundayEditEl   = $('opt-integrations-sundayedit') as HTMLInputElement | null
  const stageEl      = $('opt-integrations-stage')    as HTMLInputElement | null
  const songEl       = $('opt-integrations-song')     as HTMLInputElement | null
  const planEl       = $('opt-integrations-plan')     as HTMLInputElement | null
  if (!masterEl || !sundayEditEl || !stageEl || !songEl) return

  const cards = ['integrations-sundayedit-card', 'integrations-song-card', 'integrations-plan-card', 'integrations-stage-card', 'integrations-connection-card']

  const applyEnabledState = (): void => {
    const on = masterEl.checked
    ;[sundayEditEl, stageEl, songEl, planEl].filter(Boolean).forEach(el => { (el as HTMLInputElement).disabled = !on })
    cards.forEach(id => {
      const el = $(id)
      if (el) el.style.opacity = on ? '' : '0.5'
    })
    const keyRow = $('integrations-song-key-row')
    if (keyRow) keyRow.style.display = (on && songEl.checked) ? '' : 'none'
  }

  // Load current settings
  let current = { enabled: false, sundayedit: { enabled: false }, stage: { enabled: false }, song: { enabled: false }, connection: { churchId: '', songApiUrl: '' } }
  try {
    const s = await window.api.getIntegrationSettings()
    masterEl.checked   = !!s.enabled
    sundayEditEl.checked = !!s.sundayedit?.enabled
    stageEl.checked    = !!s.stage?.enabled
    songEl.checked     = !!s.song?.enabled
    if (planEl) planEl.checked = !!s.plan?.enabled
    current = s as typeof current

    // Connection fields
    const churchInput   = $('integration-church-id')    as HTMLInputElement | null
    const songUrlInput  = $('integration-song-api-url') as HTMLInputElement | null
    const planUrlInput  = $('integration-plan-api-url') as HTMLInputElement | null
    if (churchInput  && s.connection?.churchId)   churchInput.value   = s.connection.churchId
    if (songUrlInput && s.connection?.songApiUrl) songUrlInput.value  = s.connection.songApiUrl
    if (planUrlInput && s.connection?.planApiUrl) planUrlInput.value  = s.connection.planApiUrl

    // API key presence indicator
    const keyStatus = $('integration-song-apikey-status')
    if (keyStatus) {
      const hasKey = await window.api.songHasApiKey()
      keyStatus.textContent = hasKey ? t('integrations.keySaved', '✓ API-nøkkel lagret (kryptert)') : ''
    }
  } catch {
    masterEl.checked = false
  }
  applyEnabledState()

  // The toggles auto-apply, persisting through the integrations IPC instead of
  // the settings file — same receipt, different store.
  const integrationToggle = (
    id: string,
    payload: (checked: boolean) => Parameters<typeof window.api.setIntegrationSettings>[0],
    after?: () => void,
  ): void => {
    bindSetting(id, {
      key: id,
      apply: () => {},
      persist: async () => {
        const el = $(id) as HTMLInputElement | null
        try {
          // The backend returns the MERGED settings — keep `current` in step so
          // the connection card's Avbryt restores what is actually stored.
          const merged = await window.api.setIntegrationSettings(payload(!!el?.checked))
          current = merged as typeof current
          return true
        } catch {
          // `false` → bindSetting shows the error toast and NO «Lagret ✓».
          return false
        }
      },
      after,
    })
  }
  integrationToggle('opt-integrations-enabled',   checked => ({ enabled: checked }), applyEnabledState)
  integrationToggle('opt-integrations-sundayedit', checked => ({ sundayedit: { enabled: checked } }))
  integrationToggle('opt-integrations-stage',      checked => ({ stage: { enabled: checked } }))
  integrationToggle('opt-integrations-plan',       checked => ({ plan: { enabled: checked } }))
  integrationToggle('opt-integrations-song',       checked => ({ song: { enabled: checked } }), applyEnabledState)

  // ── API key: explicit save (a key is only ever right when complete) ────────
  $('btn-song-apikey-save')?.addEventListener('click', async () => {
    const inp = $('integration-song-apikey') as HTMLInputElement | null
    const statusEl = $('integration-song-apikey-status')
    if (!inp) return
    const key = inp.value.trim()
    if (!key) {
      setFieldError(inp, t('integrations.errKeyEmpty', 'Lim inn nøkkelen først'))
      return
    }
    setFieldError(inp, null)
    try {
      // Keychain write (integrations_song_set_apikey). The ✓ below may only
      // ever follow a store that actually happened — a rejected save shows its
      // real reason here instead of an unhandled rejection and a silent field.
      await window.api.songSetApiKey(key)
    } catch (err) {
      setFieldError(inp, `${t('integrations.errKeySaveFailed', 'Kunne ikke lagre nøkkelen')}: ${errText(err)}`)
      return
    }
    inp.value = ''
    if (statusEl) statusEl.textContent = t('integrations.keySaved', '✓ API-nøkkel lagret (kryptert)')
    showSavedChip($('btn-song-apikey-save')?.parentElement ?? null)
  })

  $('btn-song-apikey-cancel')?.addEventListener('click', () => {
    const inp = $('integration-song-apikey') as HTMLInputElement | null
    if (inp) inp.value = ''
    setFieldError(inp, null)
  })

  // R8: AI sermon-companion key (independent of the master integrations toggle —
  // it only upgrades the summary prose; chapters/highlights always run locally).
  // Stored keychain-only via the dedicated companion IPC, never in settings.
  try {
    const companionStatus = $('integration-companion-apikey-status')
    if (companionStatus) {
      const configured = await window.api.companionLlmConfigured()
      companionStatus.textContent = configured
        ? t('companion.keyStored', '✓ API-nøkkel lagret (nøkkelring)')
        : t('companion.keyNone', 'Ingen nøkkel — lokal oppsummering brukes')
    }
  } catch { /* leave status blank */ }

  $('btn-companion-apikey-save')?.addEventListener('click', async () => {
    const inp = $('integration-companion-apikey') as HTMLInputElement | null
    const statusEl = $('integration-companion-apikey-status')
    if (!inp) return
    const key = inp.value.trim()
    if (!key) {
      // Used to return silently — a click on Lagre that did nothing at all.
      setFieldError(inp, t('integrations.errKeyEmpty', 'Lim inn nøkkelen først'))
      return
    }
    setFieldError(inp, null)
    try {
      await window.api.companionSetLlmKey(key)
    } catch (err) {
      setFieldError(inp, `${t('integrations.errKeySaveFailed', 'Kunne ikke lagre nøkkelen')}: ${errText(err)}`)
      return
    }
    inp.value = ''
    if (statusEl) statusEl.textContent = t('companion.keyStored', '✓ API-nøkkel lagret (nøkkelring)')
    showSavedChip($('btn-companion-apikey-save')?.parentElement ?? null)
  })

  $('btn-companion-apikey-clear')?.addEventListener('click', async () => {
    const inp = $('integration-companion-apikey') as HTMLInputElement | null
    const statusEl = $('integration-companion-apikey-status')
    try {
      await window.api.companionClearLlmKey()
    } catch (err) {
      // «Ingen nøkkel» under er et LØFTE om at nøkkelen er borte — det får
      // ikke stå der hvis slettingen faktisk feilet.
      setFieldError(inp, `${t('integrations.errKeyClearFailed', 'Kunne ikke fjerne nøkkelen')}: ${errText(err)}`)
      return
    }
    setFieldError(inp, null)
    if (inp) inp.value = ''
    if (statusEl) statusEl.textContent = t('companion.keyNone', 'Ingen nøkkel — lokal oppsummering brukes')
  })

  // ── Connection: explicit save (church_id + the two API URLs) ──────────────
  const connectionCard = $('integrations-connection-card')
  $('btn-integrations-connection-save')?.addEventListener('click', async () => {
    const churchInput  = $('integration-church-id')    as HTMLInputElement | null
    const songUrlInput = $('integration-song-api-url') as HTMLInputElement | null
    const planUrlInput2 = $('integration-plan-api-url') as HTMLInputElement | null
    clearFieldErrors(connectionCard)

    const songProblem = urlProblem(songUrlInput?.value ?? '')
    if (songProblem) { setFieldError(songUrlInput, songProblem); return }
    const planProblem = urlProblem(planUrlInput2?.value ?? '')
    if (planProblem) { setFieldError(planUrlInput2, planProblem); return }
    const churchId = churchInput?.value.trim() ?? ''
    if (churchId && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(churchId)) {
      setFieldError(churchInput, t('integrations.errChurchId', 'Menighets-ID er en UUID fra SundaySong eller SundayPlan'))
      return
    }

    const patch = {
      connection: {
        ...(current.connection ?? {}),
        churchId:   churchId || undefined,
        songApiUrl: songUrlInput?.value.trim()   || undefined,
        planApiUrl: planUrlInput2?.value.trim()  || undefined,
      },
    }
    try {
      // «Lagret ✓» kommer ETTER lagringen — og `current` oppdateres fra det
      // backend faktisk slo sammen, ikke fra det vi håpet den ville lagre.
      current = (await window.api.setIntegrationSettings(patch)) as typeof current
    } catch (err) {
      setFieldError(churchInput, `${t('integrations.errConnectionSaveFailed', 'Kunne ikke lagre tilkoblingen')}: ${errText(err)}`)
      return
    }
    showSavedChip($('btn-integrations-connection-save')?.parentElement ?? null)
  })

  $('btn-integrations-connection-cancel')?.addEventListener('click', () => {
    clearFieldErrors(connectionCard)
    const churchInput  = $('integration-church-id')    as HTMLInputElement | null
    const songUrlInput = $('integration-song-api-url') as HTMLInputElement | null
    const planUrlInput = $('integration-plan-api-url') as HTMLInputElement | null
    const conn = (current.connection ?? {}) as { churchId?: string; songApiUrl?: string; planApiUrl?: string }
    if (churchInput) churchInput.value = conn.churchId   ?? ''
    if (songUrlInput) songUrlInput.value = conn.songApiUrl ?? ''
    if (planUrlInput) planUrlInput.value = conn.planApiUrl ?? ''
  })
}
