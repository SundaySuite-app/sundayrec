/**
 * Audio input-device enumeration (renderer-side).
 *
 * ## The blink-open that had to go
 *
 * `getAudioDevices()` used to open `getUserMedia({ audio: true })` for a blink
 * and throw the stream away immediately — not to capture anything, but because
 * `enumerateDevices()` returns EMPTY LABELS until a page has been granted a
 * microphone. So every device picker, every home-page status check and every
 * recording-overlay device name silently made the webview an owner of the
 * default input device for a moment.
 *
 * On macOS that moment is not bounded: WebKit tears a CoreAudio input down
 * asynchronously, so the "blink" could still be holding the device when the
 * recorder opened it — the 2026-07-31 Qu-5 incident, where a 32-channel mixer
 * stayed pinned to the 2-channel format gUM had negotiated.
 *
 * The list now comes from the backend (`list_audio_devices` → cpal, plus ASIO
 * on Windows), which is the same enumeration the recorder itself resolves
 * against. No permission prompt, no stream, real labels, and — for free — the
 * real channel count per device.
 *
 * Recording and metering were already backend-owned (src-tauri/src/recorder,
 * audio/vu-feed.ts). With this, the renderer holds no microphone at all.
 */

import type { AudioBackendKind } from '../../bindings/AudioBackendKind'
import { patchSettings, settings } from '../state'

/**
 * One selectable audio input.
 *
 * `deviceId`/`label` keep the `MediaDeviceInfo` field NAMES the call sites were
 * written against, but the VALUES are now the backend's: `deviceId` is the
 * device name the recorder addresses (not a Web Audio hash), and `label` is
 * that same human-readable name. That makes `deviceId` and `settings.deviceName`
 * finally speak the same language — the Web Audio hash never matched anything
 * the backend knew, which is why every call site had to fuzzy-match by label.
 */
export interface AudioInputDevice {
  deviceId: string
  label: string
  /** Which OS backend reaches it — drives the picker badge. */
  backend: AudioBackendKind
  /** Real input-channel count (a Qu-5's 32, not getUserMedia's 2). 0 = unknown. */
  channels: number
  /** The host's default input. Replaces the old `deviceId === 'default'` entry. */
  isDefault: boolean
}

/**
 * The host's input devices, for pickers and status checks.
 *
 * ASIO entries are deliberately EXCLUDED: audio-page renders those from
 * `listAsioDrivers()` under `asio::`-prefixed ids (a different id space, since
 * an ASIO card and its WASAPI stereo-pair shadow are the same hardware), and
 * listing them twice would give the user two cards for one interface.
 */
export async function getAudioDevices(): Promise<AudioInputDevice[]> {
  try {
    const list = await window.api.listAudioDevices()
    return list
      .filter(d => d.backend !== 'asio')
      .map(d => ({
        deviceId: d.id,
        label: d.name,
        backend: d.backend,
        channels: d.inputChannels,
        isDefault: d.isDefault,
      }))
  } catch {
    return []
  }
}

/**
 * Is this the machine's own microphone rather than a mixer / interface?
 *
 * Drives the "Ikke anbefalt" badge in the picker and the onboarding wizard's
 * default pick. The old test was `/built-in|innebygd|default/i` against Web
 * Audio labels, which said «Built-in Microphone» — CoreAudio calls the same
 * device «MacBook Pro Microphone», and Windows «Microphone Array», so the badge
 * would have quietly stopped appearing on exactly the machines that need it.
 */
export function isBuiltInDevice(label: string): boolean {
  return /built-?in|innebygd|internal|default|standard|macbook|imac|mac ?(mini|studio|pro)|microphone array|mikrofonrekke/i.test(label)
}

/**
 * Re-point a stored device id that no longer resolves.
 *
 * Two things make this necessary, and it has to handle both:
 *  - Windows reassigns device ids after a reboot or a driver update, which is
 *    what this heal was originally written for (it lived inline in home.ts).
 *  - The id space itself changed when enumeration moved to the backend: a
 *    settings file written by an older build holds a Web Audio hash. The saved
 *    `deviceName` still matches, so the heal migrates it on first launch.
 *
 * The channel grid's L/R picks are keyed by device id, so they MUST travel with
 * the id — otherwise a Qu-5 rig silently reverts to channels 1/2 after an
 * update, and nobody finds out until the recording is of the wrong source.
 *
 * Returns whether anything was changed (the caller persists + re-renders).
 */
export function healStoredDeviceId(devices: readonly AudioInputDevice[]): boolean {
  const storedId = settings.deviceId
  const storedName = settings.deviceName
  if (!storedId || !storedName) return false
  if (devices.some(d => d.deviceId === storedId)) return false
  const byLabel = devices.find(d => d.label.toLowerCase() === storedName.toLowerCase())
  if (!byLabel || byLabel.deviceId === storedId) return false

  const patch: Parameters<typeof patchSettings>[0] = { deviceId: byLabel.deviceId }
  const stored = settings.deviceChannels?.[storedId]
  if (stored) {
    patch.deviceChannels = { ...(settings.deviceChannels ?? {}), [byLabel.deviceId]: stored }
  }
  patchSettings(patch)
  return true
}
