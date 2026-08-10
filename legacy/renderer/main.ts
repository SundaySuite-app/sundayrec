import { loadLocale, setApplyHook, t } from './i18n'
import { settings, updateSettings } from './state'
import type { Settings, IntegrationSettings, ServiceLink, SermonCompanion, EditorSegment } from '../types'
import type { ThumbnailInfo as ThumbnailInfoDto } from '../bindings/ThumbnailInfo'
import type { ThumbnailView } from '../bindings/ThumbnailView'
import type { TrashEntry } from '../bindings/TrashEntry'

import { setupHome, refreshHome, stopVideoPreview, loadVideoInfoStrip, deactivateHome, openReviewQueueFromTray } from './pages/home'
import { stopVU, setupClipReset } from './pages/home-vu'
import { setupAudioPage, applyAudioSettingsToUI, renderDeviceList } from './pages/audio-page'
import { stopChannelGrid } from './pages/channel-grid'
import { setupSchedulePage, applyScheduleSettingsToUI, renderDayPickers, renderSlotsList } from './pages/schedule-page'
import { setupCalendarPage, renderCalendar, renderPlannedList } from './pages/calendar-page'
import { setupFilesPage, applyFilesSettingsToUI, updateFilenamePreview, toggleMp3Quality } from './pages/files-page'
import {
  setupGeneralPage,
  applyGeneralSettingsToUI,
  paintActiveUpdateChannel,
} from './pages/general-page'
import { setupRecording, openManualModal, doStopRecording } from './pages/recording'
import { setupEditorPage, openEditorWithFile, openEditorReviewMode, deactivateEditor, reactivateEditor } from './pages/editor-page'
import { checkAndShowOnboarding, showOnboarding } from './pages/onboarding'
import { initTelemetryConsentPrompt } from './telemetry-consent-prompt'
import { setupVideoPage, applyVideoSettingsToUI, refreshVideoDevices } from './pages/video-page'
import { setupPublishPage, applyPublishSettingsToUI } from './pages/publish-page'
import { setupIntegrationsPage } from './pages/integrations-page'
import { setupSearchPage, activateSearchPage, invalidateTranscriptIndex } from './pages/search-page'
import { enhanceTimeInputs } from './time-input'
import { setupModalManager } from './ui/modal-manager'
import { applyInnerTabTransition, applyPageTransition, markPageEntered } from './ui/motion'
import { navigateTo } from './ui/navigate'
import { initTrayActions } from './tray-actions'
import { initPrerollLifecycle } from './preroll-lifecycle'
import { initDeeplinks } from './deeplinks'

// Shared thumbnail IPC result shapes.
//
// These were hand-written for a backend that did not exist. It exists now
// (src-tauri/src/commands/thumbnail.rs), so they are the GENERATED types — a
// change to the Rust DTO surfaces here as a tsc error instead of as a panel
// reading a field the backend stopped sending.
export type ThumbnailInfo = ThumbnailInfoDto
export type ThumbnailResult = ThumbnailView | { error: string }

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
/** What `thumbnailResolve` returns — the same view, with `kind` telling the
 *  panel whether it got this episode's own image or the shared default.
 *  `kind` is absent from `thumbnailGetDefaultInfo` (nothing to distinguish). */
export type ThumbnailResolved = ThumbnailView

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
      /** Write the whole (validated) settings object to a user-chosen JSON
       *  file. Rejects on failure — the profile card shows the reason. */
      settingsExportToFile: (path: string) => Promise<void>
      /** Import a settings JSON file (merge-over-defaults + validate) and
       *  return the stored result. Rejects on failure. */
      settingsImportFromFile: (path: string) => Promise<Settings>
      /** Open-dialog picker for a settings-profile JSON. Cancel → null. */
      pickSettingsFile:    () => Promise<string | null>
      getNextRecording:    () => Promise<{ date: string } | null>
      getHistory:          () => Promise<unknown[]>
      deleteHistoryEntry:  (ts: number) => Promise<void>
      clearHistory:        () => Promise<void>
      pruneHistory:        () => Promise<number>
      /** Move recordings (with sidecars + video sibling) into the papirkurv.
       *  Rejects rather than reporting a delete that did not happen. */
      trashMove:           (paths: string[]) => Promise<TrashEntry[]>
      /** Everything currently recoverable, newest first. */
      trashList:           () => Promise<TrashEntry[]>
      /** Put one entry back where it came from. */
      trashRestore:        (id: string) => Promise<TrashEntry>
      /** Destroy entries permanently — an empty list empties the papirkurv. */
      trashPurge:          (ids: string[]) => Promise<number>
      getDiskSpace:        () => Promise<{ freeBytes: number | null }>
      startRecordingNow:   (opts: unknown) => Promise<{ ok?: boolean; error?: string }>
      stopRecordingNow:    () => Promise<boolean>
      /** Push the running recording's auto-stop deadline out by N minutes. */
      extendAutostop:      (minutes: number) => Promise<void>
      /** Clear the auto-stop so the recording runs until a manual stop. */
      cancelAutostop:      () => Promise<void>
      /** The live auto-stop deadline (epoch ms), or null when none is armed. */
      scheduledStopMs:     () => Promise<number | null>
      /** Start the rolling pre-roll buffer. Resolves false when the backend
       *  declined (pre-roll off in its settings copy, or no device matched). */
      prerollStart?:       () => Promise<boolean>
      /** Stop the rolling pre-roll buffer (safe when nothing is running). */
      prerollStop?:        () => Promise<void>
      /** Whether the rolling pre-roll buffer is actually running. */
      prerollStatus?:      () => Promise<{ active: boolean }>
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
      /** Store the SMTP password in the OS keychain (undefined/'' clears it).
       *  Resolves true when a password is now stored. Rejects on a keychain
       *  failure — the caller must show it, not swallow it. */
      emailSetSmtpPassword: (password?: string) => Promise<boolean>
      /** Whether an SMTP password is stored. The secret never crosses back. */
      emailHasSmtpPassword: () => Promise<boolean>
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
      /** Push the UI language to the Rust menubar tray (it renders its own labels). */
      traySetLanguage?:    (code: string) => Promise<void>
      /** macOS camera + microphone authorization (AVFoundation), for preflight. */
      mediaPermissions?:   () => Promise<{
        camera: import('../bindings/AuthStatus').AuthStatus
        microphone: import('../bindings/AuthStatus').AuthStatus
      }>
      /** Whether the bundled ffmpeg sidecar resolved, and where. */
      ffmpegHealth?:       () => Promise<{ available: boolean; version: string | null; path: string }>
      /** Whether the OS login item is really registered (not the stored boolean). */
      getLaunchAtLogin?:   () => Promise<boolean>
      /** Trackpad haptic tap (macOS Force Touch); a silent no-op elsewhere. */
      hapticPerform?:      (pattern: string) => Promise<void>
      /** The purpose-built audio-device probe behind Lyd → Diagnose. */
      diagnoseAudio?:      () => Promise<{ dshow: string[]; wasapi: string[]; wasapiAvailable: boolean }>
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
      // The generated binding, not a hand-written twin — see `Suggestion` in
      // pages/editor/state.ts for what the twin cost us.
      editorDetectSegments:   (filePath: string, force?: boolean) => Promise<EditorSegment[]>
      /** Persist a sermon-pick correction (E8). Resolves to whether it was
       *  recorded — re-picking the detector's own block is not a correction. */
      editorRecordSermonPick: (filePath: string, request: import('../bindings/EditorSermonPickRequest').EditorSermonPickRequest) => Promise<boolean>
      /** Index into `segments` of the block the human corrected us to, or null. */
      editorSermonPick:       (filePath: string, segments: EditorSegment[]) => Promise<number | null>
      /** Record what became of one companion suggestion (E8). Categories only —
       *  the generated bindings are the vocabulary, so a kind or an outcome the
       *  backend does not know fails to compile here rather than at runtime. */
      editorRecordCompanionSuggestion: (
        filePath: string,
        kind: import('../bindings/CompanionSuggestionKind').CompanionSuggestionKind,
        outcome: import('../bindings/CompanionSuggestionOutcome').CompanionSuggestionOutcome,
        editedAfterAccept: boolean,
      ) => Promise<boolean>
      editorDetectChapters:   (lines: { start: number; text: string }[], lang?: string) => Promise<{ time: number; title: string }[]>
      editorProbePeak:       (filePath: string) => Promise<number | null>
      editorDiagnoseChannels: (filePath: string) => Promise<{ code: string; imbalanceDb: number; peakLeftDb: number; peakRightDb: number | null; recommended: { mode: string; leftDb: number; rightDb: number } } | null>
      editorAutoProcess:      (filePath: string) => Promise<{ diagnosis: { code: string; recommended: { mode: string; leftDb: number; rightDb: number } }; vocalChainPreset: string; masterPreset: string; summary: string } | null>
      editorReadCutsDraft:    (filePath: string) => Promise<unknown>
      editorSaveCutsDraft:    (filePath: string, cuts: unknown) => Promise<void>
      editorDeleteCutsDraft:  (filePath: string) => Promise<void>
      pickAudioFile:          ()                 => Promise<string | null>
      /** The backend-tagged input list — the renderer's ONLY audio-device
       *  enumeration since the getUserMedia label blink-open was removed. */
      listAudioDevices:       ()                 => Promise<import('../bindings/TaggedAudioInput').TaggedAudioInput[]>
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
      /** Whether this build can write/upload the RSS feed (the `publish` cargo
       *  feature) — the Filer-page gate's truth source. `null` = could not ask. */
      podcastFeedStatus:   () => Promise<{ featureBuilt: boolean; episodeCount: number } | null>
      registerTrustedPath: (filePath: string) => Promise<boolean>

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

      reviewQueueList:                () => Promise<import('../types').ReviewQueueEntry[]>
      reviewQueueGet:                 (id: string) => Promise<import('../types').ReviewQueueEntry | null>
      reviewQueuePublish:             (id: string) => Promise<{ ok: boolean; error?: string }>
      reviewQueueDiscard:             (id: string) => Promise<boolean>
      // The three review-queue field pushes. All three answer `false` when the
      // id has left the queue — treat that as "not saved", never as a no-op.
      reviewQueueUpdateTrim:          (id: string, trim: { startSec: number; endSec: number }) => Promise<boolean>
      reviewQueueUpdateMasterPreset:  (id: string, presetId: string) => Promise<boolean>
      /** PARTIAL patch: an omitted key leaves that jingle alone, `null` clears
       *  it, a string sets it. Send one key per call. */
      reviewQueueUpdateJingles:       (id: string, jingles: { introPath?: string | null; outroPath?: string | null }) => Promise<boolean>
      listVideoDevices:  () => Promise<{ name: string; index: number }[]>
      getCameraCapabilities: (token: string) => Promise<{ maxWidth: number; maxHeight: number; maxFps: number; supportedResolutions: string[]; supportedFramerates: number[] } | null>
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
      /** Reveal the rotating log folder in Finder/Explorer (falls back to the
       *  folder itself before the first line is written). No path in, none out —
       *  resolves to whether the OS actually opened something. */
      logsReveal:              () => Promise<boolean>
      /** The tail of the live log file, clamped server-side to 512 KB
       *  regardless of `maxBytes`. Empty string means nothing logged yet. */
      logsTail:                (maxBytes: number) => Promise<string>
      /** Every IPC failure remembered this session (renderer-local ring,
       *  newest first) — answers even when the backend that IS failing can't
       *  be asked. */
      getRecentIpcFailures:    () => import('./ipc-failures-core').IpcFailure[]
      /** How many failures, which commands, and when the newest one landed. */
      getIpcFailureSummary:    () => { count: number; commands: string[]; newestAt: number | null }

      // ── Telemetry (E3.6) — opt-in, anonymous, off by default. The rest of
      // the surface (preview payload, queue status, delete-my-data) is
      // declared in E3.7's block below. ──────────────────────────────────
      /** The current consent state — status/never-asked-granted-denied, the
       *  derived needsPrompt/active — see crates/sundayrec-core/telemetry/
       *  consent.rs for the state machine this mirrors. */
      telemetryConsentGet: () => Promise<import('../bindings/TelemetryConsent').TelemetryConsent>
      /** Record the user's answer. Resolves `null` ONLY on a real IPC
       *  failure — callers must never treat `null` as "recorded", since the
       *  whole point of asking once is that a lost answer must be asked
       *  again. */
      telemetryConsentSet: (granted: boolean) => Promise<import('../bindings/TelemetryConsent').TelemetryConsent | null>
      /** Opens PRIVACY.md on the (public) GitHub repo via the opener plugin. */
      openPrivacyPolicy: () => Promise<boolean>

      // ── Telemetry (E3.7) — the settings-panel surface ────────────────────
      /** The real next payload as pretty JSON, honestly labelled — see
       *  TelemetryPreview's own doc comment for why the two consent states
       *  answer differently. `null` only on a genuine IPC failure; never
       *  fabricated. */
      telemetryPreviewPayload: () => Promise<import('../bindings/TelemetryPreview').TelemetryPreview | null>
      /** What is waiting in the local outbox. */
      telemetryQueueStatus: () => Promise<import('../bindings/TelemetryQueueStatus').TelemetryQueueStatus>
      /** "Slett mine data", the local half: retires the install id. Resolves
       *  `false` only on a real failure. */
      telemetryRegenerateInstallId: () => Promise<boolean>

      // ── Learning-feedback transparency (E8.T) ────────────────────────────
      /** Fold every recording's `<stem>.feedback.json` into the counts + the
       *  trim-direction verdict the System-tab card shows. `null` ONLY on a
       *  real IPC failure — a summary of all zeros IS the legitimate empty
       *  state, so callers must never let a failed round-trip collapse into
       *  that same shape (see `learning-summary-core.ts`'s empty-state
       *  handling for why the distinction matters). */
      learningFeedbackSummary: () => Promise<import('../bindings/LearningSummary').LearningSummary | null>

      // ── What the app has adjusted about itself (E10) ─────────────────────
      /** The two clamped boundary offsets this install has learned, or the
       *  shipped zeroes when adaptivity is off. `null` ONLY on a real IPC
       *  failure — same rule as above, and for the same reason: "nothing
       *  adjusted" is a legitimate answer that must not be reachable by a
       *  failed read. */
      learningLocalNudge: () => Promise<import('../bindings/LocalNudge').LocalNudge | null>
      /** Back to the shipped detector, now: zeroes the offsets and turns
       *  adaptivity off. `null` on failure. */
      learningLocalNudgeReset: () => Promise<import('../bindings/LocalNudge').LocalNudge | null>

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
      /** The complete Stage import: manifest JSON (its CONTENT, not a path — the
       *  webview can read the picked File, it cannot learn its fs path) →
       *  chapters into `.meta.json` + `.service.json` beside the recording. */
      stageImport:             (recordingPath: string, manifestJson: string, wasStreamed?: boolean) => Promise<{ ok: boolean; chapterCount?: number; songCount?: number; error?: string }>
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
    // Painted from settings, not from a data-i18n attribute, so
    // applyTranslations() cannot reach it — and "this machine is on BETA" is
    // the last line that should be left standing in the previous language.
    paintActiveUpdateChannel()
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
  setupSearchPage()
  setupClipReset()
  setupSettingsTabs()
  // Escape, backdrop-click, focus trap and `inert` for every .modal-backdrop.
  setupModalManager()

  // `sundayrec://` hand-offs from SundayEdit. Rust has parsed (and, for
  // captions, already applied) these since the port; nothing ever listened, so
  // the window came forward and then appeared to do nothing.
  initDeeplinks({
    openInEditor: (path: string) => openEditorWithFile(path),
    refreshTranscripts: invalidateTranscriptIndex,
  })

  // The menubar tray's one event, routed to the SAME entry points the in-app
  // buttons use — a tray "Start opptak nå" opens the very modal the Home button
  // opens, so there is one start path, not two. (Rust handles show/stop/quit
  // itself; these are the ids it hands to the renderer.)
  initTrayActions({
    startRecording: () => void openManualModal(),
    stopRecording: () => void doStopRecording(),
    openReviewQueue: openReviewQueueFromTray,
    // Rust does NOT handle this one (it falls through `emit_action`'s catch-all),
    // and the save folder is a renderer setting anyway.
    openRecordingsFolder: () => {
      const folder = settings.saveFolder
      if (folder) void window.api.openFolder(folder)
      else navigateTo('settings', { tab: 'settings-files', anchor: '#save-folder' })
    },
    runPreflight: () => {
      navigateTo('settings', { tab: 'settings-audio', anchor: 'btn-run-preflight-settings', highlight: false })
      requestAnimationFrame(() => document.getElementById('btn-run-preflight-settings')?.click())
    },
    runDiagnostics: () => {
      navigateTo('settings', { tab: 'settings-audio', anchor: 'btn-audio-diagnose', highlight: false })
      requestAnimationFrame(() => document.getElementById('btn-audio-diagnose')?.click())
    },
  })
  enhanceTimeInputs() // smooth "1430" entry on all native time fields

  window.openEditorWithFile = openEditorWithFile
  window.openEditorReviewMode = openEditorReviewMode

  // Fetch app version from main (sandbox-safe — no fs/path in preload)
  window.appVersion = await window.api.getAppVersion().catch(() => '—')

  // Load settings, which triggers locale + UI apply
  await loadSettings()

  // The pre-roll rolling buffer. AFTER loadSettings, because the decision reads
  // `prerollEnabled` / `preRollSeconds` / the device — and defaults-off means an
  // unhydrated settings cache would (correctly, but pointlessly) decide "stop"
  // and then have to change its mind.
  initPrerollLifecycle()

  // Show first-run onboarding wizard for new users
  checkAndShowOnboarding()

  // Existing installs that already finished onboarding never see its consent
  // step — ask them once, via a non-blocking corner card instead (E3.6).
  // Gated inside on `settings.onboardingDone`, so this is a no-op for the
  // fresh installs the wizard above just handled (or is about to).
  initTelemetryConsentPrompt()

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
