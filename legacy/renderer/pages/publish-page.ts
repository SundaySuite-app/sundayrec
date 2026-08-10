import { settings, patchSettings } from '../state'
import { flashSaved, localeTag } from '../helpers'
import { t } from '../i18n'
import { closeModal, openModal } from '../ui/modal-manager'
import { setupThumbPanel, refresh as refreshThumbPanel, panelElementsByPrefix } from './thumbnail-panel'
import { showSavedChip } from '../ui/bind-setting'
import { applyFeatureGate } from '../ui/feature-gate'
import { cloudGateStatus } from '../ui/feature-gate-core'
import type { CloudServiceId, CloudServiceSettings, CloudStatus, CloudQueueStatus } from '../../types'

type ServiceStatus = Record<CloudServiceId, CloudStatus>

let currentStatus: ServiceStatus = {
  'google-drive': { connected: false },
  'dropbox':      { connected: false },
  'onedrive':     { connected: false },
}

const SERVICE_NAMES: Record<CloudServiceId, string> = {
  'google-drive': 'Google Drive',
  'dropbox':      'Dropbox',
  'onedrive':     'OneDrive',
}

const configured: Record<CloudServiceId, boolean> = {
  'google-drive': true,
  'dropbox':      true,
  'onedrive':     true,
}

/** The services that have a card in the DOM. Dropbox/OneDrive were removed
 *  2026-08 (no app key in any build → the hidden cards were dead DOM);
 *  extend this list together with the markup when a key exists. */
const VISIBLE_SERVICES: CloudServiceId[] = ['google-drive']

export function setupPublishPage(): void {
  refreshStatus()
  refreshConfigured()
  refreshQueue()

  // Default-thumbnail panel ("Standard episodebilde") — sits at the top of the
  // Deling tab's Publisering section. Gated «kommer» through v0.9.0 because
  // every thumbnail* shim method was a stub; live since Fase 6, so the drop
  // zone now stores the image in app data instead of swallowing it.
  const thumbEls = panelElementsByPrefix('publish')
  if (thumbEls) {
    setupThumbPanel(thumbEls, { kind: 'default' })
    void refreshThumbPanel(thumbEls, { kind: 'default' })
  }

  // Connect/disconnect buttons
  document.querySelectorAll<HTMLElement>('[data-cloud-connect]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const service = btn.dataset.cloudConnect as CloudServiceId
      if (!configured[service]) {
        showServiceError(service, `${SERVICE_NAMES[service]} ${t('publish.errNotConfigured', 'er ikke konfigurert i denne byggingen. Be utvikleren om en build med OAuth-nøkkel.')}`)
        return
      }
      btn.textContent = t('publish.connecting', 'Kobler til…')
      btn.setAttribute('disabled', '')

      // Allow the user to cancel a stuck OAuth flow
      const cancelBtn = ensureCancelButton(btn, service)
      cancelBtn.style.display = ''

      try {
        const result = await window.api.cloudConnect(service)
        if (result.ok) {
          refreshStatus()
        } else {
          showServiceError(service, result.error ?? 'Ukjent feil')
        }
      } finally {
        btn.removeAttribute('disabled')
        btn.textContent = 'Koble til'
        cancelBtn.style.display = 'none'
      }
    })
  })

  document.querySelectorAll<HTMLElement>('[data-cloud-disconnect]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const service = btn.dataset.cloudDisconnect as CloudServiceId
      await window.api.cloudDisconnect(service)
      refreshStatus()
    })
  })

  // Folder picker buttons
  document.querySelectorAll<HTMLElement>('[data-cloud-pick-folder]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const service = btn.dataset.cloudPickFolder as CloudServiceId
      await openFolderPicker(service)
    })
  })

  // Auto-upload toggles
  document.querySelectorAll<HTMLInputElement>('[data-cloud-auto]').forEach(chk => {
    chk.addEventListener('change', () => {
      const service = chk.dataset.cloudAuto as CloudServiceId
      saveServiceSettings(service, { autoUpload: chk.checked }, chk)
    })
  })

  // Enabled toggles
  document.querySelectorAll<HTMLInputElement>('[data-cloud-enabled]').forEach(chk => {
    chk.addEventListener('change', () => {
      const service = chk.dataset.cloudEnabled as CloudServiceId
      saveServiceSettings(service, { enabled: chk.checked }, chk)
    })
  })

  // Manual upload buttons in history (delegated)
  document.addEventListener('cloud-manual-upload', async (e: Event) => {
    const detail = (e as CustomEvent).detail as { service: CloudServiceId; filePath: string }
    await window.api.cloudUploadFile(detail.service, detail.filePath)
    flashSaved(null)
  })

  wireCloudIpcListeners()
}

// Lifetime `window.api.on` subscriptions — unsubscribes kept, wiring guarded so
// a re-run of setupPublishPage can never stack duplicate handlers.
let cloudIpcWired = false
const cloudIpcUnsubs: Array<(() => void) | undefined> = []
function wireCloudIpcListeners(): void {
  if (cloudIpcWired) return
  cloudIpcWired = true

  // Listen for upload progress/done from main
  cloudIpcUnsubs.push(window.api.on('cloud-upload-progress', (data: unknown) => {
    const { service, filename } = data as { service: CloudServiceId; filename: string }
    showUploadStatus(service, `Laster opp ${filename}…`, false)
  }))
  cloudIpcUnsubs.push(window.api.on('cloud-upload-done', (data: unknown) => {
    const { service, ok, error } = data as { service: CloudServiceId; ok: boolean; error?: string }
    showUploadStatus(service, ok ? '✓ Opplastet' : `✕ ${error ?? 'Feil'}`, !ok)
    refreshStatus()
  }))
  cloudIpcUnsubs.push(window.api.on('cloud-queue-update', (data: unknown) => {
    renderQueue(data as CloudQueueStatus)
  }))
}

async function refreshStatus(): Promise<void> {
  const status = await window.api.cloudStatus() as ServiceStatus
  currentStatus = status
  renderAllCards(status)
}

async function refreshConfigured(): Promise<void> {
  await Promise.all(VISIBLE_SERVICES.map(async s => {
    try {
      configured[s] = await window.api.cloudIsConfigured(s) as boolean
    } catch { configured[s] = true }
  }))
  // Re-render so unconfigured cards show a notice
  renderAllCards(currentStatus)

  // HONEST GATE. `cloud_is_configured` is a real backend predicate: it answers
  // whether this build carries a Google OAuth client id at all. When it does
  // not, «Koble til» cannot work for anyone — so say that once, at the top of
  // the section, and turn the buttons off, instead of letting the user press a
  // button that opens nothing and reports an error they cannot act on.
  // Only the services with a card on screen count — the `configured` map's
  // optimistic defaults for card-less services must not mask a build where
  // Google Drive (the one visible card) has no client id.
  const anyConfigured = VISIBLE_SERVICES.some(s => configured[s])
  applyFeatureGate('cloud-backup-card', {
    status: cloudGateStatus(anyConfigured),
    chipText: t('gate.chipUnconfigured', 'Ikke konfigurert'),
    explanation: t(
      'publish.gateCloudExplain',
      'Sky-backup er ikke konfigurert i denne bygningen — den mangler Google-nøkkelen som trengs for å koble til en konto.',
    ),
    docsHint: t('publish.gateCloudHint', 'Be om en build med sky-backup slått på hvis menigheten trenger dette.'),
  })
}

async function refreshQueue(): Promise<void> {
  try {
    const q = await window.api.cloudQueueStatus() as CloudQueueStatus
    renderQueue(q)
  } catch (err) {
    console.error('[publish] refreshQueue failed:', err)
  }
}

function renderAllCards(status: ServiceStatus): void {
  for (const id of VISIBLE_SERVICES) {
    renderCard(id, status[id])
  }
}

function renderCard(service: CloudServiceId, status: CloudStatus): void {
  const card = document.getElementById(`cloud-card-${service}`)
  if (!card) return

  const connectedSection   = card.querySelector<HTMLElement>('.cloud-connected')
  const disconnectedSection = card.querySelector<HTMLElement>('.cloud-disconnected')
  const accountNameEl      = card.querySelector<HTMLElement>('.cloud-account-name')
  const folderNameEl       = card.querySelector<HTMLElement>('.cloud-folder-name')
  const lastUploadEl       = card.querySelector<HTMLElement>('.cloud-last-upload')
  const autoChk            = card.querySelector<HTMLInputElement>('[data-cloud-auto]')
  const enabledChk         = card.querySelector<HTMLInputElement>('[data-cloud-enabled]')

  if (status.connected) {
    connectedSection?.style.setProperty('display', '')
    disconnectedSection?.style.setProperty('display', 'none')
    if (accountNameEl) accountNameEl.textContent = status.accountName ?? ''
    if (folderNameEl)  folderNameEl.textContent  = status.folderName ?? status.folderPath ?? t('publish.rootFolder', 'Rotmappe')
    if (lastUploadEl) {
      lastUploadEl.textContent = status.lastUpload
        ? (status.lastUploadOk ? '✓ ' : '✕ ') + new Date(status.lastUpload).toLocaleString(localeTag())
        : '—'
    }
    renderReauthBanner(card, service, status.needsReauth === true)
  } else {
    connectedSection?.style.setProperty('display', 'none')
    disconnectedSection?.style.setProperty('display', '')
    renderReauthBanner(card, service, false)
  }

  renderConfiguredNotice(card, service, configured[service])

  const settingsKey = service === 'google-drive' ? 'cloudGoogleDrive'
                    : service === 'dropbox'       ? 'cloudDropbox'
                    :                               'cloudOneDrive'
  const cfg = settings[settingsKey]
  if (autoChk)    autoChk.checked    = cfg?.autoUpload ?? false
  if (enabledChk) enabledChk.checked = cfg?.enabled    ?? false
}

/** Inject (or remove) a "reconnect needed" banner inside a service card. */
function renderReauthBanner(card: HTMLElement, service: CloudServiceId, needs: boolean): void {
  let banner = card.querySelector<HTMLElement>('.cloud-reauth-banner')
  if (!needs) { banner?.remove(); return }
  if (!banner) {
    banner = document.createElement('div')
    banner.className = 'cloud-reauth-banner'
    const text = document.createElement('div')
    text.textContent = `${SERVICE_NAMES[service]} ${t('publish.needsReauth', 'trenger pålogging på nytt. Klikk for å koble til.')}`
    const btn = document.createElement('button')
    const reauthLabel = t('publish.reauth', 'Koble til på nytt')
    btn.textContent = reauthLabel
    btn.className = 'cloud-reauth-btn'
    btn.addEventListener('click', async () => {
      btn.disabled = true
      btn.textContent = t('publish.connecting', 'Kobler til…')
      try {
        const result = await window.api.cloudConnect(service)
        if (result.ok) refreshStatus()
        else { btn.disabled = false; btn.textContent = reauthLabel; showServiceError(service, result.error ?? t('publish.errUnknown', 'Ukjent feil')) }
      } catch {
        btn.disabled = false; btn.textContent = reauthLabel
      }
    })
    banner.append(text, btn)
    card.prepend(banner)
  }
}

function renderConfiguredNotice(card: HTMLElement, service: CloudServiceId, ok: boolean): void {
  let notice = card.querySelector<HTMLElement>('.cloud-not-configured')
  if (ok) { notice?.remove(); return }
  if (!notice) {
    notice = document.createElement('div')
    notice.className = 'cloud-not-configured cloud-not-configured-notice'
    notice.textContent = `${SERVICE_NAMES[service]}${t('publish.oauthKeyMissing', '-OAuth-nøkkel er ikke satt i denne byggingen.')}`
    card.prepend(notice)
  }
}

/**
 * Inject an inline "Avbryt" button next to the connect button so the user can
 * back out of a hung OAuth flow (browser closed, no callback fired).
 */
function ensureCancelButton(connectBtn: HTMLElement, service: CloudServiceId): HTMLElement {
  let cancel = connectBtn.parentElement?.querySelector<HTMLElement>(`[data-cloud-cancel="${service}"]`)
  if (cancel) return cancel
  cancel = document.createElement('button')
  cancel.dataset.cloudCancel = service
  // btn-ghost = transparent bg + subtle border, fits the dark theme much
  // better than the unstyled .btn (which renders as a bright white block
  // and visually competes with the active connect button above it).
  // cloud-card-cancel narrows + centers it so it reads as a secondary action.
  cancel.className = 'btn-ghost btn-sm cloud-card-cancel'
  cancel.textContent = 'Avbryt'
  cancel.style.display = 'none'
  cancel.addEventListener('click', async () => {
    await window.api.cloudCancelConnect(service)
  })
  connectBtn.parentElement?.appendChild(cancel)
  return cancel
}

function renderQueue(q: CloudQueueStatus): void {
  let panel = document.getElementById('cloud-queue-panel')
  if (!panel) {
    panel = document.createElement('div')
    panel.id = 'cloud-queue-panel'
    panel.className = 'cloud-queue-panel'
    // Anchor lives inside Settings → Filer (sky-backup-kortet). Fallback til body.
    const cloudSection = document.querySelector('#cloud-queue-anchor') ?? document.body
    cloudSection.appendChild(panel)
  }

  if (q.entries.length === 0) {
    panel.innerHTML = `<div class="empty-state">${t('publish.queueEmpty', 'Ingen ventende skyopplastinger.')}</div>`
    return
  }

  panel.innerHTML = `<h3 class="cloud-queue-title">${t('publish.queueTitle', 'Skyopplastinger i kø')}</h3>`
  const list = document.createElement('div')
  list.className = 'cloud-queue-list'

  for (const e of q.entries) {
    const row = document.createElement('div')
    row.className = 'cloud-queue-row'

    const statusBadge = document.createElement('span')
    statusBadge.className = `cloud-queue-badge cloud-queue-badge-${e.status}`
    statusBadge.textContent = labelForStatus(e.status)
    row.appendChild(statusBadge)

    const meta = document.createElement('div')
    meta.className = 'cloud-queue-meta'
    const line1 = document.createElement('div')
    line1.textContent = `${SERVICE_NAMES[e.service]} — ${e.filename}`
    line1.className = 'cloud-queue-line1'
    const line2 = document.createElement('div')
    line2.className = 'cloud-queue-line2'
    const nextStr = e.nextAttempt > Date.now()
      ? `${t('publish.queueNextAttempt', 'Neste forsøk')}: ${new Date(e.nextAttempt).toLocaleTimeString(localeTag())}`
      : ''
    line2.textContent = [
      `${t('publish.queueAttempts', 'Forsøk')}: ${e.attempts}`,
      nextStr,
      e.lastError ? `${t('publish.queueError', 'Feil')}: ${e.lastError}` : '',
    ].filter(Boolean).join(' · ')
    meta.append(line1, line2)
    row.appendChild(meta)

    const retryBtn = document.createElement('button')
    retryBtn.textContent = t('publish.queueRetry', 'Prøv nå')
    retryBtn.className = 'btn-secondary btn-sm cloud-queue-retry'
    retryBtn.addEventListener('click', async () => {
      await window.api.cloudQueueRetry(e.id)
      refreshQueue()
    })
    row.appendChild(retryBtn)

    const removeBtn = document.createElement('button')
    removeBtn.textContent = t('publish.queueRemove', 'Fjern')
    removeBtn.className = 'btn-ghost btn-sm cloud-queue-remove'
    removeBtn.addEventListener('click', async () => {
      await window.api.cloudQueueRemove(e.id)
      refreshQueue()
    })
    row.appendChild(removeBtn)

    list.appendChild(row)
  }
  panel.appendChild(list)
}

function labelForStatus(s: CloudQueueStatus['entries'][number]['status']): string {
  switch (s) {
    case 'uploading':       return t('publish.queueStatusUploading', 'Laster opp')
    case 'failed':          return t('publish.queueStatusFailed',    'Mislyktes')
    case 'reauth-required': return t('publish.queueStatusReauth',    'Logg inn')
    default:                return t('publish.queueStatusPending',   'Venter')
  }
}

async function openFolderPicker(service: CloudServiceId): Promise<void> {
  const list  = document.getElementById('cloud-folder-list')
  const title = document.getElementById('cloud-folder-modal-title')
  if (!list || !title) return

  title.textContent = `${t('publish.pickFolderTitle', 'Velg mappe')} — ${SERVICE_NAMES[service]}`
  list.innerHTML = `<div class="cloud-folder-loading">${t('publish.loading', 'Laster…')}</div>`
  openModal('cloud-folder-modal')

  try {
    const folders = await window.api.cloudListFolders(service)
    list.innerHTML = ''

    const rootItem = document.createElement('button')
    rootItem.className = 'cloud-folder-item'
    rootItem.textContent = '📁 Rotmappe'
    rootItem.onclick = async () => {
      await window.api.cloudSetFolder(service, '', 'Rotmappe', '')
      closeModal('cloud-folder-modal')
      refreshStatus()
    }
    list.appendChild(rootItem)

    for (const f of folders) {
      const item = document.createElement('button')
      item.className = 'cloud-folder-item'
      item.textContent = `📁 ${f.name}`
      item.onclick = async () => {
        await window.api.cloudSetFolder(service, f.id, f.name, f.path)
        closeModal('cloud-folder-modal')
        refreshStatus()
      }
      list.appendChild(item)
    }
  } catch (err) {
    list.innerHTML = `<div style="padding:16px;color:var(--red)">Feil: ${(err as Error).message}</div>`
  }
}

document.getElementById('cloud-folder-modal-close')?.addEventListener('click', () => {
  closeModal('cloud-folder-modal')
})

function saveServiceSettings(
  service: CloudServiceId,
  patch: Partial<CloudServiceSettings>,
  chipFor?: HTMLElement | null,
): void {
  const key = service === 'google-drive' ? 'cloudGoogleDrive'
            : service === 'dropbox'       ? 'cloudDropbox'
            :                               'cloudOneDrive'
  const existing = settings[key] ?? { enabled: false, autoUpload: false }
  patchSettings({ [key]: { ...existing, ...patch } })
  window.api.saveSettings(settings).then(
    () => showSavedChip(chipFor?.closest<HTMLElement>('.cloud-toggle-row') ?? null),
    console.error,
  )
}

function showServiceError(service: CloudServiceId, message: string): void {
  const card = document.getElementById(`cloud-card-${service}`)
  if (!card) return
  let errEl = card.querySelector<HTMLElement>('.cloud-error')
  if (!errEl) {
    errEl = document.createElement('div')
    errEl.className = 'cloud-error'
    card.appendChild(errEl)
  }
  errEl.textContent = message
  errEl.style.display = ''
  setTimeout(() => { if (errEl) errEl.style.display = 'none' }, 5000)
}

function showUploadStatus(service: CloudServiceId, message: string, isError: boolean): void {
  const card = document.getElementById(`cloud-card-${service}`)
  const el   = card?.querySelector<HTMLElement>('.cloud-last-upload')
  if (el) {
    el.textContent = message
    el.style.color = isError ? 'var(--red)' : 'var(--green)'
    setTimeout(() => { el.style.color = '' }, 4000)
  }
}

export function applyPublishSettingsToUI(): void {
  refreshStatus()
  refreshQueue()
  const thumbEls = panelElementsByPrefix('publish')
  if (thumbEls) void refreshThumbPanel(thumbEls, { kind: 'default' })
}
