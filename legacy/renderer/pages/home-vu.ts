/**
 * The Hjem page's «Lydnivå — live» meter.
 *
 * Fed by the shared backend VU feed (audio/vu-feed.ts), NOT by a renderer-side
 * microphone. It used to open its own `getUserMedia` stream + AudioContext,
 * which made the webview a second owner of the input device — the Qu-5 class of
 * bug (a webview-held CoreAudio input stuck in gUM's 2-channel format long
 * after the meter was "stopped", so the recorder opened a 32-channel mixer and
 * got a stereo pair; 2026-07-31). The mic now has exactly one owner in this
 * process: the Rust engine.
 *
 * Two consequences worth knowing:
 *  - The bars show the channels the RECORDING will use (the mode + the channel
 *    grid's stored L/R picks, routed exactly like the capture engine routes
 *    them — see vu-feed-core.pickLR), not "whatever the default input is".
 *  - The old 5×5 s retry loop is gone: the feed owns the retry, and a failed
 *    open no longer leaves a timer that could reopen the mic mid-recording.
 */

import { settings } from '../state'
import { acquireVuFeed } from '../audio/vu-feed'
import { pickLR } from '../audio/vu-feed-core'
import { makeVuState, paintVuPair, pushVuLevels, stopVuState } from '../audio/vu'
import type { VuState } from '../audio/vu'
import type { VuPick } from '../audio/vu-feed'
import { t } from '../i18n'

const vu = makeVuState()
/** Release handle for the feed subscription; non-null ⇔ the meter is running. */
let release: (() => void) | null = null
let raf = 0

/** The channels this meter should show — the same source the recorder reads:
 *  the mode radio's persisted value plus the channel grid's stored L/R picks
 *  for the selected device. */
function currentPick(): VuPick {
  const devChannels = settings.deviceId ? (settings.deviceChannels?.[settings.deviceId] ?? null) : null
  return {
    mode: settings.channels ?? 'stereo',
    chL: devChannels?.channelL ?? 0,
    chR: devChannels?.channelR ?? 1,
  }
}

export function stopVU(): void {
  if (release) {
    const r = release
    release = null
    try { r() } catch { /* gone */ }
  }
  if (raf) { cancelAnimationFrame(raf); raf = 0 }
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
  if (!document.getElementById('vu-l')) return

  release = acquireVuFeed({
    deviceName: settings.deviceName ?? null,
    pick: currentPick,
    onLevels: (l, r, levels) => {
      // The bar is the RMS (what the old getDbFS measured); the hold marker,
      // the clip light and the «Maks» readout are the TRUE peak.
      const p = currentPick()
      const pk = pickLR(levels.peak_dbfs, p.mode, p.chL, p.chR)
      pushVuLevels(vu, l, r, pk.l, pk.r)
      if (!raf) raf = requestAnimationFrame(paint)
    },
  })
}

/** rAF-coalesced paint: the packets arrive at ~30 Hz, so this runs at most once
 *  per frame and usually once per packet. */
function paint(): void {
  raf = 0
  const fillL = document.getElementById('vu-l')
  const pkL   = document.getElementById('vu-peak-l')
  const dbL   = document.getElementById('vu-db-l')
  const fillR = document.getElementById('vu-r')
  const pkR   = document.getElementById('vu-peak-r')
  const dbR   = document.getElementById('vu-db-r')
  paintVuPair(vu, fillL, pkL, dbL, fillR, pkR, dbR)
  updateSignalStatus(vu.smL, vu.smR, vu)
  // Clip is a PEAK verdict — the old RMS-based check (smL > −0.5) could
  // essentially never fire, so these lights were dead decoration.
  if (vu.pkL > -0.5) document.getElementById('vu-clip-l')?.classList.add('clip')
  if (vu.pkR > -0.5) document.getElementById('vu-clip-r')?.classList.add('clip')
}

// Write cache for the signal line. updateSignalStatus runs off every paint and
// used to write className on three elements and textContent on two — every
// frame, forever, for a value that changes a few times a minute. Each write
// dirties style and layout; the cache turns the steady state into five
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
  const peakTxt = pkMax > -59 ? `${t('home.peakLabel', 'Maks')}: ${pkMax.toFixed(1)} dBFS` : ''
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
