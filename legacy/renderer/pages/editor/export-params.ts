// Export request assembly — the pure half of `runExport`.
//
// Split out of export.ts so the shape the renderer sends can be asserted in a
// unit test without a DOM, an `E` state object, or the IPC shim. Two things
// here are load-bearing and were regressions before:
//
//   1. `outputFolder` is ALWAYS a string. The default destination pill ("Samme
//      mappe") never picks a folder, so it sends '' — which the backend reads
//      as "next to the source file". Sending `undefined` instead put the shim's
//      `?? ''` fallback in charge of a decision that belongs here.
//   2. There is NO `mode` field. The old 'new' | 'replace' | 'folder' mode was
//      dropped by the shim and never implemented in Rust, so "Erstatt original"
//      silently behaved like "ny fil". The pill is gone; so is the field.

import { VIDEO_EXTS } from './state'

/**
 * Can intro/outro jingles be applied when exporting `filePath`?
 *
 * The export seam concatenates jingles onto the AUDIO track, and the video
 * graph has nowhere to splice them — so `editor::export` filters `introPath`
 * and `outroPath` out for every video container (mp4/mov/mkv/m4v). That
 * filtering was SILENT: the video jingle pickers accepted files, the export
 * modal listed them, and the finished mp4 simply did not contain them.
 *
 * Pure. Two inputs because the extension alone is not the whole truth:
 * {@link VIDEO_EXTS} covers the unambiguous containers the loader routes on
 * sight, but `.mkv`/`.webm` are decided by an ffprobe of the actual streams —
 * so the loader passes the RESOLVED `E.isVideoFile` once it knows, and the
 * extension answers before it does.
 */
export function jinglesSupportedForFile(filePath: string, resolvedIsVideo = false): boolean {
  if (resolvedIsVideo) return false
  const name = filePath.split(/[/\\]/).pop() ?? ''
  const dot = name.lastIndexOf('.')
  // No extension → nothing says "video"; treat it as audio (the loader probes
  // the streams for real, and a jingle-less audio export is harmless anyway).
  if (dot <= 0) return true
  return !VIDEO_EXTS.has(name.slice(dot).toLowerCase())
}

/** One cut region on the recording timeline (seconds). */
export interface ExportCutRegion {
  start: number
  end: number
}

/** Channel-repair settings as the backend's `EditorChannelRepair` expects them. */
export interface ExportChannelRepair {
  mode: string
  leftDb: number
  rightDb: number
}

/** Everything `buildExportRequest` needs, read out of the editor state + DOM by
 *  the caller. `kind` picks the audio-format fields or the video-codec ones. */
export interface ExportRequestInput {
  kind: 'audio' | 'video'
  inputPath: string
  cutRegions: readonly ExportCutRegion[]
  duration: number
  /** '' = "Samme mappe" (the default). A picked folder is an absolute path. */
  outputFolder: string
  /** Audio container (`mp3|wav|flac|aac`) — audio exports only. */
  format?: string
  /** Audio bitrate in kbps — lossy audio formats only. */
  bitrate?: number
  /** WAV bit depth — WAV only. */
  bitDepth?: 16 | 24
  /** Video container (`mp4|mov|mkv`) — video exports only. */
  videoFormat?: string
  /** Video codec (`h264|h265`) — video exports only. */
  videoCodec?: string
  /** Peak-normalization gain in dB; 0 means "not normalized" → omitted. */
  gainDb?: number
  introPath?: string
  outroPath?: string
  metadata?: unknown
  masterPreset?: string
  vocalChainPreset?: string
  processing?: unknown
  channelRepair?: ExportChannelRepair
}

/** Drop an empty string / 0 so the backend sees `undefined` (→ `null`) rather
 *  than a falsy value it would have to special-case. */
function orUndefined<T>(value: T | undefined | ''): T | undefined {
  return value === '' || value === undefined ? undefined : value
}

/**
 * Build the params object handed to `window.api.editorExportFile` /
 * `editorExportVideo`. Pure — same input, same output, no DOM reads.
 */
export function buildExportRequest(input: ExportRequestInput): Record<string, unknown> {
  const shared: Record<string, unknown> = {
    inputPath: input.inputPath,
    cutRegions: input.cutRegions,
    duration: input.duration,
    // Always a string: '' means "same folder as the source", which the backend
    // resolves. Never `undefined`, never a `mode`.
    outputFolder: input.outputFolder ?? '',
    gainDb: input.gainDb ? input.gainDb : undefined,
    introPath: orUndefined(input.introPath),
    outroPath: orUndefined(input.outroPath),
    metadata: input.metadata,
    masterPreset: orUndefined(input.masterPreset),
    vocalChainPreset: orUndefined(input.vocalChainPreset),
    processing: input.processing,
    channelRepair: input.channelRepair,
  }

  if (input.kind === 'video') {
    return {
      ...shared,
      videoFormat: input.videoFormat || 'mp4',
      videoCodec: input.videoCodec || 'h264',
    }
  }
  return {
    ...shared,
    outputFormat: input.format || 'mp3',
    outputBitrate: input.bitrate,
    outputBitDepth: input.bitDepth,
  }
}

// ── Export modal: what the "level" row says ─────────────────────────────────

/**
 * What the export modal's processing row should state about the output LEVEL.
 *
 * The honest answer depends on the mastering preset, because the backend skips
 * the peak-normalize `volume=` filter whenever a preset is active: loudnorm
 * sets the delivery loudness, so a gain shift in front of it changes nothing in
 * the finished file. The modal used to claim "Normalisert (+4.2 dB)" anyway —
 * a promise about the export that the export did not keep.
 *
 * Returned as a decision rather than a string so it can be asserted without a
 * DOM or the i18n table; the caller localises it.
 */
export type ExportLevelSummary =
  | { kind: 'hidden' }
  | { kind: 'masterOwnsLevel' }
  | { kind: 'normalized'; gainDb: number }

export function exportLevelSummary(
  masterPreset: string | undefined,
  gainDb: number,
): ExportLevelSummary {
  if (masterPreset) return { kind: 'masterOwnsLevel' }
  if (Number.isFinite(gainDb) && gainDb !== 0) return { kind: 'normalized', gainDb }
  return { kind: 'hidden' }
}
