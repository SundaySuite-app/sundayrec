import { useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { waveformPath } from "./waveform";
import {
  addCut,
  clampMain,
  commitResize,
  deleteCut,
  fitAll,
  getRemainingDuration,
  hitTest,
  panBy,
  resizeCut,
  secToX,
  snapOutOfCut,
  snapToSegmentBoundary,
  xToSec,
  zoomBy,
  type Cut,
  type CutState,
  type Segment,
  type SegmentToggles,
  type Viewport,
} from "./editorGeometry";

/** The fixed SVG coordinate space the canvas paints into; pointer pixel offsets
 *  are converted into this space so the geometry is resolution-independent. */
const VIEW_W = 1000;
const VIEW_H = 100;
/** Top band (in VIEW_H units) that grabs the playhead — the "ruler". */
const RULER_H = 28;

/** Default snap toggles when the panel doesn't drive them (speech+music on). */
const DEFAULT_TOGGLES: SegmentToggles = {
  speech: true,
  music: true,
  silence: false,
};

export interface EditorCanvasProps {
  /** Waveform peaks (0..1), already decoded by the seam. */
  peaks: number[];
  /** Authoritative file duration in seconds. */
  duration: number;
  /** Detected segments for snap-to-boundary + colour bands (optional). */
  segments?: Segment[];
  /** Which segment kinds snap (defaults to speech+music). */
  toggles?: SegmentToggles;
  /** The controlled cut state (owned by the panel for export + autosave). */
  cutState: CutState;
  onCutStateChange: (next: CutState) => void;
  /** The playhead position in seconds (controlled). */
  playheadSec: number;
  onSeek: (sec: number) => void;
}

/** Map a pointer event's clientX/Y into the SVG's VIEW_W×VIEW_H coordinate
 *  space using the element's bounding box. Returns null if no box yet. */
function toViewCoords(
  el: SVGSVGElement | null,
  clientX: number,
  clientY: number,
): { x: number; y: number } | null {
  if (!el) return null;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return null;
  return {
    x: ((clientX - rect.left) / rect.width) * VIEW_W,
    y: ((clientY - rect.top) / rect.height) * VIEW_H,
  };
}

/**
 * The interactive editor canvas — the depth Electron has (drag-on-waveform)
 * that the Tauri panel was missing. Renders peaks + cut bands + the playhead +
 * an in-flight drag selection to SVG, and routes pointer events through the
 * pure `editorGeometry` model:
 *
 *  • drag on blank canvas → mark a cut (snap edges to segment boundaries unless
 *    shift is held);
 *  • drag a cut-boundary handle → resize, snapping unless shift;
 *  • click/drag in the ruler band → seek the playhead (snapped out of cuts);
 *  • wheel → pan; ctrl/⌘+wheel → zoom around the cursor;
 *  • a minimap strip shows the whole file with the viewport window;
 *  • "undo all" clears the cuts (kept undoable in the panel's history).
 *
 * The SVG paint itself is // GUI-UNVERIFIED, but every pointer handler mutates
 * data/state through the tested geometry, so the resulting cut-plan + viewport
 * + playhead are all asserted in the vitest via fireEvent.
 */
export function EditorCanvas({
  peaks,
  duration,
  segments = [],
  toggles = DEFAULT_TOGGLES,
  cutState,
  onCutStateChange,
  playheadSec,
  onSeek,
}: EditorCanvasProps) {
  const { t } = useTranslation();
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [vp, setVp] = useState<Viewport>(() => fitAll(duration));
  // In-flight gestures. A drag selection (`drag`) paints a live band; a handle
  // drag tracks which cut edge is moving; a playhead drag tracks the ruler grab.
  const [drag, setDrag] = useState<{ startSec: number; endSec: number } | null>(
    null,
  );
  const gesture = useRef<
    | { kind: "handle"; cutIdx: number; side: "start" | "end" }
    | { kind: "playhead" }
    | { kind: "drag" }
    | null
  >(null);

  const cuts = cutState.cuts;

  // px positions in the VIEW space for the current viewport.
  const playheadX = useMemo(
    () => secToX(playheadSec, vp, VIEW_W),
    [playheadSec, vp],
  );

  const remaining = useMemo(
    () => getRemainingDuration(cuts, duration),
    [cuts, duration],
  );

  // ── Pointer handlers ───────────────────────────────────────────────────────

  const onPointerDown = useCallback(
    (e: React.PointerEvent<SVGSVGElement>) => {
      if (peaks.length === 0) return;
      const p = toViewCoords(svgRef.current, e.clientX, e.clientY);
      if (!p) return;
      const hit = hitTest(p.x, p.y, {
        vp,
        w: VIEW_W,
        cuts,
        playheadSec,
        rulerHeight: RULER_H,
      });
      svgRef.current?.setPointerCapture?.(e.pointerId);
      if (hit.kind === "handle") {
        gesture.current = {
          kind: "handle",
          cutIdx: hit.cutIdx,
          side: hit.side,
        };
        return;
      }
      if (hit.kind === "playhead") {
        gesture.current = { kind: "playhead" };
        onSeek(clampMain(xToSec(p.x, vp, VIEW_W), duration));
        return;
      }
      // Blank → begin a cut-creation drag.
      const sec = clampMain(xToSec(p.x, vp, VIEW_W), duration);
      gesture.current = { kind: "drag" };
      setDrag({ startSec: sec, endSec: sec });
    },
    [peaks.length, vp, cuts, playheadSec, onSeek, duration],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<SVGSVGElement>) => {
      const g = gesture.current;
      if (!g) return;
      const p = toViewCoords(svgRef.current, e.clientX, e.clientY);
      if (!p) return;
      const sec = clampMain(xToSec(p.x, vp, VIEW_W), duration);

      if (g.kind === "handle") {
        const snapped = e.shiftKey
          ? sec
          : snapToSegmentBoundary(sec, segments, vp, VIEW_W, toggles);
        onCutStateChange(
          resizeCut(cutState, g.cutIdx, g.side, snapped, duration),
        );
        return;
      }
      if (g.kind === "playhead") {
        onSeek(sec);
        return;
      }
      // Drag selection.
      setDrag((d) => (d ? { ...d, endSec: sec } : d));
    },
    [vp, duration, segments, toggles, cutState, onCutStateChange, onSeek],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent<SVGSVGElement>) => {
      const g = gesture.current;
      gesture.current = null;
      svgRef.current?.releasePointerCapture?.(e.pointerId);
      if (!g) return;

      if (g.kind === "handle") {
        onCutStateChange(commitResize(cutState));
        return;
      }
      if (g.kind === "playhead") {
        // Snap the playhead out of any cut it landed in (cuts are skip-zones).
        onSeek(snapOutOfCut(playheadSec, cuts, duration));
        return;
      }
      // Drag selection → either a cut (≥ min span) or a tap-to-seek.
      const d = drag;
      setDrag(null);
      if (!d) return;
      if (Math.abs(d.endSec - d.startSec) > 0.1) {
        const s = e.shiftKey
          ? d.startSec
          : snapToSegmentBoundary(d.startSec, segments, vp, VIEW_W, toggles);
        const en = e.shiftKey
          ? d.endSec
          : snapToSegmentBoundary(d.endSec, segments, vp, VIEW_W, toggles);
        onCutStateChange(addCut(cutState, s, en, duration));
      } else {
        // Tap to seek, snapped out of any cut.
        onSeek(snapOutOfCut(d.startSec, cuts, duration));
      }
    },
    [
      cutState,
      onCutStateChange,
      onSeek,
      playheadSec,
      cuts,
      duration,
      drag,
      segments,
      vp,
      toggles,
    ],
  );

  const onWheel = useCallback(
    (e: React.WheelEvent<SVGSVGElement>) => {
      if (peaks.length === 0) return;
      if (e.ctrlKey || e.metaKey) {
        const p = toViewCoords(svgRef.current, e.clientX, e.clientY);
        const anchor = p ? xToSec(p.x, vp, VIEW_W) : undefined;
        const factor = e.deltaY > 0 ? 1.25 : 0.75;
        setVp((cur) => zoomBy(cur, factor, duration, anchor));
      } else {
        setVp((cur) =>
          panBy(cur, (e.deltaY * (cur.end - cur.start)) / 800, duration),
        );
      }
    },
    [peaks.length, vp, duration],
  );

  // Right-click a cut to delete it (mirrors Electron `onCanvasContextMenu`).
  const onContextMenu = useCallback(
    (e: React.MouseEvent<SVGSVGElement>) => {
      e.preventDefault();
      if (peaks.length === 0) return;
      const p = toViewCoords(svgRef.current, e.clientX, e.clientY);
      if (!p) return;
      const sec = xToSec(p.x, vp, VIEW_W);
      const idx = cuts.findIndex((c) => sec >= c.start && sec <= c.end);
      if (idx >= 0) onCutStateChange(deleteCut(cutState, idx));
    },
    [peaks.length, vp, cuts, cutState, onCutStateChange],
  );

  // ── Zoom / fit buttons (keyboard-free path; also drives the geometry) ───────
  const zoomIn = useCallback(
    () => setVp((cur) => zoomBy(cur, 0.5, duration)),
    [duration],
  );
  const zoomOut = useCallback(
    () => setVp((cur) => zoomBy(cur, 2, duration)),
    [duration],
  );
  const fit = useCallback(() => setVp(fitAll(duration)), [duration]);

  // Visible cut bands (in VIEW px) for the current viewport.
  const cutBands = useMemo(
    () =>
      cuts.map((c, i) => ({
        i,
        x: secToX(c.start, vp, VIEW_W),
        w: secToX(c.end, vp, VIEW_W) - secToX(c.start, vp, VIEW_W),
      })),
    [cuts, vp],
  );

  // The minimap shows the whole file with a window rect for the viewport.
  const minimapWin = useMemo(() => {
    const full = duration || 1;
    return {
      x: (vp.start / full) * VIEW_W,
      w: ((vp.end - vp.start) / full) * VIEW_W,
    };
  }, [vp, duration]);

  const dragBand = useMemo(() => {
    if (!drag) return null;
    const lo = Math.min(drag.startSec, drag.endSec);
    const hi = Math.max(drag.startSec, drag.endSec);
    const x = secToX(lo, vp, VIEW_W);
    return { x, w: secToX(hi, vp, VIEW_W) - x };
  }, [drag, vp]);

  return (
    <div className="flex flex-col gap-1" data-testid="editor-canvas-wrap">
      {/* Main interactive waveform canvas. */}
      <svg
        ref={svgRef}
        role="img"
        aria-label={t("editor.waveform", "Bølgeform")}
        data-testid="editor-canvas"
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        preserveAspectRatio="none"
        className="h-28 w-full touch-none select-none rounded border border-zinc-800 bg-zinc-900"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onWheel={onWheel}
        onContextMenu={onContextMenu}
      >
        {/* Ruler band hint. */}
        <rect
          x={0}
          y={0}
          width={VIEW_W}
          height={RULER_H}
          className="fill-zinc-800/40"
        />
        {/* Segment colour bands (sermon highlighted). */}
        {duration > 0 &&
          segments.map((s, i) => (
            <rect
              key={`seg-${i}`}
              x={secToX(s.start, vp, VIEW_W)}
              y={VIEW_H - 4}
              width={secToX(s.end, vp, VIEW_W) - secToX(s.start, vp, VIEW_W)}
              height={4}
              className={
                s.kind === "sermon" ? "fill-amber-400" : "fill-sky-500/50"
              }
            />
          ))}
        {/* Waveform polygon. */}
        {peaks.length > 0 && (
          <polygon
            points={waveformPath(peaks, VIEW_W, VIEW_H)}
            className="fill-emerald-600/70"
          />
        )}
        {/* Committed cut bands + their resize handles. */}
        {cutBands.map((b) => (
          <g key={`cut-${b.i}`}>
            <rect
              x={b.x}
              y={0}
              width={b.w}
              height={VIEW_H}
              className="fill-red-500/30 stroke-red-400"
            />
            <rect
              x={b.x - 2}
              y={0}
              width={4}
              height={VIEW_H}
              className="fill-red-400"
            />
            <rect
              x={b.x + b.w - 2}
              y={0}
              width={4}
              height={VIEW_H}
              className="fill-red-400"
            />
          </g>
        ))}
        {/* In-flight drag selection. */}
        {dragBand && (
          <rect
            x={dragBand.x}
            y={0}
            width={dragBand.w}
            height={VIEW_H}
            className="fill-amber-400/30 stroke-amber-300"
          />
        )}
        {/* Playhead. */}
        <line
          x1={playheadX}
          y1={0}
          x2={playheadX}
          y2={VIEW_H}
          className="stroke-white"
          strokeWidth={1}
        />
      </svg>

      {/* Minimap — whole file with the viewport window. */}
      <svg
        role="img"
        aria-label={t("editor.minimap", "Oversikt")}
        data-testid="editor-minimap"
        viewBox={`0 0 ${VIEW_W} 16`}
        preserveAspectRatio="none"
        className="h-4 w-full rounded border border-zinc-800 bg-zinc-900"
      >
        {peaks.length > 0 && (
          <polygon
            points={waveformPath(peaks, VIEW_W, 16)}
            className="fill-emerald-700/50"
          />
        )}
        <rect
          x={minimapWin.x}
          y={0}
          width={minimapWin.w}
          height={16}
          className="fill-white/15 stroke-white/40"
        />
      </svg>

      {/* Toolbar: zoom / fit / undo-all + a remaining-duration readout. */}
      <div className="flex items-center gap-2 text-xs">
        <button
          type="button"
          className="rounded border border-zinc-700 px-2 py-0.5 hover:bg-zinc-800"
          aria-label={t("editor.zoomIn", "Zoom inn")}
          onClick={zoomIn}
        >
          +
        </button>
        <button
          type="button"
          className="rounded border border-zinc-700 px-2 py-0.5 hover:bg-zinc-800"
          aria-label={t("editor.zoomOut", "Zoom ut")}
          onClick={zoomOut}
        >
          −
        </button>
        <button
          type="button"
          className="rounded border border-zinc-700 px-2 py-0.5 hover:bg-zinc-800"
          onClick={fit}
        >
          {t("editor.fitAll", "Vis alt")}
        </button>
        <span className="ml-auto opacity-70" data-testid="editor-remaining">
          {t("editor.remainingLabel", "Igjen")}: {remaining.toFixed(1)}s
        </span>
      </div>
    </div>
  );
}

/** Re-export the cut shape for panel typing convenience. */
export type { Cut };
