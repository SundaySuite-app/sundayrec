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
  /**
   * When the recording BEGAN, epoch ms — `recording.started_at` straight from
   * the row (P3).
   *
   * `timestamp` is `created_at ?? started_at`, i.e. when the row was WRITTEN,
   * which for a finished service is when it ENDED. Dating a row by that is off
   * by the whole length of the recording: the 11:00 service reads «12:05». The
   * old table only ever showed a date, so the hour never mattered; Bibliotek
   * puts the clock in the title, so it does.
   */
  startedAt?: number
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
/** The `<name>.meta.json` sidecar. (Until v0.15 it also carried
 *  `chapters: ChapterMarker[]`; the chapter UI left, and a sidecar that still
 *  has the key passes through untouched — the loader assigns the raw object,
 *  the saver writes it back — it is simply not drawn or exported any more.) */
export interface RecordingMetadata {
  title: string
  speaker: string
  description: string
}

