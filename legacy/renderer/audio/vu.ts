/**
 * VU meter — the shared motion + paint layer for the Hjem and Direkte meters.
 *
 * This module used to own a Web Audio graph: a `getUserMedia` stream, an
 * `AudioContext`, two `AnalyserNode`s and a 60 Hz `requestAnimationFrame` loop
 * that computed RMS off the float time domain (`getDbFS`). All of that is gone.
 * The levels now arrive from the Rust VU engine as `vu://levels` packets
 * (audio/vu-feed.ts) — the renderer never opens an input device, which is what
 * closes the Qu-5 class of bug: a webview-held CoreAudio input that stayed
 * pinned to gUM's 2-channel format long after the meter was "stopped", so the
 * recorder opened a 32-channel mixer and got a stereo pair (2026-07-31).
 *
 * What survives is everything that was never about owning a microphone: the
 * frame-rate-independent smoothing, the peak hold, and the dirty-checked DOM
 * writes that keep a 30 Hz meter from reflowing the page.
 */

import { createLevelSmoother } from './smoothing'
import type { LevelSmoother } from './smoothing'

export interface VuState {
  /** Peak-hold marker position + the timestamp it was set. */
  pkL: number; pkR: number
  pkTL: number; pkTR: number
  /** The smoothed bar level — the public readout. */
  smL: number; smR: number
  /** Session maximum (the «Maks: … dBFS» line), a TRUE peak. */
  peakL: number; peakR: number
  /** dt-based release smoothers (audio/smoothing.ts). smL/smR stay the public
   *  readout; these own the motion. */
  smootherL: LevelSmoother
  smootherR: LevelSmoother
  /** performance.now() of the previous packet, for the smoother's dt. */
  lastTickMs: number
}

const PEAK_HOLD = 1500   // ms
const PEAK_FALL = 25     // dB/sec
const FLOOR_DB  = -60

export function makeVuState(): VuState {
  return {
    pkL: FLOOR_DB, pkR: FLOOR_DB, pkTL: 0, pkTR: 0,
    smL: FLOOR_DB, smR: FLOOR_DB, peakL: FLOOR_DB, peakR: FLOOR_DB,
    smootherL: createLevelSmoother(), smootherR: createLevelSmoother(), lastTickMs: 0,
  }
}

function dbToHeight(db: number): number {
  return ((Math.max(FLOOR_DB, Math.min(0, db)) + 60) / 60) * 100
}

function computePeak(db: number, peak: number, pt: number, now: number): { p: number; t: number } {
  if (db >= peak) return { p: db, t: now }
  const age = now - pt
  if (age > PEAK_HOLD) return { p: Math.max(FLOOR_DB, peak - (age - PEAK_HOLD) / 1000 * PEAK_FALL), t: pt }
  return { p: peak, t: pt }
}

/**
 * Advance the meter by ONE `vu://levels` packet.
 *
 * The two level pairs come from the same packet but say different things, and
 * the meter needs both: `rms*` is the bar (what the old `getDbFS` measured),
 * `peak*` is the hold marker, the clip light and the session maximum. The old
 * code had only the RMS and drove the "peak" marker off the smoothed RMS —
 * which is why the clip indicators were effectively dead: an RMS above
 * −0.5 dBFS needs a near-square wave.
 *
 * Called at packet rate (~30 Hz), NOT at frame rate: the smoothing law is
 * `1 − exp(−dt/τ)`, so stepping it on packet arrival is exactly as correct as
 * stepping it per frame, and it keeps the motion independent of how often the
 * painter happens to run.
 */
export function pushVuLevels(
  state: VuState,
  rmsL: number, rmsR: number,
  peakL: number, peakR: number,
  nowPerf: number = performance.now(),
  nowMs: number = Date.now(),
): void {
  const dt = state.lastTickMs ? nowPerf - state.lastTickMs : 33
  state.lastTickMs = nowPerf
  state.smL = state.smootherL.step(rmsL, dt)
  state.smR = state.smootherR.step(rmsR, dt)
  const pL = computePeak(peakL, state.pkL, state.pkTL, nowMs)
  const pR = computePeak(peakR, state.pkR, state.pkTR, nowMs)
  state.pkL = pL.p; state.pkTL = pL.t; state.pkR = pR.p; state.pkTR = pR.t
  if (peakL > state.peakL) state.peakL = peakL
  if (peakR > state.peakR) state.peakR = peakR
}

// Per-element write cache so a caller only touches the DOM when a value
// actually changed. The fill is animated with `transform: scaleX(...)` (GPU
// composite, no layout); the peak marker's `left` and the dB text DO cause
// layout, so they are quantized/throttled — that combination is what makes the
// recording overlay smooth (it used to set style.width + textContent on every
// frame, fighting a CSS width-transition and reflowing the page 60×/s).
interface VuWriteState {
  fillScale: number
  peakPx: number
  peakVisible: boolean
  text: string
  textAt: number
  /** Cached parent-track width in px (peak translateX basis); refreshed lazily. */
  trackW: number
  trackWAt: number
}
const vuWrites = new WeakMap<HTMLElement, VuWriteState>()
const DB_TEXT_MIN_INTERVAL_MS = 150
const TRACK_W_REFRESH_MS = 1000

function writeState(el: HTMLElement): VuWriteState {
  let s = vuWrites.get(el)
  if (!s) {
    s = { fillScale: -1, peakPx: -1, peakVisible: false, text: '', textAt: 0, trackW: 0, trackWAt: 0 }
    vuWrites.set(el, s)
  }
  return s
}

export function setVUBar(
  fillEl: HTMLElement | null,
  peakEl: HTMLElement | null,
  dbEl:   HTMLElement | null,
  db: number, peakDb: number
): void {
  // .vu-bar-fill is a *mask* anchored right: it covers the right side of the
  // gradient track, leaving audioPct% visible from the left. scaleX with
  // `transform-origin: right` (styles.css) shrinks the mask without layout.
  const audioPct = dbToHeight(db)           // 0..100, 100 = loudest
  const peakPct  = dbToHeight(peakDb)       // 0..100
  if (fillEl) {
    // ~0.1% granularity — invisible steps, but skips no-op writes at silence.
    const scale = Math.round((100 - audioPct) * 10) / 1000
    const s = writeState(fillEl)
    if (s.fillScale !== scale) {
      s.fillScale = scale
      fillEl.style.transform = `scaleX(${scale})`
    }
  }
  if (peakEl) {
    const s = writeState(peakEl)
    // transform: translateX — GPU composite, NO layout. The old `left: %` write
    // reflowed on every frame during the peak release (25 dB/s fall = ~0.7 %
    // per frame, defeating any coarse bucketing) ×2 bars ×60 Hz. translateX
    // needs the track width in px; cache it and refresh lazily (a resize is
    // corrected within a second, invisible on a 2 px marker).
    const now = performance.now()
    if (now - s.trackWAt >= TRACK_W_REFRESH_MS) {
      s.trackWAt = now
      s.trackW = peakEl.parentElement?.clientWidth ?? s.trackW
    }
    const px = Math.round((peakPct / 100) * s.trackW)
    if (s.peakPx !== px) {
      s.peakPx = px
      peakEl.style.transform = `translateX(${px}px)`
    }
    const visible = peakDb > -59
    if (s.peakVisible !== visible) {
      s.peakVisible = visible
      peakEl.style.opacity = visible ? '1' : '0'
    }
  }
  if (dbEl) {
    const s = writeState(dbEl)
    const now = performance.now()
    if (now - s.textAt >= DB_TEXT_MIN_INTERVAL_MS) {
      const text = db > -59 ? db.toFixed(1) : '—'
      if (s.text !== text) {
        s.text = text
        s.textAt = now
        dbEl.textContent = text
      }
    }
  }
}

/** Paint both bars from the current state. Cheap by construction (setVUBar is
 *  dirty-checked), so a caller may run it per frame or per packet. */
export function paintVuPair(
  state: VuState,
  fillL: HTMLElement | null, peakL: HTMLElement | null, dbL: HTMLElement | null,
  fillR: HTMLElement | null, peakR: HTMLElement | null, dbR: HTMLElement | null,
): void {
  setVUBar(fillL, peakL, dbL, state.smL, state.pkL)
  setVUBar(fillR, peakR, dbR, state.smR, state.pkR)
}

/** Back to silence. No stream or context to close any more — the state IS the
 *  meter now. */
export function stopVuState(state: VuState): void {
  state.pkL = FLOOR_DB; state.pkR = FLOOR_DB; state.pkTL = 0; state.pkTR = 0
  state.smL = FLOOR_DB; state.smR = FLOOR_DB; state.peakL = FLOOR_DB; state.peakR = FLOOR_DB
  state.smootherL.reset(); state.smootherR.reset()
  state.lastTickMs = 0
}
