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
export type {
  ChannelMode,
  FileFormat,
  FilenamePattern,
  DeviceChannels,
  ScheduleSlot,
  SpecialRecording,
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
}

/**
 * The settings model (R4): THE generated binding, one vocabulary end to end —
 * `crates/sundayrec-core/src/settings.rs` is the source, sqlite is the store,
 * and a Rust field rename/removal is a tsc error on every consumer here.
 */
export type Settings = SettingsGen


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
