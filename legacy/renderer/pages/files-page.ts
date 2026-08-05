import { settings, patchSettings, saveSettingsDebounced } from '../state'
import type { FileFormat, FilenamePattern, PodcastSettings } from '../../types'
import { setVal, setRadio, isoDate } from '../helpers'
import { t } from '../i18n'
import { getChurchHolidays } from '../../shared/church-calendar'
import { loadHomeInfoStrip, refreshHomeDiskSpace } from './home'
import {
  bindRadioGroup,
  bindSetting,
  resyncBoundSettings,
  showSavedChip,
  type BindSettingOpts,
  type GuardDescriptor,
} from '../ui/bind-setting'

/** Deleting recordings on a timer is the one files-setting that destroys data,
 *  so a retention shorter than a month asks first — wherever it is set from. */
function autoDeleteGuard(days: number): GuardDescriptor | null {
  if (!(days > 0 && days < 30)) return null
  return {
    title: t('dialog.autoDeleteTitle', 'Slette opptak automatisk?'),
    message: t(
      'files.confirmAutoDeleteShort',
      'Opptak eldre enn {n} dager slettes automatisk og kan ikke gjenopprettes.',
    ).replace('{n}', String(days)),
    confirmLabel: t('dialog.autoDeleteConfirm', 'Ja, slett automatisk'),
  }
}

function currentAutoDeleteDays(): number {
  const el = document.getElementById('auto-delete-days') as HTMLInputElement | null
  return +(el?.value ?? '') || 90
}

/** Every files/podcast control writes the same way. */
function filesBinding(extra: Partial<BindSettingOpts> = {}): BindSettingOpts {
  return {
    apply: () => collectFilesSettings(),
    after: () => afterFilesSave(),
    ...extra,
  }
}

export function setupFilesPage(): void {
  // AUTO-APPLY with a visible receipt. Before this the tab had a dirty footer
  // whose «Lagre» was needed for the podcast fields but NOT for the recorder
  // ones (those already auto-saved) — and «Avbryt» could not revert either.
  document.getElementById('btn-pick-folder')?.addEventListener('click', async () => {
    const folder = await window.api.pickFolder()
    if (!folder) return
    setVal('save-folder', folder)
    patchSettings({ saveFolder: folder })
    const ok = await saveSettingsDebounced(120)
    if (ok) showSavedChip(document.querySelector<HTMLElement>('#settings-files .form-label'))
    resyncBoundSettings()
    afterFilesSave()
  })

  bindSetting('pattern-select', filesBinding({
    key: 'filenamePattern',
    after: () => { updateFilenamePreview(); afterFilesSave() },
  }))
  bindRadioGroup('format', filesBinding({
    key: 'format',
    after: () => { toggleMp3Quality(); updateFilenamePreview(); afterFilesSave() },
  }))
  bindRadioGroup('bitrate', filesBinding({ key: 'bitrate' }))

  bindSetting('opt-auto-delete', filesBinding({
    key: 'autoDeleteDays',
    confirmIf: (value) => (value === true ? autoDeleteGuard(currentAutoDeleteDays()) : null),
    after: (value) => {
      const row = document.getElementById('auto-delete-days-row')
      if (row) row.style.display = value ? 'block' : 'none'
      afterFilesSave()
    },
    revert: (previous) => {
      const el = document.getElementById('opt-auto-delete') as HTMLInputElement | null
      if (el) el.checked = previous === true
      const row = document.getElementById('auto-delete-days-row')
      if (row) row.style.display = previous ? 'block' : 'none'
    },
  }))
  bindSetting('auto-delete-days', filesBinding({
    key: 'autoDeleteDays',
    confirmIf: (value) => autoDeleteGuard(typeof value === 'number' ? value : 0),
  }))
  bindSetting('opt-trim-silence', filesBinding({ key: 'trimSilence' }))

  // Opptaksoppførsel — the silence toggle reveals its threshold/timeout config.
  bindSetting('opt-silence', filesBinding({
    key: 'stopOnSilence',
    after: (value) => {
      const silCfg = document.getElementById('silence-config')
      if (silCfg) silCfg.style.display = value ? 'block' : 'none'
      afterFilesSave()
    },
  }))
  bindSetting('opt-protect', filesBinding({ key: 'protectRecording' }))
  ;['opt-silence-threshold', 'opt-silence-timeout', 'opt-split-minutes', 'opt-manual-max', 'opt-preroll-seconds']
    .forEach(id => bindSetting(id, filesBinding({ key: id })))

  // ── Podcast (now a section of Deling) ──────────────────────────────────────
  bindSetting('opt-podcast-enabled', filesBinding({
    key: 'podcast.enabled',
    after: (value) => {
      const cfg = document.getElementById('podcast-config')
      if (cfg) cfg.style.display = value ? 'flex' : 'none'
      afterFilesSave()
    },
  }))
  ;[
    'podcast-title','podcast-author','podcast-description','podcast-language',
    'podcast-category','podcast-email','podcast-link','podcast-image','podcast-service',
    'podcast-default-master-preset','opt-podcast-auto-prep',
  ].forEach(id => bindSetting(id, filesBinding({ key: 'podcast.' + id })))

  // Prep-and-review intro/outro file pickers. The text field is read-only, so
  // its value only ever changes here — commit explicitly after the picker.
  const commitJingle = async (inputId: string): Promise<void> => {
    collectFilesSettings()
    const ok = await saveSettingsDebounced(120)
    if (ok) showSavedChip(document.getElementById(inputId)?.closest<HTMLElement>('div') ?? null)
    resyncBoundSettings()
  }
  const jingle = (btnId: string, inputId: string, pick: boolean): void => {
    document.getElementById(btnId)?.addEventListener('click', async () => {
      const inp = document.getElementById(inputId) as HTMLInputElement | null
      if (!inp) return
      if (pick) {
        const fp = await window.api.pickAudioFile()
        if (!fp) return
        inp.value = fp
      } else {
        inp.value = ''
      }
      await commitJingle(inputId)
    })
  }
  jingle('btn-podcast-pick-intro',  'podcast-default-intro', true)
  jingle('btn-podcast-clear-intro', 'podcast-default-intro', false)
  jingle('btn-podcast-pick-outro',  'podcast-default-outro', true)
  jingle('btn-podcast-clear-outro', 'podcast-default-outro', false)

  document.getElementById('btn-podcast-copy-url')?.addEventListener('click', () => {
    const inp = document.getElementById('podcast-feed-url') as HTMLInputElement | null
    if (inp?.value) {
      navigator.clipboard.writeText(inp.value).catch(() => {})
      const status = document.getElementById('podcast-status')
      if (status) {
        status.textContent = '✓ Kopiert'
        setTimeout(() => { if (status) status.textContent = '' }, 2000)
      }
    }
  })

  document.getElementById('btn-podcast-regenerate')?.addEventListener('click', async () => {
    const btn    = document.getElementById('btn-podcast-regenerate') as HTMLButtonElement | null
    const status = document.getElementById('podcast-status')
    if (!btn || !status) return
    // Flush the latest config first so the feed is generated from what is on
    // screen (auto-apply may still be inside its debounce window).
    collectFilesSettings()
    await window.api.saveSettings(settings)
    btn.disabled = true
    status.textContent = 'Genererer…'
    try {
      const service = (document.getElementById('podcast-service') as HTMLSelectElement | null)?.value ?? 'google-drive'
      const result  = await window.api.podcastRegenerate(service)
      if (result.ok) {
        const count = result.episodeCount
        const epWord = count === 1
          ? t('publish.episodeSingular', 'episode')
          : t('publish.episodePlural', 'episoder')
        status.textContent = `✓ ${count} ${epWord} ${t('publish.published', 'publisert')}`
        if (result.feedUrl) {
          settings.podcast = { ...(settings.podcast ?? {} as PodcastSettings), feedUrl: result.feedUrl }
          showFeedUrl(result.feedUrl)
        }
      } else {
        const reason = result.error === 'not_connected'    ? t('publish.errConnectFirst',    'koble til skytjenesten først')
                     : result.error === 'no_save_folder'   ? t('publish.errPickFolderFirst', 'velg lagringsmappe først')
                     : result.error === 'podcast_disabled' ? t('publish.errEnablePodcast',   'aktiver podcast først')
                     : result.error ?? t('publish.errUnknown', 'ukjent feil')
        status.textContent = `✕ ${reason}`
      }
    } catch (err) {
      status.textContent = `✕ ${(err as Error).message}`
    } finally {
      btn.disabled = false
    }
  })
}

function showFeedUrl(url: string): void {
  const row = document.getElementById('podcast-feed-url-row')
  const inp = document.getElementById('podcast-feed-url') as HTMLInputElement | null
  if (row && inp) {
    inp.value = url
    row.style.display = ''
  }
}

export function applyFilesSettingsToUI(): void {
  setVal('save-folder', settings.saveFolder ?? '')
  const patternEl = document.getElementById('pattern-select') as HTMLSelectElement | null
  if (patternEl) patternEl.value = settings.filenamePattern ?? 'date'
  setRadio('format',  settings.format          ?? 'mp3')
  setRadio('bitrate', String(settings.bitrate  ?? '256'))
  const autoDelEl = document.getElementById('opt-auto-delete') as HTMLInputElement | null
  if (autoDelEl) {
    autoDelEl.checked = !!settings.autoDeleteDays
    const daysEl = document.getElementById('auto-delete-days') as HTMLInputElement | null
    const rowEl  = document.getElementById('auto-delete-days-row')
    if (daysEl) daysEl.value = String(settings.autoDeleteDays || 90)
    if (rowEl)  rowEl.style.display = settings.autoDeleteDays ? 'block' : 'none'
  }
  const trimEl = document.getElementById('opt-trim-silence') as HTMLInputElement | null
  if (trimEl) trimEl.checked = !!settings.trimSilence

  // Opptaksoppførsel (moved here from Schedule → Avanserte valg)
  const protectEl     = document.getElementById('opt-protect')           as HTMLInputElement  | null
  const silenceEl     = document.getElementById('opt-silence')           as HTMLInputElement  | null
  const silThreshSel  = document.getElementById('opt-silence-threshold') as HTMLSelectElement | null
  const silTimeoutSel = document.getElementById('opt-silence-timeout')   as HTMLSelectElement | null
  const splitMinSel   = document.getElementById('opt-split-minutes')     as HTMLSelectElement | null
  const manualMaxSel  = document.getElementById('opt-manual-max')        as HTMLSelectElement | null
  const prerollSel    = document.getElementById('opt-preroll-seconds')   as HTMLSelectElement | null
  if (protectEl)     protectEl.checked   = settings.protectRecording !== false
  if (silenceEl) {
    silenceEl.checked = !!settings.stopOnSilence
    const silCfg = document.getElementById('silence-config')
    if (silCfg) silCfg.style.display = settings.stopOnSilence ? 'block' : 'none'
  }
  if (silThreshSel)  silThreshSel.value  = String(settings.silenceThreshold      ?? -50)
  if (silTimeoutSel) silTimeoutSel.value = String(settings.silenceTimeoutMinutes ?? 5)
  if (splitMinSel)   splitMinSel.value   = String(settings.splitMinutes          ?? 0)
  if (manualMaxSel)  manualMaxSel.value  = String(settings.manualMaxMinutes      ?? 0)
  if (prerollSel)    prerollSel.value    = String(settings.preRollSeconds        ?? 0)

  // Podcast
  const p = settings.podcast
  const enabledEl = document.getElementById('opt-podcast-enabled') as HTMLInputElement | null
  const cfgEl     = document.getElementById('podcast-config')
  if (enabledEl) enabledEl.checked = !!p?.enabled
  if (cfgEl)     cfgEl.style.display = p?.enabled ? 'flex' : 'none'
  setVal('podcast-title',       p?.title       ?? '')
  setVal('podcast-author',      p?.author      ?? '')
  setVal('podcast-description', p?.description ?? '')
  setVal('podcast-email',       p?.email       ?? '')
  setVal('podcast-link',        p?.link        ?? '')
  setVal('podcast-image',       p?.imageUrl    ?? '')
  const langEl = document.getElementById('podcast-language') as HTMLSelectElement | null
  if (langEl) langEl.value = p?.language ?? 'no'
  const catEl  = document.getElementById('podcast-category') as HTMLSelectElement | null
  if (catEl)   catEl.value = p?.category ?? 'Religion & Spirituality'
  const svcEl  = document.getElementById('podcast-service') as HTMLSelectElement | null
  if (svcEl)   svcEl.value = p?.service ?? 'google-drive'
  if (p?.feedUrl) showFeedUrl(p.feedUrl)

  // Prep-and-review extras (v5.0)
  const autoPrepEl = document.getElementById('opt-podcast-auto-prep') as HTMLInputElement | null
  if (autoPrepEl) autoPrepEl.checked = (p as { autoPrepEnabled?: boolean } | undefined)?.autoPrepEnabled !== false
  const presetEl = document.getElementById('podcast-default-master-preset') as HTMLSelectElement | null
  if (presetEl) presetEl.value = (p as { defaultMasterPreset?: string } | undefined)?.defaultMasterPreset ?? 'speech-clear'
  const introEl = document.getElementById('podcast-default-intro') as HTMLInputElement | null
  if (introEl) introEl.value = (p as { defaultIntroPath?: string } | undefined)?.defaultIntroPath ?? ''
  const outroEl = document.getElementById('podcast-default-outro') as HTMLInputElement | null
  if (outroEl) outroEl.value = (p as { defaultOutroPath?: string } | undefined)?.defaultOutroPath ?? ''

  toggleMp3Quality()
  updateFilenamePreview()
  // The DOM now mirrors settings — rebase the bindings' baselines.
  resyncBoundSettings()
}

export function toggleMp3Quality(): void {
  const fmt     = (document.querySelector('input[name="format"]:checked') as HTMLInputElement | null)?.value
  const mp3Sect = document.getElementById('mp3-quality-section')
  if (mp3Sect) mp3Sect.style.display = fmt === 'mp3' || fmt === 'aac' ? 'block' : 'none'
}

export function updateFilenamePreview(): void {
  const pattern = (document.getElementById('pattern-select') as HTMLSelectElement | null)?.value ?? 'date'
  const format  = (document.querySelector('input[name="format"]:checked')  as HTMLInputElement | null)?.value ?? 'mp3'
  const today   = new Date()
  const ds      = isoDate(today)
  let name: string
  if (pattern === 'church') {
    const names = getChurchHolidays(today.getFullYear())[ds]
    const hname = names && names.length ? names[0] : ''
    name = hname ? `${hname.replace(/\s/g, '_')}_${ds}` : `Gudstjeneste_${ds}`
  } else if (pattern === 'plain') {
    name = `Gudstjeneste_${ds}`
  } else if (pattern === 'datetime') {
    name = `${ds}_${today.toTimeString().slice(0, 5).replace(':', '-')}`
  } else {
    name = ds
  }
  const prev = document.getElementById('filename-preview')
  if (prev) prev.textContent = `${name}.${format}`
}

/** Refresh Home live: the format/device info-strip and the disk-hours estimate
 *  (which depends on format/bitrate), so the change shows without navigating
 *  away and back. */
function afterFilesSave(): void {
  void loadHomeInfoStrip()
  void refreshHomeDiskSpace()
}

/**
 * Read the Opptak tab + the podcast section into `settings`. Persistence
 * belongs to `bindSetting`; the confirmation for a short auto-delete retention
 * is a guard on the two controls that can set it (see `autoDeleteGuard`), not a
 * surprise inside the save.
 */
function collectFilesSettings(): void {
  const autoDelEl   = document.getElementById('opt-auto-delete') as HTMLInputElement | null
  const autoDelDays = document.getElementById('auto-delete-days') as HTMLInputElement | null
  const days = autoDelEl?.checked ? (+(autoDelDays?.value ?? '') || 90) : 0

  const podcastEnabled = !!(document.getElementById('opt-podcast-enabled') as HTMLInputElement | null)?.checked
  const podcast: PodcastSettings = {
    enabled:     podcastEnabled,
    service:     ((document.getElementById('podcast-service') as HTMLSelectElement | null)?.value ?? 'google-drive') as PodcastSettings['service'],
    title:       (document.getElementById('podcast-title')       as HTMLInputElement | null)?.value.trim() ?? '',
    author:      (document.getElementById('podcast-author')      as HTMLInputElement | null)?.value.trim() ?? '',
    description: (document.getElementById('podcast-description') as HTMLTextAreaElement | null)?.value.trim() ?? '',
    language:    (document.getElementById('podcast-language')    as HTMLSelectElement | null)?.value ?? 'no',
    category:    (document.getElementById('podcast-category')    as HTMLSelectElement | null)?.value ?? 'Religion & Spirituality',
    explicit:    false,
    email:       (document.getElementById('podcast-email')       as HTMLInputElement | null)?.value.trim() || undefined,
    link:        (document.getElementById('podcast-link')        as HTMLInputElement | null)?.value.trim() || undefined,
    imageUrl:    (document.getElementById('podcast-image')       as HTMLInputElement | null)?.value.trim() || undefined,
    feedUrl:     settings.podcast?.feedUrl,  // preserve last published URL across saves
    // Prep-and-review (v5.0) extras
    autoPrepEnabled:     (document.getElementById('opt-podcast-auto-prep') as HTMLInputElement | null)?.checked !== false,
    defaultMasterPreset: (document.getElementById('podcast-default-master-preset') as HTMLSelectElement | null)?.value || 'speech-clear',
    defaultIntroPath:    (document.getElementById('podcast-default-intro') as HTMLInputElement | null)?.value.trim() || undefined,
    defaultOutroPath:    (document.getElementById('podcast-default-outro') as HTMLInputElement | null)?.value.trim() || undefined,
  }

  const protectEl     = document.getElementById('opt-protect')           as HTMLInputElement  | null
  const silenceEl     = document.getElementById('opt-silence')           as HTMLInputElement  | null
  const silThreshSel  = document.getElementById('opt-silence-threshold') as HTMLSelectElement | null
  const silTimeoutSel = document.getElementById('opt-silence-timeout')   as HTMLSelectElement | null
  const splitMinSel   = document.getElementById('opt-split-minutes')     as HTMLSelectElement | null
  const manualMaxSel  = document.getElementById('opt-manual-max')        as HTMLSelectElement | null
  const prerollSel    = document.getElementById('opt-preroll-seconds')   as HTMLSelectElement | null

  patchSettings({
    saveFolder:      (document.getElementById('save-folder') as HTMLInputElement | null)?.value ?? '',
    filenamePattern: ((document.getElementById('pattern-select') as HTMLSelectElement | null)?.value ?? 'date') as FilenamePattern,
    format:          ((document.querySelector('input[name="format"]:checked')  as HTMLInputElement | null)?.value ?? 'mp3') as FileFormat,
    bitrate:         (document.querySelector('input[name="bitrate"]:checked') as HTMLInputElement | null)?.value ?? '256',
    autoDeleteDays:  days,
    trimSilence:     !!(document.getElementById('opt-trim-silence') as HTMLInputElement | null)?.checked,
    protectRecording:      protectEl?.checked ?? true,
    stopOnSilence:         silenceEl?.checked ?? false,
    silenceThreshold:      parseInt(silThreshSel?.value  ?? '-50') || -50,
    silenceTimeoutMinutes: parseInt(silTimeoutSel?.value ?? '5')   || 5,
    splitMinutes:          parseInt(splitMinSel?.value   ?? '0')   || 0,
    manualMaxMinutes:      parseInt(manualMaxSel?.value  ?? '0')   || 0,
    preRollSeconds:        parseInt(prerollSel?.value    ?? '0')   || 0,
    podcast,
  })
}
