import { loadLocale, setApplyHook, t } from './i18n'
import { updateSettings } from './state'
import type { Settings, IntegrationSettings, ServiceLink, SermonCompanion } from '../types'

import { setupHome, refreshHome, stopVideoPreview, loadVideoInfoStrip, deactivateHome } from './pages/home'
import { stopVU, setupClipReset } from './pages/home-vu'
import { setupAudioPage, applyAudioSettingsToUI, renderDeviceList } from './pages/audio-page'
import { stopChannelGrid } from './pages/channel-grid'
import { setupSchedulePage, applyScheduleSettingsToUI, renderDayPickers, renderSlotsList } from './pages/schedule-page'
import { setupCalendarPage, renderCalendar, renderPlannedList } from './pages/calendar-page'
import { setupFilesPage, applyFilesSettingsToUI, updateFilenamePreview, toggleMp3Quality } from './pages/files-page'
import { setupGeneralPage, applyGeneralSettingsToUI } from './pages/general-page'
import { setupRecording } from './pages/recording'
import { setupEditorPage, openEditorWithFile, openEditorReviewMode, deactivateEditor, reactivateEditor } from './pages/editor-page'
import { checkAndShowOnboarding, showOnboarding } from './pages/onboarding'
import { setupVideoPage, applyVideoSettingsToUI, refreshVideoDevices } from './pages/video-page'
import { setupPublishPage, applyPublishSettingsToUI } from './pages/publish-page'
import { setupIntegrationsPage } from './pages/integrations-page'
import { setupLivePage, deactivateLivePage, reactivateLivePage } from './pages/live-page'
import { setupSearchPage, activateSearchPage } from './pages/search-page'
import { enhanceTimeInputs } from './time-input'
import { setupModalManager } from './ui/modal-manager'
import { applyInnerTabTransition, applyPageTransition, markPageEntered } from './ui/motion'

// Shared thumbnail IPC result shapes
export interface ThumbnailInfo {
  width:    number
  height:   number
  byteSize: number
  format:   'jpeg' | 'png' | 'webp'
}
export type ThumbnailResult = { path: string; info: ThumbnailInfo; dataUrl: string } | { error: string }

/** The pass-1 loudnorm measurement (`EditorLoudness`). All five measured values
 *  ride along so `masterApply` can REUSE the measurement `masterMeasure` just
 *  made instead of the backend reading the whole recording a second time. */
export interface LoudnessMeasurementView {
  inputI:       number
  inputLra:     number
  inputTp:      number
  inputThresh:  number
  targetOffset: number
  /** The preset's target, not a measurement — carried for the "x → y LUFS" UI. */
  targetLufs:   number
}
export interface ThumbnailResolved {
  path:    string
  info:    ThumbnailInfo
  dataUrl: string
  kind?:   'episode' | 'default'   // present on resolve, absent on getDefaultInfo
}

// Expose globals that sub-modules need
declare global {
  interface Window {
    showPage: (id: string) => void
    loadSettings: () => Promise<void>
    showOnboarding: () => void
    __isRecording: boolean
    openEditorWithFile: (filePath: string, seekToSec?: number) => void
    openEditorReviewMode?: (prepId: string, filePath: string) => void
    api: {
      getSettings:         () => Promise<Settings>
      saveSettings:        (s: Settings) => Promise<boolean>
      getNextRecording:    () => Promise<{ date: string } | null>
      getHistory:          () => Promise<unknown[]>
      deleteHistoryEntry:  (ts: number) => Promise<void>
      clearHistory:        () => Promise<void>
      pruneHistory:        () => Promise<number>
      getDiskSpace:        () => Promise<{ freeBytes: number | null }>
      startRecordingNow:   (opts: unknown) => Promise<{ ok?: boolean; error?: string }>
      stopRecordingNow:    () => Promise<boolean>
      runTestRecording:    () => Promise<{ ok: boolean; signal?: 'silent' | 'low' | 'normal'; rmsDb?: number; error?: string }>
      runCaptureBench:     (secs: number) => Promise<import('../bindings/SelfTestReport').SelfTestReport>
      probeDeviceChannels: (deviceName: string) => Promise<number>
      scanDeviceChannels:  (deviceName: string, secs: number) => Promise<{ channel: number; peakDb: number }[]>
      runPreflight:        () => Promise<{ findings: { severity: 'warn' | 'error'; category: string; message: string }[] }>
      testWebhook:         (url: string) => Promise<{ ok: boolean; error?: string }>
      pickFolder:          () => Promise<string | null>
      openFolder:          (p: string) => Promise<void>
      revealFile:          (p: string) => Promise<void>
      clearSmtpPassword:   () => Promise<boolean>
      /** Whether this build can send e-mail at all, and whether Gmail is
       *  connected — read BEFORE offering a «Send test» (see feature-gate). */
      emailStatus:         () => Promise<import('../bindings/EmailStatus').EmailStatus>
      testEmail:           (params: {
        transport: 'gmail' | 'smtp'
        recipient: string
        language?: string
        host?: string
        port?: number
        user?: string
        pass?: string
        from?: string
      }) => Promise<{ ok: boolean; error?: string }>
      updateHistoryNote:   (ts: number, note: string) => Promise<void>
      getAppVersion:       () => Promise<string>
      checkForUpdates:     () => Promise<void>
      installUpdate:       () => void
      getPlatform:         () => Promise<string>
      scheduleOsWakes:      () => Promise<unknown>
      scheduleOsWakesAdmin: () => Promise<unknown>
      getSleepConfig:       () => Promise<unknown>
      fixMacSleep:          () => Promise<{ ok: boolean; message?: string }>
      fixWinWakeTimers:     () => Promise<{ ok: boolean; message?: string }>
      wakeDetectCapabilities: () => Promise<{
        platform: 'mac-arm' | 'mac-intel' | 'win' | 'linux' | 'other'
        canWakeFromSleep: boolean
        canWakeFromOff:   boolean
        needsAdmin:       boolean
        knownIssues:      string[]
        recommendations:  string[]
      }>
      wakeVerifyScheduled: () => Promise<{
        capabilities: {
          platform: 'mac-arm' | 'mac-intel' | 'win' | 'linux' | 'other'
          canWakeFromSleep: boolean
          canWakeFromOff:   boolean
          needsAdmin:       boolean
          knownIssues:      string[]
          recommendations:  string[]
        }
        expectedWakes:  string[]
        observedWakes:  { scheduledAt: string; ownerLabel: string }[]
        hasMismatch:    boolean
        onBattery:      boolean | null
        standbyEnabled: boolean | null
      }>
      wakeTest:         (secondsAhead?: number) => Promise<{
        ok: boolean
        reason?: 'no_sleep' | 'no_resume' | 'too_late' | 'cancelled' | 'unsupported' | 'error'
        message?:      string
        scheduledFor?: string
        actualAt?:     string
        deltaSec?:     number
      }>
      wakeCancelTest:          () => Promise<boolean>
      wakeFailureHistory:      () => Promise<{
        timestamp: number; scheduledAt: string; kind: 'missed' | 'test_ok' | 'test_fail';
        label: string; reason?: string; deltaSec?: number
      }[]>
      wakeClearFailureHistory: () => Promise<boolean>
      notifyWeakSignal:    () => void
      on:                  (channel: string, fn: (...args: unknown[]) => void) => (() => void) | undefined
      toAssetUrl:             (path: string) => string
      editorReadFile:         (filePath: string) => Promise<unknown>
      editorPickFile:         ()                 => Promise<string | null>
      editorExportFile:       (params: unknown)  => Promise<{ ok: boolean; outputPath?: string; error?: string }>
      /** Kill the in-flight export render; resolves to whether one was running. */
      editorCancelExport:     ()                 => Promise<boolean>
      editorPickOutputFolder: ()                 => Promise<string | null>
      editorReadMeta:         (filePath: string) => Promise<unknown>
      editorSaveMeta:         (filePath: string, metadata: unknown) => Promise<boolean>
      editorDetectSegments:   (filePath: string, force?: boolean) => Promise<{ start: number; end: number; duration: number; label: string; type: string }[]>
      editorDetectChapters:   (lines: { start: number; text: string }[], lang?: string) => Promise<{ time: number; title: string }[]>
      editorProbePeak:       (filePath: string) => Promise<number | null>
      editorDiagnoseChannels: (filePath: string) => Promise<{ code: string; imbalanceDb: number; peakLeftDb: number; peakRightDb: number | null; recommended: { mode: string; leftDb: number; rightDb: number } } | null>
      editorAutoProcess:      (filePath: string) => Promise<{ diagnosis: { code: string; recommended: { mode: string; leftDb: number; rightDb: number } }; vocalChainPreset: string; masterPreset: string; summary: string } | null>
      editorReadCutsDraft:    (filePath: string) => Promise<unknown>
      editorSaveCutsDraft:    (filePath: string, cuts: unknown) => Promise<void>
      editorDeleteCutsDraft:  (filePath: string) => Promise<void>
      pickAudioFile:          ()                 => Promise<string | null>
      listAsioDrivers:        ()                 => Promise<string[]>
      listAsioInputChannels:  (deviceId: string) => Promise<{ index: number; label: string }[]>
      listFfmpegAudioDevices: () => Promise<{ name: string; index: number }[]>
      startVu:                (deviceName: string | null) => Promise<number>
      stopVu:                 ()                 => Promise<void>
      listInputDevices:       ()                 => Promise<import('../bindings/AudioDeviceList').AudioDeviceList>
      cloudConnect:        (service: string) => Promise<{ ok: boolean; accountName?: string; error?: string }>
      cloudCancelConnect:  (service: string) => Promise<boolean>
      cloudDisconnect:     (service: string) => Promise<void>
      cloudStatus:         ()                => Promise<Record<string, unknown>>
      cloudIsConfigured:   (service: string) => Promise<boolean>
      cloudUploadFile:     (service: string, filePath: string, metadata?: unknown) => Promise<{ ok: boolean; error?: string }>
      cloudListFolders:    (service: string, parentId?: string) => Promise<{ id: string; name: string; path?: string }[]>
      cloudSetFolder:      (service: string, folderId: string, folderName: string, folderPath?: string) => Promise<void>
      cloudQueueStatus:    () => Promise<{ entries: { id: string; service: string; filename: string; status: string; attempts: number; nextAttempt: number; lastError?: string }[] }>
      cloudQueueRetry:     (id: string) => Promise<boolean>
      cloudQueueRemove:    (id: string) => Promise<boolean>
      cloudQueueFlush:     () => Promise<boolean>
      podcastRegenerate:   (service: string) => Promise<{ ok: boolean; feedUrl?: string; episodeCount: number; error?: string }>
      registerTrustedPath: (filePath: string) => Promise<boolean>
      gmailConnect:       () => Promise<{ ok: boolean; email?: string; error?: string }>
      gmailDisconnect:    () => Promise<{ ok: boolean }>
      gmailStatus:        () => Promise<{ connected: boolean; email?: string; needsReauth?: boolean }>

      streamStatus:       () => Promise<{ active: boolean; startedAt: number | null; bitrateKbps: number; fps: number; dropped: number; lastLine: string; destinations: Array<{ id: string; state: string }> }>
      streamStart:        (params: { resolution?: string; framerate?: number; videoBitrateKbps?: number; destinations: Array<{ id: string; name: string; rtmpUrl: string; enabled: boolean; hasKey?: boolean }>; overlays?: unknown[]; alsoRecord?: boolean }) => Promise<{ ok: boolean; error?: string }>
      streamStop:         () => Promise<boolean>
      streamPreviewPath:  () => Promise<string>
      streamSetKey:       (destId: string, key: string) => Promise<{ ok: boolean; error?: string }>
      streamDeleteKey:    (destId: string) => Promise<boolean>

      overlayListScreens:    () => Promise<Array<{ id: string; label: string; bounds: { x: number; y: number; w: number; h: number }; isPrimary: boolean }>>
      overlayListNdiSources: () => Promise<{ available: boolean; reason?: string; sources: Array<{ name: string; url: string }> }>
      overlayPickImage:      () => Promise<{ path: string; name: string } | null>

      /** Every transcribed recording's sidecar. `basePath` is the recording path
       *  with its media extension stripped — the join key against `baseNoExt(row.path)`. */
      transcriptListAll:       () => Promise<Array<{ basePath: string; transcript: import('../types').TranscriptData }>>
      /** Render a transcript to SRT/VTT/TXT at `path` (native save dialog picks it). */
      whisperExportTranscript: (data: import('../types').TranscriptData, format: 'srt' | 'vtt' | 'txt', path: string) => Promise<{ ok: boolean; error?: string }>
      /** Native "save as" picker — returns the chosen path, or null on cancel. */
      pickSavePath:            (opts: { defaultPath?: string; name?: string; extensions?: string[] }) => Promise<string | null>

      editorReadTranscript:    (filePath: string) => Promise<import('../types').TranscriptData | null>
      editorWriteTranscript:   (filePath: string, t: unknown) => Promise<boolean>
      editorDeleteTranscript:  (filePath: string) => Promise<boolean>
      whisperStatus:        () => Promise<{ binaryAvailable: boolean; models: Array<{ id: string; label: string; description: string; sizeBytes: number; quality: string; realtimeFactor: number; installed: boolean; sizeOk: boolean }> }>
      whisperDownloadModel: (modelId: string) => Promise<{ ok: boolean; error?: string }>
      whisperCancelDownload:(modelId: string) => Promise<boolean>
      whisperDeleteModel:   (modelId: string) => Promise<boolean>
      whisperTranscribe:    (params: { filePath: string; modelId: string; language?: string; translate?: boolean; jobId?: string }) => Promise<{ ok: boolean; transcript?: import('../types').TranscriptData; error?: string }>
      whisperCancelTranscribe: (jobId: string) => Promise<boolean>

      youtubeConnect:      () => Promise<{ ok: boolean; error?: string }>
      youtubeDisconnect:   () => Promise<{ ok: boolean }>
      youtubeStatus:       () => Promise<{ connected: boolean }>
      youtubeUpload:       (filePath: string, metadata: unknown) => Promise<{ ok: boolean; videoId?: string; url?: string; error?: string }>
      reviewQueueList:                () => Promise<import('../types').ReviewQueueEntry[]>
      reviewQueueGet:                 (id: string) => Promise<import('../types').ReviewQueueEntry | null>
      reviewQueuePublish:             (id: string) => Promise<{ ok: boolean; error?: string }>
      reviewQueueDiscard:             (id: string) => Promise<boolean>
      reviewQueueUpdateTrim:          (id: string, trim: { startSec: number; endSec: number }) => Promise<boolean>
      reviewQueueUpdateMasterPreset:  (id: string, presetId: string) => Promise<boolean>
      reviewQueueUpdateJingles:       (id: string, jingles: { introPath?: string | null; outroPath?: string | null }) => Promise<boolean>
      listVideoDevices:  () => Promise<{ name: string; index: number }[]>
      getCameraCapabilities: (token: string) => Promise<{ maxWidth: number; maxHeight: number; maxFps: number; supportedResolutions: string[]; supportedFramerates: number[] } | null>
      videoPreviewStart: (opts: unknown) => Promise<boolean>
      videoPreviewStop:  () => Promise<void>
      recordingPreviewFrame: () => Promise<string | null>
      editorLoadRecording:     (filePath: string) => Promise<{ durationSec: number; hasVideo: boolean; hasAudio: boolean; channels: number | null; sampleFmt: string | null; sampleRate: number | null } | null>
      editorAllowAssetPath:    (filePath: string) => Promise<boolean>
      editorExtractAudioPeaks: (filePath: string) => Promise<{ peaks: number[]; sampleRate: number } | null>
      editorExtractPlaybackProxy: (filePath: string) => Promise<string | null>
      editorPickVideoFile:     ()                 => Promise<string | null>
      editorExportVideo:       (params: unknown)  => Promise<{ ok: boolean; outputPath?: string; error?: string }>
      editorProbeStreams:      (filePath: string) => Promise<{ hasVideo: boolean; hasAudio: boolean } | null>
      masterPresets:           () => Promise<{ id: string; label: string; description: string; targetLufs: number; targetLra: number; truePeakDb: number; filters: string }[]>
      masterPreview:           (inputPath: string, presetId: string, startSec: number, durationSec: number) => Promise<{ ok: boolean; previewPath?: string; error?: string }>
      masterMeasure:           (inputPath: string, presetId: string) => Promise<{ ok: boolean; measurement?: LoudnessMeasurementView; error?: string }>
      masterApply:             (params: { inputPath: string; outputPath: string; presetId: string; measurement?: LoudnessMeasurementView; jobId: string }) => Promise<{ ok: boolean; outputPath?: string; error?: string }>
      masterCancel:            (jobId: string) => Promise<boolean>
      runDiagnostics:          () => Promise<{ markdown: string; findings: { code: string; severity: 'ok' | 'info' | 'warning' | 'critical'; title: string; detail: string; hint: string }[]; savedTo: string | null; captureOk: boolean | null; videoOk: boolean | null }>

      // Thumbnail (podcast cover art)
      thumbnailSetDefault:     (sourcePath?: string) => Promise<ThumbnailResult | null>
      thumbnailClearDefault:   () => Promise<boolean>
      thumbnailSetEpisode:     (recordingPath: string, sourcePath?: string) => Promise<ThumbnailResult | null>
      thumbnailClearEpisode:   (recordingPath: string) => Promise<boolean>
      thumbnailResolve:        (recordingPath: string) => Promise<ThumbnailResolved | null>
      thumbnailGetDefaultInfo: () => Promise<ThumbnailResolved | null>

      // Sunday-suite integrations (opt-in; inert until enabled)
      getIntegrationSettings:  () => Promise<IntegrationSettings>
      setIntegrationSettings:  (patch: Partial<IntegrationSettings>) => Promise<IntegrationSettings>
      getServiceLink:          (recordingPath: string) => Promise<ServiceLink | null>
      sundayEditSend:            (opts: { videoPath: string; language?: string; context?: string; glossary?: string[] }) => Promise<{ ok: boolean; error?: string }>
      sundayEditImport:          (recordingPath: string, subtitlePath: string, language?: string) => Promise<{ ok: boolean; transcriptPath?: string; error?: string }>
      stageImport:             (recordingPath: string, manifestPath: string, wasStreamed?: boolean) => Promise<{ ok: boolean; chapterCount?: number; songCount?: number; error?: string }>
      songSetApiKey:           (key: string) => Promise<void>
      songHasApiKey:           () => Promise<boolean>
      songSubmitUsage:         (recordingPath: string) => Promise<{ ok: boolean; submitted?: number; errors?: Array<{ key: string; error: string }>; error?: string; hint?: string }>
      planFetchServices:       (fromIso?: string) => Promise<{ ok: boolean; services?: unknown[]; error?: string }>
      planUpdateService:       (serviceId: string, wasStreamed?: boolean, recordingUrl?: string) => Promise<{ ok: boolean; error?: string }>

      // R8 AI sermon companion (chapters + highlights + Norwegian summary).
      // companionBuild returns null on any failure; the optional LLM summary is
      // keychain-only (companionSetLlmKey) and degrades to a local extractive
      // summary when no key is configured.
      companionBuild:          (transcript: unknown, useLlm?: boolean) => Promise<SermonCompanion | null>
      companionLlmConfigured:  () => Promise<boolean>
      companionSetLlmKey:      (key: string) => Promise<boolean>
      companionClearLlmKey:    () => Promise<boolean>
    }
    appVersion?: string
  }
}

export async function loadSettings(): Promise<void> {
  const s = await window.api.getSettings()
  updateSettings(s)
  await applyAllSettingsToUI(s)
}

async function applyAllSettingsToUI(s: Settings): Promise<void> {
  const lang = s.language ?? 'no'
  // Await the active locale (lazy-loaded for non-`no`) BEFORE building localized
  // UI so the apply* calls below read the right language.
  await loadLocale(lang)
  applyAudioSettingsToUI()
  applyScheduleSettingsToUI()
  applyFilesSettingsToUI()
  applyGeneralSettingsToUI()
  applyVideoSettingsToUI()
  applyPublishSettingsToUI()
  loadVideoInfoStrip()
  renderSlotsList()
  renderPlannedList()
  updateFilenamePreview()
}

function showPage(id: string): void {
  if (id !== 'home') { stopVU(); stopVideoPreview(); deactivateHome() }
  if (id !== 'editor') deactivateEditor()
  if (id !== 'live') deactivateLivePage()
  if (id !== 'settings') stopChannelGrid()

  const outgoing = document.querySelector<HTMLElement>('.page.active')
  const target   = document.getElementById(`page-${id}`)
  if (outgoing === target) return

  // Fade the outgoing page out first, then swap. applyPageTransition also
  // resets #main's scroll — arriving on a short page after scrolling a long
  // one used to leave you staring at blank space.
  applyPageTransition(outgoing, () => {
    document.querySelectorAll('.page').forEach(p => p.classList.remove('active'))
    document.querySelectorAll('.nav-link').forEach(a => a.classList.remove('active'))
    target?.classList.add('active')
    document.querySelector(`.nav-link[data-page="${id}"]`)?.classList.add('active')
    markPageEntered(target)

    if (id === 'home')     refreshHome()
    if (id === 'schedule') renderCalendar()
    if (id === 'editor')   reactivateEditor()
    if (id === 'live')     reactivateLivePage()
    if (id === 'search')   activateSearchPage()
    if (id === 'settings') {
      const activeTab = document.querySelector<HTMLElement>('#settings-tabs .inner-tab.active')?.dataset.tab
      if (!activeTab || activeTab === 'settings-audio') renderDeviceList('device-list')
      if (activeTab === 'settings-video') refreshVideoDevices()
    }
  })
}

/**
 * Five tabs since Fase 3: Lyd · Video · Opptak · Deling · System.
 *
 * «Publisering» and «Varsler» answered halves of the same question — who gets
 * the recording afterwards — and are now sections of Deling; «Sunday-suite» is
 * an Avansert disclosure at the bottom of System. Old tab ids still resolve:
 * `navigate.ts` maps them to {tab, anchor}, so deep links keep landing.
 */
function setupSettingsTabs(): void {
  document.querySelectorAll<HTMLElement>('#settings-tabs .inner-tab').forEach(btn => {
    btn.addEventListener('click', () => showSettingsTab(btn.dataset.tab ?? ''))
  })
}

/**
 * Switch to a settings tab with a 120 ms crossfade.
 *
 * Deliberately NOT exported: `navigate.ts` reaches a tab by clicking its
 * button, which is the only way to be sure the button's own side effects (the
 * device-list refresh, the channel-grid teardown) run exactly once. Exporting a
 * second entry point would fork that.
 */
function showSettingsTab(tabId: string): void {
  const target = document.getElementById(tabId)
  if (!target) return
  const outgoing = document.querySelector<HTMLElement>('#page-settings .inner-page.active')

  const swap = (): void => {
    document.querySelectorAll('#settings-tabs .inner-tab').forEach(t => t.classList.remove('active'))
    document.querySelector(`#settings-tabs .inner-tab[data-tab="${tabId}"]`)?.classList.add('active')
    document.querySelectorAll<HTMLElement>('#page-settings .inner-page').forEach(p => p.classList.remove('active'))
    target.classList.add('active')
    if (tabId === 'settings-audio') renderDeviceList('device-list')
    else stopChannelGrid()
    if (tabId === 'settings-video') refreshVideoDevices()
  }

  if (outgoing === target) { swap(); return }
  applyInnerTabTransition(outgoing, swap)
}

/**
 * Verify that blob: URLs can be loaded into <img> tags. The video preview path
 * depends on this — frames arrive as JPEG buffers and are displayed via
 * URL.createObjectURL(new Blob(...)). If the CSP forgets to allow blob:, frames
 * still arrive but img.src silently fails (CSP violation goes to console as a
 * warning, not as a JS error). To catch that regression early, this runs on
 * startup and surfaces a visible banner if it fails.
 *
 * The smallest valid JPEG is 134 bytes — we embed a 1×1 white one as base64.
 */
function verifyBlobUrlsAllowed(): void {
  const tinyJpegB64 = '/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////2wBDAf//////////////////////////////////////////////////////////////////////////////////////wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAr/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFAEBAAAAAAAAAAAAAAAAAAAAAP/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/AL+AB//Z'
  const bytes = Uint8Array.from(atob(tinyJpegB64), c => c.charCodeAt(0))
  const url = URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' }))
  const img = new Image()
  let settled = false
  const done = (ok: boolean): void => {
    if (settled) return
    settled = true
    URL.revokeObjectURL(url)
    if (!ok) {
      console.error('[main] CSP smoke test FAILED — blob: URLs are blocked. Check Content-Security-Policy meta tag in index.html — img-src must include blob:.')
      const banner = document.getElementById('global-error-banner')
      const msg    = document.getElementById('global-error-msg')
      if (msg)    msg.textContent = t('general.cspError', 'Konfigurasjonsfeil: kamera-preview vil ikke fungere (CSP blokkerer blob:-URL-er). Restart appen — hvis problemet vedvarer, kontakt support.')
      if (banner) banner.style.display = ''
    }
  }
  img.onload  = () => done(true)
  img.onerror = () => done(false)
  // Belt-and-braces: if neither fires within 3 s assume CSP block.
  setTimeout(() => done(false), 3000)
  img.src = url
}

async function init(): Promise<void> {
  // Set globals consumed by sub-modules
  window.showPage       = showPage
  window.loadSettings   = loadSettings
  window.showOnboarding = showOnboarding
  window.__isRecording  = false

  // Fail loud on CSP regressions that break the video-preview display path.
  verifyBlobUrlsAllowed()

  const ua = navigator.userAgent.toLowerCase()
  if (ua.includes('mac')) {
    document.body.classList.add('platform-darwin')
  } else if (ua.includes('windows')) {
    document.body.classList.add('platform-win32')
  }

  // Hook i18n to re-run calendar/schedule renderers after locale load
  setApplyHook(() => {
    renderDayPickers()
    renderCalendar()
    updateFilenamePreview()
  })

  // Navigation
  document.querySelectorAll('.nav-link').forEach(a => {
    a.addEventListener('click', e => {
      e.preventDefault()
      showPage((a as HTMLElement).dataset.page!)
    })
  })

  // Wire up all pages
  setupHome()
  setupAudioPage()
  setupSchedulePage()
  setupCalendarPage()
  setupFilesPage()
  setupGeneralPage()
  setupVideoPage()
  setupRecording()
  setupEditorPage()
  setupPublishPage()
  void setupIntegrationsPage()
  setupLivePage()
  setupSearchPage()
  setupClipReset()
  setupSettingsTabs()
  // Escape, backdrop-click, focus trap and `inert` for every .modal-backdrop.
  setupModalManager()
  enhanceTimeInputs() // smooth "1430" entry on all native time fields

  window.openEditorWithFile = openEditorWithFile
  window.openEditorReviewMode = openEditorReviewMode

  // Fetch app version from main (sandbox-safe — no fs/path in preload)
  window.appVersion = await window.api.getAppVersion().catch(() => '—')

  // Load settings, which triggers locale + UI apply
  await loadSettings()

  // Show first-run onboarding wizard for new users
  checkAndShowOnboarding()

  // Initial page load. The home page is marked `active` in the markup, so it
  // never goes through showPage — latch its entrance animation here instead.
  markPageEntered(document.querySelector<HTMLElement>('.page.active'))
  await refreshHome()
  renderDeviceList('device-list')
  renderDayPickers()
  renderCalendar()
  updateFilenamePreview()
  toggleMp3Quality()
}

/** Surface a message in the same global banner the CSP smoke-test uses, so
 * fatal errors are visible in the UI instead of only in the devtools console. */
function showGlobalErrorBanner(message: string): void {
  const banner = document.getElementById('global-error-banner')
  const msg    = document.getElementById('global-error-msg')
  if (msg)    msg.textContent = message
  if (banner) banner.style.display = ''
}

window.addEventListener('unhandledrejection', e => {
  console.error('Unhandled promise rejection:', e.reason)
  e.preventDefault()
})

// Synchronous uncaught errors previously went nowhere visible — log them so a
// broken handler shows up in diagnostics. (No banner: a stray non-fatal throw
// mid-session shouldn't alarm the user; init failures below do get the banner.)
window.addEventListener('error', e => {
  console.error('Uncaught error:', e.error ?? e.message)
})

init().catch(err => {
  console.error('Init failed:', err)
  showGlobalErrorBanner(
    t('general.initError', 'Oppstartsfeil: deler av appen kan mangle eller være uten funksjon. Restart appen — hvis problemet vedvarer, kontakt support.') +
    ' (' + (err instanceof Error ? err.message : String(err)) + ')',
  )
})
