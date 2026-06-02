// Coordinate model — main-file seconds ↔ canvas pixels, with optional
// intro/outro slots at the file edges. Ported from the Electron renderer
// (`editor/geometry.ts`); the singleton `E` becomes an explicit `state` param.
//
//   ┌──────────┬────────────────────────────────┬──────────┐
//   │  INTRO   │         HOVEDOPPTAK            │   OUTRO  │
//   │ (dim)    │   (main waveform — full color) │  (dim)   │
//   └──────────┴────────────────────────────────┴──────────┘
//
// Intro/outro slots only appear when the matching file edge is visible
// (vpStart≈0 / vpEnd≈duration). Cuts/chapters/peaks/seek all live in main
// coords; the extended timeline ([-introDur, duration+outroDur]) only governs
// where the playhead can rest and which jingle audio plays.

import type { EditorState } from "./types";

export interface LayoutGeom {
  introPx: number;
  outroPx: number;
  mainPxStart: number;
  mainPxEnd: number;
  effIntroDur: number;
  effOutroDur: number;
}

// One-slot cache: mousemove handlers call this twice per event and the inputs
// only change on zoom / scroll / intro-outro toggle. The key embeds every input,
// so even multiple engines share it safely (worst case: cache thrash, never a
// wrong value).
let _layoutGeomCache: { key: string; geom: LayoutGeom } | null = null;

export function getLayoutGeom(state: EditorState, W: number): LayoutGeom {
  const key = `${W}|${state.vpStart}|${state.vpEnd}|${state.includeIntroOutro ? 1 : 0}|${state.introBuffer ? 1 : 0}|${state.outroBuffer ? 1 : 0}|${state.duration}|${state.introDuration}|${state.outroDuration}`;
  if (_layoutGeomCache && _layoutGeomCache.key === key)
    return _layoutGeomCache.geom;

  const showIntro =
    state.includeIntroOutro && !!state.introBuffer && state.vpStart <= 0.001;
  const showOutro =
    state.includeIntroOutro &&
    !!state.outroBuffer &&
    state.vpEnd >= state.duration - 0.001;
  const effIntroDur = showIntro ? state.introDuration : 0;
  const effOutroDur = showOutro ? state.outroDuration : 0;
  const mainVpDur = Math.max(0.001, state.vpEnd - state.vpStart);
  const total = effIntroDur + mainVpDur + effOutroDur;
  const introPx = (effIntroDur / total) * W;
  const outroPx = (effOutroDur / total) * W;
  const geom: LayoutGeom = {
    introPx,
    outroPx,
    mainPxStart: introPx,
    mainPxEnd: W - outroPx,
    effIntroDur,
    effOutroDur,
  };
  _layoutGeomCache = { key, geom };
  return geom;
}

export function effIntroDur(state: EditorState): number {
  return state.includeIntroOutro && state.introBuffer ? state.introDuration : 0;
}
export function effOutroDur(state: EditorState): number {
  return state.includeIntroOutro && state.outroBuffer ? state.outroDuration : 0;
}
export function minPlayableSec(state: EditorState): number {
  return -effIntroDur(state);
}
export function maxPlayableSec(state: EditorState): number {
  return state.duration + effOutroDur(state);
}
export function clampPlayable(state: EditorState, sec: number): number {
  return Math.max(minPlayableSec(state), Math.min(maxPlayableSec(state), sec));
}
export function clampMain(state: EditorState, sec: number): number {
  return Math.max(0, Math.min(state.duration, sec));
}

export function secToX(state: EditorState, sec: number, W: number): number {
  const g = getLayoutGeom(state, W);
  if (sec < 0 && g.introPx > 0 && g.effIntroDur > 0) {
    const frac = (sec + g.effIntroDur) / g.effIntroDur;
    return Math.max(0, Math.min(1, frac)) * g.introPx;
  }
  if (sec > state.duration && g.outroPx > 0 && g.effOutroDur > 0) {
    const frac = (sec - state.duration) / g.effOutroDur;
    return g.mainPxEnd + Math.max(0, Math.min(1, frac)) * g.outroPx;
  }
  const mainW = g.mainPxEnd - g.mainPxStart;
  if (mainW <= 0) return g.mainPxStart;
  return (
    g.mainPxStart +
    ((sec - state.vpStart) / (state.vpEnd - state.vpStart)) * mainW
  );
}

export function xToSec(state: EditorState, x: number, W: number): number {
  const g = getLayoutGeom(state, W);
  if (g.introPx > 0 && g.effIntroDur > 0 && x < g.mainPxStart) {
    const frac = Math.max(0, Math.min(1, x / g.introPx));
    return -g.effIntroDur + frac * g.effIntroDur;
  }
  if (g.outroPx > 0 && g.effOutroDur > 0 && x > g.mainPxEnd) {
    const frac = Math.max(0, Math.min(1, (x - g.mainPxEnd) / g.outroPx));
    return state.duration + frac * g.effOutroDur;
  }
  const mainW = g.mainPxEnd - g.mainPxStart;
  if (mainW <= 0) return state.vpStart;
  if (x <= g.mainPxStart) return state.vpStart;
  if (x >= g.mainPxEnd) return state.vpEnd;
  return (
    state.vpStart +
    ((x - g.mainPxStart) / mainW) * (state.vpEnd - state.vpStart)
  );
}

/** x → main-coords only (never reads intro/outro slots). Used by cut handling. */
export function xToMainSec(state: EditorState, x: number, W: number): number {
  const g = getLayoutGeom(state, W);
  const mainW = g.mainPxEnd - g.mainPxStart;
  if (mainW <= 0) return state.vpStart;
  if (x <= g.mainPxStart) return state.vpStart;
  if (x >= g.mainPxEnd) return state.vpEnd;
  return (
    state.vpStart +
    ((x - g.mainPxStart) / mainW) * (state.vpEnd - state.vpStart)
  );
}

// ── Viewport (zoom / pan / fit) ───────────────────────────────────────────

export function fitAll(state: EditorState): void {
  state.vpStart = 0;
  state.vpEnd = state.duration || 1;
}

export function zoomBy(state: EditorState, factor: number): void {
  const center = (state.vpStart + state.vpEnd) / 2;
  const half = ((state.vpEnd - state.vpStart) * factor) / 2;
  state.vpStart = Math.max(0, center - half);
  state.vpEnd = Math.min(state.duration, center + half);
  const minSpan = 0.5;
  if (state.vpEnd - state.vpStart < minSpan) {
    const mid = (state.vpStart + state.vpEnd) / 2;
    state.vpStart = Math.max(0, mid - minSpan / 2);
    state.vpEnd = Math.min(state.duration, state.vpStart + minSpan);
  }
}

export function panBy(state: EditorState, deltaSecs: number): void {
  const span = state.vpEnd - state.vpStart;
  state.vpStart = Math.max(
    0,
    Math.min(state.duration - span, state.vpStart + deltaSecs),
  );
  state.vpEnd = state.vpStart + span;
}
