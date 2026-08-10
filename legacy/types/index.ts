// ── Generated ts-rs bindings ────────────────────────────────────────────────
// These types are re-exported from `legacy/bindings/` (generated from the Rust
// types by `npm run bindings`) so a backend contract change surfaces as a tsc
// error here instead of silently drifting. Only types whose generated shape is
// field-for-field identical to the old hand-written one are re-exported; the
// rest below stay hand-written until the Rust side matches (e.g. `?` vs
// `| null` optionality — see docs/BACKLOG-AUDIT-2026-07-07.md).
import type { ChannelMode } from '../bindings/ChannelMode'
import type { FileFormat } from '../bindings/FileFormat'
import type { FilenamePattern } from '../bindings/FilenamePattern'
import type { EpisodePrepStatus } from '../bindings/EpisodePrepStatus'
import type { PrepAnalysisSegment } from '../bindings/PrepAnalysisSegment'
import type { EditorSegment } from '../bindings/EditorSegment'
import type { ChapterMarker } from '../bindings/ChapterMarker'
import type { TranscriptSegment } from '../bindings/TranscriptSegment'
import type { SermonHighlight } from '../bindings/SermonHighlight'
import type { CompanionChapter } from '../bindings/CompanionChapter'
import type { SummarySource } from '../bindings/SummarySource'
import type { SermonCompanion } from '../bindings/SermonCompanion'
import type { UpdateChannel } from '../bindings/UpdateChannel'
import type { Settings as SettingsGen } from '../bindings/Settings'
import type { DeviceChannels } from '../bindings/DeviceChannels'
import type { ScheduleSlot } from '../bindings/ScheduleSlot'
import type { SpecialRecording } from '../bindings/SpecialRecording'
import type { PodcastSettings } from '../bindings/PodcastSettings'
import type { CloudServicePrefs } from '../bindings/CloudServicePrefs'
import type { StreamDestinationStored } from '../bindings/StreamDestinationStored'
export type {
  ChannelMode,
  FileFormat,
  FilenamePattern,
  DeviceChannels,
  ScheduleSlot,
  SpecialRecording,
  PodcastSettings,
  CloudServicePrefs,
  StreamDestinationStored,
  EpisodePrepStatus,
  PrepAnalysisSegment,
  EditorSegment,
  ChapterMarker,
  TranscriptSegment,
  SermonHighlight,
  CompanionChapter,
  SummarySource,
  SermonCompanion,
  UpdateChannel,
}
// ────────────────────────────────────────────────────────────────────────────

// DeviceChannels / ScheduleSlot / SpecialRecording are generated (re-exported above).

export interface CutRegion {
  start: number  // seconds
  end: number    // seconds
}

export interface RecordingEntry {
  date: string
  startTime: string
  duration: string
  filename: string
  path?: string
  status: 'ok' | 'error' | 'scheduled'
  error?: string
  note?: string
  timestamp?: number
  fileSizeBytes?: number    // actual file size on disk after recording
  durationSec?: number      // recording duration in seconds
  cloudUploaded?: string[]  // cloud service IDs where this file was uploaded: ['google-drive', 'dropbox', 'onedrive']
  cloudUrls?: Record<string, string>  // service ID → public/share URL (used by podcast RSS feed)
}

// PodcastSettings is generated (re-exported above) — the Rust
// `sundayrec_core::settings::PodcastSettings`, one vocabulary with the store.

// PrepAnalysisSegment + EpisodePrepStatus are generated (re-exported above).
//
// EpisodePrep status lifecycle:
//   analyzing       — background analysis running
//   ready           — prep complete, all defaults applied, no concerns
//   needs-attention — prep complete, but the suggested sermon segment is
//                     low-confidence or absent. Human review required.
//   published       — user clicked "Godkjenn og publiser" and the upload
//                     pipeline ran to completion.
//   discarded       — user clicked "Ikke publiser denne uka".
export interface EpisodePrep {
  id:                string                       // uuid
  recordingPath:     string                       // source file
  timestamp:         number                       // when recording finished
  status:            EpisodePrepStatus
  analysisSegments?: PrepAnalysisSegment[]        // raw segments from audio-analysis.ts
  /** Sermon-only range derived from segments — the area between startSec and
   *  endSec is "keep", everything else is intended to be cut. */
  suggestedTrim?:    { startSec: number; endSec: number }
  /** 0..1 — how confident we are that suggestedTrim covers the sermon. */
  sermonConfidence?: number
  masterPreset:      string                       // default 'speech-clear'
  introPath?:        string                       // null = no intro for this episode
  outroPath?:        string                       // null = no outro for this episode
  /** Norwegian — why this needs human review beyond normal QC. */
  attentionReasons?: string[]
  /** Reserved for Phase 2 YouTube auto-publish. Currently unused. */
  publishYoutube?:   boolean
  createdAt:         number
  updatedAt:         number
  /** Set after a successful publish — guards against double-publishing
   *  if the user clicks the button twice. */
  publishedAt?:      number
  /** History timestamp of the source recording entry (used to mark the
   *  recording as published when this prep is published). */
  recordingTimestamp?: number
}

/**
 * A single entry in the human-review queue. Wraps EpisodePrep with bookkeeping
 * (reminder count, age). Stored in electron-store under key `reviewQueue`.
 */
export interface ReviewQueueEntry {
  id:        string
  prep:      EpisodePrep
  addedAt:   number
  /** Reminders sent so far: 0 = none, 1 = 24h sent, 2 = 48h sent, 3 = 7d sent.
   *  At 4, the entry has been auto-discarded (14d) — but at that point the
   *  entry is removed from the queue rather than kept around. */
  reminded:  number
  /** Days since addedAt — computed on read from getQueue(), not persisted. */
  ageInDays: number
}


/**
 * The settings model (R4): THE generated binding, one vocabulary end to end —
 * `crates/sundayrec-core/src/settings.rs` is the source, sqlite is the store,
 * and a Rust field rename/removal is a tsc error on every consumer here.
 *
 * The single override: `streamOverlays`. The backend persists overlays as
 * opaque JSON (their vocabulary — type/source/chroma-key/crop — is
 * renderer-owned and differs from the ffmpeg builder's `OverlayConfig` on the
 * Rust side), so the generated type says `Array<unknown>` and the renderer
 * narrows it to its own `OverlayConfig[]` here.
 */
export type Settings = Omit<SettingsGen, 'streamOverlays'> & {
  streamOverlays: OverlayConfig[]
}

/** Back-compat alias — the generated `CloudServicePrefs` is the same shape the
 *  old hand-written `CloudServiceSettings` described (tokens live elsewhere). */
export type CloudServiceSettings = CloudServicePrefs

/**
 * Overlay placement preset. 9-grid + fullscreen + free positioning.
 * Coordinates resolve to ffmpeg overlay X:Y expressions based on output WxH.
 */
export type OverlayPosition =
  | 'tl' | 'tc' | 'tr'
  | 'cl' | 'c'  | 'cr'
  | 'bl' | 'bc' | 'br'
  | 'fullscreen'
  | 'custom'

/**
 * What kind of source feeds this overlay:
 *  - image:  static PNG/JPG on disk (logo, lower-third graphic)
 *  - screen: whole monitor capture (avfoundation/gdigrab)
 *  - window: monitor capture with crop region (used to approximate a single
 *            EasyWorship/ProPresenter window when running on the same machine)
 *  - ndi:    NDI network source — implementation lands in a follow-up release;
 *            field is reserved so settings persist across the upgrade.
 */
export type OverlaySourceType = 'image' | 'screen' | 'window' | 'ndi'

export interface OverlayChromaKey {
  /** Hex color e.g. "#00FF00" — typically the solid background EW outputs. */
  color:      string
  /** 0..1 — how close a pixel must be to `color` to be keyed (default 0.10). */
  similarity: number
  /** 0..1 — soft edge blend (default 0.10). */
  blend:      number
}

export interface OverlayCrop {
  /** All values are fractions of the SOURCE input dimensions (0..1). */
  x: number; y: number; w: number; h: number
}

export interface OverlayConfig {
  /** Stable id used to key UI controls and persisted settings. */
  id:      string
  /** User-facing label (e.g. "Logo", "Lyrics fra EasyWorship"). */
  name:    string
  /** Master on/off — when false the overlay is skipped in the filter graph. */
  enabled: boolean

  type: OverlaySourceType
  /** For type=image: absolute path. For type=screen/window: capture id
   *  ('1', 'screen:0:0' on Mac, 'desktop' or display index on Win). For
   *  type=ndi: NDI source name as discovered on the network. */
  source: string

  /** Placement preset. */
  position: OverlayPosition
  /** Only used when position='custom' — fraction of output WxH (0..1). */
  customX?: number
  customY?: number

  /** Overlay width as fraction of output width (0..1). Height auto-scales
   *  preserving aspect. For fullscreen this is forced to 1.0. */
  scale: number
  /** 0..1 — final opacity after chroma key. */
  opacity: number

  /** Chroma key (set null/undefined to disable). */
  chromaKey?: OverlayChromaKey | null

  /** Crop input before scaling. Mostly useful for type=window to grab a
   *  region of a monitor. Values are 0..1 of the source dimensions. */
  crop?: OverlayCrop | null
}

// StreamDestinationStored is generated (re-exported above): persisted WITHOUT
// the stream key — that lives in the OS keychain via `stream_set_key`, and
// `hasKey` is the only trace it leaves in settings.

export interface RecordingOpts extends Partial<Settings> {
  deviceId?: string | null
  customName?: string
  overrideName?: string | null
  splitTimestamp?: string
  maxMinutes?: number
  scheduledStopTime?: string
  channelL?: number
  channelR?: number
  /** Internal: prevents infinite sample-rate retry loop */
  _sampleRateRetried?: boolean
}

export interface DiskInfo {
  freeBytes: number | null
}

export interface WakeResult {
  ok: boolean
  count?: number
  nextWake?: string | null   // ISO string of next scheduled wake point
  reason?: 'disabled' | 'cancelled' | 'permission' | 'unsupported' | 'error'
  message?: string
}


export interface UpdateProgress {
  percent: number
  transferred: number
  total: number
  bytesPerSecond: number
}

export interface UpdateInfo {
  version: string
  releaseNotes?: string
}

// ChapterMarker is generated (re-exported above): { time (sec from start of
// main content), title }.
export interface RecordingMetadata {
  title: string
  speaker: string
  description: string
  chapters: ChapterMarker[]
}

// TranscriptSegment is generated (re-exported above): one whisper.cpp segment,
// `start`/`end` in seconds into the recording.

/** Sidecar file written alongside the recording at <name>.transcript.json.
 *  Schema-versioned so we can evolve format without breaking older files. */
export interface TranscriptData {
  /** Schema version. Bump when format changes incompatibly. */
  version:   1
  /** Whisper model id used (e.g. "ggml-base", "ggml-medium"). */
  model:     string
  /** BCP-47-ish language code Whisper detected/was told (e.g. "no", "en", "auto"). */
  language:  string
  /** Total media duration in seconds — for sanity-checking and percentage display. */
  duration:  number
  /** Epoch-ms when this transcript was generated. */
  createdAt: number
  /** True if user asked Whisper to translate output to English. */
  translated?: boolean
  segments:  TranscriptSegment[]
}

// SermonHighlight / CompanionChapter / SummarySource / SermonCompanion are
// generated (re-exported above) — the AI sermon-companion result types.

// NB: distinct from the generated `CloudService` ('google-drive'|'youtube'|
// 'gmail'), which models OAuth account kinds — this one is the backup targets.
export type CloudServiceId = 'google-drive' | 'dropbox' | 'onedrive'

// CloudServiceSettings: see the generated `CloudServicePrefs` alias above.

export interface CloudStatus {
  connected: boolean
  accountName?: string
  accountEmail?: string
  folderId?: string
  folderName?: string
  folderPath?: string
  lastUpload?: number
  lastUploadOk?: boolean
  /** True when the saved refresh token has been revoked — user must reconnect. */
  needsReauth?: boolean
}

export interface CloudUploadQueueEntry {
  id:             string         // unique entry id (uuid-ish)
  service:        CloudServiceId
  filePath:       string
  entryTimestamp?: number        // history-entry timestamp to mark as uploaded on success
  attempts:       number         // total attempts so far
  nextAttempt:    number         // unix ms — earliest time the worker may retry
  lastError?:     string         // last error message (for UI)
  enqueuedAt:     number
  status:         'pending' | 'uploading' | 'failed' | 'reauth-required'
}

export interface CloudQueueStatus {
  entries: Array<{
    id: string
    service: CloudServiceId
    filename: string
    attempts: number
    nextAttempt: number
    lastError?: string
    status: CloudUploadQueueEntry['status']
  }>
}

// ── Sunday-suite integrations ───────────────────────────────────────────────
// Opt-in connection to the sister apps (Stage, Plan, Song, SundayEdit). Every
// flag defaults off; when `enabled` is false nothing in src/main/integrations/
// runs and the renderer hides the whole "Sunday-suite" section. The recording
// core (recorder.ts / scheduler.ts) never reads these.

/** A song that was used in a service, with the cross-suite identifiers we may
 *  know about. At least one of the IDs (or the title) is always present.
 *  `firstShownSec`/`displayedSec` are offsets into the matched recording. */
export interface SongUsage {
  title: string
  tonoWorkId?: string
  ccliSongId?: string
  sundaysongId?: string
  firstShownSec?: number
  displayedSec?: number
}

/** Links one recording to its external service context. Persisted as a
 *  `<recording>.service.json` sidecar next to the audio/video file — mirrors
 *  the `.transcript.json` sidecar convention. */
export interface ServiceLink {
  source: 'stage' | 'plan' | 'manual'
  serviceId?: string
  churchId?: string
  serviceDate?: string        // YYYY-MM-DD
  wasStreamed?: boolean        // SundayRec is the source of truth for this
  setlist: SongUsage[]
  linkedAt: number             // unix ms
}

export interface IntegrationSettings {
  /** Master opt-in for the entire Sunday-suite area. */
  enabled: boolean
  sundayedit?: { enabled: boolean }
  stage?: { enabled: boolean; manifestFolder?: string }
  song?: { enabled: boolean; autoSubmitUsage?: boolean }
  plan?: { enabled: boolean; autoSchedule?: boolean }
  /** Shared cloud connection used by the Song/Plan flows. API keys are NOT
   *  stored here — they live encrypted via safeStorage (like stream keys). */
  connection?: {
    churchId?: string
    songApiUrl?: string
    planApiUrl?: string
  }
}
