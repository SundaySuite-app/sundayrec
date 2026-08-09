import { settings, patchSettings } from '../state'
import { flashSaved, escHtml, localeTag } from '../helpers'
import { t } from '../i18n'
import { notifyLivePageDestinationsChanged } from './live-page'
import { closeModal, openModal } from '../ui/modal-manager'
import { alertDialog, confirmDialog } from '../ui/dialog'
import { setupThumbPanel, refresh as refreshThumbPanel, panelElementsByPrefix } from './thumbnail-panel'
import { bindRadioGroup, bindSetting, showSavedChip } from '../ui/bind-setting'
import { applyFeatureGate } from '../ui/feature-gate'
import { cloudGateStatus } from '../ui/feature-gate-core'
import type { CloudServiceId, CloudServiceSettings, CloudStatus, CloudQueueStatus, StreamDestinationStored } from '../../types'

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
  setupStreamDestinations()

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
    if (folderNameEl)  folderNameEl.textContent  = status.folderName ?? status.folderPath ?? 'Rotmappe'
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
  applyStreamSettingsToUI()
  const thumbEls = panelElementsByPrefix('publish')
  if (thumbEls) void refreshThumbPanel(thumbEls, { kind: 'default' })
}

// ─── Live-stream destinations ───────────────────────────────────────────
//
// A draft list of destinations the user is editing. It mirrors
// settings.streamDestinations until saveStreamDestinations() persists it.
// Each entry tracks the optional user-entered stream key (`pendingKey`) so we
// can push it to the encrypted store on save without ever placing it in the
// settings JSON.

const STREAM_MAX_DESTINATIONS = 5

interface DraftDestination extends StreamDestinationStored {
  /** New key the user has typed in the input. Empty = no change. */
  pendingKey?: string
  /** Existing destination id, or '' for a newly-added row that isn't saved yet. */
  draftOnly?: boolean
}

let draftDestinations: DraftDestination[] = []
/** Ids removed by the user during this editing session — keys are deleted on save. */
const removedDestIds = new Set<string>()

function setupStreamDestinations(): void {
  applyStreamSettingsToUI()
  document.getElementById('btn-add-stream-destination')?.addEventListener('click', () => {
    if (draftDestinations.length >= STREAM_MAX_DESTINATIONS) return
    draftDestinations.push({
      id:      `dest-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      name:    '',
      rtmpUrl: '',
      enabled: true,
      hasKey:  false,
      draftOnly: true,
    })
    renderStreamDestinations()
    markStreamDirty(true)
  })

  // EXPLICIT save — one of the three exceptions to auto-apply. A destination is
  // a URL + a stream key that only work as a set, and the key leaves the app for
  // the OS keychain on save; committing it a keystroke at a time would push a
  // dozen truncated keys into the keychain and leave the last one wrong.
  document.getElementById('btn-stream-dest-save')?.addEventListener('click', () => {
    void saveStreamDestinations()
  })
  document.getElementById('btn-stream-dest-cancel')?.addEventListener('click', () => {
    removedDestIds.clear()
    draftDestinations = cloneFromSettings()
    renderStreamDestinations()
    markStreamDirty(false)
  })

  // Quality + framerate are plain settings, not part of the destination set —
  // they auto-apply like everything else.
  bindRadioGroup('stream-resolution', {
    key: 'streamResolution',
    apply: (value) => patchSettings({ streamResolution: value as '480p' | '720p' | '1080p' }),
  })
  bindSetting('stream-framerate', {
    key: 'streamFramerate',
    apply: (value) => patchSettings({ streamFramerate: (Number(value) || 30) as 25 | 30 }),
  })
}

function applyStreamSettingsToUI(): void {
  draftDestinations = cloneFromSettings()
  removedDestIds.clear()
  renderStreamDestinations()
  markStreamDirty(false)
  // Quality + framerate radios
  const res = settings.streamResolution ?? '720p'
  const radio = document.querySelector<HTMLInputElement>(`input[name="stream-resolution"][value="${res}"]`)
  if (radio) radio.checked = true
  const fr = settings.streamFramerate ?? 30
  const sel = document.getElementById('stream-framerate') as HTMLSelectElement | null
  if (sel) sel.value = String(fr)
}

function cloneFromSettings(): DraftDestination[] {
  return (settings.streamDestinations ?? []).map(d => ({ ...d }))
}

function renderStreamDestinations(): void {
  const list = document.getElementById('stream-destinations-list')
  if (!list) return
  list.innerHTML = ''
  if (draftDestinations.length === 0) {
    const empty = document.createElement('div')
    empty.className = 'empty-state'
    empty.textContent = t('publish.streamNoneYet', 'Ingen destinasjoner enda.')
    list.appendChild(empty)
  }
  draftDestinations.forEach((d, idx) => {
    const row = document.createElement('div')
    row.className = 'stream-destination-row'
    row.dataset.idx = String(idx)
    const keyPlaceholder = d.hasKey
      ? t('publish.streamKeySaved', '•••••• Lagret')
      : t('publish.streamKeyPlaceholder', 'Lim inn stream-key her')
    row.innerHTML = `
      <div class="stream-destination-grid">
        <div>
          <label class="form-label" data-i18n="publish.streamName">NAVN</label>
          <input type="text" class="form-input" data-stream-field="name" value="${escHtml(d.name)}" placeholder="YouTube / Facebook / Kirkens server" />
        </div>
        <div>
          <label class="form-label" data-i18n="publish.streamRtmpUrl">RTMP-URL</label>
          <input type="text" class="form-input" data-stream-field="rtmpUrl" value="${escHtml(d.rtmpUrl)}" placeholder="rtmp://a.rtmp.youtube.com/live2" />
        </div>
        <div>
          <label class="form-label" data-i18n="publish.streamKey">STREAM-KEY</label>
          <input type="password" class="form-input" data-stream-field="streamKey" value="" placeholder="${escHtml(keyPlaceholder)}" />
        </div>
      </div>
      <div class="stream-destination-actions">
        <label class="toggle-row stream-destination-enabled">
          <span data-i18n="publish.enabled">Aktivert</span>
          <label class="toggle">
            <input type="checkbox" data-stream-field="enabled" ${d.enabled ? 'checked' : ''} />
            <span class="toggle-track"></span>
          </label>
        </label>
        <button class="btn-ghost btn-sm" type="button" data-stream-action="delete" aria-label="Slett">✕</button>
      </div>
    `

    row.querySelectorAll<HTMLInputElement>('input[data-stream-field]').forEach(inp => {
      inp.addEventListener('input', () => { updateDraftFromRow(idx, row); markStreamDirty(true) })
      inp.addEventListener('change', () => { updateDraftFromRow(idx, row); markStreamDirty(true) })
    })
    row.querySelector<HTMLElement>('[data-stream-action="delete"]')?.addEventListener('click', async () => {
      const ok = await confirmDialog({
        title:        t('publish.streamConfirmDelete', 'Slette denne destinasjonen?'),
        message:      draftDestinations[idx]?.name || undefined,
        confirmLabel: t('dialog.delete', 'Slett'),
        danger:       true,
      })
      if (!ok) return
      const removed = draftDestinations[idx]
      if (removed && !removed.draftOnly) removedDestIds.add(removed.id)
      draftDestinations.splice(idx, 1)
      renderStreamDestinations()
      markStreamDirty(true)
    })
    list.appendChild(row)
  })
}

function updateDraftFromRow(idx: number, row: HTMLElement): void {
  const d = draftDestinations[idx]
  if (!d) return
  const name    = (row.querySelector('input[data-stream-field="name"]')    as HTMLInputElement | null)?.value ?? ''
  const rtmpUrl = (row.querySelector('input[data-stream-field="rtmpUrl"]') as HTMLInputElement | null)?.value ?? ''
  const key     = (row.querySelector('input[data-stream-field="streamKey"]') as HTMLInputElement | null)?.value ?? ''
  const enabled = !!(row.querySelector('input[data-stream-field="enabled"]') as HTMLInputElement | null)?.checked
  d.name       = name
  d.rtmpUrl    = rtmpUrl
  d.enabled    = enabled
  d.pendingKey = key
}

/** Persist current draft to settings + encrypted key store. Called after the
 *  publish-tab's main save (which writes the other settings). */
async function saveStreamDestinations(): Promise<void> {
  // Validate: keep only rows that have a name + url. Skip silently otherwise
  // (the row stays in the DOM until the user explicitly removes it).
  const valid = draftDestinations.filter(d => d.name.trim() && d.rtmpUrl.trim())

  // Push keys before settings — that way `hasKey` reflects the saved state.
  for (const d of valid) {
    if (d.pendingKey && d.pendingKey.length > 0) {
      try {
        const r = await window.api.streamSetKey(d.id, d.pendingKey)
        if (r.ok) {
          d.hasKey = true
          d.pendingKey = ''
        } else if (r.error === 'safeStorage_unavailable') {
          // Refuse to silently lose the key — surface to user so they can
          // decide (use a different machine, or accept the risk on a personal box).
          await alertDialog({
            title:   t('dialog.streamKeyUnsafeTitle', 'Stream-nøkkelen ble ikke lagret'),
            message: t('dialog.streamKeyUnsafeBody', 'Systemets nøkkelring (Mac Keychain / Windows Credential Manager) er ikke tilgjengelig. Nøkkelen lagres derfor ikke, slik at den ikke havner som klartekst på disk. Logg inn som en bruker med nøkkelring og prøv igjen.'),
            tone:    'error',
          })
        } else {
          console.error('[publish] streamSetKey failed', d.id, r.error)
        }
      } catch (err) {
        console.error('[publish] streamSetKey failed for', d.id, err)
      }
    }
  }
  // Delete keys for removed destinations
  for (const id of removedDestIds) {
    try { await window.api.streamDeleteKey(id) } catch (err) { console.error('[publish] streamDeleteKey failed', id, err) }
  }
  removedDestIds.clear()

  const resolution = (document.querySelector('input[name="stream-resolution"]:checked') as HTMLInputElement | null)?.value as ('480p' | '720p' | '1080p' | undefined)
  const framerate  = parseInt((document.getElementById('stream-framerate') as HTMLSelectElement | null)?.value ?? '30', 10) as 25 | 30

  const stored: StreamDestinationStored[] = valid.map(d => ({
    id: d.id, name: d.name, rtmpUrl: d.rtmpUrl, enabled: d.enabled, hasKey: d.hasKey,
  }))

  patchSettings({
    streamDestinations: stored,
    streamResolution:   resolution ?? settings.streamResolution ?? '720p',
    streamFramerate:    framerate,
  })
  try { await window.api.saveSettings(settings) } catch (err) { console.error('[publish] saveSettings failed', err) }

  // Refresh draft so future edits start from the saved state (hasKey may have
  // flipped, pendingKey is cleared).
  draftDestinations = cloneFromSettings()
  renderStreamDestinations()
  markStreamDirty(false)
  showSavedChip(document.getElementById('btn-stream-dest-save')?.parentElement ?? null)

  notifyLivePageDestinationsChanged()
}

/**
 * Show/clear the "unsaved destinations" state on the explicit-save card. This
 * is the ONLY place in settings that still has a dirty state, and it is honest:
 * nothing has been written until «Lagre destinasjoner» is pressed.
 */
function markStreamDirty(dirty: boolean): void {
  const card = document.getElementById('stream-destinations-card')
  card?.classList.toggle('has-unsaved', dirty)
  const hint = document.getElementById('stream-dest-dirty-hint')
  if (hint) hint.style.display = dirty ? '' : 'none'
}
