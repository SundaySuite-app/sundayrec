import { settings, patchSettings, saveSettingsDebounced } from '../state'
import type { FileFormat, FilenamePattern } from '../../types'
import { setVal, setRadio, isoDate } from '../helpers'
import { t, tn } from '../i18n'
import { getChurchHolidays } from '../../shared/church-calendar'
import { loadHomeInfoStrip, refreshHomeDiskSpace } from './home'
import { reconcilePreroll } from '../preroll-lifecycle'
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
    message: tn(
      'files.confirmAutoDeleteShort',
      days,
      {},
      'Opptak eldre enn {n} dager slettes automatisk og kan ikke gjenopprettes.',
    ),
    confirmLabel: t('dialog.autoDeleteConfirm', 'Ja, slett automatisk'),
  }
}

function currentAutoDeleteDays(): number {
  const el = document.getElementById('auto-delete-days') as HTMLInputElement | null
  return +(el?.value ?? '') || 90
}

/** Every files control writes the same way. */
function filesBinding(extra: Partial<BindSettingOpts> = {}): BindSettingOpts {
  return {
    apply: () => collectFilesSettings(),
    after: () => afterFilesSave(),
    ...extra,
  }
}

export function setupFilesPage(): void {
  // AUTO-APPLY with a visible receipt. Before this the tab had a dirty footer
  // whose «Lagre» was needed for the (since-removed) podcast fields but NOT
  // for the recorder ones (those already auto-saved) — and «Avbryt» could not
  // revert either.
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
  ;['opt-silence-threshold', 'opt-silence-timeout', 'opt-split-minutes', 'opt-manual-max']
    .forEach(id => bindSetting(id, filesBinding({ key: id })))

  // Pre-roll: both controls decide whether a background microphone owner runs,
  // so both reconcile the rolling buffer after the save lands. Without this the
  // setting would go on saving a number that changes nothing (its state before
  // this phase: `start_recording` harvested a buffer nothing ever started).
  ;['opt-preroll-seconds', 'opt-preroll-enabled'].forEach(id =>
    bindSetting(id, filesBinding({
      key: id,
      after: () => {
        afterFilesSave()
        void reconcilePreroll()
      },
    })),
  )
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
  const prerollOnEl = document.getElementById('opt-preroll-enabled') as HTMLInputElement | null
  if (prerollOnEl)   prerollOnEl.checked = settings.prerollEnabled === true

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
 * Read the Opptak tab into `settings`. Persistence
 * belongs to `bindSetting`; the confirmation for a short auto-delete retention
 * is a guard on the two controls that can set it (see `autoDeleteGuard`), not a
 * surprise inside the save.
 */
function collectFilesSettings(): void {
  const autoDelEl   = document.getElementById('opt-auto-delete') as HTMLInputElement | null
  const autoDelDays = document.getElementById('auto-delete-days') as HTMLInputElement | null
  const days = autoDelEl?.checked ? (+(autoDelDays?.value ?? '') || 90) : 0

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
    prerollEnabled:        !!(document.getElementById('opt-preroll-enabled') as HTMLInputElement | null)?.checked,
  })
}
