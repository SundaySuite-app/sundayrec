import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { RecordingRow } from "@/lib/bindings/RecordingRow";
import type { EditorMediaInfo } from "@/lib/bindings/EditorMediaInfo";
import type { EditorPeaks } from "@/lib/bindings/EditorPeaks";
import type { EditorSegment } from "@/lib/bindings/EditorSegment";
import type { EditorLoudness } from "@/lib/bindings/EditorLoudness";
import type { EditorCutRegion } from "@/lib/bindings/EditorCutRegion";
import type { EditorExportRequest } from "@/lib/bindings/EditorExportRequest";
import type { EditorExportResult } from "@/lib/bindings/EditorExportResult";
import type { EditorMasterPreviewRequest } from "@/lib/bindings/EditorMasterPreviewRequest";
import type { EditorMasterPreviewResult } from "@/lib/bindings/EditorMasterPreviewResult";
import { EDITOR_RECORDINGS_KEY } from "./queryKey";
import { EditorCanvas } from "./EditorCanvas";
import {
  addCut,
  clearAll,
  deleteCut,
  emptyCutState,
  replaceCuts,
  resizeCut,
  commitResize,
  type CutState,
  type Segment,
} from "./editorGeometry";

/** The `.cuts-draft.json` sidecar shape — the autosaved cut-plan + a timestamp,
 *  mirroring the Electron `{ cuts, ts }`. Persisting this is the editor's
 *  reopen-ability: reopen a recording mid-edit and the marked cuts come back. */
type CutsDraft = { cuts: EditorCutRegion[]; ts: number };

/**
 * A mastering target the user picks. We surface friendly publishing-target
 * labels (None / Podcast −16 LUFS / Streaming −14 LUFS) but map each to the
 * core preset id it drives (kept in sync with
 * `sundayrec_core::mastering::master_presets`); `none` skips mastering.
 */
type MasterTarget = {
  /** Stable value used in the <select>; "none" means skip mastering. */
  readonly value: string;
  /** The core preset id this drives (`null` for "none"). */
  readonly presetId: string | null;
  /** i18n key + Norwegian fallback for the option label. */
  readonly key: string;
  readonly fallback: string;
};

const MASTER_TARGETS: readonly MasterTarget[] = [
  {
    value: "none",
    presetId: null,
    key: "editor.masterNone",
    fallback: "Ingen (uendret)",
  },
  {
    value: "speech-clear",
    presetId: "speech-clear",
    key: "editor.masterPodcast",
    fallback: "Podkast (−16 LUFS)",
  },
  {
    value: "speech-punchy",
    presetId: "speech-punchy",
    key: "editor.masterStreaming",
    fallback: "Strømming (−14 LUFS)",
  },
  {
    value: "speech-natural",
    presetId: "speech-natural",
    key: "editor.masterNatural",
    fallback: "Naturlig (−19 LUFS)",
  },
  {
    value: "music-speech",
    presetId: "music-speech",
    key: "editor.masterMusic",
    fallback: "Musikk + tale (−16 LUFS)",
  },
] as const;

/** The export formats the seam renders. */
const FORMATS = ["mp3", "aac", "wav", "flac", "mp4"] as const;
type Format = (typeof FORMATS)[number];

/** The basename of a path, for display (works for both `/` and `\`). */
function fileName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** True when an IPC rejection is the default-build "editor feature off" error,
 *  so the panel can show a calm hint rather than a red error. The seam returns
 *  `feature_disabled: …` in the message of a `validation` AppError. */
function isFeatureDisabled(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? String(err);
  return msg.includes("feature_disabled");
}

/** Whole seconds → `m:ss`, for the region rows + axis ticks. */
function clock(sec: number): string {
  const s = Math.max(0, Math.round(sec));
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * R2 editor panel. Pick a recording (history list or native file dialog), load
 * it (ffprobe duration/streams), pull the waveform peaks + content segments + a
 * loudness measurement, mark cut/trim regions to remove, choose a mastering
 * target + export format, and export — showing progress/result.
 *
 * The waveform is drawn as a simple SVG polyline from the peaks array; the
 * `<svg>` paint itself is // GUI-UNVERIFIED but the peaks→geometry mapping
 * (`waveformPath`) and the whole load→peaks→regions→export data flow are tested.
 *
 * The ffmpeg work is behind the default-off `editor` feature, so in the shipping
 * build the commands reject with `feature_disabled`; the panel renders that as a
 * "not built into this build" hint rather than an error.
 */
export function EditorPanel() {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<string | null>(null);
  const [format, setFormat] = useState<Format>("mp3");
  const [target, setTarget] = useState<string>("none");
  // The cut state is the canvas's tested history machine — the single source of
  // truth for the cut-plan (drag-on-waveform, handle-resize, undo) AND the
  // numeric aria-fallback rows below. The playhead is canvas-driven seek.
  const [cutState, setCutState] = useState<CutState>(() => emptyCutState());
  const [playheadSec, setPlayheadSec] = useState(0);
  const [disabled, setDisabled] = useState(false);
  // A cut-plan found in the recording's `.cuts-draft.json` sidecar on pick —
  // surfaced as a "restore" banner so the user opts in rather than us silently
  // overwriting their fresh session. Cleared once restored or dismissed.
  const [draftToRestore, setDraftToRestore] = useState<CutsDraft | null>(null);
  // Whether the current region set has been touched since the last persisted
  // draft, so the autosave effect only writes after a real edit.
  const dirty = useRef(false);

  const presetId = useMemo(
    () => MASTER_TARGETS.find((m) => m.value === target)?.presetId ?? null,
    [target],
  );

  const recordings = useQuery<RecordingRow[]>({
    queryKey: EDITOR_RECORDINGS_KEY,
    queryFn: () => invoke<RecordingRow[]>("recordings_list"),
  });

  // Per-file derived data, refetched (mutations on demand) when the selection
  // changes. We use mutations rather than queries so each ffmpeg pass is
  // explicitly user-triggered (decoding a 3 h service isn't a background fetch).
  const loadMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<EditorMediaInfo>("editor_load_recording", { inputPath: path }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });
  const peaksMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<EditorPeaks>("editor_peaks", { inputPath: path }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });
  const segmentsMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<EditorSegment[]>("editor_segments", { inputPath: path }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });
  const analyzeMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<EditorLoudness>("editor_mastering_analyze", {
        inputPath: path,
        presetId: presetId ?? "speech-clear",
      }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });
  const exportMutation = useMutation({
    mutationFn: (request: EditorExportRequest) =>
      invoke<EditorExportResult>("editor_export", { request }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });
  // Windowed single-pass mastering preview — render a short snippet through the
  // chosen preset so the user can A/B it against the original before committing
  // to a full export. Returns a temp mp3 path the renderer plays.
  const previewMutation = useMutation({
    mutationFn: (request: EditorMasterPreviewRequest) =>
      invoke<EditorMasterPreviewResult>("editor_master_preview", { request }),
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });

  const onSelect = useCallback(
    (path: string) => {
      setSelected(path);
      setDisabled(false);
      setCutState(emptyCutState());
      setPlayheadSec(0);
      setDraftToRestore(null);
      dirty.current = false;
      peaksMutation.reset();
      segmentsMutation.reset();
      analyzeMutation.reset();
      exportMutation.reset();
      previewMutation.reset();
      // Probe immediately so the user sees duration/streams on pick, and pull
      // the waveform so they have something to mark cut regions against.
      loadMutation.mutate(path);
      peaksMutation.mutate(path);
      // Look for a `.cuts-draft.json` left by a prior session — the editor's
      // reopen-ability. If one carries cuts, surface a restore banner.
      void invoke<CutsDraft | null>("editor_read_sidecar", {
        mediaPath: path,
        sidecar: "cutsDraft",
      })
        .then((draft) => {
          if (draft && Array.isArray(draft.cuts) && draft.cuts.length > 0) {
            setDraftToRestore(draft);
          }
        })
        .catch(() => {
          /* no draft / feature off → nothing to restore */
        });
    },
    [
      loadMutation,
      peaksMutation,
      segmentsMutation,
      analyzeMutation,
      exportMutation,
      previewMutation,
    ],
  );

  // Autosave the cut-plan to the `.cuts-draft.json` sidecar after any edit, so
  // the marks survive a crash / reopen. Only writes once the user has actually
  // touched the regions (the `dirty` ref), never on the initial empty state.
  useEffect(() => {
    if (!selected || !dirty.current) return;
    const draft: CutsDraft = {
      cuts: cutState.cuts
        .filter((c) => c.end > c.start)
        .map((c) => ({ start: c.start, end: c.end })),
      ts: Date.now(),
    };
    void invoke<boolean>("editor_write_sidecar", {
      mediaPath: selected,
      sidecar: "cutsDraft",
      value: draft,
    }).catch(() => {
      /* autosave is best-effort, exactly like the Electron handler */
    });
  }, [cutState, selected]);

  // The canvas drives all cut edits (drag, resize, undo) through the tested
  // history machine; we flag the session dirty so the autosave effect fires.
  const onCutStateChange = useCallback((next: CutState) => {
    dirty.current = true;
    setCutState(next);
  }, []);

  // Restore the cuts found in the sidecar into editable cuts (one history step).
  const onRestoreDraft = useCallback(() => {
    if (!draftToRestore) return;
    setCutState((s) => replaceCuts(s, draftToRestore.cuts));
    dirty.current = true;
    setDraftToRestore(null);
  }, [draftToRestore]);

  const onDismissDraft = useCallback(() => setDraftToRestore(null), []);

  // Native file picker — edit any audio/video on disk, not only history rows.
  const onPickFile = useCallback(async () => {
    const picked = await open({
      multiple: false,
      filters: [
        {
          name: t("editor.mediaFilter", "Lyd/video"),
          extensions: ["mp3", "m4a", "aac", "wav", "flac", "mp4", "mkv", "mov"],
        },
      ],
    });
    if (typeof picked === "string") onSelect(picked);
  }, [onSelect, t]);

  const duration = loadMutation.data?.durationSec ?? 0;

  // ── Cut (trim region) editing — numeric aria fallback ───────────────────
  // The primary path is drag-on-canvas; these mirror the same edits onto the
  // tested cut-history machine so the numeric inputs (kept for accessibility /
  // headless) stay an equal first-class editor of the cut-plan.
  const addRegion = useCallback(() => {
    dirty.current = true;
    setCutState((s) => {
      // Seed a 10 s region after the last cut (or at the start), clamped to the
      // file duration when known. The user nudges start/end with the inputs.
      const last = s.cuts[s.cuts.length - 1];
      const start = last
        ? Math.min(last.end + 1, Math.max(0, duration - 1))
        : 0;
      const end = duration > 0 ? Math.min(start + 10, duration) : start + 10;
      return addCut(s, start, end, duration || end);
    });
  }, [duration]);

  const updateRegion = useCallback(
    (idx: number, patch: { start?: number; end?: number }) => {
      dirty.current = true;
      setCutState((s) => {
        let next = s;
        if (patch.start != null)
          next = resizeCut(
            next,
            idx,
            "start",
            patch.start,
            duration || Infinity,
          );
        if (patch.end != null)
          next = resizeCut(next, idx, "end", patch.end, duration || Infinity);
        return commitResize(next);
      });
    },
    [duration],
  );

  const removeRegion = useCallback((idx: number) => {
    dirty.current = true;
    setCutState((s) => deleteCut(s, idx));
  }, []);

  const clearRegions = useCallback(() => {
    dirty.current = true;
    setCutState((s) => clearAll(s));
  }, []);

  // ── Mastering preview ────────────────────────────────────────────────────
  // Render a 15 s snippet from the start of the file through the chosen preset.
  const onPreview = useCallback(() => {
    if (!selected || !presetId) return;
    previewMutation.mutate({
      inputPath: selected,
      presetId,
      startSec: 0,
      durationSec: 15,
    });
  }, [selected, presetId, previewMutation]);

  const onExport = useCallback(() => {
    if (!selected) return;
    const folder = selected.replace(/[/\\][^/\\]*$/, "");
    // The cut-plan: only well-formed cuts (end > start) become CutRegions.
    const cutRegions: EditorCutRegion[] = cutState.cuts
      .filter((c) => c.end > c.start)
      .map((c) => ({ start: c.start, end: c.end }));
    const request: EditorExportRequest = {
      inputPath: selected,
      cutRegions,
      duration,
      format,
      outputFolder: folder,
      bitrate: null,
      bitDepth: null,
      masterPreset: presetId,
    };
    exportMutation.mutate(request, {
      onSuccess: () => {
        // The edit landed — discard the autosaved draft so a future reopen
        // doesn't offer to restore stale cuts (mirrors editor-delete-cuts-draft).
        dirty.current = false;
        void invoke<boolean>("editor_delete_sidecar", {
          mediaPath: selected,
          sidecar: "cutsDraft",
        }).catch(() => {});
      },
    });
  }, [selected, duration, cutState, format, presetId, exportMutation]);

  const rows = recordings.data ?? [];
  const info = loadMutation.data;
  const peaks = peaksMutation.data;
  const loudness = analyzeMutation.data;
  // Detected segments feed the canvas snap-to-boundary + colour bands. The
  // `EditorSegment` binding already carries the `kind` the geometry expects.
  const segments: Segment[] = (segmentsMutation.data ?? []).map((s) => ({
    start: s.start,
    end: s.end,
    kind: s.kind,
  }));

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("editor.title", "Rediger lydfil")}
    >
      {disabled && (
        <p className="rounded-lg border border-accent/60 bg-accent p-2 text-xs text-accent">
          {t(
            "editor.featureDisabled",
            "Redigering er ikke bygget inn i denne versjonen.",
          )}
        </p>
      )}

      {/* ── Restore-draft banner (reopen-ability) ──────────────────────── */}
      {draftToRestore && (
        <div className="flex items-center gap-2 rounded-lg border border-sky-700 bg-sky-950/40 p-2 text-xs">
          <span className="flex-1 text-text">
            {t(
              "editor.draftFound",
              "Fant lagrede kutt fra forrige økt ({{n}}).",
              { n: draftToRestore.cuts.length },
            )}
          </span>
          <button
            type="button"
            className="rounded-lg border border-sky-600 px-2 py-0.5 text-sky-300 hover:bg-sky-900"
            onClick={onRestoreDraft}
          >
            {t("editor.draftRestore", "Gjenopprett")}
          </button>
          <button
            type="button"
            className="rounded-lg border border-border bg-surface2 px-2 py-0.5 text-text2 hover:bg-surface3"
            aria-label={t("editor.draftDismiss", "Forkast")}
            onClick={onDismissDraft}
          >
            ✕
          </button>
        </div>
      )}

      {/* ── Recording picker ───────────────────────────────────────────── */}
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-6">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium text-text">
            {t("editor.pickTitle", "Velg opptak")}
          </h2>
          <button
            type="button"
            className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
            onClick={() => void onPickFile()}
          >
            {t("editor.openFile", "Åpne lydfil…")}
          </button>
        </div>
        {rows.length === 0 ? (
          <p className="text-text3">
            {t("editor.noRecordings", "Ingen opptak ennå")}
          </p>
        ) : (
          <ul className="flex flex-col gap-1">
            {rows.map((r) => (
              <li key={r.id}>
                <button
                  type="button"
                  className={`w-full truncate rounded-lg border px-2 py-1 text-left text-xs hover:bg-surface2 ${
                    selected === r.file_path
                      ? "border-accent text-accent"
                      : "border-border text-text2"
                  }`}
                  onClick={() => onSelect(r.file_path)}
                  title={r.file_path}
                >
                  {fileName(r.file_path)}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* ── Loaded recording ───────────────────────────────────────────── */}
      {selected && (
        <div className="flex flex-col gap-3">
          {info && (
            <p className="text-xs text-text2">
              {t("editor.duration", "Varighet")}: {info.durationSec.toFixed(1)}s
              {" · "}
              {info.hasVideo
                ? t("editor.video", "video")
                : t("editor.audioOnly", "kun lyd")}
              {info.channels != null && ` · ${info.channels}ch`}
            </p>
          )}

          {/* ── Interactive waveform canvas ──────────────────────────────
              The interaction depth Electron has (drag-on-waveform): drag to
              mark a cut, drag a handle to resize (snap to detected segment
              boundaries unless shift), click the ruler to seek, wheel to pan,
              ⌘+wheel to zoom, plus a minimap. The SVG paint is GUI-UNVERIFIED,
              but every pointer handler drives the tested `editorGeometry`
              model, so the resulting cut-plan / viewport / playhead are
              asserted in vitest via fireEvent. */}
          <div className="flex flex-col gap-1">
            {peaks && peaks.peaks.length > 0 ? (
              <EditorCanvas
                peaks={peaks.peaks}
                duration={duration}
                segments={segments}
                cutState={cutState}
                onCutStateChange={onCutStateChange}
                playheadSec={playheadSec}
                onSeek={setPlayheadSec}
              />
            ) : (
              <p className="text-xs text-text3">
                {peaksMutation.isPending
                  ? t("editor.loading", "Laster inn lydfil…")
                  : t("editor.noWaveform", "Ingen bølgeform ennå")}
              </p>
            )}
            {peaks && (
              <p className="text-xs text-text2">
                {t("editor.peaksCount", "{{n}} bølgeform-punkter", {
                  n: peaks.peaks.length,
                })}
              </p>
            )}
          </div>

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
              disabled={peaksMutation.isPending}
              onClick={() => peaksMutation.mutate(selected)}
            >
              {t("editor.loadPeaks", "Bølgeform")}
            </button>
            <button
              type="button"
              className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
              disabled={segmentsMutation.isPending}
              onClick={() => segmentsMutation.mutate(selected)}
            >
              {t("editor.detectSegments", "Finn segmenter")}
            </button>
            <button
              type="button"
              className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
              disabled={analyzeMutation.isPending}
              onClick={() => analyzeMutation.mutate(selected)}
            >
              {t("editor.analyzeLoudness", "Mål lydstyrke")}
            </button>
          </div>

          {segmentsMutation.data && segmentsMutation.data.length > 0 && (
            <ul className="flex flex-col gap-1 text-xs">
              {segmentsMutation.data.map((s, i) => (
                <li
                  key={`${s.start}-${i}`}
                  className={`rounded-lg border px-2 py-1 ${
                    s.kind === "sermon"
                      ? "border-accent/60 text-accent"
                      : "border-border text-text2"
                  }`}
                >
                  {s.label} · {s.start.toFixed(0)}–{s.end.toFixed(0)}s
                </li>
              ))}
            </ul>
          )}

          {loudness && (
            <p className="text-xs text-text2">
              {t("editor.loudness", "Lydstyrke")}: {loudness.inputI.toFixed(1)}{" "}
              LUFS → {loudness.targetLufs.toFixed(0)} LUFS
            </p>
          )}

          {/* ── Cut/trim regions — numeric aria fallback ─────────────────
              The canvas above is the primary editor; this numeric list is the
              accessible / headless equal — both read and write the same tested
              cut-history machine, and on export each well-formed cut becomes an
              `EditorCutRegion` in the cut-plan the core removes. */}
          <div className="flex flex-col gap-2 border-t border-border pt-3">
            <div className="flex items-center justify-between">
              <h3 className="text-xs font-medium text-text">
                {t("editor.cutsTitle", "Kuttede regioner")}
              </h3>
              <div className="flex gap-2">
                <button
                  type="button"
                  className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                  onClick={addRegion}
                >
                  {t("editor.addCut", "Legg til kutt")}
                </button>
                {cutState.cuts.length > 0 && (
                  <button
                    type="button"
                    className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                    onClick={clearRegions}
                  >
                    {t("editor.cutsNone", "Fjern alle kutt")}
                  </button>
                )}
              </div>
            </div>
            {cutState.cuts.length === 0 ? (
              <p className="text-xs text-text3">
                {t("editor.dragHint", "Klikk og dra for å markere et kutt")}
              </p>
            ) : (
              <ul className="flex flex-col gap-1">
                {cutState.cuts.map((c, idx) => (
                  <li
                    key={idx}
                    className="flex items-center gap-2 text-xs"
                    aria-label={t("editor.cutRegion", "Kutt")}
                  >
                    <input
                      type="number"
                      min={0}
                      step={1}
                      value={c.start}
                      aria-label={t("editor.cutStart", "Start (sekunder)")}
                      className="w-16 rounded-lg border border-border bg-surface2 px-1 py-0.5 text-text"
                      onChange={(e) =>
                        updateRegion(idx, { start: Number(e.target.value) })
                      }
                    />
                    <span className="text-text3">→</span>
                    <input
                      type="number"
                      min={0}
                      step={1}
                      value={c.end}
                      aria-label={t("editor.cutEnd", "Slutt (sekunder)")}
                      className="w-16 rounded-lg border border-border bg-surface2 px-1 py-0.5 text-text"
                      onChange={(e) =>
                        updateRegion(idx, { end: Number(e.target.value) })
                      }
                    />
                    <span className="text-text3">
                      {clock(c.start)}–{clock(c.end)}
                    </span>
                    <button
                      type="button"
                      className="ml-auto rounded-lg border border-border bg-surface2 px-2 py-0.5 text-text2 hover:bg-surface3"
                      aria-label={t("editor.deleteCut", "Fjern kutt")}
                      onClick={() => removeRegion(idx)}
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {/* ── Export ───────────────────────────────────────────────── */}
          <div className="flex flex-col gap-2 border-t border-border pt-3">
            <div className="flex items-center gap-2">
              <label className="text-xs text-text2">
                {t("editor.format", "Format")}
              </label>
              <select
                className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text"
                value={format}
                onChange={(e) => setFormat(e.target.value as Format)}
                aria-label={t("editor.format", "Format")}
              >
                {FORMATS.map((f) => (
                  <option key={f} value={f}>
                    {f}
                  </option>
                ))}
              </select>
              <label className="text-xs text-text2">
                {t("editor.preset", "Mastering")}
              </label>
              <select
                className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text"
                value={target}
                onChange={(e) => setTarget(e.target.value)}
                aria-label={t("editor.preset", "Mastering")}
              >
                {MASTER_TARGETS.map((m) => (
                  <option key={m.value} value={m.value}>
                    {t(m.key, m.fallback)}
                  </option>
                ))}
              </select>
            </div>
            {/* Mastering A/B preview — only meaningful when a preset is chosen. */}
            {presetId && (
              <div className="flex flex-col gap-1">
                <button
                  type="button"
                  className="self-start rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
                  disabled={previewMutation.isPending}
                  onClick={onPreview}
                >
                  {previewMutation.isPending
                    ? t("editor.previewing", "Lager forhåndsvisning…")
                    : t("editor.preview", "Forhåndsvis mastering (15 s)")}
                </button>
                {previewMutation.data && (
                  <audio
                    controls
                    aria-label={t("editor.previewAudio", "Forhåndsvisning")}
                    src={previewMutation.data.previewPath}
                    className="w-full"
                  />
                )}
              </div>
            )}
            <button
              type="button"
              className="self-start rounded-lg bg-accent px-3 py-2 text-sm font-medium text-bg hover:bg-accent/90 disabled:opacity-50"
              disabled={exportMutation.isPending}
              onClick={onExport}
            >
              {exportMutation.isPending
                ? t("editor.saving", "Lagrer…")
                : t("editor.export", "Eksporter")}
            </button>
            {exportMutation.data && (
              <p className="text-xs text-emerald-300">
                {t("editor.exported", "Lagret")}:{" "}
                {fileName(exportMutation.data.outputPath)}
              </p>
            )}
            {exportMutation.isError && !disabled && (
              <p className="text-xs text-red-400">
                {t("editor.saveError", "✕ Feil ved lagring")}
              </p>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
