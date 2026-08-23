import { settings, patchSettings } from '../state'
import type { Settings } from '../../types'

import { t } from '../i18n'
import { updateAudioSeparateButton, loadVideoInfoStrip, updateVideoToggleButton } from './home'
import {
  bindSetting,
  recordingImminentGuard,
  resyncBoundSettings,
  type BindSettingOpts,
} from '../ui/bind-setting'

/**
 * The Video tab — «lyd + video, ett valg» (v0.15).
 *
 * Three controls: camera on/off, which camera, and whether to keep a separate
 * audio file beside the MP4. Resolution, frame rate, container, codec, encoder
 * backend, bitrate and combined-vs-separate used to be settings here; they are
 * constants in `crates/sundayrec-core/src/capture.rs` now (1080p / 30 fps /
 * mp4 / H.264 / hardware where the platform has it), each with its argument.
 * A volunteer cannot mis-set what is not a setting.
 */

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
  // inline «Lagret ✓».
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
      void describeCameraCapabilities()
      afterVideoSave()
    },
  }))

  bindSetting('opt-video-keep-audio', videoBinding({ key: 'keepSeparateAudio' }))
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
    // Describe the camera NOW that a device is actually selected (the list
    // populates async, so the page-show pass saw an empty token).
    void describeCameraCapabilities()
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

  const keepAudioEl = document.getElementById('opt-video-keep-audio') as HTMLInputElement | null
  if (keepAudioEl) keepAudioEl.checked = settings.keepSeparateAudio

  // Populate device select (best-effort — may not have been loaded yet)
  if (loadedDevices.length) {
    const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
    if (selectEl && selectEl.options.length > 0) {
      const match = loadedDevices.find(d => d.name === (settings.videoDeviceName ?? ''))
      if (match) selectEl.value = String(match.index)
    }
  }

  void describeCameraCapabilities()
  // The DOM now mirrors settings — rebase the bindings' baselines.
  resyncBoundSettings()
}

/**
 * Probe the selected camera and SAY what it delivers — «Kameraet leverer maks
 * 1080p · 30 fps». There is nothing to gate any more (the resolution/fps
 * pickers left in v0.15; the recorder targets 1080p/30 and the probe at start
 * caps that to what the camera advertises), but a volunteer still wants to
 * know before Sunday whether the camera is the 720p webcam or the HDMI card.
 * A failed probe says so rather than guessing.
 */
export async function describeCameraCapabilities(): Promise<void> {
  const selectEl = document.getElementById('video-device-select') as HTMLSelectElement | null
  const infoEl = document.getElementById('video-camera-info')
  const token = selectEl?.value
  if (!infoEl) return
  infoEl.style.display = 'none'
  if (!token) return

  let cap: { supportedResolutions: string[]; supportedFramerates: number[]; maxHeight: number; maxFps: number } | null = null
  try {
    cap = await window.api.getCameraCapabilities(token)
  } catch {
    cap = null
  }
  if (!cap || cap.supportedResolutions.length === 0) {
    infoEl.textContent = t(
      'video.probeFailed',
      'Kunne ikke lese kameraets oppløsninger — opptaket bruker det kameraet faktisk leverer, opptil 1080p.',
    )
    infoEl.style.color = 'var(--text-3, #8899bb)'
  } else {
    infoEl.textContent = `${t('video.cameraDelivers', 'Kameraet leverer maks')} ${cap.maxHeight}p · ${cap.maxFps} fps.`
    infoEl.style.color = 'var(--text-3, #8899bb)'
  }
  infoEl.style.display = ''
}

/** Mirror the change onto Home without a navigation: the «Separat lydfil»
 *  badge, the «Video på»-toggle and the camera info cards. All are no-ops when
 *  the Home DOM is not mounted (internal guard). */
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

  const enabled   = toggle?.checked ?? false
  const deviceIdx = selectEl ? parseInt(selectEl.value) : null
  const deviceName = selectEl
    ? (selectEl.selectedOptions[0]?.dataset.name ?? selectEl.selectedOptions[0]?.textContent ?? null)
    : null

  const keepAudioEl = document.getElementById('opt-video-keep-audio') as HTMLInputElement | null
  const keepAudio   = keepAudioEl ? keepAudioEl.checked : true

  const updated = {
    ...settings,
    videoEnabled:      enabled,
    videoDeviceIndex:  deviceIdx,
    videoDeviceName:   deviceName,
    keepSeparateAudio: keepAudio,
  }

  patchSettings(updated as Settings)
}
