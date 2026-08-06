/**
 * The live channel grid — "see the signal, tap to choose".
 *
 * Replaces the old L/R dropdowns + blind 3-second scan + speaker-loop "Test
 * lyd". The Rust VU engine (`start_vu` → `vu-levels` every 33 ms, one peak+RMS
 * entry per NATIVE device channel — a Qu-5's 32, not getUserMedia's 2) drives
 * one vertical meter per channel; channels carrying signal light up
 * continuously, and the user taps the lit columns to assign LEFT/RIGHT.
 *
 * Ownership rules (the 2026-07-31 device-contention lessons):
 *  - The grid's VU stream is stopped before EVERY other device open — the
 *    backend guarantees it (start_recording/bench/test/scan/probe all call
 *    `vu.stop()`), and the renderer stops eagerly as a fast path.
 *  - Persistence happens ONLY in the explicit tap handler (never from hidden
 *    DOM state) — the structural replacement for the old `pickerActive` guard
 *    that fixed the phantom L=0/R=0 mapping (2026-07-31).
 */

import { t } from '../i18n'
import { settings, patchSettings } from '../state'
import { createLevelSmoother } from '../audio/smoothing'
import type { LevelSmoother } from '../audio/smoothing'
import type { ChannelMode } from '../../types'

// Pure decisions live in channel-grid-logic.ts (unit-tested without DOM).
import {
  armedForMode,
  dbToFraction,
  nextAssignment,
  nextSignalState,
} from './channel-grid-logic'

// ── Grid state ───────────────────────────────────────────────────────────────

type GridPhase = 'idle' | 'loading' | 'live' | 'fallback'

interface Col {
  root: HTMLButtonElement
  /** Top-anchored mask over the track's fixed gradient — scaleY(1 − level).
   *  The gradient must stay pinned to the track so a quiet channel shows only
   *  the green base, never a compressed mini-gradient with a red top. */
  mask: HTMLElement
  num: HTMLElement
  badge: HTMLElement
  signal: boolean
  /** Per-channel dt-based release (audio/smoothing.ts). The bar used to be
   *  smoothed by a CSS `transition: transform .05s` — restarted by the rAF
   *  writer on every single frame, which is the "CSS transition fights the JS
   *  writer" antipattern the VU bars shed in v0.5.0. The motion belongs to
   *  exactly one owner, and it is this one. */
  level: LevelSmoother
}

const state = {
  phase: 'idle' as GridPhase,
  deviceId: null as string | null,
  deviceName: null as string | null,
  channels: 0,
  assign: { l: 0, r: 1, armed: 'l' as 'l' | 'r' },
  cols: [] as Col[],
  chLabels: [] as string[], // ASIO driver labels (sparse ok)
  unlisten: null as (() => void) | null,
  raf: 0,
  latest: null as { peak_dbfs: (number | null)[] } | null,
  /** Generation token: every (re)start bumps it so stale async completions
   *  from a previous device can't clobber the current grid. */
  gen: 0,
  onCount: null as ((count: number) => void) | null,
  /** performance.now() of the last paint — the smoothers' dt. */
  lastPaintMs: 0,
}

// ── Public lifecycle ─────────────────────────────────────────────────────────

/** One-time wiring: chips, grid tap delegation, mode-radio coupling, scan
 *  fallback. `onCount` lets audio-page update the device sub-line + auto-mono
 *  from the authoritative channel count. */
export function setupChannelGrid(onCount: (count: number) => void): void {
  state.onCount = onCount
  document.getElementById('ch-grid')?.addEventListener('click', (e) => {
    const btn = (e.target as HTMLElement).closest('.ch-col') as HTMLButtonElement | null
    if (!btn || state.channels <= 2) return
    onTapColumn(Number(btn.dataset.ch ?? 0))
  })
  document.getElementById('ch-assign-row')?.addEventListener('click', (e) => {
    const chip = (e.target as HTMLElement).closest('.ch-assign-chip') as HTMLElement | null
    if (!chip) return
    const slot = chip.dataset.slot as 'l' | 'r' | undefined
    if (slot && armedForMode(currentMode(), slot) === slot) {
      state.assign = { ...state.assign, armed: slot }
      renderChips()
      renderBadges()
    }
  })
  // Mode radios live in the same card — re-render chips/badges on change so
  // the mono/stereo coupling is visible instantly (persistence is audio-page's
  // existing autoSave on the same radios).
  document.querySelectorAll<HTMLInputElement>('input[name="channels"]').forEach(r => {
    r.addEventListener('change', () => {
      state.assign = { ...state.assign, armed: armedForMode(currentMode(), state.assign.armed) }
      renderChips()
      renderBadges()
    })
  })
  setupScanFallback()
}

/** Start (or restart) the grid for a device. Instant skeleton from the cpal
 *  device list, then the VU engine's negotiated count is the authority. */
export async function startChannelGrid(deviceId: string, deviceName: string | null): Promise<void> {
  const gen = ++state.gen
  state.deviceId = deviceId
  state.deviceName = deviceName
  state.phase = 'loading'
  const stored = settings.deviceChannels?.[deviceId]
  state.assign = {
    l: stored?.channelL ?? 0,
    r: stored?.channelR ?? 1,
    armed: armedForMode(currentMode(), 'l'),
  }
  state.chLabels = []
  setStatus(t('audio.gridConnecting', 'Kobler til lydkortet …'))
  showFallbackRow(false)

  // Instant skeleton: real channel count from cpal (no ffmpeg spawn). The
  // stored Web Audio label can differ from the cpal name — loose match, and
  // the VU engine's own fuzzy resolution is the authority anyway.
  let skeletonCount = 8
  if (deviceId.startsWith('asio::')) {
    const chans = await window.api.listAsioInputChannels(deviceId.slice('asio::'.length)).catch(() => [])
    if (gen !== state.gen) return
    if (chans.length > 0) {
      skeletonCount = chans.length
      state.chLabels = chans.map(c => c.label)
    }
  } else if (deviceName) {
    const list = await window.api.listInputDevices().catch(() => null)
    if (gen !== state.gen) return
    const hit = list?.inputs.find(d =>
      d.name.toLowerCase().includes(deviceName.toLowerCase().slice(0, 8)) ||
      deviceName.toLowerCase().includes(d.name.toLowerCase().slice(0, 8)))
    if (hit && hit.channels > 0) skeletonCount = hit.channels
  }
  state.channels = skeletonCount
  renderGrid(true)

  // Live: the VU engine negotiates the FULL channel count and starts the
  // 30 Hz vu-levels feed. One retry after 400 ms — the device may still be
  // settling out of a just-released WebKit/gUM format hold.
  try {
    let count: number
    try {
      count = await window.api.startVu(deviceName)
    } catch {
      await new Promise<void>(r => setTimeout(r, 400))
      if (gen !== state.gen) return
      count = await window.api.startVu(deviceName)
    }
    if (gen !== state.gen) { void window.api.stopVu(); return }
    state.channels = count
    state.phase = 'live'
    renderGrid(false)
    subscribe()
    setStatus(t('audio.gridMeta', '{n} kanaler — live signal')
      .replace('{n}', String(count)))
    state.onCount?.(count)
  } catch {
    if (gen !== state.gen) return
    // Fallback: static grid (tap-assign still works) + the 3 s scan helper.
    state.phase = 'fallback'
    renderGrid(false)
    setStatus(t('audio.gridFailed', 'Fikk ikke live signal fra enheten — velg kanal manuelt, eller bruk skanning.'))
    showFallbackRow(true)
    state.onCount?.(state.channels)
  }
}

/** Stop the VU stream + painting. Safe to call repeatedly / when idle. */
export function stopChannelGrid(): void {
  state.gen++
  state.phase = 'idle'
  if (state.unlisten) { try { state.unlisten() } catch { /* gone */ } state.unlisten = null }
  if (state.raf) { cancelAnimationFrame(state.raf); state.raf = 0 }
  state.latest = null
  void window.api.stopVu().catch(() => {})
}

// ── Internals ────────────────────────────────────────────────────────────────

function currentMode(): ChannelMode {
  return ((document.querySelector('input[name="channels"]:checked') as HTMLInputElement | null)
    ?.value ?? settings.channels ?? 'stereo') as ChannelMode
}

function onTapColumn(ch: number): void {
  state.assign = nextAssignment(state.assign, ch, currentMode())
  renderChips()
  renderBadges()
  persistAssignment()
}

/** THE one persistence path for channel choices (explicit taps only — the
 *  structural replacement for the old pickerActive guard; 2026-07-31 phantom-
 *  mapping incident). */
function persistAssignment(): void {
  const devId = state.deviceId
  if (!devId) return
  const deviceChannels = { ...(settings.deviceChannels ?? {}) }
  deviceChannels[devId] = { channelL: state.assign.l, channelR: state.assign.r }
  patchSettings({ deviceChannels })
  void window.api.saveSettings(settings).then(() => {
    const el = document.getElementById('ch-grid-saved')
    if (!el) return
    el.textContent = t('audio.gridSaved', 'Lagret ✓')
    el.classList.add('show')
    window.setTimeout(() => el.classList.remove('show'), 1600)
  })
}

function renderGrid(skeleton: boolean): void {
  const grid = document.getElementById('ch-grid')
  if (!grid) return
  const count = Math.max(1, state.channels)
  state.cols = []
  const frag = document.createDocumentFragment()
  for (let i = 0; i < count; i++) {
    const root = document.createElement('button')
    root.type = 'button'
    root.className = 'ch-col' + (skeleton ? ' skeleton' : '')
    root.dataset.ch = String(i)
    const label = state.chLabels[i]
    if (label) root.title = label
    root.setAttribute('aria-label', t('audio.channelNum', 'Kanal {n}').replace('{n}', String(i + 1)))

    const badge = document.createElement('span')
    badge.className = 'ch-col-badge'
    const track = document.createElement('div')
    track.className = 'ch-col-track'
    const mask = document.createElement('div')
    mask.className = 'ch-col-mask'
    track.appendChild(mask)
    const num = document.createElement('span')
    num.className = 'ch-col-num'
    num.textContent = String(i + 1)

    root.append(badge, track, num)
    frag.appendChild(root)
    state.cols.push({
      root, mask, num, badge, signal: false,
      // 0..1 meter fraction, not dB: rise instant, fall eased.
      level: createLevelSmoother({ initial: 0 }),
    })
  }
  state.lastPaintMs = 0
  grid.replaceChildren(frag)
  const multi = count > 2
  const chipRow = document.getElementById('ch-assign-row')
  if (chipRow) chipRow.style.display = multi ? '' : 'none'
  const hint = document.getElementById('ch-grid-hint')
  if (hint) hint.style.display = multi ? '' : 'none'
  grid.classList.toggle('static', state.phase === 'fallback')
  renderChips()
  renderBadges()
}

/** The chip row's current shape — the slots it holds, in order. Rebuilding is
 *  only necessary when THIS changes; a tap or an arm switch is a class toggle
 *  and a label, not two new buttons. */
let chipShape = ''
/** Slot → the chip element currently rendering it. */
const chipEls = new Map<'l' | 'r', HTMLElement>()

function chipSpec(mode: ChannelMode): Array<{ slot: 'l' | 'r'; label: string }> {
  if (mode === 'monoL') return [{ slot: 'l', label: t('audio.gridMonoChip', 'KANAL (mono)') }]
  if (mode === 'monoR') return [{ slot: 'r', label: t('audio.gridMonoChip', 'KANAL (mono)') }]
  return [
    { slot: 'l', label: t('audio.channelL', 'VENSTRE (L)') },
    { slot: 'r', label: t('audio.channelR', 'HØYRE (R)') },
  ]
}

function renderChips(): void {
  const row = document.getElementById('ch-assign-row')
  if (!row) return
  const mode = currentMode()
  const spec = chipSpec(mode)
  const chName = (ch: number): string =>
    t('audio.channelNum', 'Kanal {n}').replace('{n}', String(ch + 1))

  // Rebuild ONLY when the set of chips actually changes (a mono/stereo switch,
  // or a language change). Every tap used to throw both buttons away and parse
  // fresh HTML — which also killed any focus or :active state mid-interaction.
  const shape = spec.map(s => s.slot + ' ' + s.label).join('|')
  if (shape !== chipShape || !row.firstElementChild) {
    chipShape = shape
    chipEls.clear()
    const frag = document.createDocumentFragment()
    for (const { slot, label } of spec) {
      const btn = document.createElement('button')
      btn.type = 'button'
      btn.className = 'ch-assign-chip'
      btn.dataset.slot = slot
      const slotEl = document.createElement('span')
      slotEl.className = 'ch-assign-slot'
      slotEl.textContent = label
      const chEl = document.createElement('span')
      chEl.className = 'ch-assign-ch'
      btn.append(slotEl, chEl)
      frag.appendChild(btn)
      chipEls.set(slot, btn)
    }
    row.replaceChildren(frag)
  }

  for (const { slot } of spec) {
    const btn = chipEls.get(slot)
    if (!btn) continue
    btn.classList.toggle('armed', state.assign.armed === slot)
    const chEl = btn.querySelector<HTMLElement>('.ch-assign-ch')
    const text = chName(slot === 'l' ? state.assign.l : state.assign.r)
    if (chEl && chEl.textContent !== text) chEl.textContent = text
  }

  const hint = document.getElementById('ch-grid-hint')
  if (hint) {
    const text = mode === 'monoL' || mode === 'monoR'
      ? t('audio.gridTapHintMono', 'Trykk på kanalen med signal for å velge den.')
      : t('audio.gridTapHint', 'Trykk på kanalen for venstre, så kanalen for høyre.')
    if (hint.textContent !== text) hint.textContent = text
  }
}

function renderBadges(): void {
  const mode = currentMode()
  for (const [i, col] of state.cols.entries()) {
    const isL = state.assign.l === i && mode !== 'monoR'
    const isR = state.assign.r === i && mode !== 'monoL'
    const label = isL && isR ? 'L+R' : isL ? 'L' : isR ? 'R' : ''
    col.badge.textContent = label
    col.root.classList.toggle('assigned', label !== '')
    col.root.setAttribute('aria-pressed', label !== '' ? 'true' : 'false')
  }
}

function subscribe(): void {
  if (state.unlisten) { try { state.unlisten() } catch { /* gone */ } }
  const un = window.api.on('vu-levels', (payload: unknown) => {
    state.latest = payload as { peak_dbfs: (number | null)[] }
    if (!state.raf) state.raf = requestAnimationFrame(paint)
  })
  state.unlisten = typeof un === 'function' ? un : null
}

/** rAF-coalesced painter: quantized, cached transform writes so 32 columns at
 *  30 Hz stay cheap (the setVUBar discipline), with the fall eased on real
 *  elapsed time rather than by a CSS transition the writer kept restarting. */
function paint(): void {
  state.raf = 0
  const levels = state.latest?.peak_dbfs
  if (!levels || state.phase !== 'live') return
  const now = performance.now()
  const dt = state.lastPaintMs ? now - state.lastPaintMs : 33
  state.lastPaintMs = now
  for (const [i, col] of state.cols.entries()) {
    const db = levels[i] ?? null
    const frac = col.level.step(dbToFraction(db), dt)
    const q = Math.round(frac * 100) / 100
    const prev = Number(col.mask.dataset.v ?? -1)
    if (q !== prev) {
      col.mask.dataset.v = String(q)
      // The mask HIDES the unlit top part of the fixed gradient.
      col.mask.style.transform = `scaleY(${Math.round((1 - q) * 100) / 100})`
    }
    // Presence uses the RAW level: hysteresis on a smoothed value would hold a
    // channel "lit" for the length of the release after the signal stopped.
    const sig = nextSignalState(col.signal, db ?? -120)
    if (sig !== col.signal) {
      col.signal = sig
      col.root.classList.toggle('has-signal', sig)
    }
  }
  // No self-scheduling: the engine emits `vu-levels` every ~33 ms for as long
  // as the grid is live, so the release is stepped by the packet stream. When
  // the stream stops the grid is being torn down anyway.
}

function setStatus(text: string): void {
  const el = document.getElementById('ch-grid-meta')
  if (el) el.textContent = text
}

function showFallbackRow(show: boolean): void {
  const el = document.getElementById('ch-grid-fallback')
  if (el) el.style.display = show ? '' : 'none'
}

/** The 3 s ffmpeg scan — survives ONLY as the fallback when the live grid
 *  can't open the device. Marks columns that carried signal. */
function setupScanFallback(): void {
  const btn = document.getElementById('btn-scan-channels') as HTMLButtonElement | null
  const status = document.getElementById('channel-scan-status')
  if (!btn || !status) return
  btn.addEventListener('click', async () => {
    const name = state.deviceName ?? settings.deviceName
    if (!name) {
      status.textContent = t('audio.scanNone', 'Ingen kanaler med signal — spill lyd og prøv igjen')
      return
    }
    btn.disabled = true
    status.textContent = t('audio.scanRunning', 'Lytter i 3 sek — spill lyd på kanalene …')
    stopChannelGrid() // the scan opens the device itself (backend also stops VU)
    try {
      const peaks = await window.api.scanDeviceChannels(name, 3)
      const live = peaks.filter(p => p.peakDb > -50)
      for (const [i, col] of state.cols.entries()) {
        const p = peaks.find(x => x.channel === i + 1)
        const loud = !!p && p.peakDb > -50
        col.root.classList.toggle('has-signal', loud)
        if (p) col.root.title = `${t('audio.channelNum', 'Kanal {n}').replace('{n}', String(i + 1))} · ${p.peakDb.toFixed(0)} dB`
      }
      status.textContent = live.length
        ? t('audio.scanFound', '{n} kanaler med signal: {list}')
            .replace('{n}', String(live.length))
            .replace('{list}', live.map(p => p.channel).join(', '))
        : t('audio.scanNone', 'Ingen kanaler med signal — spill lyd og prøv igjen')
    } catch (err) {
      status.textContent = '❌ ' + errTextLocal(err)
    }
    btn.disabled = false
  })
}

function errTextLocal(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  if (err && typeof err === 'object') {
    const o = err as Record<string, unknown>
    for (const k of ['message', 'recording', 'validation', 'internal', 'audio']) {
      if (typeof o[k] === 'string') return o[k] as string
    }
    try { return JSON.stringify(err) } catch { /* fall through */ }
  }
  return String(err)
}
