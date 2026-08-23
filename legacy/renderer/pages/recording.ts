/**
 * Recording session UI — overlay, VU meter, silence-warning banner, split timer.
 *
 * Recording itself runs entirely in the native Rust capture engine
 * (src-tauri/src/recorder — cpal capture → ring buffer → the app's own WAV/
 * container writer, with a legacy ffmpeg-audio escape hatch behind a settings
 * flag). This module only drives the UI: it never opens its own microphone
 * stream for metering — the overlay's level meter, waveform and clip
 * indicators are driven entirely by the recording's own `recording://levels`
 * telemetry (see startLevelsMeter), so the mic has exactly ONE owner (the
 * recorder) for the whole take.
 *
 * Start flow:
 *   1. window.api.startRecordingNow(opts) → Tauri `plan_recording_opts` +
 *      `start_recording` spawn the native capture engine
 *   2. showOverlay() → recording UI becomes visible
 *   3. startMonitoring(opts) → releases every meter's hold on the VU engine and
 *      subscribes to the recording's own telemetry (no second device open)
 *
 * Stop flow:
 *   1. window.api.stopRecordingNow() → Tauri `stop_recording` asks the engine
 *      to finalize (a graceful stop request, not a raw ffmpeg stdin 'q')
 *   2. stopMonitoring() → unsubscribes from the telemetry + tears down the UI timers
 *   3. The backend emits `recording://finished` → renderer hides overlay, shows history
 */
import { t, tf, onLocaleApplied } from '../i18n'
import { settings } from '../state'
import { getAudioDevices } from '../audio/capture'
import { setVUBar } from '../audio/vu'
import { RELEASE_TAU_MS, alphaFor } from '../audio/smoothing'
import { RecordingWaveform } from '../audio/waveform'
import { fmtCountdown, flashMsg, isoDate } from '../helpers'
import { stopVU as stopHomeVU, startVU as startHomeVU } from './home-vu'
import { stopChannelGrid as stopAudioPageMonitoring } from './channel-grid'
import { renderRecentRecordings, stopVideoPreview, startVideoPreview } from './home'
import { closeModal, openModal } from '../ui/modal-manager'
import { hideEl, showEl } from '../ui/motion'
import { banner, toast } from '../ui/toast'
import { navigateTo } from '../ui/navigate'
import { showEditorPrompt } from './editor-page'
import type { RecordingOpts } from '../../types'

let recTimerIval:      ReturnType<typeof setInterval> | null = null
let signalCheckTimer:  ReturnType<typeof setTimeout>  | null = null
export let isRecording = false

let recStartTime = 0
let recBytes     = 0
let previewRestartTimer: ReturnType<typeof setTimeout> | null = null
let recPreviewUnsub:      (() => void) | undefined
let recVideoDimsSet   = false
let recFrameBlobUrl:  string | null = null

function readJpegDims(arr: Uint8Array): { w: number; h: number } | null {
  let i = 0
  while (i < arr.length - 8) {
    if (arr[i] !== 0xff) { i++; continue }
    const m = arr[i + 1]
    if (m === 0xc0 || m === 0xc2) {
      const h = (arr[i + 5] << 8) | arr[i + 6]
      const w = (arr[i + 7] << 8) | arr[i + 8]
      if (w > 0 && h > 0) return { w, h }
    }
    if (m !== 0xd8 && m !== 0xd9 && m !== 0x01 && i + 3 < arr.length) {
      const seg = (arr[i + 2] << 8) | arr[i + 3]
      if (seg >= 2) { i += 2 + seg; continue }
    }
    i++
  }
  return null
}

// ── Auto-stop ────────────────────────────────────────────────────────────────
//
// The deadline belongs to the RECORDER, not to this file. `+30 min` and
// `Avbryt auto-stopp` used to be renderer-local setTimeouts that re-implemented
// the engine's timer: two clocks, one recording, and whichever fired first won.
// Extending by 30 minutes armed a fresh renderer timeout while the engine's own
// deadline sat untouched — so a take could stop at the original time anyway, and
// the flag that was supposed to paper over that (`stopOverridden`) did it by
// SWALLOWING the terminal state, leaving the overlay up over a dead recording.
//
// Now both buttons call the real commands (`recording_extend_autostop` /
// `recording_cancel_autostop`); the engine moves its deadline, re-pins its timer
// and re-emits `recording://state` with the new `scheduled_stop_ms`. This module
// only RENDERS that number — the countdown ticks locally, nothing here stops a
// recording, and there is nothing left to swallow.
let scheduledStop: Date | null = null
let schedCntTimer: ReturnType<typeof setInterval> | null = null

// Premium scrolling waveform for the recording overlay, driven by the
// recording's own level telemetry (see startLevelsMeter).
let recWaveform: RecordingWaveform | null = null

/** dBFS (−60..0) → 0..1 envelope height (matches the VU bar mapping). */
function dbToEnvHeight(db: number): number {
  return (Math.max(-60, Math.min(0, db)) + 60) / 60
}

// ── Level meter driven by the recording's own `recording://levels` telemetry
//    (computed by the native capture engine directly off its ring buffer; a
//    legacy ffmpeg-astats path only exists behind the classic_ffmpeg_audio
//    escape hatch) — NOT a second open of the device. Opening the built-in mic
//    twice (the recorder's capture + a monitoring stream) made macOS
//    re-configure the shared device and drop samples → choppy ("hakkete")
//    recordings. Reading the already-captured signal's levels means the mic is
//    opened EXACTLY once. ──────
const meter = {
  tL: -60, tR: -60,   // latest target dBFS from the last event
  smL: -60, smR: -60, // smoothed (rise instant, fall eased)
  pkL: -60, pkR: -60, pkTL: 0, pkTR: 0, // peak-hold
  mono: false,        // right channel was null → collapse to one bar
}
let levelsUnsub: (() => void) | undefined
let meterRaf = 0

// ── Setup ────────────────────────────────────────────────────────────────────

export function setupRecording(): void {
  // Opening the stop-confirm modal also focuses the SAFE cancel button so
  // an accidental Enter keeps the recording going.
  function openStopConfirm(): void {
    // Already finalizing — there is nothing left to confirm stopping.
    if (finalizing) return
    openModal('modal-confirm-stop')
    // Defer focus to next tick so the browser has rendered the modal
    setTimeout(() => {
      (document.getElementById('btn-confirm-cancel') as HTMLButtonElement | null)?.focus()
    }, 0)
  }

  document.getElementById('btn-start-recording')?.addEventListener('click', () => {
    if (isRecording) {
      if (settings.protectRecording !== false) openStopConfirm()
      else doStopRecording()
    } else {
      openManualModal()
    }
  })

  document.getElementById('btn-stop-overlay')?.addEventListener('click', () => {
    if (settings.protectRecording !== false) openStopConfirm()
    else doStopRecording()
  })

  document.getElementById('btn-confirm-stop')?.addEventListener('click', () => {
    closeModal('modal-confirm-stop')
    doStopRecording()
  })

  document.getElementById('btn-confirm-cancel')?.addEventListener('click', () => {
    closeModal('modal-confirm-stop')
  })

  document.getElementById('btn-manual-cancel')?.addEventListener('click', () => {
    closeModal('modal-manual')
  })

  document.getElementById('btn-manual-start')?.addEventListener('click', handleManualStart)

  // Both buttons ask the ENGINE to move its deadline and then wait for the
  // `recording://state` it re-emits — no optimistic local guess, so what the
  // countdown shows is what the recorder will actually do. A failed invoke says
  // so instead of silently pretending the extension took.
  document.getElementById('btn-extend-30')?.addEventListener('click', () => {
    void withAutostopButton('btn-extend-30', () => window.api.extendAutostop(EXTEND_MINUTES))
  })

  document.getElementById('btn-cancel-autostop')?.addEventListener('click', () => {
    void withAutostopButton('btn-cancel-autostop', () => window.api.cancelAutostop())
  })

  const ipcCleanups = [
    window.api.on('recording-overlay-start', (opts) => {
      // Tauri's `recording://started` carries NO opts (the manual start path
      // already showed the overlay locally with the real opts). Guard so we don't
      // re-render the overlay with `undefined` and throw.
      const o = opts as RecordingOpts | undefined
      if (!o || typeof o !== 'object') return
      showOverlay(o)
      startMonitoring(o).catch(err => {
        console.error('[recording] monitoring start error:', err)
        try { stopMonitoring() } catch {}
      })
    }),
    window.api.on('recording-overlay-stop', (data) => {
      // This is mapped to `recording://state`, which fires on EVERY transition
      // (preparing/recording/reconnecting/…), not just on stop. Only tear the
      // overlay down on a TERMINAL state, or a preparing→recording mid-session
      // event would hide the live overlay.
      const payload = data as { state?: string; scheduled_stop_ms?: number | null } | undefined
      const st = payload?.state
      // The auto-stop deadline rides along on EVERY state emit — including the
      // one the engine fires purely because the deadline moved (live extend /
      // cancel). Applying it before the branch below is what makes the countdown
      // backend-authoritative rather than a local guess.
      if (payload && 'scheduled_stop_ms' in payload) {
        applyScheduledStop(payload.scheduled_stop_ms ?? null)
      }
      if (st === 'recording' || st === 'reconnecting') {
        // Resync: the engine says a session is LIVE. If the UI thinks it's idle
        // (a torn-down overlay after a transient error), bring the overlay back —
        // otherwise the user has a running recording with no stop button (the
        // 2026-07-31 rig incident).
        if (!isRecording) resyncOverlayToLiveSession()
        return
      }
      if (st !== 'stopped' && st !== 'failed' && st !== 'idle') return
      // Nothing is filtered here any more. The old `stopOverridden` guard existed
      // to hide the stop that the renderer's private auto-stop timer could no
      // longer prevent; with the deadline owned by the engine there is no
      // spurious terminal state to swallow — and swallowing one was how the
      // overlay used to strand over a finished recording.
      stopMonitoring().catch(err => console.error('[recording] monitoring stop error:', err)).finally(() => hideOverlay())
    }),
    window.api.on('recording-finished', (entry) => {
      const rec = entry as { path?: string; splitRestart?: boolean } | undefined
      if (!rec?.splitRestart) hideOverlay()
      renderRecentRecordings()
      if (rec?.path && !rec.splitRestart && settings.askOpenEditor !== false) showEditorPrompt(rec.path)
    }),
    window.api.on('recording-error', (data) => {
      // TERMINAL: the backend only emits recording://error when the session is
      // over (fatal code / recovery given up) — transient hiccups arrive on
      // recording-warning instead and must NOT tear the overlay down.
      const d = data as { error?: string; message?: string } | undefined
      // Stop monitoring (VU timer + mic stream) before hiding overlay — same as normal stop
      stopMonitoring().catch(err => console.error('[recording] stopMonitoring on error:', err))
      hideOverlay()
      renderRecentRecordings()
      // The localized code text leads (the raw `message` is an ffmpeg stderr
      // line like ":2: Input/output error" — diagnostics material, not UI copy).
      const msg = (d?.error ? translateNativeError(d.error) : null) ?? d?.message ?? null
      if (msg) showGlobalError(msg)
    }),
    window.api.on('recording-warning', (data) => {
      // NON-terminal: the engine's reconnect policy is (about to start) retrying.
      // Keep the overlay; show the reconnect banner so the hiccup is visible.
      const d = data as { error?: string; message?: string } | undefined
      console.warn('[recording] transient recorder error:', d?.error, d?.message)
      showReconnectBanner()
    }),
    // NON-terminal, same as recording-warning: the stop-on-silence detector
    // fired a warning ahead of the auto-stop timeout, so the user gets a chance
    // to notice before the take ends.
    //
    // This used to write into the SAME #rec-reconnect banner as the reconnect
    // path. Two unrelated problems sharing one element means whichever fired
    // last erased the other — a device that dropped out and came back silent
    // would show only one of the two things wrong with the take. Silence has
    // its own line in the overlay now.
    window.api.on('recording-silence', (data) => {
      const d = data as { code?: string; message?: string } | undefined
      console.warn('[recording] silence detected:', d?.code, d?.message)
      // The backend message is a hardcoded Norwegian string (not run through
      // this app's i18n) — fall back to our own localized copy whenever the
      // payload carries that generic text (or nothing at all).
      showSilenceLine(() => d?.message && d.message !== 'Stillhet oppdaget i lydsignalet'
        ? d.message
        : t('recording.silenceWarn', 'Stillhet oppdaget — opptaket stopper automatisk hvis stillheten fortsetter.'))
    }),
    window.api.on('recording-quality', (data) => {
      // Session-end truth verdict FAILED: the delivered file provably holds
      // less audio than the session lasted (or the drop counters crossed the
      // fail line). This must never pass silently — it is the alarm the
      // 2026-07-31 incident lacked (15–56 % loss reported as "clean").
      const r = data as { expectedSec?: number; measuredSec?: number; reasons?: string[] } | undefined
      const expected = Math.round(r?.expectedSec ?? 0)
      const measured = Math.round(r?.measuredSec ?? 0)
      const reasons = (r?.reasons ?? []).filter(x => typeof x === 'string' && x.length)
      console.error('[recording] QUALITY ALARM:', r?.reasons, `${measured}/${expected}s`)
      // Data loss gets its OWN banner, not the shared error strip. It used to
      // call showGlobalError, which (a) is overwritten by the next error of any
      // kind and (b) FORCE-NAVIGATES to home — yanking the user off whatever
      // they were doing, and away from the recording the message is about. A
      // keyed banner persists until dismissed and carries the way forward.
      let msg = tf('recording.qualityAlarm', { m: measured, e: expected },
        'ADVARSEL: Opptaket mangler lyd — fila inneholder {m} av {e} sekunder. Sjekk opptaket før du stoler på det.')
      // The engine's reasons are the diagnostic detail that decides whether
      // this is a device problem or a disk problem — carry them.
      if (reasons.length) {
        msg += ' ' + tf('recording.qualityReasons', { r: reasons.join(', ') }, 'Årsak: {r}')
      }
      banner('rec-quality', 'error', msg, [
        {
          label: t('recording.qualityAction', 'Vis opptak'),
          onClick: () => navigateTo('search'),
        },
      ])
    }),
    window.api.on('recording-progress', (data) => {
      const d = data as { bytes?: number } | undefined
      if (d?.bytes !== undefined) recBytes = d.bytes
    }),
    // NB: there's no separate 'video-progress' event from the Tauri backend — the
    // combined file's bytes_written (recording-progress, above) is the only size
    // signal. The KAMERA badge is updated from recBytes in the 1 s timer instead,
    // so it no longer stays stuck at "0 MB".
    window.api.on('recording-reconnecting', () => showReconnectBanner()),
    window.api.on('recording-reconnected',  () => hideReconnectBanner()),
    // The tray's start/stop no longer arrive on these Electron-era channels —
    // the Rust tray emits ONE `tray://action` event, adapted in tray-actions.ts,
    // which calls openManualModal / doStopRecording directly (both exported).
  ]
  window.addEventListener('beforeunload', () => ipcCleanups.forEach(fn => fn?.()))
}

// ── Manual recording modal ───────────────────────────────────────────────────

export async function openManualModal(): Promise<void> {
  openModal('modal-manual')

  // R4: warm the backend ffmpeg device enumeration now, while the user is picking
  // options in the modal, so the recorder's start path reuses it (within its short
  // freshness window) instead of paying another `ffmpeg -list_devices` on the
  // record press. Fire-and-forget; the recorder re-enumerates if this is stale.
  void window.api.listVideoDevices().catch(() => {})
  const nameEl  = document.getElementById('manual-filename') as HTMLInputElement | null
  if (nameEl) nameEl.value = ''

  // Audio devices
  const devSel  = document.getElementById('manual-device') as HTMLSelectElement | null
  const devices = await getAudioDevices()
  if (devSel) {
    devSel.replaceChildren(...devices.map(d => {
      const opt = document.createElement('option')
      opt.value = d.deviceId
      opt.textContent = d.label || d.deviceId
      return opt
    }))
    if (settings.deviceId) {
      devSel.value = settings.deviceId
      if (!devSel.value && devices.length) devSel.selectedIndex = 0
    }
  }

  // Video devices — show section only when video mode is on
  const videoSection = document.getElementById('manual-video-section')
  const videoSel     = document.getElementById('manual-video-device') as HTMLSelectElement | null
  const videoHint    = document.getElementById('manual-video-hint')

  if (!settings.videoEnabled) {
    if (videoSection) videoSection.style.display = 'none'
    return
  }
  if (videoSection) videoSection.style.display = ''

  if (videoSel) {
    videoSel.innerHTML = '<option value="">Laster enheter…</option>'
    videoSel.disabled  = true
  }
  if (videoHint) videoHint.style.display = 'none'

  try {
    const videoDevices = await window.api.listVideoDevices()
    if (!videoSel) return

    videoSel.innerHTML = ''

    // "No video for this recording" option
    const noVideoOpt = document.createElement('option')
    noVideoOpt.value = '__none__'
    noVideoOpt.textContent = t('recording.videoNone', 'Ingen video (bare lyd)')
    videoSel.appendChild(noVideoOpt)

    videoDevices.forEach(d => {
      const opt = document.createElement('option')
      opt.value = String(d.index)
      opt.dataset.name = d.name
      opt.textContent = d.name
      videoSel.appendChild(opt)
    })

    // Pre-select the saved device
    if (settings.videoDeviceName) {
      const match = videoDevices.find(d => d.name === settings.videoDeviceName)
      videoSel.value = match ? String(match.index) : '__none__'
    } else {
      videoSel.value = '__none__'
    }

    videoSel.disabled = false

    if (!videoDevices.length) {
      if (videoHint) {
        videoHint.textContent = t('home.cameraNoneFound', 'Ingen kameraer funnet — sjekk tilkobling')
        videoHint.style.display = ''
      }
    }
  } catch {
    if (videoSel) {
      videoSel.innerHTML = ''
      videoSel.appendChild(Object.assign(document.createElement('option'), { value: '__none__', textContent: t('recording.videoListError', 'Feil ved lasting av kameraer') }))
      videoSel.disabled  = false
    }
  }
}

async function handleManualStart(): Promise<void> {
  const btn = document.getElementById('btn-manual-start') as HTMLButtonElement | null
  if (btn) btn.disabled = true

  const devSel      = document.getElementById('manual-device')    as HTMLSelectElement | null
  const videoSel    = document.getElementById('manual-video-device') as HTMLSelectElement | null
  const nameEl      = document.getElementById('manual-filename')  as HTMLInputElement  | null
  const deviceId    = devSel?.value || settings.deviceId || null
  const devChannels = deviceId ? (settings.deviceChannels?.[deviceId] ?? null) : null
  const deviceName  = devSel?.options[devSel?.selectedIndex ?? 0]?.textContent ?? settings.deviceName ?? null

  // Resolve video source from the modal selection
  const videoVal  = videoSel?.value ?? '__none__'
  const noVideo   = !settings.videoEnabled || videoVal === '__none__'
  const videoOpt  = videoSel?.options[videoSel.selectedIndex ?? 0]
  const videoName = (videoOpt?.dataset.name ?? videoOpt?.textContent ?? '').trim() || null
  const videoIdx  = (videoVal && videoVal !== '__none__') ? parseInt(videoVal) : null

  const opts: RecordingOpts = {
    ...settings,
    deviceId,
    deviceName:       deviceName ?? undefined,
    customName:       nameEl?.value.trim() ?? '',
    channelL:         devChannels?.channelL ?? 0,
    channelR:         devChannels?.channelR ?? 1,
    maxMinutes:       settings.manualMaxMinutes || undefined,
    videoEnabled:     !noVideo,
    videoDeviceName:  noVideo ? null : videoName,
    videoDeviceIndex: noVideo ? null : videoIdx,
  }

  // Release every meter's hold on the VU engine BEFORE the capture engine opens
  // the device, so the hand-over is orderly rather than a teardown underneath a
  // running session. (This started life as a LEAK GUARD — 2026-07-31 audit —
  // when the home VU's own getUserMedia stream stayed open across the whole
  // device-open. The renderer holds no microphone at all now.)
  releaseRendererAudioCaptures()

  // Do NOT close the modal before we know if the recording started —
  // closing first makes error messages invisible to the user.
  let res: { ok?: boolean; error?: string } | null = null
  try {
    res = await window.api.startRecordingNow(opts)
  } catch (err) {
    closeModal('modal-manual')
    showGlobalError(err instanceof Error ? err.message : String(err))
    if (btn) btn.disabled = false
    // The start failed — give the home meter back.
    startHomeVU()
    return
  }

  if (res?.ok) {
    closeModal('modal-manual')
    showOverlay(opts)
    try { await startMonitoring(opts) }
    catch (err) {
      // Monitoring is only for VU display — keep recording alive, just log
      console.warn('[recording] VU monitor failed (non-fatal):', err)
      try { stopMonitoring() } catch {}
    }
  } else {
    // Keep modal open so the error is visible on the button
    const errMsg = res?.error ? translateNativeError(res.error) : t('general.unknownError', 'ukjent feil')
    flashMsg(btn, '✕ ' + errMsg, false)
  }
  if (btn) btn.disabled = false
}

export async function startRecordingWithOpts(opts: RecordingOpts): Promise<void> {
  let res: { ok?: boolean; error?: string } | null = null
  try { res = await window.api.startRecordingNow(opts) } catch { return }
  if (!res?.ok) {
    const errMsg = res?.error ? translateNativeError(res.error) : t('general.unknownError', 'ukjent feil')
    showGlobalError(errMsg)
    return
  }
  showOverlay(opts)
  try { await startMonitoring(opts) }
  catch (err) {
    console.warn('[recording] VU monitor failed (non-fatal):', err)
    try { stopMonitoring() } catch {}
  }
}

// ── Audio error translation ──────────────────────────────────────────────────

// Maps error codes from native-recorder (main process) to user-facing strings
export function translateNativeError(code: string): string {
  switch (code) {
    case 'no_device':
    case 'device_not_found':     return t('recording.errorDeviceNotFound', 'Lydenheten ble ikke funnet — sjekk lydkort og tillatelser')
    case 'device_permission_denied': return t('recording.errorPermission', 'Tilgang til lydenheten ble nektet — sjekk systeminnstillingene')
    case 'device_busy':          return t('recording.errorNotReadable',    'Lydenheten er i bruk av et annet program — lukk DAW eller lydprogram')
    case 'device_error':         return t('recording.errorDeviceError',    'Feil ved åpning av lydenhet — prøv å koble til på nytt')
    case 'already_recording':      return t('recording.errorAlreadyRecording',  'Et opptak er allerede i gang')
    case 'empty_output':           return t('recording.errorEmpty',              'Opptaket er tomt — ingen lyd ble mottatt fra enheten')
    case 'save_folder_permission': return t('recording.errorFolderPermission',   'Ingen tilgang til lagringsmappen — sjekk at mappen er skrivbar')
    case 'save_folder_error':      return t('recording.errorFolderError',        'Kan ikke opprette lagringsmappe — sjekk diskplass og tillatelser')
    case 'device_disconnected':    return t('recording.errorDeviceDisconnected', 'Lydenheten ble koblet fra under opptak — sjekk tilkoblingen')
    case 'disk_full':              return t('recording.errorDiskFull',           'Disken er full — frigjør plass og prøv igjen')
    case 'ffmpeg_missing':         return t('recording.errorFfmpegMissing',      'Intern feil: opptaksbinær mangler — reinstaller appen')
    case 'stuck_recording':        return t('recording.errorStuck',              'Opptaket stoppet — ingen lyd fra enheten i 60 sekunder')
    case 'invalid_opts':           return t('recording.errorInvalidOpts',        'Ugyldige opptaksinnstillinger — start på nytt og prøv igjen')
    case 'no_save_folder':         return t('recording.errorNoSaveFolder',       'Lagringsmappen er ikke valgt — gå til Innstillinger → Opptak')
    default:
      // Unknown error code — show a generic message instead of raw machine code.
      // The technical detail is still logged for diagnostics.
      console.warn('[recording] unknown native error code:', code)
      return t('recording.errorUnknown', 'Noe gikk galt under opptak — sjekk at lydenhet og lagringsmappe er klare')
  }
}

export function showGlobalError(msg: string): void {
  const banner  = document.getElementById('global-error-banner')
  const msgEl   = document.getElementById('global-error-msg')
  const closeEl = document.getElementById('global-error-close')
  if (!banner || !msgEl) return
  msgEl.textContent = msg
  banner.style.display = 'flex'
  if (closeEl && !closeEl.dataset.bound) {
    closeEl.dataset.bound = '1'
    closeEl.addEventListener('click', () => { banner.style.display = 'none' })
  }
  // Navigate to home so user sees the banner
  if (typeof window.showPage === 'function') window.showPage('home')
}

// ── Monitoring stream (VU only) ──────────────────────────────────────────────

/** Release every meter's hold on the shared backend VU feed (home VU,
 *  audio-settings channel grid). Called BEFORE the recorder opens
 *  the device and from the engine-resync path.
 *
 *  Since the renderer stopped owning microphones (audio/vu-feed.ts), this is no
 *  longer the thing that prevents a second device owner — the HARD guarantee is
 *  `start_recording`'s own `vu.stop()`, and the webview holds no input stream at
 *  all any more. What this still buys is the fast path: the engine is released
 *  before the capture engine asks for the device, instead of being torn down
 *  underneath it. */
export function releaseRendererAudioCaptures(): void {
  try { stopHomeVU() } catch {}
  try { stopAudioPageMonitoring() } catch {}
  // NOT the pre-roll buffer. It is a mic owner too, but stopping it HERE would
  // destroy the very thing it exists for: `start_recording` harvests the clip
  // from the running loop (`preroll.is_active()`), and a loop we killed a tick
  // earlier harvests nothing. The hand-over is already correct in Rust — harvest
  // (graceful `q`) → `preroll.stop()` unconditionally → `vu.stop()` → a 400 ms
  // settle → the capture engine opens the device. The renderer's job is only to
  // keep the buffer DOWN while a recording runs; see preroll-lifecycle.ts.
}

/** Time-based release easing for the meter fall: the fraction of the remaining
 *  distance to cover after `dt` ms, for a τ≈80 ms exponential release. Pure —
 *  unit-tested. Kept as a named export (this is where the idea started, and the
 *  overlay's loop reads better with it) but the maths now lives in
 *  audio/smoothing.ts, so every meter in the app shares ONE release law. */
export function easeFallAlpha(dtMs: number): number {
  return alphaFor(dtMs, RELEASE_TAU_MS)
}

/** Meter + waveform + timer WITHOUT RecordingOpts — for sessions the renderer
 *  didn't start itself (scheduler-started recordings, engine resync after a
 *  transient error). The full startMonitoring needs opts it doesn't have; this
 *  lite variant drives everything that runs off `recording://levels`. Fixes
 *  scheduler-started takes showing a dead meter/waveform/timer. */
export function startMonitoringLite(): void {
  recStartTime = Date.now()
  recBytes = 0
  const wfCanvas = document.getElementById('rec-waveform') as HTMLCanvasElement | null
  if (wfCanvas && !recWaveform) {
    recWaveform = new RecordingWaveform(wfCanvas)
    recWaveform.start()
  }
  if (!levelsUnsub) startLevelsMeter()
  if (!recTimerIval) {
    recTimerIval = setInterval(() => {
      if (!isRecording) return
      const elapsed = Math.floor((Date.now() - recStartTime) / 1000)
      const h = Math.floor(elapsed / 3600)
      const m = Math.floor((elapsed % 3600) / 60)
      const s = elapsed % 60
      const timerEl = document.getElementById('rec-timer')
      if (timerEl) timerEl.textContent = `${String(h).padStart(2,'0')}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`
      const sizeEl = document.getElementById('rec-size')
      if (sizeEl) sizeEl.textContent = (recBytes / 1e6).toFixed(1) + ' MB'
    }, 1000)
  }
}

async function startMonitoring(_opts: RecordingOpts): Promise<void> {
  stopHomeVU()
  stopAudioPageMonitoring()
  recStartTime = Date.now()
  recBytes     = 0

  // Premium scrolling waveform — driven by the recording's own level telemetry.
  const wfCanvas = document.getElementById('rec-waveform') as HTMLCanvasElement | null
  if (wfCanvas) {
    recWaveform = new RecordingWaveform(wfCanvas)
    recWaveform.start()
  }

  // Meter + waveform from `recording://levels` (the recording's OWN telemetry),
  // so the device is opened exactly once — nothing else meters it while a take
  // is running. A mono take collapses to one bar.
  startLevelsMeter()

  // Signal check — warn if the input is near-silent 15 s into recording.
  signalCheckTimer = setTimeout(() => {
    signalCheckTimer = null
    if (!isRecording) return
    const quietR = meter.mono || meter.smR <= -55
    if (meter.smL <= -55 && quietR) window.api.notifyWeakSignal()
  }, 15000)

  // Elapsed timer + size display
  recTimerIval = setInterval(() => {
    if (!isRecording) return
    const elapsed = Math.floor((Date.now() - recStartTime) / 1000)
    const h = Math.floor(elapsed / 3600)
    const m = Math.floor((elapsed % 3600) / 60)
    const s = elapsed % 60
    const timerEl = document.getElementById('rec-timer')
    if (timerEl) timerEl.textContent = `${String(h).padStart(2,'0')}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`
    const mb = (recBytes / 1e6).toFixed(1) + ' MB'
    const sizeEl = document.getElementById('rec-size')
    if (sizeEl) sizeEl.textContent = mb
    // The KAMERA badge gets the same growing file size (no separate video-byte
    // event exists) — was stuck at "0 MB" before.
    const camBytesEl = document.getElementById('rec-video-bytes')
    if (camBytesEl) camBytesEl.textContent = mb
  }, 1000)
}

/** Animate the overlay meters + waveform from `recording://levels` (per-channel
 *  peak dBFS from the native capture engine's own ring buffer). Rise is instant,
 *  fall is eased; a null right channel hides the R row and labels the bar "Mono". */
function startLevelsMeter(): void {
  const fillL = document.getElementById('rec-vu-l')
  const pkElL = document.getElementById('rec-vu-peak-l')
  const dbElL = document.getElementById('rec-vu-db-l')
  const fillR = document.getElementById('rec-vu-r')
  const pkElR = document.getElementById('rec-vu-peak-r')
  const dbElR = document.getElementById('rec-vu-db-r')
  const cL    = document.getElementById('rec-vu-clip-l')
  const cR    = document.getElementById('rec-vu-clip-r')
  const rRow  = fillR?.closest('.vu-bar-row') as HTMLElement | null
  const lLbl  = fillL?.closest('.vu-bar-row')?.querySelector('.vu-bar-lbl') as HTMLElement | null

  meter.tL = meter.tR = meter.smL = meter.smR = meter.pkL = meter.pkR = -60
  meter.pkTL = meter.pkTR = 0
  meter.mono = false

  levelsUnsub = window.api.on?.('recording-levels', (payload: unknown) => {
    const d = payload as { peak_db_left?: number; peak_db_right?: number | null } | undefined
    meter.tL = typeof d?.peak_db_left === 'number' ? d.peak_db_left : -60
    const r = d?.peak_db_right
    if (r === null || r === undefined) {
      if (!meter.mono) {
        meter.mono = true
        if (rRow) rRow.style.display = 'none'
        if (lLbl) lLbl.textContent = 'Mono'
      }
      meter.tR = -60
    } else {
      if (meter.mono) {
        meter.mono = false
        if (rRow) rRow.style.display = ''
        if (lLbl) lLbl.textContent = 'L'
      }
      meter.tR = r
    }
  })

  const PEAK_HOLD = 1500, PEAK_FALL = 25
  const hold = (sm: number, pk: number, pt: number, now: number): { p: number; t: number } => {
    if (sm >= pk) return { p: sm, t: now }
    const age = now - pt
    return age > PEAK_HOLD
      ? { p: Math.max(-60, pk - ((age - PEAK_HOLD) / 1000) * PEAK_FALL), t: pt }
      : { p: pk, t: pt }
  }

  let lastLoopTs = performance.now()
  const loop = (): void => {
    if (!isRecording) { meterRaf = 0; return }
    const now = Date.now()
    // Rise instant, fall eased with a TIME-based constant (τ ≈ 80 ms). The old
    // per-frame factor (0.6/0.4) defined the release in FRAMES, so whenever the
    // frame rate dipped the needle's motion law visibly changed — a jank
    // amplifier. exp(−dt/τ) keeps the release identical at any frame rate.
    const nowPerf = performance.now()
    const dt = Math.min(200, nowPerf - lastLoopTs)
    lastLoopTs = nowPerf
    const alpha = easeFallAlpha(dt)
    meter.smL = meter.tL > meter.smL ? meter.tL : meter.smL + (meter.tL - meter.smL) * alpha
    meter.smR = meter.tR > meter.smR ? meter.tR : meter.smR + (meter.tR - meter.smR) * alpha
    const a = hold(meter.smL, meter.pkL, meter.pkTL, now); meter.pkL = a.p; meter.pkTL = a.t
    const b = hold(meter.smR, meter.pkR, meter.pkTR, now); meter.pkR = b.p; meter.pkTR = b.t

    setVUBar(fillL, pkElL, dbElL, meter.smL, meter.pkL)
    if (cL && meter.smL > -0.5 && !cL.classList.contains('clip')) cL.classList.add('clip')
    if (!meter.mono) {
      setVUBar(fillR, pkElR, dbElR, meter.smR, meter.pkR)
      if (cR && meter.smR > -0.5 && !cR.classList.contains('clip')) cR.classList.add('clip')
    }

    updateRecSignalStatus(meter.smL, meter.mono ? meter.smL : meter.smR)
    // A silence warning that outlives the silence is a warning people learn to
    // ignore. The engine emits no "silence over" event, so the meter is the
    // authority: sound is back, the line goes.
    if (silenceShown && Math.max(meter.smL, meter.mono ? -60 : meter.smR) > -50) hideSilenceLine()
    if (recWaveform) {
      const rmsH  = dbToEnvHeight(Math.max(meter.smL, meter.mono ? -60 : meter.smR))
      const peakH = dbToEnvHeight(Math.max(meter.pkL, meter.mono ? -60 : meter.pkR))
      recWaveform.push(peakH, rmsH)
    }
    meterRaf = requestAnimationFrame(loop)
  }
  meterRaf = requestAnimationFrame(loop)
}

function stopLevelsMeter(): void {
  if (meterRaf) { cancelAnimationFrame(meterRaf); meterRaf = 0 }
  if (levelsUnsub) { levelsUnsub(); levelsUnsub = undefined }
}

async function stopMonitoring(): Promise<void> {
  stopLevelsMeter()
  if (recWaveform) { recWaveform.destroy(); recWaveform = null }
  if (recTimerIval)     { clearInterval(recTimerIval);     recTimerIval     = null }
  if (signalCheckTimer) { clearTimeout(signalCheckTimer);  signalCheckTimer = null }
  // Restore the R meter row + label for the next take (mono may have hidden it).
  const rRow = document.getElementById('rec-vu-r')?.closest('.vu-bar-row') as HTMLElement | null
  if (rRow) rRow.style.display = ''
  const lLbl = document.getElementById('rec-vu-l')?.closest('.vu-bar-row')?.querySelector('.vu-bar-lbl') as HTMLElement | null
  if (lLbl) lLbl.textContent = 'L'
}

// ── Finalizing ───────────────────────────────────────────────────────────────
//
// Pressing stop does not end a recording; it ASKS the engine to finalize, and
// the engine then has real work to do — flush the ring buffer, close the
// container, run the truth measurement. The old flow fired stopRecordingNow()
// without awaiting anything and tore the overlay down in the same tick, so the
// user was returned to a home screen that showed no recording, while a file
// was still being written. If that write failed, the error arrived out of
// nowhere; if it succeeded, the recording simply appeared some seconds later.
//
// The overlay now stays up in an explicit finalizing state until a TERMINAL
// engine event arrives (recording://state stopped|failed|idle, or finished, or
// error) — every one of those paths funnels through hideOverlay(), which is
// where the state is cleared.

let finalizing = false
let finalizeTimer: ReturnType<typeof setTimeout> | null = null
/** Paint the stop button from the finalizing state — used both by the
 *  enter/exit transitions and by a language switch (i18n.onLocaleApplied),
 *  which used to reset a mid-finalize «Fullfører opptak …» back to the
 *  default stop label via the data-i18n pass. */
function paintStopButton(): void {
  const btn = document.getElementById('btn-stop-overlay') as HTMLButtonElement | null
  if (!btn) return
  btn.textContent = finalizing
    ? t('recording.finalizing', 'Fullfører opptak …')
    : t('recording.stop', 'Trykk for å stoppe opptaket')
}
/** Long enough that a slow disk finishing a 90-minute FLAC is never cut short;
 *  short enough that a user is not stranded staring at a frozen overlay. The
 *  REAL backstop is Rust-side — this only guarantees the UI can't strand. */
const FINALIZE_TIMEOUT_MS = 30_000

function enterFinalizing(): void {
  if (finalizing) return
  finalizing = true
  document.getElementById('recording-overlay')?.classList.add('is-finalizing')
  const btn = document.getElementById('btn-stop-overlay') as HTMLButtonElement | null
  if (btn) btn.disabled = true
  paintStopButton()
  const hint = document.getElementById('rec-finalizing-hint')
  if (hint) hint.style.display = ''
  // The elapsed clock stops: the take is over, and a timer still counting up
  // would be claiming otherwise. The meters keep running so they can ease down
  // to silence instead of freezing mid-level.
  if (recTimerIval)     { clearInterval(recTimerIval);    recTimerIval = null }
  if (signalCheckTimer) { clearTimeout(signalCheckTimer); signalCheckTimer = null }
  meter.tL = -60
  meter.tR = -60

  finalizeTimer = setTimeout(() => {
    finalizeTimer = null
    if (!finalizing) return
    console.warn('[recording] no terminal event within', FINALIZE_TIMEOUT_MS, 'ms — force-closing the overlay')
    toast('warn', t('recording.finalizeTimeout',
      'Opptaket bruker uvanlig lang tid på å fullføre. Sjekk at fila ble lagret.'))
    stopMonitoring()
      .catch(err => console.error('[recording] stopMonitoring on finalize timeout:', err))
      .finally(() => hideOverlay())
  }, FINALIZE_TIMEOUT_MS)
}

function exitFinalizing(): void {
  if (finalizeTimer) { clearTimeout(finalizeTimer); finalizeTimer = null }
  finalizing = false
  document.getElementById('recording-overlay')?.classList.remove('is-finalizing')
  const btn = document.getElementById('btn-stop-overlay') as HTMLButtonElement | null
  if (btn) btn.disabled = false
  paintStopButton()
  const hint = document.getElementById('rec-finalizing-hint')
  if (hint) hint.style.display = 'none'
}

export async function doStopRecording(): Promise<void> {
  // A second press while the engine is finalizing is not another stop request.
  if (finalizing) return
  enterFinalizing()
  try {
    await window.api.stopRecordingNow()
  } catch (err) {
    console.error('[recording] stopRecordingNow error:', err)
    // The request itself failed, so no terminal event is coming — do the
    // teardown here rather than waiting out the 30 s fallback.
    try { await stopMonitoring() } catch (e) { console.error('[recording] stopMonitoring error:', e) }
    hideOverlay()
  }
}

// ── Overlay / UI state ───────────────────────────────────────────────────────

function showOverlay(opts: RecordingOpts): void {
  isRecording = true
  window.__isRecording = true
  // Pause the invisible home-page animations for the duration (styles.css).
  document.body.classList.add('recording-active')
  // Cancel any pending preview restart and stop home preview (device now used by recorder)
  if (previewRestartTimer) { clearTimeout(previewRestartTimer); previewRestartTimer = null }
  stopVideoPreview()

  // Show overlay video preview if video is configured
  if (opts.videoEnabled && opts.videoDeviceName) {
    const recVideoSection = document.getElementById('rec-video-section')
    const recImg          = document.getElementById('rec-video-preview-img') as HTMLImageElement | null
    const recPh           = document.getElementById('rec-video-placeholder')
    if (recVideoSection) recVideoSection.style.display = ''
    if (recImg)  { recImg.src = ''; recImg.style.display = 'none' }
    if (recPh)   { recPh.textContent = t('home.cameraStarting', 'Starter kamera…'); recPh.style.display = '' }

    recPreviewUnsub?.()
    recVideoDimsSet = false
    // DURING recording the backend recorder owns the camera and writes a low-fps
    // preview JPEG to a file; we POLL it (base64) here. (The old Electron app got
    // IPC frames; the Tauri recorder writes a file instead — a poll is the match.)
    // Poll roughly at the backend preview rate (12 fps → ~83 ms). The cap was
    // 150 ms (~6.7 fps), which threw away half the preview frames and made the
    // image feel laggy even when the backend produced more. (Until v0.15 this
    // derived from the frame-rate setting; the recording is 30 fps now, and
    // the preview JPEG is written at 12 fps regardless.)
    const recPollMs = 83
    // In-flight guard: the tick awaits an IPC round trip, and setInterval does
    // not care whether the previous tick finished. On a slow disk or a busy
    // backend the calls pile up — 12 overlapping reads a second, each decoding
    // base64 on the main thread, all of them racing to set the same img.src.
    // A skipped frame is invisible at 12 fps; a queue of them is not.
    let recPollBusy = false
    const recPollTimer = setInterval(async () => {
      if (recPollBusy) return
      recPollBusy = true
      let b64: string | null = null
      try { b64 = await window.api.recordingPreviewFrame?.() ?? null } catch { b64 = null }
      finally { recPollBusy = false }
      if (!b64 || !recImg) return
      if (!recVideoDimsSet) {
        const bytes = Uint8Array.from(atob(b64.slice(0, 1400)), c => c.charCodeAt(0))
        const dims = readJpegDims(bytes)
        if (dims) {
          recVideoDimsSet = true
          const wrap = document.querySelector<HTMLElement>('.rec-video-wrap')
          if (wrap) wrap.style.setProperty('--rec-video-ar', `${dims.w} / ${dims.h}`)
        }
      }
      recImg.src = `data:image/jpeg;base64,${b64}`
      recImg.style.display = ''
      if (recPh) recPh.style.display = 'none'
    }, recPollMs)
    recPreviewUnsub = () => clearInterval(recPollTimer)
    // (The old 'video-capture-error' listener died with the idle-preview
    // engine: its only emitter was media/preview.rs' `preview://error`, which
    // never fired during recording — the in-recording preview is a file sink.)
  }
  const overlay = document.getElementById('recording-overlay')
  if (overlay) {
    if (opts.videoEnabled && opts.videoDeviceName) {
      // Set BEFORE the reveal: video-active changes the layout, and switching
      // it mid-transition would animate a reflow.
      overlay.classList.add('video-active')
    }
    // Presentation only. showEl reveals in this same frame and the CSS fade
    // runs on top of a fully live overlay — the meters, waveform and timer are
    // started by the caller immediately after, exactly as before.
    showEl(overlay)
  }
  // The sidebar dot/label is NOT written here: it renders `status/next-recording`,
  // which learns about this take from the recorder's own state events. Writing it
  // from both places is what let the sidebar say "Alt er klart" mid-recording
  // (whichever handler ran last won).
  document.getElementById('btn-start-recording')?.classList.add('recording')

  // The deadline comes from the ENGINE, not from the opts this screen was
  // handed: the engine clamps `manual_max_minutes` and is the only thing that
  // will actually stop the take. Ask it directly rather than guessing from
  // `scheduledStopTime`, and every later change arrives on `recording://state`.
  applyScheduledStop(null)
  rehydrateScheduledStop()

  // Device name display
  const deviceEl = document.getElementById('rec-device-name')
  if (deviceEl) {
    deviceEl.textContent = opts.deviceName ?? ''
    getAudioDevices().then(devices => {
      const dev = devices.find(d => d.deviceId === (opts.deviceId ?? settings.deviceId))
      if (deviceEl && dev?.label) deviceEl.textContent = dev.label
    }).catch(() => {})
  }

  // Save path hint
  const pathEl = document.getElementById('rec-savepath')
  if (pathEl) {
    const folder = opts.saveFolder ?? settings.saveFolder ?? ''
    const date   = isoDate(new Date())
    const ext    = opts.format ?? 'mp3'
    let name     = date
    if (opts.customName?.trim()) {
      name = opts.customName.trim().replace(/[/\\:*?"<>|]/g, '_') + '_' + date
    } else if (opts.filenamePattern === 'plain') {
      name = 'gudstjeneste_' + date
    } else if (opts.filenamePattern === 'datetime') {
      name = date + '_' + new Date().toTimeString().slice(0, 5).replace(':', '')
    } else if (opts.overrideName) {
      name = opts.overrideName.replace(/[/\\:*?"<>|]/g, '_') + '_' + date
    }
    pathEl.textContent = `${t('recording.savingAs', 'Lagres som:')} ${folder}/${name}.${ext}`
  }
}

function hideOverlay(): void {
  isRecording = false
  window.__isRecording = false
  document.body.classList.remove('recording-active')
  // Every terminal path lands here — state, finished and error alike — so this
  // is the one place the finalizing state can be cleared without leaving a
  // disabled stop button behind on some branch.
  exitFinalizing()
  hideSilenceLine()
  // Idempotent teardown. The stop press no longer tears the meters down itself
  // (they keep running so they can ease to silence while the engine finalizes),
  // and `recording-finished` can reach this point without having gone through
  // the state handler's stopMonitoring — which would leave the waveform's rAF
  // and the levels subscription running behind a hidden overlay. Calling it
  // twice is a no-op; not calling it is a leak.
  void stopMonitoring().catch(err => console.error('[recording] stopMonitoring on hide:', err))

  // Clean up overlay video preview
  recPreviewUnsub?.(); recPreviewUnsub = undefined
  if (recFrameBlobUrl) { URL.revokeObjectURL(recFrameBlobUrl); recFrameBlobUrl = null }
  recVideoDimsSet = false
  const recVideoSection = document.getElementById('rec-video-section')
  if (recVideoSection) recVideoSection.style.display = 'none'
  const recVideoWrap = document.querySelector<HTMLElement>('.rec-video-wrap')
  if (recVideoWrap) recVideoWrap.style.removeProperty('--rec-video-ar')

  // Restart preview + the home "Lydnivå — live" meter after a short delay —
  // gives time for split auto-restart to cancel it, and lets the capture engine
  // release the audio device before the VU engine asks for it. The home VU was
  // stopped by startMonitoring() and previously NEVER came back after a
  // recording — the meter sat dead until the user re-navigated to home.
  if (previewRestartTimer) clearTimeout(previewRestartTimer)
  previewRestartTimer = setTimeout(() => {
    previewRestartTimer = null
    if (!isRecording) {
      startVideoPreview()
      startHomeVU()
    }
  }, 3000)
  const overlay = document.getElementById('recording-overlay')
  if (overlay) {
    hideEl(overlay)
    // `video-active` is a LAYOUT class; dropping it now would reflow the
    // overlay while it is still fading out. It goes once the element is
    // actually gone (hideEl's own fallback is 220 ms).
    setTimeout(() => { if (!isRecording) overlay.classList.remove('video-active') }, 240)
  }
  scheduledStop = null
  if (schedCntTimer) { clearInterval(schedCntTimer); schedCntTimer = null }
  const autostopEl = document.getElementById('rec-autostop')
  if (autostopEl) autostopEl.style.display = 'none'
  document.getElementById('btn-start-recording')?.classList.remove('recording')
  // Sidebar status: see showOverlay — the store owns that element now.
}

// Bring the overlay back for a session the ENGINE says is live but the UI lost
// track of (e.g. a torn-down overlay after a transient error, before the
// warning/error split existed). Deliberately minimal: no opts are available at
// this point, so the video preview poller and save-path hint stay off — the
// meters, timer, reconnect banner and (crucially) the stop button all work.
function resyncOverlayToLiveSession(): void {
  console.warn('[recording] engine reports a live session while UI was idle — resyncing overlay')
  isRecording = true
  window.__isRecording = true
  // The engine is recording (e.g. scheduler-started or after a transient
  // error) — make sure no renderer-side capture holds the mic alongside it.
  releaseRendererAudioCaptures()
  if (previewRestartTimer) { clearTimeout(previewRestartTimer); previewRestartTimer = null }
  stopVideoPreview()
  const overlay = document.getElementById('recording-overlay')
  if (overlay) showEl(overlay)
  // The sidebar dot/label is NOT written here: it renders `status/next-recording`,
  // which learns about this take from the recorder's own state events. Writing it
  // from both places is what let the sidebar say "Alt er klart" mid-recording
  // (whichever handler ran last won).
  document.getElementById('btn-start-recording')?.classList.add('recording')
  const deviceEl = document.getElementById('rec-device-name')
  if (deviceEl && !deviceEl.textContent) deviceEl.textContent = settings.deviceName ?? ''
  document.body.classList.add('recording-active')
  // Bring the meters/waveform/timer to life — the engine is recording and
  // emitting levels; without this a resynced (or scheduler-started) session
  // showed a dead overlay.
  startMonitoringLite()
  // …and the auto-stop row, read from the engine rather than guessed. The old
  // resync left the countdown blank until the next lifecycle transition, so a
  // scheduler-started take looked like it would run forever.
  rehydrateScheduledStop()
}

/** Show the recording overlay's reconnect banner (#rec-reconnect) — the
 *  device dropped out and the engine's retry policy is running. This element
 *  is now the reconnect's alone: the silence warning has its own line, because
 *  two problems sharing one banner meant the second one erased the first. */
function showReconnectBanner(): void {
  const el = document.getElementById('rec-reconnect')
  if (!el) return
  const textEl      = el.querySelector<HTMLElement>('.rec-reconnect-text')
  const countdownEl = document.getElementById('rec-reconnect-countdown')
  const unitEl      = el.querySelector<HTMLElement>('.rec-reconnect-unit')
  if (textEl) textEl.textContent = t('recording.reconnecting', 'Lydkilde frakoblet — kobler til på nytt')
  if (countdownEl) countdownEl.style.display = ''
  if (unitEl) unitEl.style.display = ''
  el.style.display = 'flex'
}

function hideReconnectBanner(): void {
  const el = document.getElementById('rec-reconnect')
  if (el) el.style.display = 'none'
}

/** The overlay's own silence line (#rec-silence). Cleared automatically once
 *  the meter sees signal again — the engine emits no "silence over" event, and
 *  a warning that outlives its cause is a warning people learn to ignore. */
let silenceShown = false
let silencePaint: (() => string) | null = null
function showSilenceLine(message: () => string): void {
  const el = document.getElementById('rec-silence')
  if (!el) return
  silencePaint = message
  const textEl = document.getElementById('rec-silence-text')
  if (textEl) textEl.textContent = message()
  el.style.display = 'flex'
  silenceShown = true
}

// Språkbytte: repaint the live overlay surfaces from state, after the
// data-i18n pass has re-applied the static defaults.
onLocaleApplied(() => {
  paintStopButton()
  if (silenceShown && silencePaint) {
    const textEl = document.getElementById('rec-silence-text')
    if (textEl) textEl.textContent = silencePaint()
  }
})

function hideSilenceLine(): void {
  if (!silenceShown) return
  silenceShown = false
  const el = document.getElementById('rec-silence')
  if (el) el.style.display = 'none'
}

/** How much "+30 min" adds. One constant so the button label, the invoke and
 *  the toast can never drift apart. */
const EXTEND_MINUTES = 30

/** Run an auto-stop command with the button disabled for the round trip, and
 *  surface a failure instead of leaving the user believing it took. The engine
 *  re-emits `recording://state`, so the countdown updates from the event — this
 *  never writes `scheduledStop` itself. */
async function withAutostopButton(id: string, run: () => Promise<void>): Promise<void> {
  const btn = document.getElementById(id) as HTMLButtonElement | null
  if (btn) btn.disabled = true
  try {
    await run()
  } catch (err) {
    console.error('[recording] auto-stop command failed:', err)
    toast('error', t('recording.autostopFailed',
      'Kunne ikke endre auto-stopp. Opptaket fortsetter — stopp manuelt hvis du må.'))
  } finally {
    if (btn) btn.disabled = false
  }
}

/** Adopt the engine's auto-stop deadline (absolute epoch ms, or null for none)
 *  and re-render the countdown row. The ONE place `scheduledStop` is written. */
function applyScheduledStop(deadlineMs: number | null): void {
  const next = typeof deadlineMs === 'number' && deadlineMs > 0 ? new Date(deadlineMs) : null
  if (next?.getTime() === scheduledStop?.getTime()) return
  scheduledStop = next
  updateScheduledStopUI()
}

/** Ask the engine for its current deadline. Used where no state event is due —
 *  the overlay opening on a manual start, and the resync after a lost session —
 *  so the countdown is right immediately instead of only after the next
 *  lifecycle transition (which may be an hour away). */
function rehydrateScheduledStop(): void {
  window.api.scheduledStopMs()
    .then(ms => applyScheduledStop(ms))
    .catch(err => console.warn('[recording] auto-stop rehydrate failed:', err))
}

function updateScheduledStopUI(): void {
  const section = document.getElementById('rec-autostop')
  if (!section) return
  if (schedCntTimer) { clearInterval(schedCntTimer); schedCntTimer = null }
  if (!scheduledStop) { section.style.display = 'none'; return }
  section.style.display = 'flex'
  updateScheduledStopCountdown()
  // A display timer only — the engine owns the stop. Nothing in this file can
  // end a recording on a clock any more.
  schedCntTimer = setInterval(updateScheduledStopCountdown, 1000)
}

function updateScheduledStopCountdown(): void {
  const el = document.getElementById('rec-autostop-countdown')
  if (!el || !scheduledStop) return
  const diff = scheduledStop.getTime() - Date.now()
  el.textContent = diff > 0 ? fmtCountdown(diff) : '—'
}

let lastSigCls = '§init§' // sentinel ≠ any real class so the first call writes
function updateRecSignalStatus(dbL: number, dbR: number): void {
  const db  = Math.max(dbL, dbR)
  const dot = document.getElementById('rec-sig-dot')
  const lbl = document.getElementById('rec-sig-label')
  if (!dot || !lbl) return
  let cls = '', text = '—'
  if      (db >= -3)  { cls = 'klipping'; text = t('recording.sigClipping', 'KLIPPING') }
  else if (db >= -12) { cls = 'hoyt';     text = t('recording.sigHigh',     'HØYT')     }
  else if (db >= -40) { cls = 'god';      text = t('recording.sigGood',     'GOD')      }
  else if (db > -55)  { cls = 'svak';     text = t('recording.sigWeak',     'SVAK')     }
  // Called from the 60 fps meter loop — only touch the DOM when the tier
  // actually flips (className/textContent writes dirty style + layout).
  if (cls === lastSigCls) return
  lastSigCls = cls
  dot.className  = 'rec-sig-dot'   + (cls ? ' ' + cls : '')
  lbl.className  = 'rec-sig-label' + (cls ? ' ' + cls : '')
  lbl.textContent = text
}
