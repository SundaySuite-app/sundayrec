// Waveform + minimap canvas rendering. Ported near-verbatim from the Electron
// renderer (`editor/waveform.ts`). Colours and the current playback second are
// passed in (instead of read from the singleton / getComputedStyle) so the draw
// is a pure function of (state, ctx, size). Norwegian labels are inlined to
// match the Electron defaults; i18n is threaded in a later phase.

import type { EditorState } from "./types";
import { getLayoutGeom, secToX, effIntroDur, effOutroDur } from "./geometry";
import { gainFactor } from "./peaks";
import { formatTime, formatDuration } from "./format";
import { isInCut, isInDrag } from "./cuts";

export interface WaveColors {
  surface: string; // canvas background (hex)
  accent: string; // waveform bars (hex)
}

export function hexToRgb(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `${r},${g},${b}`;
}

function shouldShowSegment(state: EditorState, type: string): boolean {
  if (type === "speech") return state.showSpeechSegments;
  if (type === "music") return state.showMusicSegments;
  if (type === "silence") return state.showSilenceSegments;
  return true; // sermon + anything else always shown
}

export function drawWaveform(
  state: EditorState,
  ctx: CanvasRenderingContext2D,
  W: number,
  H: number,
  colors: WaveColors,
  curSec: number,
): void {
  if (!state.peaks) return;

  const surfaceColor = colors.surface || "#13131c";
  ctx.fillStyle = surfaceColor;
  ctx.fillRect(0, 0, W, H);

  const RULER = 22;
  const midY = RULER + (H - RULER) / 2;
  const maxBar = (H - RULER - 10) / 2;
  const ACCENT = colors.accent || "#F0BB47";
  const RED = "#ef4444";

  drawRuler(state, ctx, W, RULER);

  // Subtle centre line
  ctx.strokeStyle = "rgba(255,255,255,0.05)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, midY);
  ctx.lineTo(W, midY);
  ctx.stroke();

  const geom = getLayoutGeom(state, W);

  // ── Suggested segment backgrounds (main region only) ──────────────
  for (const seg of state.suggestions) {
    if (!shouldShowSegment(state, seg.type)) continue;
    const x1 = secToX(state, seg.start, W);
    const x2 = secToX(state, seg.end, W);
    if (x2 < geom.mainPxStart || x1 > geom.mainPxEnd) continue;
    const clampX1 = Math.max(geom.mainPxStart, x1);
    const clampX2 = Math.min(x2, geom.mainPxEnd);
    let fillCol = "rgba(120,120,140,0.10)";
    let strokeCol = "rgba(120,120,140,0.4)";
    if (seg.type === "sermon") {
      fillCol = "rgba(240,187,71,0.22)";
      strokeCol = "#f0bb47";
    } else if (seg.type === "speech") {
      fillCol = "rgba(72,187,120,0.15)";
      strokeCol = "#48bb78";
    } else if (seg.type === "music") {
      fillCol = "rgba(99,179,237,0.15)";
      strokeCol = "#63b3ed";
    } else if (seg.type === "silence") {
      fillCol = "rgba(150,150,160,0.10)";
      strokeCol = "rgba(150,150,160,0.45)";
    }
    ctx.fillStyle = fillCol;
    ctx.fillRect(clampX1, RULER, clampX2 - clampX1, H - RULER);
    for (const bx of [x1, x2]) {
      if (bx < geom.mainPxStart - 2 || bx > geom.mainPxEnd + 2) continue;
      ctx.strokeStyle = strokeCol;
      ctx.lineWidth = 1.5;
      ctx.globalAlpha = 0.55;
      ctx.setLineDash([5, 4]);
      ctx.beginPath();
      ctx.moveTo(bx, RULER);
      ctx.lineTo(bx, H);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }
    if (clampX2 - clampX1 > 40) {
      ctx.font = "600 9px system-ui, -apple-system, sans-serif";
      ctx.textBaseline = "top";
      ctx.fillStyle = strokeCol;
      ctx.globalAlpha = 0.95;
      let lbl = seg.label;
      if (seg.type === "sermon") {
        const mins = Math.round((seg.end - seg.start) / 60);
        lbl = `★ ${state.labels.sermon} — ${mins} min`;
      } else if (lbl.length > 18) lbl = lbl.slice(0, 17) + "…";
      ctx.fillText(
        lbl,
        Math.max(clampX1 + 4, geom.mainPxStart + 2),
        RULER + 24,
      );
      ctx.globalAlpha = 1;
    }
  }

  // ── Cut region backgrounds (clipped to main region) ───────────────
  for (const c of state.cuts) {
    const x1 = secToX(state, c.start, W);
    const x2 = secToX(state, c.end, W);
    if (x2 < geom.mainPxStart || x1 > geom.mainPxEnd) continue;
    ctx.fillStyle = "rgba(239,68,68,0.13)";
    ctx.fillRect(
      Math.max(geom.mainPxStart, x1),
      RULER,
      Math.min(x2, geom.mainPxEnd) - Math.max(geom.mainPxStart, x1),
      H - RULER,
    );
  }

  // ── Active drag region ────────────────────────────────────────────
  if (state.isDragging && state.dragStartSec >= 0) {
    const x1 = secToX(state, Math.min(state.dragStartSec, state.dragEndSec), W);
    const x2 = secToX(state, Math.max(state.dragStartSec, state.dragEndSec), W);
    ctx.fillStyle = "rgba(251,146,60,0.18)";
    ctx.fillRect(x1, RULER, x2 - x1, H - RULER);
    ctx.strokeStyle = "#fb923c";
    ctx.lineWidth = 1.5;
    ctx.strokeRect(x1 + 0.5, RULER + 0.5, x2 - x1 - 1, H - RULER - 1);
  }

  // ── Intro waveform (dimmed, left slot) ────────────────────────────
  if (geom.introPx > 0 && state.introPeaks && state.introDuration > 0) {
    ctx.fillStyle = "#7AAAFF";
    for (let px = 0; px < geom.introPx; px++) {
      const sec = (px / geom.introPx) * state.introDuration;
      const pi = Math.floor(sec * 100);
      if (pi < 0 || pi >= state.introPeaks.length) continue;
      const barH = Math.min(maxBar, state.introPeaks[pi] * maxBar);
      ctx.globalAlpha = 0.55;
      ctx.fillRect(px, midY - barH, 1, barH * 2);
    }
    ctx.globalAlpha = 1;
    ctx.strokeStyle = "rgba(122,170,255,0.55)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(geom.introPx, RULER);
    ctx.lineTo(geom.introPx, H);
    ctx.stroke();
  }

  // ── Outro waveform (dimmed, right slot) ───────────────────────────
  if (geom.outroPx > 0 && state.outroPeaks && state.outroDuration > 0) {
    ctx.fillStyle = "#7AAAFF";
    for (let px = 0; px < geom.outroPx; px++) {
      const sec = (px / geom.outroPx) * state.outroDuration;
      const pi = Math.floor(sec * 100);
      if (pi < 0 || pi >= state.outroPeaks.length) continue;
      const barH = Math.min(maxBar, state.outroPeaks[pi] * maxBar);
      ctx.globalAlpha = 0.55;
      ctx.fillRect(geom.mainPxEnd + px, midY - barH, 1, barH * 2);
    }
    ctx.globalAlpha = 1;
    ctx.strokeStyle = "rgba(122,170,255,0.55)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(geom.mainPxEnd, RULER);
    ctx.lineTo(geom.mainPxEnd, H);
    ctx.stroke();
  }

  // ── Waveform bars (symmetric, scaled by normalize gain) ───────────
  const gFac = gainFactor(state);
  const mainPxStart = Math.floor(geom.mainPxStart);
  const mainPxEnd = Math.floor(geom.mainPxEnd);
  const mainPxWidth = Math.max(1, mainPxEnd - mainPxStart);
  for (let px = mainPxStart; px < mainPxEnd; px++) {
    const sec =
      state.vpStart +
      ((px - mainPxStart) / mainPxWidth) * (state.vpEnd - state.vpStart);
    const pi = Math.floor(sec * 100);
    if (pi < 0 || pi >= state.peaks.length) continue;

    const barH = Math.min(maxBar, state.peaks[pi] * gFac * maxBar);
    const inCut =
      isInCut(state, sec) || (state.isDragging && isInDrag(state, sec));
    const isPast = sec < curSec && (state.isPlaying || state.playStartSec > 0);

    ctx.fillStyle = inCut ? RED : ACCENT;
    ctx.globalAlpha = inCut ? 0.6 : isPast ? 0.3 : 0.82;
    ctx.fillRect(px, midY - barH, 1, barH * 2);
  }
  ctx.globalAlpha = 1;

  // ── Vignette — fades bars toward top + bottom ─────────────────────
  const sRgb = surfaceColor.startsWith("#")
    ? hexToRgb(surfaceColor)
    : "19,19,28";
  const vignette = ctx.createLinearGradient(0, RULER, 0, H);
  vignette.addColorStop(0, `rgba(${sRgb},0.70)`);
  vignette.addColorStop(0.22, `rgba(${sRgb},0.0)`);
  vignette.addColorStop(0.78, `rgba(${sRgb},0.0)`);
  vignette.addColorStop(1, `rgba(${sRgb},0.70)`);
  ctx.fillStyle = vignette;
  ctx.fillRect(0, RULER, W, H - RULER);

  // ── Cut boundary lines ────────────────────────────────────────────
  for (const c of state.cuts) {
    for (const s of [c.start, c.end]) {
      const x = secToX(state, s, W);
      if (x < -2 || x > W + 2) continue;
      ctx.strokeStyle = RED;
      ctx.lineWidth = 1.5;
      ctx.globalAlpha = 0.75;
      ctx.beginPath();
      ctx.moveTo(x, RULER);
      ctx.lineTo(x, H);
      ctx.stroke();
      ctx.globalAlpha = 1;
    }
  }

  // ── Cut duration labels inside cut regions ────────────────────────
  ctx.font = "600 10px system-ui, -apple-system, sans-serif";
  ctx.textBaseline = "middle";
  for (const c of state.cuts) {
    const x1 = secToX(state, c.start, W);
    const x2 = secToX(state, c.end, W);
    if (x2 - x1 < 28) continue;
    const label = formatDuration(c.end - c.start);
    const cx = Math.min(Math.max((x1 + x2) / 2, x1 + 4), x2 - 4);
    const tw = ctx.measureText(label).width;
    ctx.fillStyle = "rgba(239,68,68,0.22)";
    ctx.beginPath();
    if (ctx.roundRect) ctx.roundRect(cx - tw / 2 - 5, midY - 9, tw + 10, 18, 4);
    else ctx.rect(cx - tw / 2 - 5, midY - 9, tw + 10, 18);
    ctx.fill();
    ctx.fillStyle = "#fca5a5";
    ctx.textAlign = "center";
    ctx.fillText(label, cx, midY);
    ctx.textAlign = "left";
  }

  // ── Drag time labels ──────────────────────────────────────────────
  if (
    state.isDragging &&
    state.dragStartSec >= 0 &&
    Math.abs(state.dragEndSec - state.dragStartSec) > 0.05
  ) {
    const sA = Math.min(state.dragStartSec, state.dragEndSec);
    const sB = Math.max(state.dragStartSec, state.dragEndSec);
    ctx.font = "600 11px system-ui, -apple-system, sans-serif";
    ctx.textBaseline = "alphabetic";
    for (const [sec, anchor] of [
      [sA, "start"],
      [sB, "end"],
    ] as [number, string][]) {
      const x = secToX(state, sec, W);
      const label = formatTime(sec);
      const tw = ctx.measureText(label).width;
      const isLeft = anchor === "start";
      const tx = isLeft ? Math.max(x + 4, 4) : Math.min(x - tw - 4, W - tw - 4);
      ctx.fillStyle = "rgba(30,30,46,0.88)";
      if (ctx.roundRect) ctx.roundRect(tx - 3, RULER + 4, tw + 6, 16, 3);
      else ctx.rect(tx - 3, RULER + 4, tw + 6, 16);
      ctx.fill();
      ctx.fillStyle = "#fb923c";
      ctx.fillText(label, tx, RULER + 15);
    }
  }

  // ── Chapter markers ───────────────────────────────────────────────
  const CHAPTER_COLOR = "#06b6d4";
  for (const ch of state.meta.chapters) {
    const x = secToX(state, ch.time, W);
    if (x < -2 || x > W + 2) continue;
    ctx.strokeStyle = CHAPTER_COLOR;
    ctx.lineWidth = 1.5;
    ctx.globalAlpha = 0.85;
    ctx.setLineDash([4, 3]);
    ctx.beginPath();
    ctx.moveTo(x, RULER);
    ctx.lineTo(x, H);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
    ctx.fillStyle = CHAPTER_COLOR;
    ctx.beginPath();
    ctx.moveTo(x - 4, RULER);
    ctx.lineTo(x + 4, RULER);
    ctx.lineTo(x, RULER + 7);
    ctx.closePath();
    ctx.fill();
    ctx.font = "600 9px system-ui, -apple-system, sans-serif";
    ctx.textBaseline = "top";
    const label = ch.title.length > 14 ? ch.title.slice(0, 13) + "…" : ch.title;
    const tw = ctx.measureText(label).width;
    const tx = Math.min(Math.max(x + 3, 2), W - tw - 4);
    ctx.fillStyle = "rgba(6,182,212,0.15)";
    if (ctx.roundRect) ctx.roundRect(tx - 2, RULER + 8, tw + 4, 13, 2);
    else ctx.rect(tx - 2, RULER + 8, tw + 4, 13);
    ctx.fill();
    ctx.fillStyle = CHAPTER_COLOR;
    ctx.fillText(label, tx, RULER + 9);
    ctx.textBaseline = "middle";
  }

  // ── Section labels in the ruler (Intro / Hovedopptak / Outro) ──────
  if (geom.introPx > 0 || geom.outroPx > 0) {
    ctx.font = "600 10px system-ui, -apple-system, sans-serif";
    ctx.textBaseline = "middle";
    ctx.textAlign = "center";
    if (geom.introPx > 36) {
      ctx.fillStyle = "#7AAAFF";
      ctx.globalAlpha = 0.9;
      ctx.fillText(
        `${state.labels.intro} · ${formatDuration(state.introDuration)}`,
        geom.introPx / 2,
        RULER / 2,
      );
    }
    if (geom.outroPx > 36) {
      ctx.fillStyle = "#7AAAFF";
      ctx.globalAlpha = 0.9;
      ctx.fillText(
        `${state.labels.outro} · ${formatDuration(state.outroDuration)}`,
        geom.mainPxEnd + geom.outroPx / 2,
        RULER / 2,
      );
    }
    if (
      (geom.introPx > 36 || geom.outroPx > 36) &&
      geom.mainPxEnd - geom.mainPxStart > 80
    ) {
      ctx.fillStyle = ACCENT;
      ctx.globalAlpha = 0.85;
      ctx.fillText(
        state.labels.main,
        (geom.mainPxStart + geom.mainPxEnd) / 2,
        RULER / 2,
      );
    }
    ctx.globalAlpha = 1;
    ctx.textAlign = "left";
  }

  // ── Ghost cursor ──────────────────────────────────────────────────
  const hoverX = secToX(state, state.hoverSec, W);
  if (
    !state.isDragging &&
    state.hoverSec > -9999 &&
    hoverX >= 0 &&
    hoverX <= W
  ) {
    ctx.setLineDash([3, 4]);
    ctx.strokeStyle = "rgba(255,255,255,0.25)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(hoverX, RULER);
    ctx.lineTo(hoverX, H);
    ctx.stroke();
    ctx.setLineDash([]);

    let label: string;
    if (state.hoverSec < 0 && effIntroDur(state) > 0) {
      label = `${state.labels.intro} ${formatTime(state.hoverSec + effIntroDur(state))}`;
    } else if (state.hoverSec > state.duration && effOutroDur(state) > 0) {
      label = `${state.labels.outro} ${formatTime(state.hoverSec - state.duration)}`;
    } else {
      label = formatTime(state.hoverSec);
    }
    const hoveredSeg = state.suggestions.find(
      (s) =>
        state.hoverSec >= s.start &&
        state.hoverSec <= s.end &&
        shouldShowSegment(state, s.type),
    );
    if (hoveredSeg && state.hoverSec >= 0 && state.hoverSec <= state.duration) {
      const typeLbl =
        hoveredSeg.type === "sermon"
          ? state.labels.sermon
          : hoveredSeg.type === "speech"
            ? state.labels.speech
            : hoveredSeg.type === "music"
              ? state.labels.music
              : hoveredSeg.type === "silence"
                ? state.labels.silence
                : state.labels.mixed;
      label = `${typeLbl} · ${formatDuration(hoveredSeg.duration)}  (${formatTime(state.hoverSec)})`;
    }
    ctx.font = "600 10px system-ui, -apple-system, sans-serif";
    ctx.textBaseline = "middle";
    const tw = ctx.measureText(label).width;
    const tx = Math.min(Math.max(hoverX - tw / 2 - 5, 2), W - tw - 12);
    ctx.fillStyle = "rgba(20,20,36,0.9)";
    if (ctx.roundRect) ctx.roundRect(tx, H - 22, tw + 10, 16, 4);
    else ctx.rect(tx, H - 22, tw + 10, 16);
    ctx.fill();
    ctx.fillStyle = "rgba(255,255,255,0.75)";
    ctx.textAlign = "center";
    ctx.fillText(label, tx + tw / 2 + 5, H - 14);
    ctx.textAlign = "left";
  }

  // ── Playhead ──────────────────────────────────────────────────────
  {
    const x = secToX(state, curSec, W);
    if (x >= 0 && x <= W) {
      ctx.shadowColor = "rgba(255,255,255,0.6)";
      ctx.shadowBlur = 8;
      ctx.strokeStyle = "#ffffff";
      ctx.lineWidth = 1.5;
      ctx.globalAlpha = 0.95;
      ctx.beginPath();
      ctx.moveTo(x, RULER + 10);
      ctx.lineTo(x, H);
      ctx.stroke();
      ctx.shadowBlur = 0;
      ctx.globalAlpha = 1;
      ctx.fillStyle = "#ffffff";
      ctx.beginPath();
      ctx.moveTo(x - 5, RULER);
      ctx.lineTo(x + 5, RULER);
      ctx.lineTo(x, RULER + 9);
      ctx.closePath();
      ctx.fill();
    }
  }

  // ── Clipping indicators ───────────────────────────────────────────
  if (state.clipTimes.length > 0) {
    ctx.fillStyle = "#ef4444";
    ctx.globalAlpha = 0.8;
    for (const tt of state.clipTimes) {
      const x = secToX(state, tt, W);
      if (x < 0 || x > W) continue;
      ctx.fillRect(x - 0.5, RULER, 1, 5);
    }
    ctx.globalAlpha = 1;
  }

  // ── Cut handle hover highlights ───────────────────────────────────
  if (
    state.hoverSec >= state.vpStart &&
    state.hoverSec <= state.vpEnd &&
    !state.isDragging &&
    !state.handleDrag
  ) {
    const threshold = ((state.vpEnd - state.vpStart) / W) * 10;
    for (const c of state.cuts) {
      for (const side of ["start", "end"] as const) {
        const tt = c[side];
        if (Math.abs(state.hoverSec - tt) < threshold) {
          const x = secToX(state, tt, W);
          ctx.strokeStyle = "#fbbf24";
          ctx.lineWidth = 2;
          ctx.beginPath();
          ctx.moveTo(x, RULER);
          ctx.lineTo(x, H);
          ctx.stroke();
        }
      }
    }
  }
}

export function drawRuler(
  state: EditorState,
  ctx: CanvasRenderingContext2D,
  W: number,
  RULER: number,
): void {
  ctx.fillStyle = "#10101a";
  ctx.fillRect(0, 0, W, RULER);
  ctx.strokeStyle = "rgba(255,255,255,0.07)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, RULER);
  ctx.lineTo(W, RULER);
  ctx.stroke();

  const geom = getLayoutGeom(state, W);
  const mainW = Math.max(1, geom.mainPxEnd - geom.mainPxStart);
  const rawInterval = ((state.vpEnd - state.vpStart) * 80) / mainW;
  const intervals = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
  const tickInterval = intervals.find((v) => v >= rawInterval) ?? 600;
  const firstTick = Math.ceil(state.vpStart / tickInterval) * tickInterval;

  ctx.font = "500 9px system-ui, -apple-system, sans-serif";
  ctx.textBaseline = "middle";
  ctx.fillStyle = "rgba(255,255,255,0.32)";

  for (let s = firstTick; s <= state.vpEnd; s += tickInterval) {
    const x = secToX(state, s, W);
    if (x < geom.mainPxStart - 1 || x > geom.mainPxEnd + 1) continue;
    ctx.strokeStyle = "rgba(255,255,255,0.12)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x, RULER - 5);
    ctx.lineTo(x, RULER);
    ctx.stroke();
    ctx.fillStyle = "rgba(255,255,255,0.32)";
    const globalSec = s + (geom.effIntroDur > 0 ? geom.effIntroDur : 0);
    ctx.fillText(formatTime(globalSec), x + 3, RULER / 2);
  }
}

export function drawMinimap(
  state: EditorState,
  ctx: CanvasRenderingContext2D,
  W: number,
  H: number,
  colors: WaveColors,
): void {
  if (!state.peaks) return;
  ctx.fillStyle = "#0d0d16";
  ctx.fillRect(0, 0, W, H);

  const ACCENT = colors.accent || "#F0BB47";
  const midY = H / 2;
  const gFac = gainFactor(state);
  const maxBar = (H - 6) / 2;
  for (let px = 0; px < W; px++) {
    const sec = (px / W) * state.duration;
    const pi = Math.floor(sec * 100);
    if (pi < 0 || pi >= state.peaks.length) continue;
    const barH = Math.min(maxBar, state.peaks[pi] * gFac * maxBar);
    const inCut = isInCut(state, sec);
    ctx.fillStyle = inCut ? "#ef4444" : ACCENT;
    ctx.globalAlpha = 0.55;
    ctx.fillRect(px, midY - barH, 1, barH * 2);
  }
  ctx.globalAlpha = 1;

  const vg = ctx.createLinearGradient(0, 0, 0, H);
  vg.addColorStop(0, "rgba(13,13,22,0.5)");
  vg.addColorStop(0.3, "rgba(13,13,22,0)");
  vg.addColorStop(0.7, "rgba(13,13,22,0)");
  vg.addColorStop(1, "rgba(13,13,22,0.5)");
  ctx.fillStyle = vg;
  ctx.fillRect(0, 0, W, H);
}
