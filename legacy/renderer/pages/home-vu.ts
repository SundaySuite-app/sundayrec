import { settings } from '../state'
import { buildInputRouter, rVuChannel } from '../audio/capture'
import { makeVuState, tickVU, stopVuState } from '../audio/vu'
import type { VuState } from '../audio/vu'
import { t } from '../i18n'

const vu = makeVuState()
let vuRetries = 0
const MAX_VU_RETRIES = 5
// The gUM-failure retry timer (see tryStartVU's .catch). Must be cancelled by
// stopVU() — otherwise a retry fired 5 s after a failed open could reopen the
// mic mid-recording: #vu-l always exists (pages are display-toggled, not
// removed), so the old guard `!vu.stream && document.getElementById('vu-l')`
// never actually blocked a retry once the recorder's ffmpeg was the one
// holding the device (single-mic-owner invariant, 2026-07-31 audit).
let vuRetryTimer: ReturnType<typeof setTimeout> | null = null

export function stopVU(): void {
  if (vuRetryTimer) { clearTimeout(vuRetryTimer); vuRetryTimer = null }
  stopVuState(vu)
  const fills = ['vu-l', 'vu-r'].map(id => document.getElementById(id))
  const peaks = ['vu-peak-l', 'vu-peak-r'].map(id => document.getElementById(id))
  const dbs   = ['vu-db-l', 'vu-db-r'].map(id => document.getElementById(id))
  // The fill is a transform-driven mask now (audio/vu.ts) — resetting `width`
  // left the stale scaleX in place, freezing the home bars after a recording.
  fills.forEach(el => { if (el) el.style.transform = 'scaleX(1)' })
  peaks.forEach(el => { if (el) el.style.opacity = '0' })
  dbs.forEach(el   => { if (el) el.textContent = '—' })
  resetSignalStatus()
}

export function startVU(): void {
  stopVU()
  vuRetries = 0
  tryStartVU()
}

function tryStartVU(): void {
  if (!document.getElementById('vu-l')) return

  const devId = settings.deviceId && settings.deviceId !== 'default' ? settings.deviceId : null
  const devChannels = settings.deviceId ? (settings.deviceChannels?.[settings.deviceId] ?? null) : null
  const chL  = devChannels?.channelL ?? 0
  const chR  = devChannels?.channelR ?? 1
  const need = Math.max(2, chL + 1, chR + 1)

  navigator.mediaDevices.getUserMedia({
    audio: {
      ...(devId ? { deviceId: { ideal: devId } } : {}),
      channelCount:     { ideal: need },
      echoCancellation: false,
      noiseSuppression: false,
      autoGainControl:  false
    },
    video: false
  })
    .then(stream => {
      vuRetries = 0
      vu.stream = stream
      vu.ctx    = new AudioContext()
      const src    = vu.ctx.createMediaStreamSource(stream)
      const routed = buildInputRouter(vu.ctx, src, stream, chL, chR)
      const split  = vu.ctx.createChannelSplitter(2)
      vu.analyserL = vu.ctx.createAnalyser(); vu.analyserL.fftSize = 1024
      vu.analyserR = vu.ctx.createAnalyser(); vu.analyserR.fftSize = 1024
      routed.connect(split)
      split.connect(vu.analyserL, 0)
      // Mirror mono → R so the R meter isn't dead on a mono mic (see rVuChannel).
      split.connect(vu.analyserR, rVuChannel(stream))

      const fillL = document.getElementById('vu-l')
      const pkL   = document.getElementById('vu-peak-l')
      const dbL   = document.getElementById('vu-db-l')
      const fillR = document.getElementById('vu-r')
      const pkR   = document.getElementById('vu-peak-r')
      const dbR   = document.getElementById('vu-db-r')
      const clipL = document.getElementById('vu-clip-l')
      const clipR = document.getElementById('vu-clip-r')

      tickVU(vu, fillL, pkL, dbL, fillR, pkR, dbR, (dbL, dbR, state) => {
        updateSignalStatus(dbL, dbR, state)
        if (clipL && state.smL > -0.5) clipL.classList.add('clip')
        if (clipR && state.smR > -0.5) clipR.classList.add('clip')
      })
    })
    .catch(() => {
      vuRetries++
      if (vuRetries < MAX_VU_RETRIES) {
        vuRetryTimer = setTimeout(() => {
          vuRetryTimer = null
          // Also bail if a recording started while we were waiting — the
          // recorder's ffmpeg now owns the mic (single-mic-owner invariant).
          if (!vu.stream && !window.__isRecording && document.getElementById('vu-l')) tryStartVU()
        }, 5000)
      }
    })
}

// Write cache for the signal line. updateSignalStatus runs at 60 Hz off the VU
// tick and used to write className on three elements and textContent on two —
// every frame, forever, for a value that changes a few times a minute. Each
// write dirties style and layout; the cache turns the steady state into five
// comparisons and zero DOM touches. `§init§` is a sentinel that can never equal
// a real class, so the first call after a (re)start always paints.
const CLS_INIT = '§init§'
let lastSigCls  = CLS_INIT
let lastPeakTxt = CLS_INIT
let lastPeakAt  = 0
/** The peak readout falls at 25 dB/s, i.e. it would change its 0.1-dB string on
 *  nearly every frame. Throttled like the dB text in audio/vu.ts. */
const PEAK_TEXT_MIN_INTERVAL_MS = 150

function resetSignalStatus(): void {
  const dot  = document.getElementById('signal-dot')
  const homeDot = document.getElementById('home-device-signal')
  const text = document.getElementById('signal-text')
  const peak = document.getElementById('signal-peak')
  if (dot)  dot.className = 'signal-dot'
  if (homeDot) homeDot.className = 'signal-dot'
  if (text) { text.className = 'signal-text'; text.textContent = '—' }
  if (peak) peak.textContent = ''
  lastSigCls = CLS_INIT
  lastPeakTxt = CLS_INIT
  lastPeakAt = 0
}

function updateSignalStatus(dbL: number, dbR: number, state: VuState): void {
  const db = Math.max(dbL, dbR)
  let cls = '', label = '—'
  if      (db >= -3)  { cls = 'klipping'; label = t('home.signalClipping', 'Klipper!') }
  else if (db >= -12) { cls = 'hoyt';     label = t('home.signalLoud',     'Høyt')     }
  else if (db >= -40) { cls = 'god';      label = t('home.signalGood',     'Bra')      }
  else if (db > -55)  { cls = 'svak';     label = t('home.signalWeak',     'Svakt')    }

  if (cls !== lastSigCls) {
    const dot  = document.getElementById('signal-dot')
    const text = document.getElementById('signal-text')
    if (!dot || !text) return
    lastSigCls = cls
    const suffix = cls ? ' ' + cls : ''
    dot.className  = 'signal-dot' + suffix
    text.className = 'signal-text' + suffix
    text.textContent = label
    const homeDot = document.getElementById('home-device-signal')
    if (homeDot) homeDot.className = 'signal-dot' + suffix
  }

  const now = performance.now()
  if (now - lastPeakAt < PEAK_TEXT_MIN_INTERVAL_MS) return
  const pkMax = Math.max(state.peakL, state.peakR)
  const peakTxt = pkMax > -59 ? `Maks: ${pkMax.toFixed(1)} dBFS` : ''
  if (peakTxt === lastPeakTxt) return
  lastPeakTxt = peakTxt
  lastPeakAt = now
  const peak = document.getElementById('signal-peak')
  if (peak) peak.textContent = peakTxt
}

// Click clip indicators to reset
export function setupClipReset(): void {
  ;['vu-clip-l', 'vu-clip-r', 'rec-vu-clip-l', 'rec-vu-clip-r', 'live-vu-clip-l', 'live-vu-clip-r'].forEach(id =>
    document.getElementById(id)?.addEventListener('click', () =>
      document.getElementById(id)?.classList.remove('clip'))
  )
}
