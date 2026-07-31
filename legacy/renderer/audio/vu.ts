/**
 * VU meter — shared logic for both the home page and the recording overlay.
 */

export interface VuState {
  animFrame:   number | null
  stream:      MediaStream | null
  ctx:         AudioContext | null
  analyserL:   AnalyserNode | null
  analyserR:   AnalyserNode | null
  pkL: number; pkR: number
  pkTL: number; pkTR: number
  smL: number; smR: number
  peakL: number; peakR: number
  /** Pre-allocated read buffers — sized to fftSize once when analyser is set,
   *  reused every frame so the 60 Hz tick produces zero GC pressure. */
  bufL: Float32Array | null
  bufR: Float32Array | null
}

const SMOOTH    = 0.55
const PEAK_HOLD = 1500   // ms
const PEAK_FALL = 25     // dB/sec

export function makeVuState(): VuState {
  return { animFrame: null, stream: null, ctx: null, analyserL: null, analyserR: null,
    pkL: -60, pkR: -60, pkTL: 0, pkTR: 0, smL: -60, smR: -60, peakL: -60, peakR: -60,
    bufL: null, bufR: null }
}

/**
 * Computes dBFS from an analyser using a caller-provided buffer. The buffer
 * MUST be at least `analyser.fftSize` long. Reusing the same buffer per
 * frame avoids ~4 KB allocation at 60 fps × 2 channels = ~480 KB/sec of GC
 * pressure that previously surfaced as occasional jank.
 */
export function getDbFS(analyser: AnalyserNode, buf?: Float32Array): number {
  const sampleBuf = buf && buf.length >= analyser.fftSize ? buf : new Float32Array(analyser.fftSize)
  // Cast away the ArrayBufferLike↔ArrayBuffer generic variance the DOM lib
  // introduced (TS 5.7): the buffer is a real Float32Array at runtime regardless.
  analyser.getFloatTimeDomainData(sampleBuf as Float32Array<ArrayBuffer>)
  const len = analyser.fftSize
  let sum = 0
  for (let i = 0; i < len; i++) sum += sampleBuf[i] * sampleBuf[i]
  const rms = Math.sqrt(sum / len)
  return rms > 0 ? Math.max(-60, 20 * Math.log10(rms)) : -60
}

function dbToHeight(db: number): number {
  return ((Math.max(-60, Math.min(0, db)) + 60) / 60) * 100
}

function computePeak(db: number, peak: number, pt: number, now: number): { p: number; t: number } {
  if (db >= peak) return { p: db, t: now }
  const age = now - pt
  if (age > PEAK_HOLD) return { p: Math.max(-60, peak - (age - PEAK_HOLD) / 1000 * PEAK_FALL), t: pt }
  return { p: peak, t: pt }
}

// Per-element write cache so a 60 fps caller only touches the DOM when a value
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

export function tickVU(
  state: VuState,
  fillL: HTMLElement | null, peakL: HTMLElement | null, dbL: HTMLElement | null,
  fillR: HTMLElement | null, peakR: HTMLElement | null, dbR: HTMLElement | null,
  onSignal?: (dbL: number, dbR: number, state: VuState) => void
): void {
  if (!state.analyserL || !state.analyserR) return
  // Lazily size the reusable sample buffers to match the analyser fftSize.
  // analyser.fftSize is constant for the lifetime of the analyser, so this
  // allocates exactly once per (re)attach.
  if (!state.bufL || state.bufL.length !== state.analyserL.fftSize) {
    state.bufL = new Float32Array(state.analyserL.fftSize)
  }
  if (!state.bufR || state.bufR.length !== state.analyserR.fftSize) {
    state.bufR = new Float32Array(state.analyserR.fftSize)
  }
  const now  = Date.now()
  const rawL = getDbFS(state.analyserL, state.bufL), rawR = getDbFS(state.analyserR, state.bufR)
  state.smL = rawL > state.smL ? rawL : state.smL * SMOOTH + rawL * (1 - SMOOTH)
  state.smR = rawR > state.smR ? rawR : state.smR * SMOOTH + rawR * (1 - SMOOTH)
  const pL = computePeak(state.smL, state.pkL, state.pkTL, now)
  const pR = computePeak(state.smR, state.pkR, state.pkTR, now)
  state.pkL = pL.p; state.pkTL = pL.t; state.pkR = pR.p; state.pkTR = pR.t
  if (state.smL > state.peakL) state.peakL = state.smL
  if (state.smR > state.peakR) state.peakR = state.smR
  setVUBar(fillL, peakL, dbL, state.smL, state.pkL)
  setVUBar(fillR, peakR, dbR, state.smR, state.pkR)
  onSignal?.(state.smL, state.smR, state)
  state.animFrame = requestAnimationFrame(() =>
    tickVU(state, fillL, peakL, dbL, fillR, peakR, dbR, onSignal))
}

export function stopVuState(state: VuState): void {
  if (state.animFrame)  { cancelAnimationFrame(state.animFrame); state.animFrame = null }
  if (state.stream)     { state.stream.getTracks().forEach(t => t.stop()); state.stream = null }
  if (state.ctx)        { state.ctx.close(); state.ctx = null }
  state.analyserL = null; state.analyserR = null
  state.bufL = null; state.bufR = null
  state.pkL = -60; state.pkR = -60; state.pkTL = 0; state.pkTR = 0
  state.smL = -60; state.smR = -60; state.peakL = -60; state.peakR = -60
}
