/**
 * Live streaming page — RTMP broadcast UI.
 *
 * Pulls destinations from settings, renders an enable-toggle for each, shows a
 * 16:9 preview that reloads from disk every 2 s while streaming, and surfaces
 * live stats (bitrate / fps / dropped frames / uptime) from the `stream-stats`
 * IPC event — which, as of Fase 6, the backend actually emits. Before that the
 * subscription below was real and the event never existed, so the statistics
 * card was gated «Kommer» rather than show four zeros through a live service.
 *
 * Lifecycle mirrors editor-page: setupLivePage() wires DOM once at boot,
 * reactivateLivePage()/deactivateLivePage() handle entering/leaving the tab
 * (start/stop the preview-refresh interval, subscribe/unsubscribe to stats).
 */

import { navigateTo } from '../ui/navigate'
import { t } from '../i18n'
import { settings } from '../state'
import { escHtml } from '../helpers'
import { makeVuState, paintVuPair, pushVuLevels, stopVuState } from '../audio/vu'
import type { VuState } from '../audio/vu'
import { acquireVuFeed } from '../audio/vu-feed'
import type { VuPick } from '../audio/vu-feed'
import { pickLR } from '../audio/vu-feed-core'
import type { StreamDestinationStored } from '../../types'
import { setupLiveOverlays, reactivateLiveOverlays } from './live-overlays'
import { normalizeFrameData } from '../../shared/normalize-frame-data'
import { liveBlockReason } from '../ui/feature-gate-core'
import {
  classifyStartError,
  emptyStreamStatus,
  formatBitrate,
  formatDropped,
  formatFps,
  formatUptime,
  isQualityReduced,
  livePillState,
  mapDestinationStates,
  type LiveDestinationRow,
  type LiveStartErrorCode,
  type StreamStatusView,
} from './live-stats-core'

// ── State ────────────────────────────────────────────────────────────────

let previewInterval: ReturnType<typeof setInterval> | null = null
let uptimeInterval:  ReturnType<typeof setInterval> | null = null
let unsubStats: (() => void) | undefined
let previewPathCached = ''
let lastStats: StreamStatusView = emptyStreamStatus()
/** Per-destination enabled state — keyed by destination id. Mirrors settings
 *  but lets the user toggle a destination off for one session without
 *  persisting. We persist only when the user changes it from Innstillinger. */
const sessionEnabled = new Map<string, boolean>()

// ── Setup ────────────────────────────────────────────────────────────────

export function setupLivePage(): void {
  document.getElementById('btn-live-start')?.addEventListener('click', () => onStartStopClick(true))
  document.getElementById('btn-live-start-stream-only')?.addEventListener('click', () => onStartStopClick(false))

  // The statistics card is NO LONGER GATED. It carried a «Kommer» chip for an
  // honest reason — nothing in the backend emitted `streaming://stats`, so the
  // four numbers were frozen at 0 for the whole broadcast — and that reason is
  // gone: the supervisor now pushes a full StreamStatus at ~1 Hz plus on every
  // state transition (src-tauri/src/streaming/mod.rs). The card shows real
  // measurements while live and «—» while idle, which is the distinction the
  // gate existed to make.

  document.getElementById('live-config-link')?.addEventListener('click', e => {
    e.preventDefault()
    // Destinations live in the Deling tab's Publisering section since Fase 3.
    navigateTo('settings', { tab: 'settings-sharing', anchor: '#stream-destinations-card' })
  })

  // Quality + framerate changes affect the params we pass to streamStart; no
  // need to persist unless the user clicks save on the publish settings.
  document.querySelectorAll<HTMLInputElement>('input[name="live-resolution"]').forEach(r => {
    r.addEventListener('change', () => updateStartButtonState())
  })
  document.getElementById('live-framerate')?.addEventListener('change', () => updateStartButtonState())

  // Overlay configuration UI — separate module so this file doesn't balloon.
  setupLiveOverlays()
}

// ── Activation lifecycle ─────────────────────────────────────────────────

export function reactivateLivePage(): void {
  // Sync UI from settings every time we land on the page (the user may have
  // added/removed destinations on the settings tab while we were away).
  syncQualityFromSettings()
  renderDestinations()
  reactivateLiveOverlays()
  refreshPreviewPath()
  // Pick up current stream state from main in case a stream is already alive.
  refreshStatus()
  startPreviewInterval()
  startUptimeInterval()
  subscribeStats()
  startVuMeter()
  // Idle camera-preview so the user sees the camera BEFORE clicking Start.
  // Skipped when a stream is already running (avfoundation locks the device,
  // and the live stream's snapshot-JPG path covers the active state).
  if (!lastStats.active) startIdleCameraPreview()
}

export function deactivateLivePage(): void {
  if (previewInterval) { clearInterval(previewInterval); previewInterval = null }
  if (uptimeInterval)  { clearInterval(uptimeInterval);  uptimeInterval  = null }
  if (unsubStats) { unsubStats(); unsubStats = undefined }
  stopVuMeter()
  stopIdleCameraPreview()
}

// ── Idle camera preview (before stream starts) ─────────────────────────────
//
// Same MJPEG-frame mechanism the Home page uses for its preview. We only run
// this while idle — when the user starts the stream, ffmpeg takes the camera
// over and the active-stream branch in startPreviewInterval() takes the
// snapshot JPG path instead. avfoundation locks the device exclusively, so
// trying to keep preview running alongside the stream would compete with
// streamer.ts for the camera handle.

let idlePreviewFrameUnsub: (() => void) | undefined
let idlePreviewLastFrameTs = 0
let idlePreviewActive = false

function startIdleCameraPreview(): void {
  if (idlePreviewActive) return
  // Skip when there's no camera configured — user is doing audio-only,
  // the preview will stay on the "waiting for stream" placeholder.
  if (!settings.videoDeviceName && settings.videoDeviceIndex == null) return
  idlePreviewActive = true

  const img         = document.getElementById('live-preview-img') as HTMLImageElement | null
  const placeholder = document.getElementById('live-preview-placeholder') as HTMLElement | null

  window.api.videoPreviewStart?.({
    videoDeviceName:  settings.videoDeviceName,
    videoDeviceIndex: settings.videoDeviceIndex,
    videoFramerate:   settings.videoFramerate,
  })?.catch(() => { /* main-side handles the error path */ })

  const frameIntervalMs = Math.floor(1000 / (settings.videoFramerate ?? 30)) - 2
  idlePreviewFrameUnsub = window.api.on('video-preview-frame', (data: unknown) => {
    if (!idlePreviewActive || lastStats.active) return
    const now = Date.now()
    if (now - idlePreviewLastFrameTs < frameIntervalMs) return
    idlePreviewLastFrameTs = now
    const arr = normalizeFrameData(data)
    if (!img || !arr || arr.length < 4) return
    const url = URL.createObjectURL(new Blob([arr as BlobPart], { type: 'image/jpeg' }))
    const prev = img.src
    img.src = url
    img.style.display = ''
    if (placeholder) placeholder.style.display = 'none'
    if (prev.startsWith('blob:')) URL.revokeObjectURL(prev)
  }) ?? undefined
}

function stopIdleCameraPreview(): void {
  if (!idlePreviewActive) return
  idlePreviewActive = false
  if (idlePreviewFrameUnsub) { idlePreviewFrameUnsub(); idlePreviewFrameUnsub = undefined }
  window.api.videoPreviewStop?.().catch(() => {})
  // Restore placeholder so the next visit doesn't show a stale frame.
  const img         = document.getElementById('live-preview-img') as HTMLImageElement | null
  const placeholder = document.getElementById('live-preview-placeholder') as HTMLElement | null
  if (img) {
    if (img.src.startsWith('blob:')) URL.revokeObjectURL(img.src)
    img.removeAttribute('src')
    img.style.display = 'none'
  }
  if (placeholder) placeholder.style.display = ''
}

// ── VU meter (pre-stream audio confidence check) ─────────────────────────
//
// Re-uses the same peak-hold / smoothing engine that powers the home-page VU,
// so the meter on the live tab is visually + numerically identical to the one
// on Hjem (and the recording overlay). It reads the shared backend feed
// (audio/vu-feed.ts) — the page never opens a microphone of its own, which is
// what removes the "second device owner" hazard the old getUserMedia meter
// carried onto this tab (2026-07-31 Qu-5 incident).

const liveVu = makeVuState()
/** Release handle for the feed subscription; non-null ⇔ the meter is running. */
let liveRelease: (() => void) | null = null
let liveRaf = 0

/** The channels this meter shows — the recorder's own mode + L/R picks. */
function liveVuPick(): VuPick {
  const devChannels = settings.deviceId ? (settings.deviceChannels?.[settings.deviceId] ?? null) : null
  return {
    mode: settings.channels ?? 'stereo',
    chL: devChannels?.channelL ?? 0,
    chR: devChannels?.channelR ?? 1,
  }
}

function startVuMeter(): void {
  if (liveRelease) return  // already running
  if (!document.getElementById('live-vu-l')) return
  // LEAK GUARD (2026-07-31 audit): the recorder owns the device outright during
  // a take (`start_recording` stops the VU engine itself), so don't ask for a
  // metering session it would only have to take away again.
  if (window.__isRecording) return

  liveRelease = acquireVuFeed({
    deviceName: settings.deviceName ?? null,
    pick: liveVuPick,
    onLevels: (l, r, levels) => {
      const p = liveVuPick()
      const pk = pickLR(levels.peak_dbfs, p.mode, p.chL, p.chR)
      pushVuLevels(liveVu, l, r, pk.l, pk.r)
      if (!liveRaf) liveRaf = requestAnimationFrame(paintLiveVu)
    },
  })
}

function paintLiveVu(): void {
  liveRaf = 0
  const fillL = document.getElementById('live-vu-l')
  const pkL   = document.getElementById('live-vu-peak-l')
  const dbL   = document.getElementById('live-vu-db-l')
  const fillR = document.getElementById('live-vu-r')
  const pkR   = document.getElementById('live-vu-peak-r')
  const dbR   = document.getElementById('live-vu-db-r')
  paintVuPair(liveVu, fillL, pkL, dbL, fillR, pkR, dbR)
  updateLiveSignalStatus(liveVu.smL, liveVu.smR, liveVu)
  // Clip is a PEAK verdict — see home-vu.ts.
  if (liveVu.pkL > -0.5) document.getElementById('live-vu-clip-l')?.classList.add('clip')
  if (liveVu.pkR > -0.5) document.getElementById('live-vu-clip-r')?.classList.add('clip')
}

export function stopVuMeter(): void {
  if (liveRelease) {
    const r = liveRelease
    liveRelease = null
    try { r() } catch { /* gone */ }
  }
  if (liveRaf) { cancelAnimationFrame(liveRaf); liveRaf = 0 }
  stopVuState(liveVu)
  const fills = ['live-vu-l', 'live-vu-r'].map(id => document.getElementById(id))
  const peaks = ['live-vu-peak-l', 'live-vu-peak-r'].map(id => document.getElementById(id))
  const dbs   = ['live-vu-db-l', 'live-vu-db-r'].map(id => document.getElementById(id))
  // The fill is a transform-driven mask (audio/vu.ts) — resetting `width` left
  // the stale scaleX in place and froze these bars after a stop, the exact bug
  // already fixed on home (home-vu.ts stopVU) but never here.
  fills.forEach(el => { if (el) el.style.transform = 'scaleX(1)' })
  peaks.forEach(el => { if (el) el.style.opacity = '0' })
  dbs.forEach(el   => { if (el) el.textContent = '—' })
  resetLiveSignalStatus()
}

// Same 60 Hz write-cache discipline as home-vu.ts: the signal line changes a few
// times a minute, so the steady state must cost comparisons, not DOM writes.
const LIVE_CLS_INIT = '§init§'
let lastLiveSigCls  = LIVE_CLS_INIT
let lastLivePeakTxt = LIVE_CLS_INIT
let lastLivePeakAt  = 0
const LIVE_PEAK_TEXT_MIN_INTERVAL_MS = 150

function resetLiveSignalStatus(): void {
  const dot  = document.getElementById('live-signal-dot')
  const text = document.getElementById('live-signal-text')
  const peak = document.getElementById('live-signal-peak')
  if (dot)  dot.className = 'signal-dot'
  if (text) { text.className = 'signal-text'; text.textContent = '—' }
  if (peak) peak.textContent = ''
  lastLiveSigCls = LIVE_CLS_INIT
  lastLivePeakTxt = LIVE_CLS_INIT
  lastLivePeakAt = 0
}

function updateLiveSignalStatus(dbL: number, dbR: number, state: VuState): void {
  const db = Math.max(dbL, dbR)
  let cls = '', label = '—'
  if      (db >= -3)  { cls = 'klipping'; label = t('home.signalClipping', 'Klipper!') }
  else if (db >= -12) { cls = 'hoyt';     label = t('home.signalLoud',     'Høyt')     }
  else if (db >= -40) { cls = 'god';      label = t('home.signalGood',     'Bra')      }
  else if (db > -55)  { cls = 'svak';     label = t('home.signalWeak',     'Svakt')    }

  if (cls !== lastLiveSigCls) {
    const dot  = document.getElementById('live-signal-dot')
    const text = document.getElementById('live-signal-text')
    if (!dot || !text) return
    lastLiveSigCls = cls
    const suffix = cls ? ' ' + cls : ''
    dot.className  = 'signal-dot' + suffix
    text.className = 'signal-text' + suffix
    text.textContent = label
  }

  const now = performance.now()
  if (now - lastLivePeakAt < LIVE_PEAK_TEXT_MIN_INTERVAL_MS) return
  const pkMax = Math.max(state.peakL, state.peakR)
  const peakTxt = pkMax > -59 ? `${t('home.peakLabel', 'Maks')}: ${pkMax.toFixed(1)} dBFS` : ''
  if (peakTxt === lastLivePeakTxt) return
  lastLivePeakTxt = peakTxt
  lastLivePeakAt = now
  const peak = document.getElementById('live-signal-peak')
  if (peak) peak.textContent = peakTxt
}

// ── Stats subscription ───────────────────────────────────────────────────

function subscribeStats(): void {
  if (unsubStats) return
  unsubStats = window.api.on('stream-stats', (data: unknown) => {
    applyStatus({ ...emptyStreamStatus(), ...(data as Partial<StreamStatusView>) })
  }) ?? undefined
}

/**
 * The ONE place a status becomes pixels — used by both the live event and the
 * activation poll, so the two can no longer disagree about what a stopped
 * stream looks like. (They did: the event handler only touched the pill when
 * the stream was active, so a stream that stopped while the tab was open kept
 * a red «🔴 Live» pill until the user navigated away and back.)
 */
function applyStatus(s: StreamStatusView): void {
  lastStats = s
  renderStats(s)
  renderDestinationStates(s)
  setStatusPill(livePillState(s), pillLabel(s))
  updateStartButton(s.active)
  const tag = document.getElementById('live-preview-overlay-tag')
  if (tag) tag.style.display = s.active ? '' : 'none'
}

function pillLabel(s: StreamStatusView): string {
  if (s.active) return t('live.statusLive', '🔴 Live')
  if (s.lastLine.trim()) return t('live.statusStopped', 'Stoppet')
  return t('live.statusReady', 'Klar')
}

async function refreshStatus(): Promise<void> {
  try {
    const s = await window.api.streamStatus()
    applyStatus({ ...emptyStreamStatus(), ...(s as Partial<StreamStatusView>) })
  } catch (err) {
    console.warn('[live-page] refreshStatus failed', err)
  }
}

// ── Preview refresh ──────────────────────────────────────────────────────

async function refreshPreviewPath(): Promise<void> {
  try {
    const p = await window.api.streamPreviewPath()
    previewPathCached = typeof p === 'string' ? p : ''
  } catch { previewPathCached = '' }
}

function startPreviewInterval(): void {
  if (previewInterval) return
  // Reload the preview <img> every 2 s — a cache-busting query string forces
  // the browser to refetch the JPEG even though the path stays the same.
  previewInterval = setInterval(() => {
    if (!previewPathCached) return
    const img         = document.getElementById('live-preview-img') as HTMLImageElement | null
    const placeholder = document.getElementById('live-preview-placeholder')
    if (!img) return
    // Only show the preview while a stream is live (or recently was). When
    // idle, the snapshot file may not exist — fall back to placeholder.
    if (!lastStats.active) {
      img.style.display = 'none'
      if (placeholder) placeholder.style.display = ''
      return
    }
    img.style.display = ''
    if (placeholder) placeholder.style.display = 'none'
    // asset:// via the shim — WKWebView blocks file:// (same fix as the editor
    // previews); the query string still cache-busts the refetch.
    img.src = `${window.api.toAssetUrl(previewPathCached)}?t=${Date.now()}`
  }, 2000)
}

function startUptimeInterval(): void {
  if (uptimeInterval) return
  // The clock ticks locally rather than waiting for the backend — the stats
  // feed is paced to ~1 Hz and a seconds counter that stutters when the network
  // does looks like the stream is stuttering.
  uptimeInterval = setInterval(() => {
    const el = document.getElementById('live-stat-uptime')
    if (el) el.textContent = formatUptime(lastStats, Date.now())
  }, 1000)
}

// ── Destinations rendering ───────────────────────────────────────────────

function renderDestinations(): void {
  const list  = document.getElementById('live-destinations-list')
  const empty = document.getElementById('live-destinations-empty')
  if (!list) return
  const dests = settings.streamDestinations ?? []
  list.innerHTML = ''

  if (dests.length === 0) {
    if (empty) empty.style.display = ''
    updateStartButtonState()
    return
  }
  if (empty) empty.style.display = 'none'

  for (const d of dests) {
    if (!sessionEnabled.has(d.id)) sessionEnabled.set(d.id, d.enabled)
    const row = document.createElement('div')
    row.className = 'live-destination-row'
    row.dataset.destId = d.id
    row.innerHTML = `
      <span class="live-dest-dot" data-state="${d.enabled ? 'idle' : 'disabled'}"></span>
      <div class="live-dest-info">
        <div class="live-dest-name">${escHtml(d.name || d.rtmpUrl || '—')}</div>
        <div class="live-dest-state" data-i18n-fallback>${
          d.hasKey ? '' : '<span class="live-dest-warn">⚠ Mangler stream-key</span>'
        }</div>
      </div>
      <label class="toggle live-dest-toggle">
        <input type="checkbox" ${sessionEnabled.get(d.id) ? 'checked' : ''} />
        <span class="toggle-track"></span>
      </label>
    `
    const chk = row.querySelector<HTMLInputElement>('input[type="checkbox"]')
    chk?.addEventListener('change', () => {
      sessionEnabled.set(d.id, !!chk.checked)
      const dot = row.querySelector<HTMLElement>('.live-dest-dot')
      if (dot) dot.dataset.state = chk.checked ? 'idle' : 'disabled'
      updateStartButtonState()
    })
    list.appendChild(row)
  }
  updateStartButtonState()
}

/**
 * Paint each destination's dot from the backend's per-destination health.
 *
 * This used to read `{ id, state }` off the status — two fields that have never
 * existed on the Rust `StreamStatus`, which carries `{ name, ok }` in tee-slave
 * order. Every dot was therefore assigned `undefined` on every event and never
 * moved, so a YouTube ingest that dropped mid-service looked exactly like one
 * that was fine. The join lives in `mapDestinationStates` (pure, tested).
 */
function renderDestinationStates(s: StreamStatusView): void {
  const rows: LiveDestinationRow[] = (settings.streamDestinations ?? []).map(d => ({
    id: d.id,
    name: d.name || d.rtmpUrl || '—',
    enabled: sessionEnabled.get(d.id) ?? d.enabled,
  }))
  const states = mapDestinationStates(rows, s)
  for (const [id, state] of states) {
    const row = document.querySelector<HTMLElement>(`.live-destination-row[data-dest-id="${CSS.escape(id)}"]`)
    const dot = row?.querySelector<HTMLElement>('.live-dest-dot')
    if (dot) dot.dataset.state = state
  }
}

// ── Stats rendering ──────────────────────────────────────────────────────

function renderStats(s: StreamStatusView): void {
  const setText = (id: string, v: string): void => { const el = document.getElementById(id); if (el) el.textContent = v }
  setText('live-stat-bitrate', formatBitrate(s))
  setText('live-stat-fps',     formatFps(s))
  setText('live-stat-dropped', formatDropped(s))
  setText('live-stat-uptime',  formatUptime(s, Date.now()))

  // The supervisor's own commentary: reconnect attempts, a destination that
  // dropped, an adaptive step-down. It was maintained in the Rust status from
  // the start and had nowhere to go.
  const note = document.getElementById('live-stats-note')
  if (!note) return
  const lines: string[] = []
  if (isQualityReduced(s)) {
    lines.push(t('live.qualityReduced', 'Redusert kvalitet for å holde strømmen stabil.'))
  }
  if (s.lastLine.trim()) lines.push(s.lastLine.trim())
  if (lines.length) {
    note.textContent = lines.join('  •  ')
    note.style.display = ''
  } else {
    note.textContent = ''
    note.style.display = 'none'
  }
}

// ── Start / stop ─────────────────────────────────────────────────────────

async function onStartStopClick(alsoRecord: boolean): Promise<void> {
  const btn          = document.getElementById('btn-live-start') as HTMLButtonElement | null
  const streamOnlyBtn = document.getElementById('btn-live-start-stream-only') as HTMLButtonElement | null
  if (!btn) return
  hideError()
  if (lastStats.active) {
    btn.disabled = true
    if (streamOnlyBtn) streamOnlyBtn.disabled = true
    try { await window.api.streamStop() }
    finally {
      btn.disabled = false
      if (streamOnlyBtn) streamOnlyBtn.disabled = false
    }
    return
  }

  const dests = (settings.streamDestinations ?? []).filter(d => sessionEnabled.get(d.id) && d.hasKey)
  if (dests.length === 0) {
    showError(t('live.errNoActive', 'Ingen aktive destinasjoner med lagret stream-key.'))
    return
  }

  const resolution = (document.querySelector('input[name="live-resolution"]:checked') as HTMLInputElement | null)?.value
                  ?? settings.streamResolution ?? '720p'
  const framerate  = parseInt((document.getElementById('live-framerate') as HTMLSelectElement | null)?.value
                  ?? String(settings.streamFramerate ?? 30), 10) as 25 | 30

  setStatusPill('is-preparing', t('live.statusPreparing', 'Forbereder…'))
  btn.disabled = true
  if (streamOnlyBtn) streamOnlyBtn.disabled = true

  // Stop idle camera preview BEFORE asking main to spawn the stream
  // ffmpeg — avfoundation holds an exclusive lock on the camera, and
  // failing to release it first deadlocks the stream startup. We restart
  // the idle preview again in updateStartButton() when active=false.
  stopIdleCameraPreview()

  try {
    const result = await window.api.streamStart({
      resolution,
      framerate,
      videoBitrateKbps: settings.streamVideoBitrate ?? undefined,
      // Full destination view (incl. hasKey — these are pre-filtered on hasKey)
      // so the backend's StreamDestinationView deserialize succeeds.
      destinations: dests.map(d => ({ id: d.id, name: d.name, rtmpUrl: d.rtmpUrl, enabled: true, hasKey: true })),
      overlays: settings.streamOverlays ?? [],
      alsoRecord,
    })
    if (!result.ok) {
      showError(startErrorText(result.error ?? ''))
      setStatusPill('is-idle', t('live.statusReady', 'Klar'))
      // Stream-start failed — restart idle preview so the user isn't
      // staring at a black box wondering what's going on.
      startIdleCameraPreview()
      return
    }
    // The 'stream-stats' event will flip the pill to live; refresh preview path
    // now in case it just became available.
    refreshPreviewPath()
  } catch (err) {
    showError(startErrorText((err as Error).message))
    setStatusPill('is-idle', t('live.statusReady', 'Klar'))
    startIdleCameraPreview()
  } finally {
    btn.disabled = false
    if (streamOnlyBtn) streamOnlyBtn.disabled = false
  }
}

/**
 * A sentence for the operator, not a sentence for the compiler.
 *
 * `stream_start` fails with `AppError` strings meant for a developer. The worst
 * of them shipped in every release: a build without `--features streaming`
 * answered the START button with
 *
 *     feature_disabled: streaming.start requires a build with `--features streaming`
 *
 * printed verbatim into the error strip, which is neither actionable nor
 * comprehensible to someone twenty minutes from a service. The classification
 * is pure + tested (`classifyStartError`); this only picks the wording.
 */
function startErrorText(raw: string): string {
  const messages: Record<LiveStartErrorCode, string> = {
    featureDisabled: t(
      'live.errFeatureDisabled',
      'Denne versjonen av appen kan ikke direktesende. Alt annet virker som normalt — oppdater til en nyere versjon for å sende direkte.',
    ),
    noCamera: t(
      'live.errNoCamera',
      'Fant ikke kameraet. Sjekk at det er koblet til, og velg det under Innstillinger → Video.',
    ),
    alreadyActive: t('live.errAlreadyActive', 'En direktesending kjører allerede.'),
    invalidDestination: t(
      'live.errInvalidDestination',
      'En destinasjon er satt opp feil — sjekk RTMP-adressen og stream-key under Deling → Publisering.',
    ),
    spawnFailed: t(
      'live.errSpawnFailed',
      'Klarte ikke å starte videomotoren (ffmpeg). Start appen på nytt, og gi beskjed hvis det gjentar seg.',
    ),
    unknown: raw.trim() || t('live.connectionFailed', 'Tilkobling feilet'),
  }
  return messages[classifyStartError(raw)]
}

function updateStartButton(active: boolean): void {
  const btn          = document.getElementById('btn-live-start') as HTMLButtonElement | null
  const streamOnly   = document.getElementById('btn-live-start-stream-only') as HTMLButtonElement | null
  if (!btn) return
  const span = btn.querySelector('span')
  const wasActive = btn.classList.contains('is-active')
  if (active) {
    btn.classList.add('is-active')
    if (span) span.textContent = t('live.stopBtn', '■ Stopp')
    // While streaming, hide the secondary "stream-only" CTA — it would be
    // confusing to show a "start" button alongside an active stream.
    if (streamOnly) streamOnly.style.display = 'none'
  } else {
    btn.classList.remove('is-active')
    if (span) span.textContent = t('live.startBtn', '🔴 Start direktesending + opptak')
    if (streamOnly) streamOnly.style.display = ''
    // Stream just transitioned active → idle: restart idle camera-preview
    // so the user sees the camera again instead of a frozen last-snapshot.
    if (wasActive) startIdleCameraPreview()
  }
  updateStartButtonState()
}

/**
 * Enable/disable START — and SAY WHY when it is disabled.
 *
 * The button knew perfectly well that no destination had a stream key. It just
 * greyed out and left the operator to guess, five minutes before a service,
 * with no hint that the answer was two pages away. Now the specific missing
 * piece is named under the button, with a link straight to it.
 */
function updateStartButtonState(): void {
  const btn = document.getElementById('btn-live-start') as HTMLButtonElement | null
  const reasonEl = document.getElementById('live-start-reason')
  if (!btn) return
  if (lastStats.active) {
    btn.disabled = false
    if (reasonEl) reasonEl.style.display = 'none'
    return
  }

  const dests = settings.streamDestinations ?? []
  const enabled = dests.filter(d => sessionEnabled.get(d.id))
  const reason = liveBlockReason({
    total: dests.length,
    enabled: enabled.length,
    ready: enabled.filter(d => d.hasKey).length,
  })
  btn.disabled = reason !== null

  if (!reasonEl) return
  if (!reason) { reasonEl.style.display = 'none'; return }

  const text =
    reason === 'noDestinations'
      ? t('live.blockedNoDestinations', 'Ingen destinasjon er satt opp ennå — legg til YouTube, Facebook eller en egen RTMP-server.')
      : reason === 'noEnabled'
        ? t('live.blockedNoEnabled', 'Ingen destinasjon er slått på. Aktiver minst én i listen over.')
        : t('live.blockedNoKey', 'Destinasjonen mangler stream-key. Uten nøkkelen slipper ikke YouTube/Facebook sendingen inn.')

  reasonEl.textContent = ''
  reasonEl.append(text + ' ')
  if (reason !== 'noEnabled') {
    const link = document.createElement('a')
    link.href = '#'
    link.textContent = t('live.blockedGoSettings', 'Åpne Deling → Publisering')
    link.addEventListener('click', e => {
      e.preventDefault()
      navigateTo('settings', { tab: 'settings-sharing', anchor: '#stream-destinations-card' })
    })
    reasonEl.appendChild(link)
  }
  reasonEl.style.display = ''
}

// ── Status pill helpers ──────────────────────────────────────────────────

function setStatusPill(stateClass: 'is-idle' | 'is-preparing' | 'is-live' | 'is-error', label: string): void {
  const pill = document.getElementById('live-status-pill')
  const text = document.getElementById('live-status-pill-text')
  if (!pill) return
  pill.classList.remove('is-idle', 'is-preparing', 'is-live', 'is-error')
  pill.classList.add(stateClass)
  if (text) text.textContent = label
}

function showError(msg: string): void {
  const el = document.getElementById('live-error')
  if (!el) return
  el.textContent = msg
  el.style.display = ''
}

function hideError(): void {
  const el = document.getElementById('live-error')
  if (el) el.style.display = 'none'
}

// ── Quality sync ─────────────────────────────────────────────────────────

function syncQualityFromSettings(): void {
  const res = settings.streamResolution ?? '720p'
  const r = document.querySelector<HTMLInputElement>(`input[name="live-resolution"][value="${res}"]`)
  if (r) r.checked = true
  const fr = settings.streamFramerate ?? 30
  const sel = document.getElementById('live-framerate') as HTMLSelectElement | null
  if (sel) sel.value = String(fr)
}

// ── Cross-tab refresh hook ───────────────────────────────────────────────

/** Called from publish-page when the user saves a new destinations list, so
 *  the Direkte-tab picks it up without needing a page navigation. Safe to
 *  call even if the page is not currently visible. */
export function notifyLivePageDestinationsChanged(): void {
  // Clear session toggles so the new destinations adopt their stored enabled
  // value next time the page activates.
  sessionEnabled.clear()
  const livePage = document.getElementById('page-live')
  if (livePage?.classList.contains('active')) {
    renderDestinations()
  }
}

// Tag unused declarations as referenced for stricter tsconfig settings.
export type { StreamDestinationStored }
