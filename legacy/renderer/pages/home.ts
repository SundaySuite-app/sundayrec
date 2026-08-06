import { t, currentLang } from '../i18n'
import { settings, patchSettings } from '../state'
import { fmtCountdown, fmtStorageHours, fmtDate } from '../helpers'
import { startVU } from './home-vu'
import { releaseRendererAudioCaptures } from './recording'
import { errText } from './audio-page'
import { getAudioDevices } from '../audio/capture'
import { refreshReviewQueue, setupReviewQueueListeners } from './review-queue-home'
import { navigateTo } from '../ui/navigate'
import { subscribePrerollStatus } from '../preroll-lifecycle'
import { buildHealthFindings } from '../status/health-findings'
import { toWarningView } from '../status/backend-warning-core'
import { firstMount, resetMount, showEl, hideEl } from '../ui/motion'
import { banner, dismissBanner, toast } from '../ui/toast'
import {
  dismissMissed,
  dismissPreflight,
  getNextRecordingState,
  initNextRecordingStore,
  refreshNextRecording,
  setPreflightFindings,
  subscribe as subscribeNextRecording,
  syncScheduleSettings,
} from '../status/next-recording'
import {
  formatCountdown,
  formatMissed,
  formatMissedBanner,
  formatNextDate,
  formatNextTitle,
  formatPreflightHeadline,
  formatSidebarStatus,
  formatWakeHint,
  intlParts,
  type DeviceStatus,
  type FormatCtx,
  type NextRecordingState,
} from '../status/next-recording-core'
import type { PreflightFinding } from '../../bindings/PreflightFinding'
import type { RecordingEntry } from './history'

let countdownTimer: ReturnType<typeof setInterval> | null = null

export function deactivateHome(): void {
  if (countdownTimer) { clearInterval(countdownTimer); countdownTimer = null }
  // Bug 3: restore VU section + info-cards to original DOM positions when
  // navigating away from home (so the page stays clean if returned to).
  relocateVuForVideoMode(false)
}

// ── Video preview state ──────────────────────────────────────────────────────

let previewActive         = false
let previewStream:        MediaStream | null = null

// ── Video-mode layout: relocate VU, preview section and info cards ──────────
//
// In audio-only mode the page is a vertical stack: hero → quick-row →
// horizontal VU → info-strip (3 cards) → history. When video is toggled on
// we physically move those elements into a 3-column grid (#video-mode-layout):
// left = vertical VU, middle = video preview, right = info-card column. When
// video is toggled off we move every element back to its original position
// so audio-only mode remains pixel-identical to v4.40.0.
//
// Doing this by DOM-relocation (rather than duplicating elements) means every
// existing event handler / live-update / ID reference keeps working without
// modification. The function is idempotent — safe to call repeatedly.
// FLAGGED for future refactor: this relocation physically moves live nodes
// between two layouts and remembers where each came from. It works, and every
// handler survives because the elements are the same objects — but it makes the
// DOM's shape depend on a boolean and on the order the moves happened in, which
// is why "restore" has to walk the record backwards. A CSS-grid layout with both
// arrangements expressible in place would remove the bookkeeping entirely.
// Deliberately NOT touched in the 2026-08 UX overhaul: it is load-bearing for
// video mode and deserves its own change with its own testing.
interface MoveRecord { el: HTMLElement; parent: Element; next: Node | null }
let _videoLayoutMoves: MoveRecord[] = []
let _videoLayoutActive = false

function relocateVuForVideoMode(enabled: boolean): void {
  const layout = document.getElementById('video-mode-layout')
  if (!layout) return
  const previewSlot = layout.querySelector<HTMLElement>('.video-mode-preview-slot')
  const cardSlot    = layout.querySelector<HTMLElement>('.info-card-column')
  if (!previewSlot || !cardSlot) return

  if (enabled) {
    if (_videoLayoutActive) return
    _videoLayoutActive = true
    layout.style.display = 'grid'

    const move = (el: HTMLElement | null, target: HTMLElement): void => {
      if (!el || !el.parentElement) return
      _videoLayoutMoves.push({ el, parent: el.parentElement, next: el.nextSibling })
      target.appendChild(el)
    }

    // Video preview section first — so the VU can be appended into the same
    // card right after, giving the user the one-card "video + lyd-helhet"
    // look that the Direktesending page has.
    const preview = document.getElementById('video-preview-section') as HTMLElement | null
    move(preview, previewSlot)

    // VU goes INSIDE the .video-preview-card so preview and Lydnivå appear
    // as a single unified card (matching the Direktesending design). CSS
    // targets `.video-preview-card .vu-section` to re-skin it as a flat
    // bottom-strip like .live-vu-section.
    const vu = document.querySelector<HTMLElement>('#page-home > .vu-section')
    const previewCard = preview?.querySelector<HTMLElement>('.video-preview-card')
    if (vu && previewCard) move(vu, previewCard)

    // Place info-cards in the side column in a specific order so video-
    // mode reads top→bottom as: Lydkilde, Kamera, Videokvalitet, Lagring,
    // Format. The cards live in two physically separate strips on the
    // page (the audio strip + #video-info-strip), so we pick each by a
    // stable inner anchor and append individually — order in the source
    // markup doesn't matter.
    const audioStrip = document.querySelector<HTMLElement>('#page-home > .info-strip:not(.video-info-strip)')
    const videoStrip = document.getElementById('video-info-strip') as HTMLElement | null

    const findAudioCard = (innerId: string): HTMLElement | null =>
      audioStrip?.querySelector<HTMLElement>(`#${innerId}`)?.closest<HTMLElement>('.info-card') ?? null
    const findVideoCard = (innerId: string): HTMLElement | null =>
      videoStrip?.querySelector<HTMLElement>(`#${innerId}`)?.closest<HTMLElement>('.info-card') ?? null

    const ordered: Array<HTMLElement | null> = [
      findAudioCard('home-device-name'),     // LYDKILDE
      findVideoCard('home-video-device-name'), // KAMERA
      findVideoCard('home-video-quality'),     // VIDEOKVALITET
      findAudioCard('home-storage-value'),     // LAGRING
      document.getElementById('home-format-card'), // FORMAT
    ]
    for (const card of ordered) {
      if (card) move(card, cardSlot)
    }

    // «Siste opptak» fills the empty space UNDER the cards in the right column
    // (it spans the full column width via CSS). When the window is narrow / the
    // video is large, the whole column stacks below the preview and carries the
    // history with it — so no extra scroll in the roomy case, graceful in the
    // cramped one.
    move(document.getElementById('home-lower'), cardSlot)
  } else {
    if (!_videoLayoutActive) return
    _videoLayoutActive = false
    // Move everything back in reverse order so insertBefore(nextSibling)
    // targets are valid even when we re-insert into a now-empty parent.
    for (let i = _videoLayoutMoves.length - 1; i >= 0; i--) {
      const { el, parent, next } = _videoLayoutMoves[i]
      try { parent.insertBefore(el, next) } catch { parent.appendChild(el) }
    }
    _videoLayoutMoves = []
    layout.style.display = 'none'
  }
}

type HomeVideoDevice = { name: string; index: number }

function applyVideoFlipState(): void {
  const flipped = settings.videoFlip ?? false
  document.getElementById('video-preview-img')?.classList.toggle('video-flip', flipped)
  document.getElementById('video-preview-video')?.classList.toggle('video-flip', flipped)
  document.getElementById('btn-home-video-flip')?.classList.toggle('flip-active', flipped)
}

/** Apply a Home video-feed size preset: 'l' (large, default) | 'm' | 's'. Smaller
 *  presets shrink the video column and reflow the info cards into the freed width
 *  (CSS classes on #page-home), so there's no wasted space. Also reflects the
 *  active state on the segmented control. */
function applyHomeVideoSize(size: 's' | 'm' | 'l'): void {
  const page = document.getElementById('page-home')
  if (page) {
    page.classList.remove('vsize-s', 'vsize-m', 'vsize-l')
    page.classList.add(`vsize-${size}`)
  }
  document.querySelectorAll<HTMLElement>('.video-size-seg button').forEach(b =>
    b.classList.toggle('active', b.dataset.vsize === size))
}

export function updateVideoToggleButton(): void {
  const btn   = document.getElementById('btn-video-toggle')
  const label = document.getElementById('video-toggle-label')
  const on    = settings.videoEnabled ?? false
  if (!btn || !label) return
  label.textContent = on ? t('home.videoOn', 'Video på') : t('home.videoOff', 'Video av')
  btn.classList.toggle('video-toggle-on', on)
  updateAudioSeparateButton()
}

export function updateAudioSeparateButton(): void {
  const btn   = document.getElementById('btn-audio-separate') as HTMLElement | null
  const label = document.getElementById('audio-separate-label')
  const card  = document.getElementById('home-format-card')
  if (!btn || !label) return
  const videoOn   = settings.videoEnabled ?? false
  const keepAudio = settings.videoKeepAudio ?? true
  btn.style.display = videoOn ? 'inline-flex' : 'none'
  btn.classList.toggle('audio-separate-on', keepAudio)
  btn.setAttribute('aria-checked', keepAudio ? 'true' : 'false')
  label.textContent = keepAudio ? t('home.audioSeparate', 'Separat lydfil') : t('home.audioNoFile', 'Ingen lydfil')
  // Grey out the whole FORMAT card when video is on but separate audio is off
  card?.classList.toggle('format-inactive', videoOn && !keepAudio)
}

// ── Silent preflight (proactive issue surfacing) ─────────────────────────
//
// We run the same preflight check the user can trigger manually from the
// Lyd settings page, but silently in the background after home loads. Any
// findings — typically "disk almost full", "mic permission denied", "saved
// device not found" — go into the shared store, so they land on the SAME card
// the backend's pre-start check (scheduler://preflight, 30 min before a
// scheduled start) renders. Two sources, one surface: the user should not have
// to learn two different renderings of the same finding.
//
// Runs ONCE per app launch (not per home-tab visit) to avoid pestering the
// user with stale issues they've already seen.

let silentPreflightHasRun = false

/**
 * The OS-permission + sidecar findings, in front of whatever `run_preflight`
 * found. A blocked microphone must be visible BEFORE the user meets a generic
 * getUserMedia failure, and `run_preflight` does not ask AVFoundation — the two
 * commands that do (`media_permissions`, `ffmpeg_health`) had no caller at all.
 * Best-effort: an unavailable probe adds nothing rather than blocking the card.
 */
export async function collectHealthFindings(): Promise<PreflightFinding[]> {
  const [permissions, ffmpeg] = await Promise.all([
    window.api.mediaPermissions?.().catch(() => null) ?? null,
    window.api.ffmpegHealth?.().catch(() => null) ?? null,
  ])
  return buildHealthFindings({
    permissions,
    ffmpeg,
    videoEnabled: !!settings.videoEnabled,
    t,
  })
}

async function runSilentPreflightOnce(): Promise<void> {
  if (silentPreflightHasRun) return
  silentPreflightHasRun = true
  try {
    const [health, r] = await Promise.all([
      collectHealthFindings(),
      window.api.runPreflight() as Promise<{ findings?: PreflightFinding[] }>,
    ])
    const findings = [...health, ...(r.findings ?? [])]
    if (findings.length) setPreflightFindings(findings)
  } catch {
    // Preflight unavailable — silently ignore (not user-facing failure)
  }
}

export async function refreshHomeVideoDevices(): Promise<void> {
  const sel   = document.getElementById('home-video-device-select') as HTMLSelectElement | null
  if (!sel) return
  sel.disabled = true
  sel.replaceChildren(Object.assign(document.createElement('option'), {
    value: '', textContent: t('home.cameraSearching', 'Leter etter kameraer…'),
  }))
  const phTxt = document.getElementById('video-preview-placeholder-text')

  try {
    const devices = await window.api.listVideoDevices() as HomeVideoDevice[]
    sel.innerHTML = ''

    const blank = document.createElement('option')
    blank.value = ''; blank.textContent = t('home.cameraSelect', 'Velg kamera…')
    sel.appendChild(blank)

    devices.forEach(d => {
      const opt = document.createElement('option')
      opt.value = String(d.index)
      opt.dataset.name = d.name
      opt.textContent = d.name
      sel.appendChild(opt)
    })

    if (!devices.length) {
      if (phTxt) phTxt.textContent = t('home.cameraNoneFound', 'Ingen kameraer funnet — sjekk tilkobling')
      sel.disabled = false
      return
    }

    const savedName = settings.videoDeviceName ?? ''
    const match = savedName ? devices.find(d => d.name === savedName) : null
    sel.value = match ? String(match.index) : ''
    sel.disabled = false

    // Bug 6: inform user when previously saved camera is no longer available
    if (savedName && !match && phTxt) {
      phTxt.textContent = t('home.cameraSavedMissing', 'Kamera "{name}" ikke funnet — velg et annet').replace('{name}', savedName)
    } else if (phTxt) {
      phTxt.textContent = sel.value ? t('home.cameraStarting', 'Starter kamera…') : t('home.cameraPickAndRefresh', 'Velg kamera og trykk oppdater')
    }
  } catch (err) {
    console.warn('[home] device list failed:', err)
    sel.innerHTML = ''
    sel.appendChild(Object.assign(document.createElement('option'), { value: '', textContent: t('home.cameraListError', 'Feil ved lasting') }))
    sel.disabled = false
    const phTxt2 = document.getElementById('video-preview-placeholder-text')
    if (phTxt2) phTxt2.textContent = t('home.cameraListFailed', 'Kunne ikke hente kameraliste — sjekk tillatelser')
    const phDiv2 = document.getElementById('video-preview-placeholder')
    if (phDiv2) phDiv2.style.display = ''
  }
}

async function applyHomeVideoDeviceSelection(): Promise<void> {
  const sel = document.getElementById('home-video-device-select') as HTMLSelectElement | null
  if (!sel) return
  const idx  = sel.value
  const opt  = sel.selectedOptions[0]
  const name = (opt?.dataset.name ?? opt?.textContent ?? '').trim() || null
  const idxN = idx ? parseInt(idx) : null

  stopVideoPreview()
  patchSettings({ videoDeviceName: name, videoDeviceIndex: idxN })
  await window.api.saveSettings({ ...settings })
  loadVideoInfoStrip()

  if (name) {
    startVideoPreview()
  } else {
    const phDiv = document.getElementById('video-preview-placeholder')
    const phTxt = document.getElementById('video-preview-placeholder-text')
    if (phTxt) phTxt.textContent = t('home.cameraPickAndRefresh', 'Velg kamera og trykk oppdater')
    if (phDiv) phDiv.style.display = ''
  }
}

/** Show the live feed's true resolution + fps as an overlay on the preview. */
function showFeedResolution(video: HTMLVideoElement, stream: MediaStream): void {
  const el = document.getElementById('video-preview-res')
  if (!el) return
  const s = stream.getVideoTracks()[0]?.getSettings()
  const w = video.videoWidth || s?.width || 0
  const h = video.videoHeight || s?.height || 0
  const fps = s?.frameRate ? Math.round(s.frameRate) : 0
  if (w && h) {
    el.textContent = fps ? `${w}×${h} · ${fps} fps` : `${w}×${h}`
    el.style.display = ''
  } else {
    el.style.display = 'none'
  }
}

export function stopVideoPreview(): void {
  previewActive = false
  // Release the camera (client-side getUserMedia preview) so the recorder can
  // open it when recording starts.
  if (previewStream) { previewStream.getTracks().forEach(t => t.stop()); previewStream = null }
  const video = document.getElementById('video-preview-video') as HTMLVideoElement | null
  const img   = document.getElementById('video-preview-img') as HTMLImageElement | null
  const phDiv = document.getElementById('video-preview-placeholder')
  const resEl = document.getElementById('video-preview-res')
  if (video) { video.srcObject = null; video.style.display = 'none'; video.onloadedmetadata = null }
  if (img)   { img.src = ''; img.style.display = 'none' }
  if (resEl) { resEl.style.display = 'none' }
  if (phDiv) { phDiv.style.display = '' }
}

// The live camera preview is a CLIENT-SIDE getUserMedia stream piped into a
// <video> element — it works in WKWebView with no backend, where the old
// Electron MJPEG-over-IPC preview did not (the Tauri backend writes a preview
// JPEG to a file, not IPC frames). The RECORDING still uses the backend ffmpeg
// device; this is preview only, and it's released (stopVideoPreview) the moment
// recording starts so the recorder can take the camera (macOS gives one client
// the capture device at a time).
export async function startVideoPreview(): Promise<void> {
  const section = document.getElementById('video-preview-section')
  updateVideoToggleButton()

  if (!settings.videoEnabled) {
    if (section) section.style.display = 'none'
    return
  }
  if (section) section.style.display = ''

  const phDiv  = document.getElementById('video-preview-placeholder')
  const phTxt  = document.getElementById('video-preview-placeholder-text')
  const video  = document.getElementById('video-preview-video') as HTMLVideoElement | null

  if (!settings.videoDeviceName) {
    if (phTxt) phTxt.textContent = t('home.cameraPickAndRefresh', 'Velg kamera og trykk oppdater')
    if (phDiv) phDiv.style.display = ''
    return
  }

  if (previewActive) return
  previewActive = true
  if (phTxt) phTxt.textContent = t('home.cameraStarting', 'Starter kamera…')
  if (phDiv) phDiv.style.display = ''

  try {
    // Request the configured resolution in 16:9 so the preview matches the
    // recording (the default getUserMedia mode is 640×480 4:3 → letterboxed).
    const RES_DIMS: Record<string, [number, number]> = {
      '480p': [854, 480], '720p': [1280, 720], '1080p': [1920, 1080], '2160p': [3840, 2160],
    }
    const [rw] = RES_DIMS[settings.videoResolution ?? '720p'] ?? [1280, 720]
    // The preview is only a MONITOR — it never needs more than 1080p. Asking a
    // 1080p camera (e.g. FaceTime HD) for 4K made WKWebView collapse the
    // unsatisfiable width+height+aspectRatio ideals into a cropped 1920×1920
    // SQUARE (zoomed in). Cap the request at 1080p and specify width + aspectRatio
    // ONLY (no fighting height) so the browser always returns a clean 16:9 frame
    // at the camera's real max. The overlay still reports the true delivered size.
    const videoConstraint: MediaTrackConstraints = {
      width:       { ideal: Math.min(rw, 1920) },
      aspectRatio: { ideal: 16 / 9 },
    }
    // Map the chosen camera (an ffmpeg device NAME) to a browser deviceId by
    // label; fall back to the default camera. enumerateDevices only exposes
    // labels after a getUserMedia grant, so on first run we just use the default.
    try {
      const devs = await navigator.mediaDevices.enumerateDevices()
      const cam  = devs.find(d =>
        d.kind === 'videoinput' && !!settings.videoDeviceName &&
        d.label && d.label.includes(settings.videoDeviceName))
      if (cam?.deviceId) videoConstraint.deviceId = { ideal: cam.deviceId }
    } catch { /* enumerate needs permission first — fall back to default device */ }

    const stream = await navigator.mediaDevices.getUserMedia({ video: videoConstraint, audio: false })
    if (!previewActive) { stream.getTracks().forEach(t => t.stop()); return } // stopped while awaiting
    previewStream = stream
    if (video) {
      video.srcObject = stream
      video.style.display = ''
      await video.play().catch(() => {})
      // Show the camera's ACTUAL delivered resolution/fps once metadata is in —
      // makes it obvious when the live feed differs from the recording setting
      // (e.g. a 1080p webcam with "4K" chosen). videoWidth/Height is the real
      // decoded frame size; the track's frameRate is the negotiated rate.
      showFeedResolution(video, stream)
      video.onloadedmetadata = () => showFeedResolution(video, stream)
    }
    if (phDiv) phDiv.style.display = 'none'
  } catch (err) {
    previewActive = false
    const name = (err as DOMException)?.name
    if (phTxt) phTxt.textContent = name === 'NotAllowedError'
      ? t('home.cameraDenied', 'Kameratilgang nektet — sjekk Systeminnstillinger')
      : t('home.cameraNoResponse', 'Kamera svarte ikke — prøv å oppdatere')
    if (phDiv) phDiv.style.display = ''
    if (video) video.style.display = 'none'
  }
}

/** Exported so other pages can trigger a disk-space refresh after changing format/channels/samplerate */
export { loadDiskSpace as refreshHomeDiskSpace }

// ── Status alert cards: missed recordings + pre-start check ─────────────────

function alertItem(text: string, className?: string): HTMLLIElement {
  const li = document.createElement('li')
  li.textContent = text
  if (className) li.className = className
  return li
}

/** How many findings/missed rows a card lists before collapsing the rest. */
const ALERT_LIST_MAX = 4

/**
 * Render both alert cards from the shared state.
 *
 * A missed recording ALSO raises a persistent banner: the card is on Home, and
 * "the church service did not get recorded" is not news that should wait until
 * someone navigates there.
 */
function renderStatusAlerts(state: NextRecordingState): void {
  const ctx = fmtCtx()

  // ── Missed recordings ─────────────────────────────────────────────────────
  const missedCard = document.getElementById('missed-card')
  const missedTitle = document.getElementById('missed-card-title')
  const missedList = document.getElementById('missed-card-list')
  const missedHeadline = formatMissedBanner(state, ctx)

  if (missedHeadline) showEl(missedCard); else hideEl(missedCard)
  if (missedHeadline) {
    if (missedTitle) missedTitle.textContent = missedHeadline
    if (missedList) {
      const rows = state.missed.slice(0, ALERT_LIST_MAX).map(m => alertItem(formatMissed(m, ctx)))
      const rest = state.missed.length - rows.length
      if (rest > 0) {
        rows.push(alertItem(`+ ${rest} ${t('missed.more', 'flere')}`, 'home-banner-list-more'))
      }
      missedList.replaceChildren(...rows)
    }
    banner('scheduler-missed', 'error', missedHeadline, [
      {
        label: t('missed.bannerAction', 'Vis detaljer'),
        onClick: () => navigateTo('home', { anchor: 'missed-card' }),
      },
    ])
  } else {
    dismissBanner('scheduler-missed')
  }

  // ── Pre-start check ───────────────────────────────────────────────────────
  const pfCard = document.getElementById('preflight-card')
  const pfTitle = document.getElementById('preflight-card-title')
  const pfList = document.getElementById('preflight-card-list')
  const pf = formatPreflightHeadline(state.preflight, ctx)

  if (pfCard) {
    if (pf) showEl(pfCard); else hideEl(pfCard)
    // Errors and warnings are different news; the card says which.
    pfCard.classList.toggle('home-banner-error', pf?.severity === 'error')
    pfCard.classList.toggle('home-banner-warn', pf?.severity !== 'error')
  }
  if (pf) {
    if (pfTitle) pfTitle.textContent = pf.text
    if (pfList) {
      // Errors first — they are what stops a recording.
      const sorted = [...state.preflight].sort(
        (a, b) => (a.severity === 'error' ? 0 : 1) - (b.severity === 'error' ? 0 : 1),
      )
      const rows = sorted.slice(0, ALERT_LIST_MAX).map(f => alertItem(f.message))
      const rest = sorted.length - rows.length
      if (rest > 0) {
        rows.push(alertItem(`+ ${rest} ${t('status.preflightMore', 'flere — se Innstillinger → Lyd')}`, 'home-banner-list-more'))
      }
      pfList.replaceChildren(...rows)
    }
  }
}

// ── Post-recording summary helpers ──────────────────────────────────────────

function fmtDurationSec(sec: number): string {
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = sec % 60
  if (h > 0 && m > 0) return `${h}t ${m}m`
  if (h > 0)           return `${h}t`
  if (m > 0 && s > 0)  return `${m}m ${s}s`
  if (m > 0)           return `${m}m`
  return `${s}s`
}

function fmtFileSizeBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`
  return `${Math.round(bytes / 1e3)} KB`
}

/**
 * "Fullført — 1t 12m · 84 MB · ☁ GD" after a take.
 *
 * This used to reach into the EDITOR PROMPT's toast and overwrite its title
 * element — two unrelated messages sharing one surface, so the summary only
 * appeared if the prompt happened to be showing, and it destroyed that prompt's
 * own headline when it did. It now has its own toast, and touches nothing else.
 */
function showRecordingFinishedSummary(entry: RecordingEntry): void {
  const parts: string[] = []
  if (entry.durationSec != null && entry.durationSec > 0)
    parts.push(fmtDurationSec(entry.durationSec))
  if (entry.fileSizeBytes != null && entry.fileSizeBytes > 0)
    parts.push(fmtFileSizeBytes(entry.fileSizeBytes))

  const cloudNames: Record<string, string> = { 'google-drive': 'GD', 'dropbox': 'DB', 'onedrive': 'OD' }
  const uploadedServices = (entry.cloudUploaded ?? []).map(s => cloudNames[s] ?? s)
  if (uploadedServices.length) parts.push('☁ ' + uploadedServices.join(' ☁ '))

  const done = t('history.complete', 'Fullført')
  const msg = parts.length ? `${done} — ${parts.join(' · ')}` : done

  // Only offer the editor here when the editor PROMPT isn't going to (it is
  // shown by pages/recording.ts unless the user turned it off). Two buttons
  // opening the same file is one button too many.
  const promptWillShow = settings.askOpenEditor !== false
  const path = entry.path
  toast('success', msg, path && !promptWillShow
    ? {
        action: {
          label: t('home.openInEditor', 'Åpne i redigering'),
          onClick: () => window.openEditorWithFile(path),
        },
      }
    : {})
}

// ── Progress bar shared by the two long audio checks ────────────────────────
//
// Neither check reports real progress from the backend, so the UI must not
// pretend otherwise: the 30 s test recording shows a determinate bar the copy
// explicitly calls an estimate ("ca."), and the 60 s capture bench — which
// previously showed NOTHING for a full minute — gets an indeterminate bar plus
// a truthful elapsed-seconds counter.

function healthProgress(mode: 'determinate' | 'indeterminate' | 'off', pct = 0): void {
  const bar = document.getElementById('health-progress')
  const fill = document.getElementById('health-progress-fill')
  if (!bar || !fill) return
  if (mode === 'off') {
    bar.style.display = 'none'
    bar.classList.remove('indeterminate')
    bar.removeAttribute('aria-valuenow')
    fill.style.width = '0'
    return
  }
  bar.style.display = ''
  bar.classList.toggle('indeterminate', mode === 'indeterminate')
  if (mode === 'determinate') {
    const clamped = Math.max(0, Math.min(100, pct))
    fill.style.width = `${clamped}%`
    bar.setAttribute('aria-valuenow', String(Math.round(clamped)))
  } else {
    fill.style.width = ''
    bar.removeAttribute('aria-valuenow')
  }
}

export function setupHome(): void {
  // Wire up Test-recording and Preflight buttons. Both used to live on Home but
  // were moved to Settings → Lyd in the UX reorganization. The "btn-go-health"
  // anchor on Home jumps to that section. Buttons are bound by ID so both the
  // old IDs (if present anywhere) and the new "-settings" IDs are handled.
  const runTestRecording = async (btnId: string, statusId: string): Promise<void> => {
    const btn = document.getElementById(btnId) as HTMLButtonElement | null
    const status = document.getElementById(statusId)
    if (!btn || !status) return
    if (window.__isRecording) {
      status.textContent = t('home.testBusy', 'Kan ikke kjøre test mens et opptak pågår.')
      status.style.color = 'var(--red)'
      return
    }
    btn.disabled = true
    const originalText = btn.textContent ?? ''
    let elapsed = 0
    const TOTAL = 30
    status.style.color = 'var(--text2)'
    // "ca." because this counter is a local timer, not backend progress: the
    // command returns when it returns, and the number can reach 30/30 while the
    // recording is still finishing.
    const fmtProgress = (n: number): string =>
      t('home.testProgress', 'Tar opp test… ca. {n}/{total} s')
        .replace('{n}', String(n)).replace('{total}', String(TOTAL))
    status.textContent = fmtProgress(0)
    healthProgress('determinate', 0)
    const tick = setInterval(() => {
      elapsed++
      status.textContent = fmtProgress(Math.min(elapsed, TOTAL))
      healthProgress('determinate', (elapsed / TOTAL) * 100)
      if (elapsed >= TOTAL) clearInterval(tick)
    }, 1000)
    try {
      const r = await window.api.runTestRecording() as {
        ok: boolean
        signal?: 'silent' | 'low' | 'normal'
        sizeBytes?: number
        error?: string
        detail?: string
      }
      clearInterval(tick)
      if (r.ok) {
        const sizeKb = r.sizeBytes ? Math.round(r.sizeBytes / 1024) : 0
        const signalLabel = r.signal === 'normal' ? t('home.testSignalOk',     '✅ Lyd OK')
                          : r.signal === 'low'    ? t('home.testSignalLow',    '⚠️ Svak lyd — sjekk gain på mikser')
                          :                         t('home.testSignalSilent', '⚠️ Stillhet — mikser av?')
        status.textContent = `${signalLabel} (${sizeKb} KB)`
        status.style.color = r.signal === 'normal' ? 'var(--green)' : 'var(--orange, #ffb46b)'
      } else {
        // run_test_recording returns a machine code in `r.error`
        // (device_not_found / device_permission_denied / ffmpeg_error / no_audio);
        // localize it rather than print the raw code (there is no `r.detail`).
        const testErrMap: Record<string, string> = {
          device_not_found: t('recording.errorDeviceNotFound', 'Lydenheten ble ikke funnet — sjekk lydkort og tillatelser'),
          device_permission_denied: t('recording.errorPermission', 'Tilgang til lydenheten ble nektet — sjekk systeminnstillingene'),
          ffmpeg_error: t('home.testFfmpegError', 'Feil i opptaksmotoren — prøv igjen'),
          no_audio: t('home.testSignalSilent', '⚠️ Stillhet — mikser av?'),
        }
        const detail = (r.error && testErrMap[r.error]) ?? r.error ?? t('home.testUnknownError', 'Ukjent feil')
        status.textContent = `❌ ${detail}`
        status.style.color = 'var(--red)'
      }
    } catch (err) {
      clearInterval(tick)
      status.textContent = `❌ ${(err as Error).message}`
      status.style.color = 'var(--red)'
    } finally {
      clearInterval(tick)
      healthProgress('off')
      btn.disabled = false
      btn.textContent = originalText
    }
  }

  const runPreflight = async (btnId: string, statusId: string, listId: string): Promise<void> => {
    const btn = document.getElementById(btnId) as HTMLButtonElement | null
    const status = document.getElementById(statusId)
    const list = document.getElementById(listId) as HTMLUListElement | null
    if (!btn || !status || !list) return
    btn.disabled = true
    status.textContent = t('home.checking', 'Sjekker…')
    status.style.color = 'var(--text2)'
    list.style.display = 'none'
    list.innerHTML = ''
    try {
      // Same two sources as the silent run — the manual button must not be able
      // to report "alt ser bra ut" while the OS is blocking the microphone.
      const [health, raw] = await Promise.all([
        collectHealthFindings(),
        window.api.runPreflight() as Promise<{ findings?: PreflightFinding[] }>,
      ])
      const r = { findings: [...health, ...(raw.findings ?? [])] }
      if (!r.findings || r.findings.length === 0) {
        status.textContent = t('home.preflightAllOk', '✅ Alt ser bra ut — systemet er klart for opptak.')
        status.style.color = 'var(--green)'
      } else {
        const errors = r.findings.filter(f => f.severity === 'error').length
        const warns  = r.findings.filter(f => f.severity === 'warn').length
        const parts: string[] = []
        if (errors > 0) parts.push(`${errors} ${t('home.preflightErrors', 'feil')}`)
        if (warns  > 0) parts.push(`${warns} ${t('home.preflightWarns', 'advarsel')}`)
        status.textContent = `${errors > 0 ? '❌' : '⚠️'} ${parts.join(', ')}`
        status.style.color = errors > 0 ? 'var(--red)' : 'var(--orange, #ffb46b)'

        const sorted = [...r.findings].sort((a, b) => (a.severity === 'error' ? -1 : 1) - (b.severity === 'error' ? -1 : 1))
        for (const f of sorted) {
          const li = document.createElement('li')
          const isErr = f.severity === 'error'
          li.style.cssText = `padding:6px 10px;margin:4px 0;border-radius:6px;background:${isErr ? 'rgba(232,120,120,0.12)' : 'rgba(255,180,107,0.12)'};color:${isErr ? 'var(--red)' : 'var(--orange, #ffb46b)'};display:flex;gap:8px`
          const icon = document.createElement('span')
          icon.textContent = isErr ? '❌' : '⚠️'
          icon.style.flexShrink = '0'
          const text = document.createElement('span')
          text.textContent = f.message
          li.append(icon, text)
          list.appendChild(li)
        }
        list.style.display = 'block'
      }
    } catch (err) {
      status.textContent = `❌ ${(err as Error).message}`
      status.style.color = 'var(--red)'
    } finally {
      btn.disabled = false
    }
  }

  // Legacy IDs (btn-test-recording / btn-run-preflight) were removed from the
  // Home card in v4.31 — buttons now live exclusively on Innstillinger → Lyd.
  document.getElementById('btn-test-recording-settings')?.addEventListener('click', () => runTestRecording('btn-test-recording-settings', 'health-status-settings'))
  // Precision capture bench: real recording argv for 60 s, ffprobed and judged
  // — the zero-loss proof tool (2026-07-31). Uses the shared health status line.
  document.getElementById('btn-capture-bench')?.addEventListener('click', async () => {
    const btn = document.getElementById('btn-capture-bench') as HTMLButtonElement | null
    const status = document.getElementById('health-status-settings')
    if (!btn || !status) return
    btn.disabled = true
    // 60 seconds used to pass with a single static line and no other sign of
    // life — indistinguishable from a hang. An indeterminate bar says "working",
    // the counter says how long it has been working, and neither claims to know
    // how far along the backend is (it does not report that).
    const benchStarted = Date.now()
    const benchLine = (): string => {
      const secs = Math.floor((Date.now() - benchStarted) / 1000)
      return `${t('audio.benchRunning', 'Måler i 60 sek — spill av lyd/snakk i mikrofonen …')} (${secs} s)`
    }
    status.textContent = benchLine()
    healthProgress('indeterminate')
    const benchTick = setInterval(() => { status.textContent = benchLine() }, 1000)
    // The bench must measure the RECORDING PATH alone: release every
    // renderer-side mic consumer first (terminal-verified 2026-07-31: a live
    // getUserMedia on the same device skews the source itself).
    releaseRendererAudioCaptures()
    try {
      const r = await window.api.runCaptureBench(60)
      clearInterval(benchTick)
      const loss = Math.max(0, (r.expectedSec ?? 0) - (r.measuredSec ?? 0))
      const pct = r.expectedSec ? (loss / r.expectedSec * 100) : 0
      status.textContent = `${r.verdict === 'pass' ? '✅' : r.verdict === 'warn' ? '⚠️' : '❌'} ` +
        t('audio.benchResult', 'Målt {m}s av {e}s ({p} % tap)')
          .replace('{m}', (r.measuredSec ?? 0).toFixed(2))
          .replace('{e}', (r.expectedSec ?? 0).toFixed(0))
          .replace('{p}', pct.toFixed(2)) +
        (r.reasons?.length ? ` — ${r.reasons.join('; ')}` : '')
    } catch (err) {
      status.textContent = '❌ ' + errText(err)
    } finally {
      clearInterval(benchTick)
      healthProgress('off')
    }
    if (!window.__isRecording) startVU() // give the home meter back
    btn.disabled = false
  })
  document.getElementById('btn-run-preflight-settings')?.addEventListener('click',  () => runPreflight('btn-run-preflight-settings',  'health-status-settings', 'preflight-findings-settings'))

  // Home → Settings → Lyd quick-jump (replaces the old inline test buttons)
  document.getElementById('btn-go-health')?.addEventListener('click', e => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-audio', anchor: 'btn-test-recording-settings', highlight: false })
  })

  // Video toggle button — always toggles, loads devices inline if turning on
  document.getElementById('btn-video-toggle')?.addEventListener('click', async () => {
    const nowEnabled = !(settings.videoEnabled ?? false)
    patchSettings({ videoEnabled: nowEnabled })
    await window.api.saveSettings({ ...settings })
    updateVideoToggleButton()
    loadVideoInfoStrip()
    // The Home "Video på" toggle and Innstillinger → Video are ONE setting —
    // keep the settings checkbox in sync live so they never disagree (mirrors the
    // audio-separate toggle's cross-sync).
    const settingsToggle = document.getElementById('opt-video-enable') as HTMLInputElement | null
    if (settingsToggle) {
      settingsToggle.checked = nowEnabled
      const panel = document.getElementById('video-settings-panel')
      if (panel) panel.style.display = nowEnabled ? '' : 'none'
    }

    const pageHome = document.getElementById('page-home')
    if (nowEnabled) {
      pageHome?.classList.add('video-mode')
      const section = document.getElementById('video-preview-section')
      if (section) section.style.display = ''
      relocateVuForVideoMode(true)
      await refreshHomeVideoDevices()
      if (settings.videoDeviceName && !window.__isRecording) startVideoPreview()
    } else {
      pageHome?.classList.remove('video-mode')
      relocateVuForVideoMode(false)
      stopVideoPreview()
      const section = document.getElementById('video-preview-section')
      if (section) section.style.display = 'none'
    }
  })

  // Separate audio toggle — keep a high-quality audio file alongside the
  // combined MP4. Mirrors Innstillinger → Video → "Behold separat lydfil", and
  // propagates to that toggle live so the two never disagree.
  //
  // The SWITCH is the control. Clicking anywhere on the FORMAT card used to
  // flip this setting too, which meant a volunteer who tapped the card to read
  // its bitrate silently changed what the next recording would produce — an
  // invisible state change from a gesture that looked like inspection.
  const toggleSeparateAudio = async (): Promise<void> => {
    const nowKeep = !(settings.videoKeepAudio ?? true)
    patchSettings({ videoKeepAudio: nowKeep })
    await window.api.saveSettings({ ...settings })
    updateAudioSeparateButton()
    // Sync the Video-tab toggle (no-op if the tab hasn't been opened yet)
    const videoToggle = document.getElementById('opt-video-keep-audio') as HTMLInputElement | null
    if (videoToggle && videoToggle.checked !== nowKeep) videoToggle.checked = nowKeep
  }
  const separateSwitch = document.getElementById('btn-audio-separate')
  separateSwitch?.addEventListener('click', e => {
    e.stopPropagation()
    if (settings.videoEnabled ?? false) void toggleSeparateAudio()
  })
  // Keyboard activation for the switch (role="switch", tabindex=0)
  separateSwitch?.addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      if (settings.videoEnabled ?? false) void toggleSeparateAudio()
    }
  })

  // Inline camera refresh button
  document.getElementById('btn-home-video-refresh')?.addEventListener('click', async () => {
    stopVideoPreview()
    await refreshHomeVideoDevices()
    await applyHomeVideoDeviceSelection()
  })

  // Horizontal flip toggle — CSS-only for preview (instant, no restart), ffmpeg hflip for recording
  document.getElementById('btn-home-video-flip')?.addEventListener('click', async () => {
    const nowFlipped = !(settings.videoFlip ?? false)
    patchSettings({ videoFlip: nowFlipped })
    await window.api.saveSettings({ ...settings })
    applyVideoFlipState()
  })

  // Inline camera device selector — save + restart preview on change
  document.getElementById('home-video-device-select')?.addEventListener('change', async () => {
    await applyHomeVideoDeviceSelection()
  })

  // Video-feed size — small / medium / large. A smaller feed reflows the info
  // cards into the freed width (no wasted space), useful when the window fills the
  // screen. Persisted, so the choice sticks across sessions.
  const sizeSeg = document.querySelector<HTMLElement>('.video-size-seg')
  if (sizeSeg) {
    const saved = localStorage.getItem('sundayrec.homeVideoSize')
    const init: 's' | 'm' | 'l' = saved === 's' || saved === 'm' ? saved : 'l'
    applyHomeVideoSize(init)
    sizeSeg.querySelectorAll<HTMLElement>('button').forEach(b => {
      b.addEventListener('click', () => {
        const size = (b.dataset.vsize as 's' | 'm' | 'l') ?? 'l'
        applyHomeVideoSize(size)
        try { localStorage.setItem('sundayrec.homeVideoSize', size) } catch { /* ignore */ }
      })
    })
  }

  const goVideoSettings = (e: Event) => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-video' })
  }
  document.getElementById('btn-go-video-source')?.addEventListener('click', goVideoSettings)
  document.getElementById('btn-go-video-quality')?.addEventListener('click', goVideoSettings)

  document.getElementById('btn-go-audio-page')?.addEventListener('click', e => {
    e.preventDefault()
    e.stopPropagation()
    navigateTo('settings', { tab: 'settings-audio', anchor: '#settings-audio .card' })
  })
  // Tapping the LYDKILDE card itself lands on the CHANNEL GRID — the «is the
  // right channel feeding the recording?» check, one tap from Home.
  document.getElementById('home-audio-card')?.addEventListener('click', () => {
    navigateTo('settings', { tab: 'settings-audio', anchor: 'channel-grid-card' })
  })
  document.getElementById('btn-go-audio-fmt')?.addEventListener('click', e => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-files', anchor: 'format-group' })
  })
  document.getElementById('btn-go-general-page')?.addEventListener('click', e => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-files', anchor: '#settings-files .card' })
  })

  // Publish-strip cards — all three route to the Publisering SECTION of the
  // Deling tab (cloud + thumbnail UI lives there; Whisper has no dedicated
  // settings surface yet, so we land users there and they can browse from
  // there until we promote Whisper config out of the editor).
  const goPublish = (anchor?: string) => (e: Event) => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-sharing', anchor: anchor ?? '#settings-publish' })
  }
  document.getElementById('btn-go-cloud')?.addEventListener('click',   goPublish('#settings-publish .cloud-grid'))
  document.getElementById('btn-go-thumb')?.addEventListener('click',   goPublish('#publish-thumb-preview'))
  document.getElementById('btn-go-whisper')?.addEventListener('click', goPublish())
  document.getElementById('btn-how-to-fix')?.addEventListener('click', () => {
    navigateTo('settings', { tab: 'settings-audio' })
  })
  document.getElementById('btn-how-to-fix-audio')?.addEventListener('click', e => {
    e.preventDefault()
    navigateTo('settings', { tab: 'settings-audio' })
  })

  // "Se alle →" jumps to the merged «Søk & historikk» tab — the full history +
  // its tools (delete / note / prune / clear) and sermon search now live there.
  // Home only shows the 5 most recent recordings.
  document.getElementById('home-see-all')?.addEventListener('click', e => {
    e.preventDefault()
    navigateTo('search')
  })

  const onDeviceChange = (): void => {
    // Skip during active recording — opening getUserMedia competes with ffmpeg's AVFoundation session
    if (!window.__isRecording) void checkStatus()
  }
  navigator.mediaDevices.addEventListener('devicechange', onDeviceChange)
  window.addEventListener('beforeunload', () =>
    navigator.mediaDevices.removeEventListener('devicechange', onDeviceChange))

  // Alert-card actions. The cards themselves are rendered by the store
  // subscriber below; these buttons only ever change state.
  document.getElementById('btn-missed-dismiss')?.addEventListener('click', () => dismissMissed())
  document.getElementById('btn-missed-schedule')?.addEventListener('click', () => {
    navigateTo('schedule')
  })
  document.getElementById('btn-preflight-dismiss')?.addEventListener('click', () => dismissPreflight())
  document.getElementById('btn-preflight-settings')?.addEventListener('click', () => {
    navigateTo('settings', { tab: 'settings-audio', anchor: 'btn-run-preflight-settings', highlight: false })
  })

  // One subscription for the whole app lifetime: the sidebar status is chrome,
  // visible from every page, so it must keep up whether or not Home is open.
  initNextRecordingStore()
  subscribeNextRecording(state => {
    renderNextRecording(state)
    renderStatusAlerts(state)
  })

  wireHomeIpcListeners()
}

// The `window.api.on` subscriptions below live for the app's lifetime, but the
// unsubscribes are kept and the wiring is guarded so a re-run of `setupHome`
// can never stack duplicate handlers.
let homeIpcWired = false
const homeIpcUnsubs: Array<(() => void) | undefined> = []
function wireHomeIpcListeners(): void {
  if (homeIpcWired) return
  homeIpcWired = true

  // Backend warning — preroll/cloud/recovery/device/disk issues.
  //
  // The day referred to by the 2026-08-05 channel audit ("the intended receiver
  // the day the backend starts emitting") is here: `crate::notify::warn` emits
  // on `backend://warning`, now mapped in api-shim. The payload carries a stable
  // `code`, so the toast is LOCALIZED rather than showing the backend's
  // Norwegian `msg` to a German user — with `msg` kept as the fallback for a
  // code this renderer build does not know yet. See status/backend-warning-core.
  homeIpcUnsubs.push(window.api.on('backend-warning', (data: unknown) => {
    const view = toWarningView(data, t)
    if (view) toast(view.kind, view.text)
  }))

  // Post-recording summary in existing editor prompt toast. (recording.ts also
  // listens to 'recording-finished', but for a different job — overlay teardown
  // + editor prompt; this one only shows the summary toast.)
  homeIpcUnsubs.push(window.api.on('recording-finished', (entry: unknown) => {
    const rec = entry as RecordingEntry & { splitRestart?: boolean } | undefined
    if (rec && !rec.splitRestart) showRecordingFinishedSummary(rec)
  }))

  // Wire up the review-queue card — listens to IPC events from main so the card
  // updates instantly when a new prep lands or the user publishes/discards.
  setupReviewQueueListeners()

  // The pre-roll buffer's own surface on the LYDKILDE card.
  homeIpcUnsubs.push(subscribePrerollStatus(renderPrerollChip))

  // Tray menu hooks used to live here as `tray-open-review-queue` /
  // `tray-run-preflight` listeners — Electron channel names no Rust code has
  // ever emitted, so both were unreachable. The Rust tray emits ONE
  // `tray://action` event; it is adapted in tray-actions.ts, wired once in
  // main.ts, and calls openReviewQueueFromTray / the preflight button from there.
}

/** Render the pre-roll chip on the LYDKILDE card. The rolling buffer holds the
 *  microphone in the background, so while it runs it says so — driven by the
 *  BACKEND's status, never by the setting alone (the backend declines to start
 *  when it can't match the device, and a chip that claimed otherwise would be
 *  exactly the kind of lie this phase is removing). */
function renderPrerollChip(active: boolean, seconds: number): void {
  const chip = document.getElementById('home-preroll-chip')
  const text = document.getElementById('home-preroll-text')
  if (!chip) return
  if (!active) { chip.style.display = 'none'; return }
  if (text) {
    text.textContent = t('home.prerollActive', 'Forhåndsbuffer aktiv ({n} s)')
      .replace('{n}', String(seconds))
  }
  chip.style.display = ''
}

/** Bring the review-queue card to the front, freshly loaded — the destination of
 *  the tray's "📬 N episoder klare" row. Exported for tray-actions.ts. */
export function openReviewQueueFromTray(): void {
  navigateTo('home', { anchor: '#review-queue-card' })
  refreshReviewQueue().catch(err =>
    console.warn('[home] review-queue refresh from tray failed:', err),
  )
}

/** The info cards that wait on an async load, so they can show a skeleton
 *  instead of a "—" that reads as a real (and alarming) answer. */
const SKELETON_CARDS = ['home-audio-card', 'home-format-card', 'home-storage-card']

function setCardLoading(id: string, loading: boolean): void {
  document.getElementById(id)?.classList.toggle('is-loading', loading)
}

/**
 * Skeleton only what is genuinely unknown. A card that already shows a real
 * value keeps showing it while the refresh runs — flashing a shimmer over
 * correct data on every return to Home would be motion for its own sake.
 */
function markUnknownCardsLoading(): void {
  for (const id of SKELETON_CARDS) {
    const value = document.getElementById(id)?.querySelector('.info-card-value')?.textContent?.trim()
    if (!value || value === '—') setCardLoading(id, true)
  }
}

export async function refreshHome(): Promise<void> {
  // The next recording comes from the store (event-fed, polled as a fallback).
  // `syncScheduleSettings` re-derives against the settings this app now has —
  // `setupHome` subscribes before `loadSettings` has run, so without this the
  // hero could show "set up a schedule" for one frame to a user who has one.
  // It renders synchronously through the subscription; the poll then confirms.
  syncScheduleSettings()
  startCountdownTicker()
  void refreshNextRecording()

  // Each loader clears its own card when its data lands, so a slow disk query
  // doesn't hold the device card hostage.
  markUnknownCardsLoading()

  await Promise.all([
    loadDiskSpace(),
    renderRecentRecordings(),
    checkStatus(),
    loadHomeInfoStrip(),
    refreshReviewQueue(),
  ])
  // LEAK GUARD (2026-07-31 audit): navigating home mid-recording used to
  // reopen the getUserMedia meter stream — a second microphone owner beside
  // the recorder's ffmpeg for the rest of the take.
  if (!window.__isRecording) startVU()

  // Once-per-session silent preflight. Surfaces critical issues (disk full,
  // mic permission denied, device missing) on home as a banner *without*
  // requiring the user to click "Sjekk system". This is the "proactive
  // disk-space warning" requested by the external review.
  void runSilentPreflightOnce()

  updateVideoToggleButton()
  applyVideoFlipState()
  loadVideoInfoStrip()

  const pageHome = document.getElementById('page-home')
  if (settings.videoEnabled) {
    pageHome?.classList.add('video-mode')
    const section = document.getElementById('video-preview-section')
    if (section) section.style.display = ''
    relocateVuForVideoMode(true)
    refreshHomeVideoDevices().then(() => {
      if (settings.videoDeviceName && !window.__isRecording) startVideoPreview()
    }).catch((err) => {
      console.warn('[home] device list failed:', err)
      const phTxt = document.getElementById('video-preview-placeholder-text')
      if (phTxt) phTxt.textContent = t('home.cameraListFailed', 'Kunne ikke hente kameraliste — sjekk tillatelser')
      const phDiv = document.getElementById('video-preview-placeholder')
      if (phDiv) phDiv.style.display = ''
    })
  } else {
    pageHome?.classList.remove('video-mode')
    relocateVuForVideoMode(false)
    stopVideoPreview()
    const section = document.getElementById('video-preview-section')
    if (section) section.style.display = 'none'
  }
}

// ── "Next recording" rendering (store-driven) ───────────────────────────────
//
// The hero title, the countdown, the wake badge and the sidebar status label
// all render `status/next-recording`'s state. None of them computes anything:
// before this they each fetched and formatted the next start themselves and
// could disagree — the sidebar showing one time while the hero showed another,
// the countdown frozen mid-take, the wake badge promising a time the backend
// never planned.

/** The one place the app's language becomes a date format. */
function fmtCtx(nowMs = Date.now()): FormatCtx {
  return {
    t,
    parts: intlParts(currentLang === 'no' ? 'nb-NO' : currentLang),
    nowMs,
  }
}

/** Device connectivity is the sidebar's other input; `checkStatus` owns it. */
let deviceStatus: DeviceStatus = { connected: true }

function renderSidebarStatus(state: NextRecordingState, ctx: FormatCtx): void {
  const dot = document.getElementById('status-dot')
  const lbl = document.getElementById('status-label')
  const s = formatSidebarStatus(state, ctx, deviceStatus)
  if (dot) dot.className = 'status-dot' + (s.dot ? ` ${s.dot}` : '')
  if (lbl) lbl.textContent = s.text
}

function renderNextRecording(state: NextRecordingState = getNextRecordingState()): void {
  const ctx = fmtCtx()

  const titleEl = document.getElementById('hero-ready-title')
  const dateEl = document.getElementById('next-date')
  const cntEl = document.getElementById('next-countdown')
  const heroNextEl = document.getElementById('hero-next-section')
  const wakeBadge = document.getElementById('next-wake-badge')

  if (titleEl) titleEl.textContent = formatNextTitle(state, ctx)
  if (heroNextEl) heroNextEl.style.display = state.next ? '' : 'none'
  if (dateEl) dateEl.textContent = formatNextDate(state, ctx)
  if (cntEl) cntEl.textContent = formatCountdown(state, ctx, fmtCountdown)

  if (wakeBadge) {
    const hint = formatWakeHint(state, ctx)
    wakeBadge.textContent = hint ?? ''
    wakeBadge.style.display = hint ? '' : 'none'
  }

  renderSidebarStatus(state, ctx)
}

/**
 * 1 Hz countdown tick, running only while Home is the visible page
 * (`deactivateHome` clears it). It no longer stops during a recording: the one
 * moment you most want to know when the next service starts is mid-take, and
 * that was exactly when the number used to freeze. One text write a second is
 * a price worth paying for a number that is true.
 */
function startCountdownTicker(): void {
  if (countdownTimer) clearInterval(countdownTimer)
  countdownTimer = setInterval(() => {
    const cntEl = document.getElementById('next-countdown')
    if (!cntEl) return
    cntEl.textContent = formatCountdown(getNextRecordingState(), fmtCtx(), fmtCountdown)
  }, 1000)
}

async function loadDiskSpace(): Promise<void> {
  const disk       = await window.api.getDiskSpace()
  setCardLoading('home-storage-card', false)
  const storageVal = document.getElementById('home-storage-value')
  const storageSub = document.getElementById('home-storage-sub')

  const folder = settings.saveFolder ?? ''
  let folderShort = t('home.defaultFolder', 'Dokumenter/SundayRec')
  if (folder) {
    const parts = folder.replace(/\\/g, '/').split('/').filter(Boolean)
    folderShort = parts.length > 1 ? `…/${parts.at(-2)}/${parts.at(-1)}` : (parts[0] ?? folder)
  }

  if (!disk?.freeBytes) {
    if (storageVal) { storageVal.textContent = '—'; storageVal.style.color = '' }
    if (storageSub) storageSub.textContent = folderShort
    return
  }

  const gb  = disk.freeBytes / 1e9
  const fmt = (settings.format ?? 'mp3').toLowerCase()
  let kbps: number
  if (fmt === 'wav') {
    const sr = parseInt(String(settings.sampleRate ?? 48000))
    const ch = settings.channels === 'stereo' ? 2 : 1
    kbps = Math.round(sr * ch * 16 / 1000)
  } else if (fmt === 'flac') {
    kbps = settings.channels === 'stereo' ? 600 : 350
  } else {
    kbps = parseInt(String(settings.bitrate ?? 256))
  }
  const hours  = Math.floor(disk.freeBytes / (kbps * 125 * 3600))
  const recEst = fmtStorageHours(hours)
  if (storageVal) {
    storageVal.textContent = `${gb.toFixed(1)} GB ${t('home.storageFree', 'ledig')}`
    storageVal.style.color = gb < 1 ? 'var(--red)' : gb < 5 ? 'var(--yellow, #fbbf24)' : ''
  }
  if (storageSub) storageSub.textContent = `${folderShort} · ca. ${recEst}`

  const diskMetaEl = document.getElementById('rec-disk')
  if (diskMetaEl) diskMetaEl.textContent = `${gb.toFixed(0)} GB`
}

/**
 * Compact «Siste opptak» on home: the 5 most-recent recordings as a light,
 * read-only list (open-in-editor on row click + reveal/edit icons). The full
 * history with its tools (delete / note / prune / clear) and sermon search now
 * live in the «Søk & historikk» tab — reached via the "Se alle →" link.
 */
export async function renderRecentRecordings(): Promise<void> {
  const tbody = document.getElementById('home-recent')
  if (!tbody) return
  const history = ((await window.api.getHistory()) ?? []) as RecordingEntry[]
  const recent = history.slice(0, 5)
  tbody.innerHTML = ''
  // Entrance on ARRIVAL only. This list is re-rendered after every finished
  // recording, every delete and every editor save — restaggering all five rows
  // each time made the page look like it was reloading itself.
  const animate = firstMount(tbody)
  if (!recent.length) {
    resetMount(tbody)
    const td = Object.assign(document.createElement('td'), {
      colSpan: 4,
      textContent: t('history.empty', 'Ingen opptak ennå')
    })
    td.style.cssText = 'color:var(--text3);text-align:center;padding:16px'
    const tr = document.createElement('tr'); tr.appendChild(td); tbody.appendChild(tr)
    return
  }
  recent.forEach((r, idx) => {
    const tr = document.createElement('tr')
    tr.className = animate ? 'hist-row row-in' : 'hist-row'
    if (animate) tr.style.animationDelay = `${idx * 0.04}s`
    const badgeCls = r.status === 'ok' || r.status === 'complete' ? 'ok' : r.status === 'error' ? 'error' : 'sched'
    tr.dataset.status = badgeCls

    const timeStr = r.startTime ? ` kl. ${r.startTime}` : ''
    const cells = [r.date ? `${fmtDate(r.date)}${timeStr}` : '—', r.duration ?? '—', r.filename ?? '—']
    cells.forEach((text, i) => {
      const td = document.createElement('td')
      td.textContent = text
      if (i === 2 && r.path) td.title = r.path
      tr.appendChild(td)
    })

    // Read-only actions: reveal + open-in-editor (no delete/note on the home
    // overview — those live in the «Søk & historikk» tab).
    const tdActions = document.createElement('td')
    tdActions.style.cssText = 'white-space:nowrap;display:flex;align-items:center;gap:3px'
    if (r.path) {
      const aReveal = document.createElement('a')
      aReveal.href = '#'; aReveal.className = 'hist-action'
      aReveal.title = 'Vis i Finder / Utforsker'
      aReveal.innerHTML = '<svg viewBox="0 0 20 20"><path d="M11 3a1 1 0 100 2h2.586l-6.293 6.293a1 1 0 101.414 1.414L15 6.414V9a1 1 0 102 0V4a1 1 0 00-1-1h-5zM5 5a2 2 0 00-2 2v8a2 2 0 002 2h8a2 2 0 002-2v-3a1 1 0 10-2 0v3H5V7h3a1 1 0 000-2H5z"/></svg>'
      aReveal.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); window.api.revealFile(r.path!) })
      tdActions.appendChild(aReveal)

      const aEdit = document.createElement('a')
      aEdit.href = '#'; aEdit.className = 'hist-action'
      aEdit.title = t('editor.title', 'Rediger lydfil')
      aEdit.innerHTML = '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3 10h14M3 6h3m11 0h-3M3 14h3m11 0h-3" stroke-linecap="round"/><circle cx="7.5" cy="6" r="1.5" fill="currentColor" stroke="none"/><circle cx="12.5" cy="14" r="1.5" fill="currentColor" stroke="none"/></svg>'
      aEdit.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); window.openEditorWithFile(r.path!) })
      tdActions.appendChild(aEdit)

      tr.style.cursor = 'pointer'
      tr.addEventListener('click', () => window.openEditorWithFile(r.path!))
    }
    tr.appendChild(tdActions)
    tbody.appendChild(tr)
  })
}

async function checkStatus(): Promise<void> {
  const devices = await getAudioDevices()
  let connected = !settings.deviceId || devices.some(d => d.deviceId === settings.deviceId)

  // Auto-heal: Windows often reassigns device IDs after reboot or driver update.
  // If the stored ID is gone but a device with the same label exists, silently update.
  if (!connected && settings.deviceId && settings.deviceName) {
    const byLabel = devices.find(d =>
      d.label && d.label.toLowerCase() === (settings.deviceName ?? '').toLowerCase()
    )
    if (byLabel) {
      patchSettings({ deviceId: byLabel.deviceId })
      await window.api.saveSettings({ ...settings })
      connected = true
    }
  }

  const heroOk   = document.getElementById('hero-ok')
  const heroWarn = document.getElementById('hero-warn')
  if (heroOk)   heroOk.style.display   = connected ? 'flex' : 'none'
  if (heroWarn) heroWarn.style.display = connected ? 'none' : 'flex'

  // Update hero-warn detail with device name so user knows what to reconnect
  if (!connected && settings.deviceName) {
    const warnDetail = document.getElementById('hero-warn-detail')
    if (warnDetail) {
      warnDetail.textContent = t('home.reconnectDevice', 'Koble til {name} via USB')
        .replace('{name}', settings.deviceName)
    }
  }

  // The sidebar dot/label is rendered from the shared state, not computed here:
  // this function's only contribution is whether the device is present.
  deviceStatus = { connected, name: settings.deviceName ?? null }
  renderSidebarStatus(getNextRecordingState(), fmtCtx())
}

export function loadVideoInfoStrip(): void {
  const strip = document.getElementById('video-info-strip')
  if (!strip) return

  if (!settings.videoEnabled) {
    strip.style.display = 'none'
    return
  }
  strip.style.display = ''

  const nameEl    = document.getElementById('home-video-device-name')
  const statusEl  = document.getElementById('home-video-device-status')
  const qualityEl = document.getElementById('home-video-quality')
  const modeEl    = document.getElementById('home-video-mode')

  if (nameEl)   nameEl.textContent  = settings.videoDeviceName ?? '—'
  if (statusEl) {
    if (settings.videoDeviceName) {
      statusEl.textContent = t('home.videoSourceConfigured', 'Kilde konfigurert')
      statusEl.style.color = 'var(--green)'
    } else {
      statusEl.textContent = t('home.videoNoCamera', 'Ingen kamera valgt')
      statusEl.style.color = 'var(--text3)'
    }
  }

  const res     = settings.videoResolution ?? '720p'
  const fps     = settings.videoFramerate  ?? 30
  const bitrate = (settings.videoBitrate && settings.videoBitrate > 0)
    ? ` · ${settings.videoBitrate} kbps`
    : ''
  if (qualityEl) qualityEl.textContent = `${res} · ${fps} fps${bitrate}`
  if (modeEl)    modeEl.textContent    = settings.videoSeparate ? 'Separate filer (video + lyd)' : 'Kombinert MP4'
}

export async function loadHomeInfoStrip(): Promise<void> {
  const devices  = await getAudioDevices()
  setCardLoading('home-audio-card', false)
  setCardLoading('home-format-card', false)
  const device   = settings.deviceId ? devices.find(d => d.deviceId === settings.deviceId) : devices[0]
  const nameEl   = document.getElementById('home-device-name')
  const statusEl = document.getElementById('home-device-status-text')
    ?? document.getElementById('home-device-status')
  if (nameEl)   nameEl.textContent   = device?.label ?? t('audio.builtIn', 'Standardenhet')
  if (statusEl) {
    const connected = !settings.deviceId || devices.some(d => d.deviceId === settings.deviceId)
    // Know-before-you-record: show WHICH channels feed the recording (the
    // channel grid's stored mapping) so a wrong source is visible from Home.
    const stored = settings.deviceId ? settings.deviceChannels?.[settings.deviceId] : null
    const mode = settings.channels ?? 'stereo'
    let chLine = ''
    if (stored) {
      chLine = mode === 'monoL'
        ? t('home.sourceChannelMono', 'Kanal {n}').replace('{n}', String((stored.channelL ?? 0) + 1))
        : mode === 'monoR'
          ? t('home.sourceChannelMono', 'Kanal {n}').replace('{n}', String((stored.channelR ?? 1) + 1))
          : t('home.sourceChannels', 'Kanal {l}/{r}')
              .replace('{l}', String((stored.channelL ?? 0) + 1))
              .replace('{r}', String((stored.channelR ?? 1) + 1))
      const modeLabel = mode === 'stereo' ? t('audio.stereo', 'Stereo') : 'Mono'
      chLine = `${chLine} · ${modeLabel} — `
    }
    statusEl.textContent = chLine + t(connected ? 'home.deviceConnected' : 'home.deviceMissing')
    statusEl.style.color = connected ? 'var(--green)' : 'var(--red)'
  }

  const fmt     = (settings.format ?? 'mp3').toUpperCase()
  const hasBr   = settings.format !== 'flac' && settings.format !== 'wav'
  const br      = hasBr ? `${settings.bitrate ?? 256}k` : ''
  const ch      = settings.channels === 'stereo' ? t('audio.stereo', 'Stereo') : t('audio.monoL', 'Mono')
  const srHz    = parseInt(String(settings.sampleRate ?? 44100))
  const srLabel = `${(srHz / 1000).toFixed(srHz % 1000 === 0 ? 0 : 1)} kHz`
  const fmtEl   = document.getElementById('home-format-value')
  const fmtSub  = document.getElementById('home-format-sub')
  if (fmtEl) fmtEl.textContent = br ? `${fmt} · ${br}` : fmt
  if (fmtSub) fmtSub.textContent = `${ch} · ${srLabel}`

  // Refresh the publish/cloud/transcript strip — each card decides whether
  // to show itself based on settings + actual disk/network state. Smart
  // visibility: nothing is rendered when none of the three are configured,
  // keeping the home page short for fresh users.
  void loadPublishInfoStrip()
}

/**
 * Loads the bottom info-strip with: sky-backup status, episodebilde
 * (cover art) and transkripsjon (Whisper). Each card individually toggles
 * its own display — the parent strip is hidden when all three are off.
 */
async function loadPublishInfoStrip(): Promise<void> {
  const strip = document.getElementById('publish-info-strip')
  if (!strip) return

  const cloudShown   = renderCloudCard()
  const thumbShown   = renderThumbCard()
  // Whisper status is async (queries main for installed models) — we run
  // it without awaiting so the synchronous cards above don't block on it.
  const whisperShownPromise = renderWhisperCard()

  // Show the strip as soon as ONE card decided it has something to render.
  // Without this the strip would briefly flash on every load while we wait
  // on whisper-status.
  if (cloudShown || thumbShown) {
    strip.style.display = ''
  }
  const whisperShown = await whisperShownPromise
  strip.style.display = (cloudShown || thumbShown || whisperShown) ? '' : 'none'
}

/** @returns true when the cloud card was rendered visible. */
function renderCloudCard(): boolean {
  const card = document.getElementById('home-cloud-card')
  if (!card) return false
  const services: Array<{ key: 'cloudGoogleDrive' | 'cloudDropbox' | 'cloudOneDrive'; label: string }> = [
    { key: 'cloudGoogleDrive', label: 'Drive' },
    { key: 'cloudDropbox',     label: 'Dropbox' },
    { key: 'cloudOneDrive',    label: 'OneDrive' },
  ]
  const active = services.filter(s => settings[s.key]?.enabled)
  if (active.length === 0) {
    card.style.display = 'none'
    return false
  }
  card.style.display = ''
  const valEl = document.getElementById('home-cloud-services')
  const subEl = document.getElementById('home-cloud-status')
  if (valEl) valEl.textContent = active.map(a => a.label).join(' · ')

  // Show queue length if any cloud uploads are pending — this is the most
  // useful runtime info: "1 venter på opplasting" vs "Alle synkronisert".
  if (subEl) {
    subEl.textContent = t('home.cloudActive', 'Aktiv')
    subEl.style.color = ''
    void (async () => {
      try {
        const q = await window.api.cloudQueueStatus()
        const pending = q.entries?.filter(e => e.status === 'pending' || e.status === 'retrying').length ?? 0
        const failed  = q.entries?.filter(e => e.status === 'failed').length ?? 0
        if (failed > 0)       { subEl.textContent = `${failed} ${t('home.cloudFailed', 'feilet')}`;   subEl.style.color = 'var(--red)' }
        else if (pending > 0) { subEl.textContent = `${pending} ${t('home.cloudQueued', 'i kø')}`;    subEl.style.color = 'var(--text2)' }
        else                  { subEl.textContent = t('home.cloudAllSynced', 'Alle synkronisert');   subEl.style.color = 'var(--green)' }
      } catch {
        // Queue status unavailable — leave the static "Aktiv" label.
      }
    })()
  }
  return true
}

/** @returns true when the thumbnail card was rendered visible. */
function renderThumbCard(): boolean {
  const card = document.getElementById('home-thumb-card')
  if (!card) return false
  const path = settings.defaultThumbnailPath
  if (!path) {
    card.style.display = 'none'
    return false
  }
  card.style.display = ''
  const nameEl = document.getElementById('home-thumb-name')
  const subEl  = document.getElementById('home-thumb-sub')
  const iconSlot = card.querySelector<HTMLElement>('.home-thumb-icon-slot')
  if (nameEl) {
    const base = path.split('/').pop() ?? path
    nameEl.textContent = base
  }
  if (subEl) {
    // HONEST: nothing burns this image into anything — the whole thumbnail
    // backend is unwritten (every thumbnail* IPC method is a stub), so the old
    // green «Brennes inn i podcast-MP3» was a promise the app cannot keep. The
    // card only appears at all for users carrying a path from an older build.
    subEl.textContent = t('home.thumbComing', 'Episodebilde kommer — brukes ikke ennå')
    subEl.style.color = 'var(--text3)'
  }
  // The «Endre» action would land on a panel that is itself gated as «kommer».
  const action = document.getElementById('btn-go-thumb')
  if (action) {
    action.setAttribute('inert', '')
    action.classList.add('gate-off')
  }
  // Swap the placeholder SVG for an actual <img> preview via the asset://
  // protocol (WKWebView blocks file://). Falling back to the icon keeps the slot
  // from collapsing if the file disappeared (error listener — NOT an inline
  // onerror attribute, which the strict CSP (script-src 'self') would block).
  if (iconSlot) {
    const img = document.createElement('img')
    img.className = 'thumb-card-icon thumb-card-icon-home'
    img.alt = ''
    img.addEventListener('error', () => { img.style.display = 'none' })
    img.src = window.api.toAssetUrl(path)
    iconSlot.replaceChildren(img)
  }
  return true
}

/** @returns true when the transkripsjon card was rendered visible. */
async function renderWhisperCard(): Promise<boolean> {
  const card = document.getElementById('home-whisper-card')
  if (!card) return false
  let installedModel: { label: string; quality?: string } | null = null
  try {
    const status = await window.api.whisperStatus()
    const installed = status.models?.find(m => (m as { installed?: boolean }).installed) as
      | { id: string; label: string; quality?: string }
      | undefined
    if (status.binaryAvailable && installed) installedModel = installed
  } catch {
    // Whisper IPC unavailable — skip card.
  }
  if (!installedModel) {
    card.style.display = 'none'
    return false
  }
  card.style.display = ''
  const valEl = document.getElementById('home-whisper-model')
  const subEl = document.getElementById('home-whisper-status')
  if (valEl) valEl.textContent = installedModel.label
  if (subEl) {
    subEl.textContent = installedModel.quality ?? 'Klar'
    subEl.style.color = 'var(--green)'
  }
  return true
}
