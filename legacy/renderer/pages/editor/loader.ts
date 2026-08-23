import { settings } from '../../state'
import { t } from '../../i18n'
import type { RecordingMetadata } from '../../../types'
import { E, $, clearDirty, VIDEO_EXTS } from './state'
import { jinglesSupportedForFile } from './export-params'
import { routePlayback } from './play-regions'
import { sharedAudioCtx } from './audio-ctx'
import { computeClipTimes, computeJinglePeaks, setNormalizeUI } from './peaks'
import { fitAll } from './viewport'
import { clampPlayable, clampMain } from './geometry'
import { snapOutOfCut } from './canvas-input'
import { stopPlay, startPlay, seekMediaTo, updateTimecode, updateTotalTime } from './playback'
import { renderAnalyzePanel, runDetection } from './detection'
import { renderMetaPanel, renderChapterList } from './metadata'
import { renderCutList, updateRemainingDisplay, cancelDraftSave } from './cuts'
import { drawWaveform, drawMinimap, updateMinimapViewport, syncCanvasSize } from './waveform'
import { loadTranscriptForFile } from '../editor-transcript'

import { showState, showEditorError, updateHeaderSummary } from '../editor-page'
import { attachProgress, type ProgressHandle } from '../../ui/progress'

// ── File loading (probe, waveform, transport, sidecars) ─────────────────────
//
// Opening a recording is four independent things, in this order:
//
//   1. ffprobe for the duration — container headers only, so the timeline is
//      ready in milliseconds even for a multi-gigabyte service.
//   2. The TRANSPORT: a media element streaming the ORIGINAL over `asset://`.
//      A transcoded AAC proxy is the fallback, taken only when the webview has
//      no decoder for the container (`routePlayback`) or when the original
//      turns out not to open. Nothing decodes the recording into renderer
//      memory — the old path built an AudioBuffer (or an 8 kHz extract) of the
//      whole file, which is why big files sounded like a telephone.
//   3. The waveform: the backend streams the decode into 100 peaks/s and caches
//      it beside the recording, so every reopen is a JSON read.
//   4. Sidecars — metadata, transcript, unsaved cuts — none of them blocking.
//
// `E.loadSeq` guards all of it: every await re-checks it, so a user who opens a
// second file mid-load never gets the first one's peaks, duration or transport.

export async function pickAndLoad(): Promise<void> {
  const fp = await window.api.editorPickFile()
  if (fp) loadFile(fp)
}

/** How long we give a media element to report metadata before declaring the
 *  transport dead and falling back. Generous enough for a cold external drive,
 *  short enough that the user isn't left with a play button that does nothing. */
const PLAYER_READY_TIMEOUT_MS = 5000

/** The ONE <audio> element that streams the main recording. Created on first
 *  use and reused for every file — a fresh element per load leaked a decoder +
 *  an open file handle each time. `E.playerEl` points at it only while an AUDIO
 *  file is loaded (video files play through `E.videoEl`). */
let persistentPlayerEl: HTMLAudioElement | null = null

function ensurePlayerEl(): HTMLAudioElement {
  if (!persistentPlayerEl) {
    const el = new Audio()
    el.preload = 'auto'
    persistentPlayerEl = el
  }
  E.playerEl = persistentPlayerEl
  return persistentPlayerEl
}

/**
 * Resolve once the element can report where it is (`loadedmetadata`), or `false`
 * if it errors out or simply never gets there. ONE cleanup for the timer and
 * both listeners: an orphaned timer plus stale listeners on a REUSED element is
 * exactly how the previous file's load corrupted the next one when the user
 * switched files quickly (see the video branch's identical lesson).
 */
function whenPlayable(el: HTMLMediaElement, timeoutMs: number): Promise<boolean> {
  if (el.readyState >= 1) return Promise.resolve(true)
  return new Promise<boolean>(resolve => {
    let timer = 0
    const cleanup = () => {
      clearTimeout(timer)
      el.removeEventListener('loadedmetadata', onMeta)
      el.removeEventListener('error', onErr)
    }
    const onMeta = () => { cleanup(); resolve(true) }
    const onErr  = () => { cleanup(); resolve(false) }
    el.addEventListener('loadedmetadata', onMeta, { once: true })
    el.addEventListener('error', onErr, { once: true })
    timer = window.setTimeout(() => { cleanup(); resolve(false) }, timeoutMs)
  })
}

/**
 * Point the player at a transcoded AAC proxy of `fp`. This is the LAST resort
 * transport: either the container has no decoder in the webview at all, or the
 * original refused to open. Costs a full transcode (a minute-plus on a long
 * service), so it is never taken speculatively.
 *
 * Returns whether the proxy was produced and attached. The element's own load is
 * watched in the background — if even the proxy won't open there is nothing left
 * to try, and the notice says so rather than leaving a dead play button.
 */
async function attachPlaybackProxy(fp: string, seq: number): Promise<boolean> {
  let proxyPath: string | null = null
  try {
    // No reveal delay: a full transcode of a service is never quick, so the
    // explanation and its bar should be on screen from the first frame.
    proxyPath = await withLoadingDetail(
      t('editor.preparingPlayback', 'Klargjør avspilling…'),
      window.api.editorExtractPlaybackProxy(fp),
      { channel: 'editor-proxy-progress', delayMs: 0 },
    )
  } catch { proxyPath = null }
  if (seq !== E.loadSeq) return false
  if (!proxyPath) {
    console.warn('[editor] playback proxy transcode failed for', fp)
    showQualityNotice(t('editor.qualityFallback', 'Avspilling er ikke tilgjengelig for denne filen — formatet kan ikke spilles av her. Klipping og eksport virker som vanlig.'))
    return false
  }

  await window.api.editorAllowAssetPath(proxyPath)
  if (seq !== E.loadSeq) return false

  const el = ensurePlayerEl()
  el.src = window.api.toAssetUrl(proxyPath)
  el.load()
  E.playbackSource = 'proxy'
  showQualityNotice(t('editor.playbackViaProxy', 'Avspilling går via en midlertidig mellomfil (full kvalitet). Eksport bruker alltid originalen.'))
  void whenPlayable(el, PLAYER_READY_TIMEOUT_MS).then(ok => {
    // Only speak for the transport we actually attached: a newer load, or a
    // swap back to the original, owns the notice from then on.
    if (seq === E.loadSeq && E.playbackSource === 'proxy' && !ok) {
      console.warn('[editor] playback proxy would not open:', proxyPath)
      showQualityNotice(t('editor.qualityFallback', 'Avspilling er ikke tilgjengelig for denne filen — formatet kan ikke spilles av her. Klipping og eksport virker som vanlig.'))
    }
  })
  return true
}

/**
 * Prepare playback + waveform for an AUDIO file. Playback is ALWAYS a media
 * element streaming from disk — the original when the webview can decode it,
 * a transcoded proxy otherwise. Nothing about the main recording goes through
 * Web Audio any more: the old path decoded (or 8 kHz-extracted) the whole file
 * into renderer memory, which is both why big files sounded like a telephone and
 * why Normalize/jingles/cut-preview behaved differently per file size.
 *
 * Returns false when the file could not be loaded at all (an error state is
 * already shown), or when a newer load superseded this one.
 */
async function loadAudioFile(fp: string, ext: string, seq: number): Promise<boolean> {
  // Duration first, from ffprobe — it reads container headers only, so the
  // timeline is ready in milliseconds even for a multi-gigabyte recording.
  const info = await window.api.editorLoadRecording(fp)
  if (seq !== E.loadSeq) return false
  let duration = info && isFinite(info.durationSec) && info.durationSec > 0 ? info.durationSec : 0

  const el = ensurePlayerEl()
  let readyForFallback: Promise<boolean> | null = null

  if (routePlayback(ext) === 'proxy') {
    // No decoder for this container in the webview — there is nothing to try
    // first, so transcode up front. `attachPlaybackProxy` owns the explanation
    // and its progress bar (it is also reached from the background fallback
    // path, and only one of the two should be writing that line).
    await attachPlaybackProxy(fp, seq)
    if (seq !== E.loadSeq) return false
  } else {
    // The webview decodes this format natively: stream the ORIGINAL. The scope
    // grant is what makes recordings on external volumes loadable at all.
    await window.api.editorAllowAssetPath(fp)
    if (seq !== E.loadSeq) return false
    E.playbackSource = 'original'
    el.src = window.api.toAssetUrl(fp)
    el.load()
    // Deliberately NOT awaited: the workspace paints as soon as the waveform is
    // in. If the element turns out unable to open the original we swap in the
    // proxy underneath it (below).
    readyForFallback = whenPlayable(el, PLAYER_READY_TIMEOUT_MS)
  }

  // Waveform: the backend streams the decode straight into 100 peaks/s — the
  // same pipeline the video path has always used. No WAV bytes cross IPC and no
  // AudioBuffer is built, so a 4 h FLAC costs the same renderer memory as a
  // 4 min one. The result is cached beside the recording, so this is a file read
  // on every open after the first.
  const result = await withLoadingDetail(
    t('editor.analyzingWaveform', 'Analyserer bølgeform…'),
    window.api.editorExtractAudioPeaks(fp),
    { channel: 'editor-peaks-progress' },
  )
  if (seq !== E.loadSeq) return false
  if (result && Array.isArray(result.peaks) && result.peaks.length) {
    E.peaks = Float32Array.from(result.peaks)
    E.clipTimes = computeClipTimes(E.peaks)
    if (duration <= 0) duration = E.peaks.length / 100
  }

  if (duration <= 0) {
    // Neither ffprobe nor the extract knew: ask the element itself, the way the
    // video path does. Only reachable when ffmpeg/ffprobe are unavailable.
    const ok = readyForFallback ? await readyForFallback : await whenPlayable(el, PLAYER_READY_TIMEOUT_MS)
    if (seq !== E.loadSeq) return false
    if (ok && isFinite(el.duration) && el.duration > 0) duration = el.duration
  }

  if (duration <= 0) {
    console.error('[editor] could not determine duration for', fp)
    showEditorError('Kunne ikke lese lydfilen — formatet støttes kanskje ikke, eller filen er korrupt')
    showState('empty')
    return false
  }
  E.duration = duration

  if (!E.peaks) {
    // Flat waveform rather than an empty screen: cuts, chapters and export all
    // work off the timeline, which we now have.
    E.peaks = new Float32Array(Math.ceil(E.duration * 100))
    console.log('[editor] no peaks available (flat waveform), duration:', E.duration.toFixed(1) + 's')
  }

  // Watch the ORIGINAL element load in the background and fall back to a proxy
  // if it never comes up (DRM-free but unsupported profile, damaged header, a
  // network volume that stalls). Not awaited — the editor is already usable.
  if (readyForFallback) void watchPlayerLoad(readyForFallback, fp, seq)
  return true
}

/** Swap in the proxy transport if the original never became playable, resuming
 *  playback from the same spot when the user had already pressed play. */
async function watchPlayerLoad(ready: Promise<boolean>, fp: string, seq: number): Promise<void> {
  const ok = await ready
  if (seq !== E.loadSeq || ok || E.playbackSource !== 'original') return
  console.warn('[editor] original would not open in the player — preparing a proxy:', fp)
  const wasPlaying = E.isPlaying
  const preview = E.isPreview
  if (wasPlaying) stopPlay()   // syncs E.playStartSec to the current position
  const attached = await attachPlaybackProxy(fp, seq)
  if (seq !== E.loadSeq) return
  if (attached && wasPlaying) startPlay(preview)
}

export async function loadFile(fp: string): Promise<void> {
  const seq = ++E.loadSeq
  stopPlay()

  // FIRST, before a single field is reset: disarm the previous file's pending
  // draft write. It is debounced 2 s, so switching files quickly left a timer
  // armed that fired mid-load and wrote the (now empty) cut list into a sidecar
  // — in the worst case the NEW file's, moments before its draft-restore read
  // it. The scheduler also captures its own path/payload, so this is the second
  // of two belts; see draft-scheduler.ts.
  cancelDraftSave()

  E.cuts = []
  E.cutHistory = []
  E.cutHistoryIdx = -1
  E.suggestions = []
  // A stale auto-pick index would be read against the NEW file's segments.
  E.autoSermonIndex = null
  E.filePath = fp
  E.peaks = null
  // Reset BEFORE any work so a failed load can never leave the previous file's
  // clip markers showing on the new one.
  E.clipTimes = []
  // Drop the previous file's transport (element src + any quality notice) — the
  // new file's is set up below.
  teardownPlayback()
  E.playStartSec = 0
  E.meta = { title: '', speaker: '', description: '', chapters: [] }
  E.metaDirty = false
  // Fresh file → drop any previous peak-normalize gain and reset the UI.
  E.audioGainDb = 0
  setNormalizeUI(0, false)
  E.lastAnalyzedAt = 0
  renderAnalyzePanel()
  // Fresh file → not dirty
  clearDirty()

  showState('loading')

  // Determine if this is a video file. Fast paths for known video + the audio
  // formats the webview streams itself; everything else (ambiguous containers
  // like .webm/.mka, exotic audio, or an unknown extension forced through the
  // "Alle filer" picker) is probed with ffprobe so we route by actual stream
  // content, not the name.
  const ext = ('.' + (fp.split('.').pop()?.toLowerCase() ?? '')).toLowerCase()
  if (VIDEO_EXTS.has(ext)) {
    E.isVideoFile = true
  } else if (routePlayback(ext) === 'element') {
    E.isVideoFile = false
  } else {
    const streams = await window.api.editorProbeStreams(fp)
    if (seq !== E.loadSeq) return // a newer load started during the probe
    E.isVideoFile = !!streams && streams.hasVideo
  }

  // Show/hide video panel and video intro/outro section
  const vPanel = $('editor-video-panel')
  if (vPanel) vPanel.style.display = E.isVideoFile ? '' : 'none'

  const audioIoSection = $('editor-audio-io-section')
  const videoIoSection = $('editor-video-io-section')
  if (audioIoSection) audioIoSection.style.display = E.isVideoFile ? 'none' : ''
  if (videoIoSection) videoIoSection.style.display = E.isVideoFile ? '' : 'none'
  applyJingleSupportUI()

  if (E.isVideoFile) {
    // Load the video via the Tauri asset:// protocol (the old Electron renderer
    // used a custom `media://current` scheme that doesn't exist in WKWebView, so
    // the video editor never showed a frame). convertFileSrc handles the path.
    //
    // (There is no `editorSetVideoPath` step any more: the shim mapped it to a
    // full `editor_load_recording` ffprobe whose result was thrown away — one
    // extra process per video open, buying nothing.)
    // Widen the asset:// scope to this one file first — a service recorded onto
    // an external drive matches none of the static scope globs, and the <video>
    // src then fails with nothing but an opaque media error.
    await window.api.editorAllowAssetPath(fp)
    if (seq !== E.loadSeq) return
    if (E.videoEl) {
      E.videoEl.src = window.api.toAssetUrl(fp)
      E.videoEl.load()
    }

    // Waveform for video: the backend extracts the audio + down-samples to peaks
    // (100/s, the renderer's rate) — we CAN'T decode the raw video bytes
    // client-side (a 1080p service is multi-GB, over the inline limit), and video
    // PLAYBACK uses the <video> element (asset://) so no AudioBuffer is needed.
    // E.peaks drives the waveform; E.duration comes from the video element.
    const result = await withLoadingDetail(
      t('editor.analyzingWaveform', 'Analyserer bølgeform…'),
      window.api.editorExtractAudioPeaks(fp),
      { channel: 'editor-peaks-progress' },
    )

    if (seq !== E.loadSeq) return

    let haveBackendPeaks = false
    if (result && Array.isArray(result.peaks) && result.peaks.length) {
      E.peaks = Float32Array.from(result.peaks)
      E.clipTimes = computeClipTimes(E.peaks)
      haveBackendPeaks = true
    }

    // Duration always comes from the video element for video files.
    {
      try {
        E.duration = await new Promise<number>((resolve, reject) => {
          if (!E.videoEl) { reject(new Error('no video element')); return }
          if (E.videoEl.readyState >= 1 && isFinite(E.videoEl.duration)) {
            resolve(E.videoEl.duration); return
          }
          // Single cleanup so neither the timeout nor the listeners outlive the
          // promise — an orphaned 15 s timer + stale listeners on a reused video
          // element corrupted the NEXT file's load when the user switched fast.
          let timer = 0
          const cleanup = () => {
            clearTimeout(timer)
            E.videoEl?.removeEventListener('loadedmetadata', onMeta)
            E.videoEl?.removeEventListener('error', onErr)
          }
          const onMeta = () => { cleanup(); resolve(E.videoEl?.duration ?? 0) }
          const onErr  = () => { cleanup(); reject(new Error('video error')) }
          E.videoEl.addEventListener('loadedmetadata', onMeta, { once: true })
          E.videoEl.addEventListener('error', onErr, { once: true })
          timer = window.setTimeout(() => {
            cleanup()
            reject(new Error('timeout waiting for video metadata'))
          }, 15000)
        })
        if (seq !== E.loadSeq) return
        // Only flat-fill when the backend gave NO peaks (else keep the real ones).
        if (!haveBackendPeaks) {
          E.peaks = new Float32Array(Math.ceil(E.duration * 100))
          console.log('[editor] video-only mode (flat waveform), duration:', E.duration.toFixed(1) + 's')
        }
      } catch (err) {
        console.error('[editor] could not determine video duration:', err)
        showEditorError('Kunne ikke laste videofil — filen er kanskje korrupt')
        showState('empty')
        return
      }
    }
  } else if (!(await loadAudioFile(fp, ext, seq))) {
    // Audio: one path for every format — element playback of the original, or
    // of a proxy when the webview has no decoder for it. Errors + the empty
    // state are handled inside.
    return
  }
  setLoadingDetail(null)

  fitAll()
  const fname = fp.split(/[/\\]/).pop() ?? fp
  const el = $('editor-filename')
  if (el) el.textContent = fname
  // Refresh header summary now that duration/cut state is known
  updateHeaderSummary()

  // Load intro/outro buffers from settings (non-blocking, audio only)
  if (!E.isVideoFile) loadIntroOutroBuffers(seq)

  // Load metadata sidecar (fire-and-forget — it carries `seq` so a slow read
  // for THIS file can't overwrite a newer file's metadata when it lands).
  void loadMetadataSidecar(fp, fname, seq)
  void loadTranscriptForFile(fp)

  // Restore unsaved cuts from a previous editing session that ended abruptly.
  // The sidecar is written every 2 s during editing and cleared on successful
  // export — finding one here means we crashed or were closed mid-edit.
  try {
    const draft = await window.api.editorReadCutsDraft(fp) as { cuts?: Array<{ start: number; end: number }>; ts?: number } | null
    if (draft && Array.isArray(draft.cuts) && draft.cuts.length > 0 && seq === E.loadSeq) {
      // Only restore if draft is fresher than 7 days (avoid surprising the user
      // with months-old leftover edits).
      const ageMs = draft.ts ? Date.now() - draft.ts : 0
      if (!draft.ts || ageMs < 7 * 86400_000) {
        E.cuts = draft.cuts.filter(c => typeof c.start === 'number' && typeof c.end === 'number' && c.end > c.start)
        E.cutHistory = [JSON.parse(JSON.stringify(E.cuts))]
        E.cutHistoryIdx = 0
        console.log('[editor] restored', E.cuts.length, 'unsaved cut(s) from draft')
      }
    }
  } catch {}

  renderCutList()
  updateRemainingDisplay()
  updateTimecode(0)
  updateTotalTime()

  // Default `Inkluder ved eksport` to ON when the user has at least one
  // intro/outro path configured — they almost always want their jingles
  // included, and showing the dimmed waveform on the timeline is the
  // whole point of the new layout.
  if (settings.editorIntroPath || settings.editorOutroPath) {
    E.includeIntroOutro = true
    const chk = $('editor-include-io') as HTMLInputElement | null
    if (chk) chk.checked = true
  }

  // Clipping badge (from the peak array we just built)
  const clipBadge = $('editor-clip-badge')
  if (clipBadge) {
    clipBadge.style.display = E.clipTimes.length > 0 ? '' : 'none'
    if (E.clipTimes.length > 0) clipBadge.textContent = `⚠ ${E.clipTimes.length} klipp`
  }

  showState('workspace')
  requestAnimationFrame(() => {
    syncCanvasSize()
    drawWaveform()
    drawMinimap()
    updateMinimapViewport()
  })

  if (E.pendingSeekSec != null) {
    const target = E.pendingSeekSec
    E.pendingSeekSec = null
    E.playStartSec = clampPlayable(snapOutOfCut(target))
    updateTimecode(E.playStartSec)
    seekMediaTo(clampMain(E.playStartSec))
    drawWaveform()
  }

  // Mastering section is only meaningful for audio files (the entire ffmpeg
  // pipeline + LUFS measurement is audio-only; mastering a video would not
  // touch the video stream and would just re-encode the audio track).
  const masterSection = $('editor-master-section')
  if (masterSection) masterSection.style.display = E.isVideoFile ? 'none' : ''

  // Auto-run segment analysis. Runs in the background so the editor is
  // immediately interactive — when analysis completes we surface the
  // auto-trim suggestion banner so the user can one-click trim to the sermon.
  // Skipped if cuts were restored from a draft (they're already editing).
  //
  // Fired after the peaks + transport are resolved (never off a load-time
  // timer): detection can be another full ffmpeg pass over the recording, and
  // starting it early raced the proxy transcode on the fallback path — two
  // ffmpeg runs over the same multi-gigabyte file at once. Deferred one step
  // further to idle time so the FIRST PAINT of the workspace is never competing
  // with a decode; on a reopen the backend answers from its segments cache and
  // this returns almost immediately anyway.
  if (!E.isVideoFile && E.cuts.length === 0) {
    whenIdle(() => {
      if (seq === E.loadSeq) void runDetection(true)
    })
  }
}

/**
 * Release the current file's playback transport: pause the player, drop its
 * `src` so the webview closes the file handle (and any temp proxy can be swept),
 * and clear the quality notice. Called on every load before a new file is read —
 * the element itself is kept for reuse.
 */
export function teardownPlayback(): void {
  // Tearing the current file down invalidates any queued draft write for it.
  cancelDraftSave()
  const notice = document.getElementById('editor-quality-notice')
  if (notice) notice.style.display = 'none'
  setLoadingDetail(null)
  const el = E.playerEl
  E.playerEl = null
  E.playbackSource = 'original'
  if (el) {
    try { el.pause() } catch {}
    el.volume = 1
    el.removeAttribute('src')
    try { el.load() } catch {}
  }
}

/** Extra line under the loading spinner ("Klargjør avspilling…"), or `null` to
 *  clear it. Only steps that take REAL time say anything here — a transcode or a
 *  first-time waveform decode. An unexplained wait reads as a hang; a label that
 *  flashes up for 30 ms on a cached open reads as jank. */
function setLoadingDetail(text: string | null): void {
  const el = $('editor-loading-detail')
  if (!el) return
  el.textContent = text ?? ''
  el.style.display = text ? '' : 'none'
}

/** Below this, a wait is imperceptible and labelling it just makes the loading
 *  screen flicker. A cached-peaks open lands well under it. */
const LOADING_DETAIL_DELAY_MS = 400

interface LoadingDetailOpts {
  /** Shim channel carrying `{ fraction }` for this step, if the backend has one. */
  channel?: string
  /** Override the reveal delay. 0 for a step that is never quick (a transcode). */
  delayMs?: number
}

/**
 * Await `work`, showing `text` under the spinner ONLY if it is still running
 * after {@link LOADING_DETAIL_DELAY_MS}. With the peaks sidecar cache a reopen
 * resolves in a few milliseconds, so the "Analyserer bølgeform…" line must never
 * get the chance to paint — but the first open of a 2 h service still has to
 * explain itself.
 *
 * The detail line is cleared only if this call is the one that set it, so a
 * slower step that owns the line afterwards isn't wiped by a fast one.
 *
 * With `opts.channel` the line gets a REAL bar and a remaining-time estimate
 * under it. Note the subscription starts immediately while the widget appears on
 * the same delay as the text: the backend's first tick can easily beat 400 ms,
 * and the newest fraction is held so the bar arrives already in the right place
 * instead of starting from zero.
 */
async function withLoadingDetail<T>(
  text: string,
  work: Promise<T>,
  opts: LoadingDetailOpts = {},
): Promise<T> {
  let shown = false
  // A holder rather than two `let`s: both are written from callbacks that run
  // long after this function's control flow has moved on.
  const bar: { ui: ProgressHandle | null; latest: number | null } = { ui: null, latest: null }
  const unsub = opts.channel
    ? window.api.on?.(opts.channel, (payload: unknown) => {
        const f = (payload as { fraction?: number } | null)?.fraction
        if (typeof f !== 'number' || !isFinite(f)) return
        bar.latest = Math.max(0, Math.min(1, f))
        bar.ui?.update(bar.latest)
      })
    : undefined
  const timer = window.setTimeout(() => {
    shown = true
    setLoadingDetail(text)
    const host = $('editor-loading-progress')
    if (opts.channel && host) {
      host.style.display = ''
      bar.ui = attachProgress(host, { compact: true })
      bar.ui.update(bar.latest)
    }
  }, opts.delayMs ?? LOADING_DETAIL_DELAY_MS)
  try {
    return await work
  } finally {
    clearTimeout(timer)
    unsub?.()
    bar.ui?.destroy()
    const host = $('editor-loading-progress')
    if (host) host.style.display = 'none'
    if (shown) setLoadingDetail(null)
  }
}

/**
 * Run `fn` when the main thread is next idle, so it can't compete with the
 * workspace's first paint. Falls back to a plain timer where
 * `requestIdleCallback` is missing (Safari/WKWebView, i.e. every macOS build) —
 * the deadline is the same either way, this is a deferral, not a scheduler.
 */
function whenIdle(fn: () => void): void {
  const ric = (window as unknown as {
    requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number
  }).requestIdleCallback
  if (typeof ric === 'function') ric(fn, { timeout: 1500 })
  else window.setTimeout(fn, 1500)
}

/** Show a small, non-blocking notice in the editor workspace: playback is going
 *  through a proxy, or could not be prepared at all. Never an error state — cuts
 *  and export run on the original either way. Reuses one element; auto-created. */
function showQualityNotice(text: string): void {
  let el = document.getElementById('editor-quality-notice')
  if (!el) {
    el = document.createElement('div')
    el.id = 'editor-quality-notice'
    el.style.cssText =
      'margin:8px 0;padding:8px 12px;border-radius:8px;font-size:13px;' +
      'background:rgba(239,180,75,.12);border:1px solid rgba(239,180,75,.4);color:var(--text2)'
    const anchor = document.getElementById('editor-workspace')
    anchor?.prepend(el)
  }
  el.textContent = text
  el.style.display = ''
}

export async function reloadIntroOutro(): Promise<void> {
  await loadIntroOutroBuffers(E.loadSeq)
}

export async function loadIntroOutroBuffers(seq: number): Promise<void> {
  const introPath = settings.editorIntroPath
  const outroPath = settings.editorOutroPath
  E.introBuffer = null; E.introDuration = 0; E.introPeaks = null
  E.outroBuffer = null; E.outroDuration = 0; E.outroPeaks = null

  updateEditorIntroOutroDisplay()

  // Jingles are the ONLY audio the renderer still decodes itself: they are a few
  // seconds long, and having them as AudioBuffers is what lets us schedule them
  // sample-accurately around the streamed recording. They are read as bytes
  // (`editorReadFile`, well under the inline limit) rather than streamed, so no
  // asset:// grant is needed for them. Decoded on the ONE shared context — a
  // fresh `new AudioContext()` per jingle used to leak two hardware contexts per
  // opened file, and macOS stops handing them out after a handful.
  async function decodeAudio(path: string): Promise<AudioBuffer | null> {
    try {
      const raw = await window.api.editorReadFile(path)
      if (!raw) return null
      const u8 = raw instanceof Uint8Array ? raw : new Uint8Array(raw as ArrayBuffer)
      return await sharedAudioCtx().decodeAudioData(
        u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength) as ArrayBuffer,
      )
    } catch { return null }
  }

  if (introPath) {
    const buf = await decodeAudio(introPath)
    if (seq === E.loadSeq && buf) {
      E.introBuffer = buf
      E.introDuration = buf.duration
      // Compute peaks via the same routine used for the main file — gives
      // a dimmed waveform on the left slot of the timeline.
      E.introPeaks = computeJinglePeaks(buf)
    }
  }
  if (outroPath) {
    const buf = await decodeAudio(outroPath)
    if (seq === E.loadSeq && buf) {
      E.outroBuffer = buf
      E.outroDuration = buf.duration
      E.outroPeaks = computeJinglePeaks(buf)
    }
  }
  if (seq === E.loadSeq) drawWaveform()
}

/**
 * Grey out the intro/outro rows the export cannot honour, and say why.
 *
 * The seam drops `introPath`/`outroPath` for video containers — silently, until
 * now: the pickers accepted a jingle, the export modal listed it, and the
 * finished mp4 did not contain it. The rows stay VISIBLE (so the feature is
 * discoverable when it lands) but are disabled behind the
 * `editor.jinglesVideoUnsupported` hint. Idempotent; called on every load.
 */
function applyJingleSupportUI(): void {
  const supported = jinglesSupportedForFile(E.filePath, E.isVideoFile)
  const hint = $('editor-video-io-unsupported')
  if (hint) hint.style.display = supported ? 'none' : ''
  const section = $('editor-video-io-section')
  if (!section) return
  const reason = supported
    ? ''
    : t('editor.jinglesVideoUnsupported', 'Jingler støttes ikke for video ennå')
  section.querySelectorAll<HTMLElement>('.editor-io-file-row').forEach(row => {
    row.style.opacity = supported ? '' : '.45'
    if (reason) row.title = reason
    else row.removeAttribute('title')
  })
  section.querySelectorAll<HTMLButtonElement>('button').forEach(btn => {
    btn.disabled = !supported
  })
}

export function updateVideoIntroOutroDisplay(): void {
  const introEl  = $('editor-video-intro-display')
  const outroEl  = $('editor-video-outro-display')
  const clrIntro = $('btn-editor-clear-video-intro') as HTMLElement | null
  const clrOutro = $('btn-editor-clear-video-outro') as HTMLElement | null
  if (introEl) {
    const name = E.videoIntroPath.split(/[/\\]/).pop() ?? ''
    introEl.textContent = name || 'Ingen fil valgt'
    introEl.style.color = name ? '' : 'var(--text3)'
    if (clrIntro) clrIntro.style.display = name ? '' : 'none'
  }
  if (outroEl) {
    const name = E.videoOutroPath.split(/[/\\]/).pop() ?? ''
    outroEl.textContent = name || 'Ingen fil valgt'
    outroEl.style.color = name ? '' : 'var(--text3)'
    if (clrOutro) clrOutro.style.display = name ? '' : 'none'
  }
}

export function updateEditorIntroOutroDisplay(): void {
  const introEl  = $('editor-intro-display')
  const outroEl  = $('editor-outro-display')
  const clrIntro = $('btn-editor-clear-intro') as HTMLElement | null
  const clrOutro = $('btn-editor-clear-outro') as HTMLElement | null
  const introPath = settings.editorIntroPath
  const outroPath = settings.editorOutroPath
  if (introEl) {
    const name = introPath?.split(/[/\\]/).pop() ?? ''
    introEl.textContent = name || 'Ingen fil valgt'
    introEl.style.color = name ? '' : 'var(--text3)'
    if (clrIntro) clrIntro.style.display = name ? '' : 'none'
  }
  if (outroEl) {
    const name = outroPath?.split(/[/\\]/).pop() ?? ''
    outroEl.textContent = name || 'Ingen fil valgt'
    outroEl.style.color = name ? '' : 'var(--text3)'
    if (clrOutro) clrOutro.style.display = name ? '' : 'none'
  }
}

/**
 * Read the `.meta` sidecar for `fp` and paint the metadata/chapter panels.
 *
 * `seq` is the caller's `E.loadSeq` at the time it started, re-checked after the
 * await like every other step in this file (see the header's invariant). It was
 * the one loader step that did NOT: called fire-and-forget, a slow read for file
 * A resolved after the user had opened B and stamped A's title, speaker,
 * description and chapters onto B — which then got SAVED under B on the next
 * metadata edit. Omitting `seq` keeps the old unguarded behaviour for any caller
 * that genuinely has no load in flight.
 */
export async function loadMetadataSidecar(
  fp: string,
  fname: string,
  seq?: number,
): Promise<void> {
  const raw = await window.api.editorReadMeta(fp)
  if (seq !== undefined && seq !== E.loadSeq) return
  if (raw && typeof raw === 'object') {
    E.meta = raw as RecordingMetadata
  } else {
    // Auto-fill title from filename (strip extension)
    E.meta = {
      title: fname.replace(/\.[^.]+$/, '').replace(/_redigert(_\d+)?$/, '').replace(/_/g, ' '),
      speaker: '',
      description: '',
      chapters: [],
    }
  }
  renderMetaPanel()
  renderChapterList()
}
