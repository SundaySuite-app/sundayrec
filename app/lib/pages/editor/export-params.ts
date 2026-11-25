// Export request assembly — the pure half of `runExport`.
//
// Split out of export.ts so the shape the renderer sends can be asserted in a
// unit test without a DOM, an `E` state object, or the IPC shim. Two things
// here are load-bearing and were regressions before:
//
//   1. The destination is NEVER a path (finding A2). It is
//      `outputFolderToken`: `null` for the default pill ("Samme mappe", which
//      the backend reads as "next to the source file"), or the opaque token
//      Rust minted when the operator picked a folder in the dialog RUST opened
//      (`editor_pick_output_folder`). Always present, never `undefined`, so the
//      shim's fallback is not in charge of a decision that belongs here — the
//      lesson of the old `outputFolder: ''`, which this replaced.
//   2. There is NO `mode` field. The old 'new' | 'replace' | 'folder' mode was
//      dropped by the shim and never implemented in Rust, so "Erstatt original"
//      silently behaved like "ny fil". The pill is gone; so is the field.

import { VIDEO_EXTS } from "./types";
import type { EditorExportRequest } from "@legacy/bindings/EditorExportRequest";

// ── Export progress phases (wire constants) ─────────────────────────────────
//
// The backend labels every `editor-export-progress` tick with a bare phase CODE
// the renderer localises. The codes are defined ONCE here and matched nowhere
// else in the renderer, so a rename can only happen in one place.
//
// The Rust side of the wire is `EXPORT_PHASE_MEASURING` / `EXPORT_PHASE_ENCODING`
// in `src-tauri/src/editor/mod.rs`, pinned to these same two strings by
// `export_phase_codes_match_the_renderer_literals`. Nothing type-checks across
// the IPC boundary: rename one side only and the export label silently degrades
// to the encoding fallback for the whole mastering measure pass. The two tests
// (that one, and `export_phases_are_the_wire_codes` here) are what break loudly.

/** Mastering pass 1 — a full-length loudness read with no percentage of its own. */
export const EXPORT_PHASE_MEASURING = "measuring";
/** The render itself — percentage against the kept duration. */
export const EXPORT_PHASE_ENCODING = "encoding";

/** Every phase code the backend can send, for exhaustiveness checks + the test. */
export const EXPORT_PHASES = [
  EXPORT_PHASE_MEASURING,
  EXPORT_PHASE_ENCODING,
] as const;

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
export function jinglesSupportedForFile(
  filePath: string,
  resolvedIsVideo = false,
): boolean {
  if (resolvedIsVideo) return false;
  const name = filePath.split(/[/\\]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  // No extension → nothing says "video"; treat it as audio (the loader probes
  // the streams for real, and a jingle-less audio export is harmless anyway).
  if (dot <= 0) return true;
  return !VIDEO_EXTS.has(name.slice(dot).toLowerCase());
}

/** One cut region on the recording timeline (seconds). */
export interface ExportCutRegion {
  start: number;
  end: number;
}

/**
 * What «Etter»-lyttingen ber bakenden gjengi (`editor_master_preview`). Lydfeltet
 * (`podcastAudio`) er det SAMME som eksportforespørselen bærer, og kutt og lengde er de
 * samme som eksportens: nivået utsnittet får er nivået programmet får (M2R #3).
 */
export interface MasterPreviewParams {
  podcastAudio: boolean;
  cutRegions: readonly ExportCutRegion[];
  fullLengthSec: number;
  startSec: number;
  durationSec: number;
}

/** Everything `buildExportRequest` needs, read out of the editor state + DOM by
 *  the caller. `kind` picks the audio-format fields or the video-codec ones. */
export interface ExportRequestInput {
  kind: "audio" | "video";
  /** The File token of the recording (`OpenedRecording.token`) — never a path
   *  (A2): Rust resolves it and re-validates the file. */
  sourceToken: string;
  cutRegions: readonly ExportCutRegion[];
  duration: number;
  /** `null` = "Samme mappe" (the default). A picked folder is Rust's token
   *  for it — never a path. */
  outputFolderToken: string | null;
  /** Audio container (`mp3|wav|flac|aac`) — audio exports only. */
  format?: string;
  /** Audio bitrate in kbps — lossy audio formats only. */
  bitrate?: number;
  /** WAV bit depth — WAV only. */
  bitDepth?: 16 | 24;
  /** Video container (`mp4|mov|mkv`) — video exports only. */
  videoFormat?: string;
  /** Video codec (`h264|h265`) — video exports only. */
  videoCodec?: string;
  /** Peak-normalization gain in dB; 0 means "not normalized" → omitted. */
  gainDb?: number;
  /** Splice in the SAVED intro/outro clip. A switch, not a path: the clips live
   *  in the settings and Rust reads them from there (A2). */
  useIntro?: boolean;
  useOutro?: boolean;
  metadata?: unknown;
  /** «Podkastklar lyd»: the one fixed `speech-clear` recipe. Absent = as recorded. */
  podcastAudio?: boolean;
}

/**
 * Build the params object handed to `window.api.editorExportFile` /
 * `editorExportVideo`. Pure — same input, same output, no DOM reads.
 */
export function buildExportRequest(
  input: ExportRequestInput,
): Record<string, unknown> {
  const shared: Record<string, unknown> = {
    sourceToken: input.sourceToken,
    cutRegions: input.cutRegions,
    duration: input.duration,
    // Always present: `null` means "same folder as the source", which the
    // backend resolves; a token is a folder Rust's own dialog answered. Never
    // `undefined`, never a path, never a `mode`.
    outputFolderToken: input.outputFolderToken ?? null,
    gainDb: input.gainDb ? input.gainDb : undefined,
    useIntro: input.useIntro ? true : undefined,
    useOutro: input.useOutro ? true : undefined,
    metadata: input.metadata,
    podcastAudio: input.podcastAudio ? true : undefined,
  };

  if (input.kind === "video") {
    return {
      ...shared,
      videoFormat: input.videoFormat || "mp4",
      videoCodec: input.videoCodec || "h264",
    };
  }
  return {
    ...shared,
    outputFormat: input.format || "mp3",
    outputBitrate: input.bitrate,
    outputBitDepth: input.bitDepth,
  };
}

/**
 * Map `buildExportRequest`'s OLD Electron-shaped params — `outputFormat` /
 * `outputBitrate` / `outputBitDepth` for audio, `videoFormat` / `videoCodec`
 * for video — onto the wire shape `editor_export` actually reads. Pure, and
 * the ONLY place that does this rename: `api-shim.ts`'s `editorExportFile`/
 * `editorExportVideo` both call it (they used to duplicate this mapping
 * inline, one copy each, silently drifted apart — the audio copy never sent
 * `videoCodec` at all).
 *
 * The return type is the GENERATED `EditorExportRequest` binding, so a Rust
 * rename of any field fails `npm run typecheck` right here — the same class
 * of bug a `kind`/`type` split once was for a wire struct: the shim would go on building a payload the backend can no
 * longer parse, quietly, with every other gate green.
 */
export function toEditorExportRequest(
  kind: "audio" | "video",
  params: Record<string, unknown>,
): EditorExportRequest {
  const o = params;
  const m = (o.metadata ?? {}) as Record<string, unknown>;

  return {
    sourceToken: o.sourceToken as string,
    cutRegions: (o.cutRegions ?? []) as EditorExportRequest["cutRegions"],
    duration: (o.duration ?? 0) as number,
    format:
      kind === "video"
        ? (o.videoFormat as string) || "mp4"
        : ((o.outputFormat ?? o.format ?? "mp3") as string),
    outputFolderToken: (o.outputFolderToken ?? null) as string | null,
    bitrate:
      kind === "video" ? null : ((o.outputBitrate ?? null) as number | null),
    bitDepth:
      kind === "video" ? null : ((o.outputBitDepth ?? null) as number | null),
    podcastAudio: o.podcastAudio === true,
    useIntro: o.useIntro === true,
    useOutro: o.useOutro === true,
    gainDb: (o.gainDb ?? null) as number | null,
    title: (m.title as string) || null,
    speaker: (m.speaker as string) || null,
    description: (m.description as string) || null,
    album: (m.album as string) || null,
    date: (m.date as string) || null,
    videoCodec: kind === "video" ? (o.videoCodec as string) || null : null,
  };
}

// ── Export modal: what the "level" row says ─────────────────────────────────

/**
 * What the export modal's processing row should state about the output LEVEL.
 *
 * The honest answer depends on «Podkastklar lyd», because the backend skips
 * the peak-normalize `volume=` filter whenever it is on: the preset's
 * level step sets the delivery loudness, so a gain shift in front of it changes
 * nothing in the finished file. The modal used to claim "Normalisert (+4.2 dB)" anyway —
 * a promise about the export that the export did not keep.
 *
 * Returned as a decision rather than a string so it can be asserted without a
 * DOM or the i18n table; the caller localises it.
 */
export type ExportLevelSummary =
  | { kind: "hidden" }
  | { kind: "masterOwnsLevel" }
  | { kind: "normalized"; gainDb: number };

export function exportLevelSummary(
  podcastAudio: boolean,
  gainDb: number,
): ExportLevelSummary {
  if (podcastAudio) return { kind: "masterOwnsLevel" };
  if (Number.isFinite(gainDb) && gainDb !== 0)
    return { kind: "normalized", gainDb };
  return { kind: "hidden" };
}
