import { settings, patchSettings, saveSettingsDebounced } from '../state'
import type { Settings } from '../../types'

import { t } from '../i18n'
import { updateAudioSeparateButton, loadVideoInfoStrip, updateVideoToggleButton } from './home'
import {
  bindRadioGroup,
  bindSetting,
  recordingImminentGuard,
  resyncBoundSettings,
  showSavedChip,
  type BindSettingOpts,
} from '../ui/bind-setting'
import { setFieldError } from '../ui/field-error'

function updateKeepAudioVisibility(): void {
  const modeEl    = document.querySelector<HTMLInputElement>('input[name="video-mode"]:checked')
  const separate  = modeEl?.value === 'separate'
  const row       = document.getElementById('video-keep-audio-row')
  if (row) row.style.display = separate ? 'none' : ''
}

type VideoDevice = { name: string; index: number }
let loadedDevices: VideoDevice[] = []

/** Every video control writes the same way: collect the panel into settings,
 *  persist through the shared debounce, refresh Home. Only the guard and the
 *  side effects differ per control. */
function videoBinding(extra: Partial<BindSettingOpts> = {}): BindSettingOpts {
  return {
    apply: () => collectVideoSettings(),
    after: () => afterVideoSave(),
    ...extra,
  }
}

export function setupVideoPage(): void {
  // AUTO-APPLY: every video control persists on change and says so with an
  // inline «Lagret ✓». The old flow had BOTH — silent auto-save AND a Lagre /
  // Avbryt footer that no code path could ever reach (#btn-video-save sat below
  // a panel that is hidden whenever video is off, and the tab had no dirty bar
  // wired), so the page looked unsaved while already being saved.
  bindSetting('opt-video-enable', videoBinding({
    key: 'videoEnabled',
    after: (value) => {
      const panel = document.getElementById('video-settings-panel')
      if (panel) panel.style.display = value ? '' : 'none'
      afterVideoSave()
    },
  }))

  document.getElementById('btn-video-refresh-devices')?.addEventListener('click', async () => {
    await refreshVideoDevices()
  })

  // Swapping the camera is the video-side twin of swapping the audio device —
  // the one change that can quietly cost you the recording about to start.
  bindSetting('video-device-select', videoBinding({
    key: 'videoDeviceName',
    confirmIf: recordingImminentGuard(t('video.guardDevice', 'Bytte kamera')),
    after: () => {
      void applyCameraCapabilities()
      afterVideoSave()
    },
  }))

  bindRadioGroup('video-resolution', videoBinding({ key: 'videoResolution' }))
  bindSetting('video-container-select', videoBinding({ key: 'videoContainer' }))
  bindSetting('video-encoder-select',   videoBinding({ key: 'videoEncoder' }))
  // Codec and fps also move the per-resolution GB/t estimates (H.265 ≈ 45 %
  // smaller, 50/60 fps a bit larger).
  bindSetting('video-codec-select', videoBinding({
    key: 'videoCodec',
    after: () => { updateSizeEstimates(); afterVideoSave() },
  }))
  bindSetting('video-fps-select', videoBinding({
    key: 'videoFramerate',
    after: () => { updateSizeEstimates(); afterVideoSave() },
  }))

  bindRadioGroup('video-mode', videoBinding({
    key: 'outputMode',
    after: () => { updateKeepAudioVisibility(); afterVideoSave() },
  }))
  bindSetting('opt-video-keep-audio',  videoBinding({ key: 'keepSeparateAudio' }))
  bindSetting('opt-editor-hw-encode',  videoBinding({ key: 'editorHwEncode' }))

  // Auto vs custom bitrate. These two radios share a name but carry no distinct
  // `value`, so they are bound individually rather than as a group.
  const toggleBitrateRow = () => {
    const autoCheck = document.getElementById('opt-video-bitrate-auto') as HTMLInputElement | null
    const row = document.getElementById('video-bitrate-custom-row')
    if (row) row.style.display = autoCheck?.checked ? 'none' : ''
    updateSizeEstimates()
  }
  ;['opt-video-bitrate-auto', 'opt-video-bitrate-custom'].forEach(id => {
    document.getElementById(id)?.addEventListener('change', () => {
      toggleBitrateRow()
      void commitVideo(document.getElementById(id))
    })
  })

  // The custom bitrate is clamped, not rejected: any number encodes, the
  // out-of-range ones just encode badly. The clamp is now explained under the
  // field instead of in a warning that appeared four lines away.
  bindSetting('video-bitrate-value', videoBinding({
    key: 'videoBitrate',
    coerce: (raw) => {
      const el = document.getElementById('video-bitrate-value') as HTMLInputElement | null
      const n = typeof raw === 'number' ? raw : NaN
      if (!el) return raw
      if (!Number.isFinite(n) || n < 500) {
        el.value = '500'
        setFieldError(el, t('video.minBitrateWarn', 'Minimum bitrate er 500 kbps'))
        return 500
      }
      if (n > 50000) {
        el.value = '50000'
        setFieldError(el, t('video.maxBitrateWarn', 'Maksimum bitrate er 50 000 kbps'))
        return 50000
      }
      setFieldError(el, null)
      return n
    },
    after: () => { updateSizeEstimates(); afterVideoSave() },
  }))
}

/** Collect + persist + receipt, for the controls that are not bound (the two
 *  bitrate-mode radios). */
async function commitVideo(host: HTMLElement | null): Promise<void> {
  collectVideoSettings()
  const ok = await saveSettingsDebounced(120)
  if (ok) showSavedChip(host?.closest<HTMLElement>('.video-bitrate-radio') ?? host)
  afterVideoSave()
}

export async function refreshVideoDevices(): Promise<void> {
  const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
  if (!selectEl) return

  selectEl.disabled = true
  // (selection is restored by videoDeviceName below, not the transient value)
  selectEl.innerHTML = '<option>Leter etter kameraer…</option>'

  try {
    const devices = (await window.api.listVideoDevices()) as VideoDevice[]
    loadedDevices = devices
    selectEl.innerHTML = ''

    if (!devices.length) {
      selectEl.innerHTML = '<option value="">Ingen kameraer funnet</option>'
      selectEl.disabled = true
      return
    }

    devices.forEach(d => {
      const opt = document.createElement('option')
      opt.value = String(d.index)
      opt.dataset.name = d.name
      opt.textContent = d.name
      selectEl.appendChild(opt)
    })

    // Restore previously selected device by name
    const currentName = settings.videoDeviceName ?? ''
    const match = devices.find(d => d.name === currentName) ?? devices[0]
    selectEl.value = String(match?.index ?? 0)
    selectEl.disabled = false
    // Gate resolution/fps NOW that a device is actually selected. Without this,
    // the gating that runs from applyVideoSettingsToUI() on page-show saw an empty
    // token (the list populates async) and re-enabled everything → 4K stayed
    // selectable on a 1080p camera.
    void applyCameraCapabilities()
  } catch {
    selectEl.innerHTML = '<option value="">Feil ved lasting</option>'
    selectEl.disabled = true
  }
}

export function applyVideoSettingsToUI(): void {
  const toggle  = document.getElementById('opt-video-enable') as HTMLInputElement | null
  const panel   = document.getElementById('video-settings-panel')
  const enabled = settings.videoEnabled ?? false

  if (toggle)  toggle.checked = enabled
  if (panel)   panel.style.display = enabled ? '' : 'none'

  // Resolution
  const res = settings.videoResolution ?? '720p'
  const resEl = document.querySelector<HTMLInputElement>(`input[name="video-resolution"][value="${res}"]`)
  if (resEl) resEl.checked = true

  // Bitrate
  const bitrate     = settings.videoBitrate ?? 0
  const autoCheck   = document.getElementById('opt-video-bitrate-auto') as HTMLInputElement | null
  const customCheck = document.getElementById('opt-video-bitrate-custom') as HTMLInputElement | null
  const bitrateInput = document.getElementById('video-bitrate-value') as HTMLInputElement | null
  const bitrateRow  = document.getElementById('video-bitrate-custom-row')
  if (bitrate === 0) {
    if (autoCheck) autoCheck.checked = true
    if (bitrateRow) bitrateRow.style.display = 'none'
  } else {
    if (customCheck) customCheck.checked = true
    if (bitrateInput) bitrateInput.value = String(bitrate)
    if (bitrateRow) bitrateRow.style.display = ''
  }

  // Framerate
  const fps = settings.videoFramerate ?? 30
  const fpsEl = document.getElementById('video-fps-select') as HTMLSelectElement | null
  if (fpsEl) fpsEl.value = String(fps)

  // Container + codec
  const containerEl = document.getElementById('video-container-select') as HTMLSelectElement | null
  if (containerEl) containerEl.value = settings.videoContainer ?? 'mp4'
  const codecEl = document.getElementById('video-codec-select') as HTMLSelectElement | null
  if (codecEl) codecEl.value = settings.videoCodec ?? 'h264'
  const encoderEl = document.getElementById('video-encoder-select') as HTMLSelectElement | null
  if (encoderEl) encoderEl.value = settings.videoEncoder ?? 'hardware'

  // Output mode
  const separate   = settings.outputMode === 'separate'
  const modeEl     = document.querySelector<HTMLInputElement>(`input[name="video-mode"][value="${separate ? 'separate' : 'combined'}"]`)
  if (modeEl) modeEl.checked = true

  // Keep audio toggle — only visible when NOT separate
  const keepAudioEl = document.getElementById('opt-video-keep-audio') as HTMLInputElement | null
  if (keepAudioEl) keepAudioEl.checked = settings.keepSeparateAudio
  updateKeepAudioVisibility()

  // Editor video-export hardware encoder — opt-in, default off.
  const hwEncodeEl = document.getElementById('opt-editor-hw-encode') as HTMLInputElement | null
  if (hwEncodeEl) hwEncodeEl.checked = settings.editorHwEncode === true

  // Unified-recorder toggle — default ON since v4.51. Treat `undefined`
  // Perfekt A/V-synk (unified recorder) er ALLTID på — valget er fjernet fra UI.

  // Populate device select (best-effort — may not have been loaded yet)
  if (loadedDevices.length) {
    const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
    if (selectEl && selectEl.options.length > 0) {
      const match = loadedDevices.find(d => d.name === (settings.videoDeviceName ?? ''))
      if (match) selectEl.value = String(match.index)
    }
  }

  // Warn when split recording is active: combined MP4 is not available in that mode
  const hasSplit = (settings.splitMinutes ?? 0) > 0
  const splitWarning = document.getElementById('video-split-warning')
  const splitHint    = document.getElementById('video-split-hint')
  if (splitWarning) splitWarning.style.display = hasSplit ? '' : 'none'
  if (splitHint)    splitHint.style.display    = hasSplit ? 'none' : ''

  // Gate resolution/fps to the selected camera's advertised modes.
  void applyCameraCapabilities()
  updateSizeEstimates()
  // The DOM now mirrors settings — rebase the bindings' baselines.
  resyncBoundSettings()
}

// Per-resolution "auto" video bitrate (Mb/s) at H.264 / 30 fps — the same ladder
// the recorder's bitrate_kbps() uses. The per-card GB/t labels are derived from
// this so they react to codec (H.265 ≈ 45 % smaller), fps and a custom bitrate.
const RES_BASE_MBPS: Record<string, number> = {
  '480p': 3.3, '720p': 7.8, '1080p': 15.6, '2160p': 48.9,
}

function fmtGb(gbPerHour: number): string {
  return gbPerHour >= 10 ? String(Math.round(gbPerHour)) : gbPerHour.toFixed(1)
}

/** Recompute and write the ~X GB/t estimate on each resolution card from the
 *  current codec / fps / bitrate. Approximate but reactive (was hardcoded). */
function updateSizeEstimates(): void {
  const codec     = (document.getElementById('video-codec-select') as HTMLSelectElement | null)?.value ?? 'h264'
  const fps       = parseInt((document.getElementById('video-fps-select') as HTMLSelectElement | null)?.value ?? '30', 10)
  const customOn  = !!(document.getElementById('opt-video-bitrate-custom') as HTMLInputElement | null)?.checked
  const customKbps = parseInt((document.getElementById('video-bitrate-value') as HTMLInputElement | null)?.value ?? '0', 10)

  const codecFactor = codec === 'h265' ? 0.55 : 1   // HEVC ≈ 45 % smaller
  const fpsFactor   = fps >= 50 ? 1.4 : 1           // 50/60 fps needs more bits

  for (const tag of ['480p', '720p', '1080p', '2160p']) {
    const el = document.getElementById('est-' + tag)
    if (!el) continue
    // A custom bitrate is fixed regardless of resolution → same size on every card.
    const mbps = customOn && customKbps > 0
      ? customKbps / 1000
      : (RES_BASE_MBPS[tag] ?? 7.8) * codecFactor * fpsFactor
    const gbPerHour = (mbps * 3600) / 8 / 1000 // Mb/s → MB/s → GB/h
    el.textContent = `~${fmtGb(gbPerHour)} GB / t`
  }
}

/**
 * Probe the selected camera and DISABLE the resolution cards / fps options it
 * can't deliver — a camera only records modes in its hardware descriptor, so
 * offering 4K/60 on a 720p webcam would just fail to open. On a failed probe
 * (or a platform that doesn't list modes) we leave everything enabled (let the
 * user try) rather than blocking. If the currently-selected resolution/fps is
 * not supported, we fall back to the best supported one and show a hint.
 */
export async function applyCameraCapabilities(): Promise<void> {
  const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
  const warnEl = document.getElementById('video-res-warning')
  const token = selectEl?.value
  // Re-enable everything first (clean slate before re-gating).
  const resInputs = Array.from(document.querySelectorAll<HTMLInputElement>('input[name="video-resolution"]'))
  const fpsEl = document.getElementById('video-fps-select') as HTMLSelectElement | null
  resInputs.forEach(r => { (r.closest('.option-card') as HTMLElement | null)?.classList.remove('is-disabled'); r.disabled = false })
  if (fpsEl) Array.from(fpsEl.options).forEach(o => { o.disabled = false })
  if (warnEl) warnEl.style.display = 'none'
  if (!token) return

  let cap: { supportedResolutions: string[]; supportedFramerates: number[]; maxHeight: number; maxFps: number } | null = null
  try {
    cap = await window.api.getCameraCapabilities(token)
  } catch {
    cap = null
  }
  // Empty/FAILED probe → be CONSERVATIVE, don't offer everything: we must never
  // UPSCALE to a resolution we can't confirm the source delivers natively. Assume
  // a safe 1080p ceiling (the common case) and flag it, so 4K stays gated until a
  // probe actually confirms a native-4K source. (Down-scaling is always fine.)
  let probeFailed = false
  if (!cap || cap.supportedResolutions.length === 0) {
    cap = {
      supportedResolutions: ['480p', '720p', '1080p'],
      supportedFramerates: [24, 25, 30, 50, 60],
      maxHeight: 1080,
      maxFps: 60,
    }
    probeFailed = true
  }

  // The highest supported tag = the camera's native ceiling (list is ascending).
  const nativeTag = [...cap.supportedResolutions].pop()

  // Disable unsupported resolution cards + badge the native one.
  for (const r of resInputs) {
    const ok = cap.supportedResolutions.includes(r.value)
    r.disabled = !ok
    const card = r.closest('.option-card') as HTMLElement | null
    card?.classList.toggle('is-disabled', !ok)
    // Refresh the per-card capability badge.
    card?.querySelector('.option-card-cap-badge')?.remove()
    if (card) {
      const badge = document.createElement('div')
      badge.className = 'option-card-cap-badge'
      if (!ok) {
        badge.textContent = t('video.resNotSupported', 'ikke støttet')
        badge.classList.add('cap-unsupported')
        card.appendChild(badge)
      } else if (r.value === nativeTag) {
        badge.textContent = t('video.resNative', 'kameraets maks')
        badge.classList.add('cap-native')
        card.appendChild(badge)
      }
    }
  }
  // Disable unsupported fps options.
  if (fpsEl) {
    for (const o of Array.from(fpsEl.options)) {
      o.disabled = !cap.supportedFramerates.includes(parseInt(o.value))
    }
  }

  // If the current pick is now unsupported, fall back to the best supported.
  const checked = resInputs.find(r => r.checked)
  const fellBack = !!(checked && checked.disabled)
  if (fellBack) {
    const fallback = resInputs.find(r => r.value === nativeTag)
    if (fallback) fallback.checked = true
  }
  if (fpsEl && fpsEl.selectedOptions[0]?.disabled) {
    const bestFps = [...cap.supportedFramerates].pop()
    if (bestFps != null) fpsEl.value = String(bestFps)
  }

  // Always show the camera's native ceiling; prepend a warning when we had to
  // fall back from an unsupported pick.
  if (warnEl) {
    if (probeFailed) {
      // Honest about the assumption — this is NOT a confirmed camera ceiling.
      warnEl.textContent = t(
        'video.probeFailed',
        'Kunne ikke lese kameraets oppløsninger — begrenset til 1080p for å unngå oppskalering. 4K krever et kamera som leverer ekte 4K.',
      )
      ;(warnEl as HTMLElement).style.color = 'var(--warning, #d08700)'
    } else {
      const info = `${t('video.cameraDelivers', 'Kameraet leverer maks')} ${cap.maxHeight}p · ${cap.maxFps} fps.`
      warnEl.textContent = fellBack
        ? `${t('video.resUnsupportedShort', 'Valgt oppløsning støttes ikke — satt til kameraets maks.')} ${info}`
        : info
      ;(warnEl as HTMLElement).style.color = fellBack ? 'var(--warning, #d08700)' : 'var(--text-3, #8899bb)'
    }
    warnEl.style.display = ''
  }
}

/** Mirror the change onto Home without a navigation: the «Separat lydfil»
 *  badge, the «Video på»-toggle and the video-quality/camera info cards. All
 *  are no-ops when the Home DOM is not mounted (internal guard). */
function afterVideoSave(): void {
  updateAudioSeparateButton()
  updateVideoToggleButton()
  loadVideoInfoStrip()
}

/** Read the whole video panel into `settings`. Persistence belongs to
 *  `bindSetting` (one debounced write, one visible receipt). */
function collectVideoSettings(): void {
  const toggle  = document.getElementById('opt-video-enable') as HTMLInputElement | null
  const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
  const fpsEl   = document.getElementById('video-fps-select') as HTMLSelectElement | null
  const bitrateInput = document.getElementById('video-bitrate-value') as HTMLInputElement | null

  const enabled   = toggle?.checked ?? false
  const deviceIdx = selectEl ? parseInt(selectEl.value) : null
  const deviceName = selectEl
    ? (selectEl.selectedOptions[0]?.dataset.name ?? selectEl.selectedOptions[0]?.textContent ?? null)
    : null

  const res = (document.querySelector<HTMLInputElement>('input[name="video-resolution"]:checked')?.value ?? '720p') as '2160p' | '1080p' | '720p' | '480p'
  const fps = fpsEl ? parseInt(fpsEl.value) : 30
  const containerEl = document.getElementById('video-container-select') as HTMLSelectElement | null
  const codecEl = document.getElementById('video-codec-select') as HTMLSelectElement | null
  const videoContainer = (containerEl?.value ?? 'mp4') as 'mp4' | 'mov'
  const videoCodec = (codecEl?.value ?? 'h264') as 'h264' | 'h265'
  const encoderEl = document.getElementById('video-encoder-select') as HTMLSelectElement | null
  const videoEncoder = (encoderEl?.value ?? 'hardware') as 'software' | 'hardware'

  const autoMode = document.getElementById('opt-video-bitrate-auto') as HTMLInputElement | null
  const bitrate  = (autoMode?.checked) ? 0 : parseInt(bitrateInput?.value ?? '0') || 0

  const modeSel   = document.querySelector<HTMLInputElement>('input[name="video-mode"]:checked')
  const separate  = modeSel?.value === 'separate'

  const keepAudioEl = document.getElementById('opt-video-keep-audio') as HTMLInputElement | null
  const keepAudio   = keepAudioEl ? keepAudioEl.checked : true

  const hwEncodeEl  = document.getElementById('opt-editor-hw-encode') as HTMLInputElement | null
  const editorHwEncode = hwEncodeEl ? hwEncodeEl.checked : false

  const updated = {
    ...settings,
    videoEnabled:      enabled,
    videoDeviceIndex:  deviceIdx,
    videoDeviceName:   deviceName,
    videoResolution:   res,
    videoFramerate:    fps,
    videoContainer,
    videoCodec,
    videoEncoder,
    videoBitrate:      bitrate,
    outputMode:        separate ? 'separate' : 'combined',
    keepSeparateAudio: keepAudio,
    editorHwEncode,
  }

  patchSettings(updated as Settings)
}
