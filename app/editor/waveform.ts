/**
 * Bølgeformen og minimapet — tegningen, og bare den.
 *
 * ## Algoritmen er legacys, oppkoblingen er ny
 *
 * Stolpene, linjalens tikk-tetthet, «det som er spilt av er dempet» og
 * spillehodets trekant kommer rett fra `legacy/renderer/pages/editor/
 * waveform.ts`. Det som er byttet ut er hvor fargene og størrelsen kommer fra:
 * `cssVar(--accent)` på `documentElement` er blitt `getComputedStyle(canvas)`
 * på lerretet selv, slik at `app/styles/tokens.css` er den eneste ordboka —
 * gaten `check-app-css-tokens.mjs` finnes for å holde det slik.
 *
 * Segmentlagene (tale · musikk · stillhet) er IKKE med. De hører til
 * lag-popoveren i legacys verktøylinje, og canvasens 4.1 tegner dem ikke: der
 * er bølgeformen prekenvinduet, kuttene og spillehodet. Analysen kjører
 * likevel — den er det forslaget bygger på.
 *
 * ## Tegne-scheduleren
 *
 * Ett rAF, uansett hvor mange som ber om en tegning i samme frame. Legacys
 * `scheduleDrawWaveform` med samme begrunnelse: en styreflate sender
 * `mousemove` raskere enn skjermen oppdaterer seg, og en tegning per hendelse
 * er frames som males og kastes.
 *
 * ⚠️ Ingenting her leser et signal. Tegningen leser `E`, og `WaveformHost`
 * abonnerer på speilene og ber om en tegning. Et signal lest inne i en
 * rAF-løkke ville abonnert løkka på seg selv — og `@preact/signals` sporer
 * lesninger uansett hvor dypt i kallstakken de skjer.
 */

import { E } from "./model";
import { secToX } from "./geometry";

/** Høyden på linjalstripa øverst i lerretet. */
const RULER = 22;
/** Topper per sekund i `E.peaks` — bakendens egen oppløsning. */
const PEAKS_PER_SEC = 100;
/** Tikk-avstandene linjalen får velge mellom, i sekunder. Legacys egen liste. */
const TICK_INTERVALS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600];

/** Fargene, lest fra lerretets egne beregnede stiler — altså fra tokens.css. */
function palette(canvas: HTMLCanvasElement): (name: string) => string {
  const style = getComputedStyle(canvas);
  return (name) => style.getPropertyValue(name).trim();
}

/**
 * La lerretets bakgrunnslager følge boksen sin.
 *
 * Å skrive `canvas.width` NULLSTILLER lerretet, så det skjer bare når tallet
 * faktisk er et annet. Samme regel som `VuMeter`.
 */
export function syncCanvasSize(canvas: HTMLCanvasElement): void {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.round(canvas.clientWidth * dpr);
  const h = Math.round(canvas.clientHeight * dpr);
  if (w <= 0 || h <= 0) return;
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
}

// ── Scheduleren ─────────────────────────────────────────────────────────────

let raf = 0;

/** Be om én tegning på neste frame. Idempotent innen samme frame. */
export function scheduleDraw(): void {
  if (raf) return;
  raf = requestAnimationFrame(() => {
    raf = 0;
    draw();
  });
}

/** Avbryt en ventende tegning. `WaveformHost` kaller den ved unmount — en
 *  levende rAF etter at editoren er lukket koster et frame-budsjett opptaket
 *  trenger. */
export function cancelDraw(): void {
  if (!raf) return;
  cancelAnimationFrame(raf);
  raf = 0;
}

/** Tegn begge lerretene nå. */
export function draw(): void {
  drawWaveform();
  drawMinimap();
}

// ── Bølgeformen ─────────────────────────────────────────────────────────────

export function drawWaveform(): void {
  const canvas = E.canvas;
  if (!canvas || !E.peaks) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const W = canvas.width / dpr;
  const H = canvas.height / dpr;
  if (W <= 0 || H <= 0) return;

  const col = palette(canvas);
  ctx.save();
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, W, H);

  ctx.fillStyle = col("--surface");
  ctx.fillRect(0, 0, W, H);

  drawRuler(ctx, col, W);

  const midY = RULER + (H - RULER) / 2;
  const maxBar = Math.max(2, (H - RULER - 14) / 2);
  const cur = E.playStartSec;

  // Midtlinja — så en stille passasje fortsatt ser ut som en tidslinje og
  // ikke som et tomt felt.
  ctx.strokeStyle = col("--line");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, midY);
  ctx.lineTo(W, midY);
  ctx.stroke();

  // Kuttene som flater, under stolpene.
  ctx.fillStyle = col("--bg");
  ctx.globalAlpha = 0.55;
  for (const cut of E.cuts) {
    const x1 = secToX(cut.start, W);
    const x2 = secToX(cut.end, W);
    if (x2 < 0 || x1 > W) continue;
    ctx.fillRect(x1, RULER, x2 - x1, H - RULER);
  }
  ctx.globalAlpha = 1;

  // Stolpene.
  const gold = col("--gold");
  const dim = col("--ink-3");
  for (let px = 0; px < Math.floor(W); px++) {
    const sec = E.vpStart + (px / W) * (E.vpEnd - E.vpStart);
    const index = Math.floor(sec * PEAKS_PER_SEC);
    if (index < 0 || index >= E.peaks.length) continue;
    const barH = Math.min(maxBar, E.peaks[index] * maxBar);
    const inCut = isInCut(sec) || isInDrag(sec);
    ctx.fillStyle = inCut ? dim : gold;
    ctx.globalAlpha = inCut ? 0.45 : sec < cur ? 0.34 : 0.85;
    ctx.fillRect(px, midY - barH, 1, barH * 2);
  }
  ctx.globalAlpha = 1;

  // Den aktive markeringen (dra for å kutte).
  if (E.isDragging && E.dragStartSec >= 0) {
    const x1 = secToX(Math.min(E.dragStartSec, E.dragEndSec), W);
    const x2 = secToX(Math.max(E.dragStartSec, E.dragEndSec), W);
    ctx.fillStyle = col("--bad-soft");
    ctx.fillRect(x1, RULER, x2 - x1, H - RULER);
    ctx.strokeStyle = col("--bad");
    ctx.lineWidth = 1.5;
    ctx.strokeRect(x1 + 0.5, RULER + 0.5, x2 - x1 - 1, H - RULER - 1);
  }

  // Kuttgrensene, som streker.
  ctx.strokeStyle = col("--bad-line");
  ctx.lineWidth = 1.5;
  for (const cut of E.cuts) {
    for (const at of [cut.start, cut.end]) {
      const x = secToX(at, W);
      if (x < -2 || x > W + 2) continue;
      ctx.beginPath();
      ctx.moveTo(x, RULER);
      ctx.lineTo(x, H);
      ctx.stroke();
    }
  }

  // Spøkelseslinja der musa står.
  if (E.hoverSec > -9999 && !E.isDragging) {
    const x = secToX(E.hoverSec, W);
    if (x >= 0 && x <= W) {
      ctx.setLineDash([3, 4]);
      ctx.strokeStyle = col("--line-2");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, RULER);
      ctx.lineTo(x, H);
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }

  drawPlayhead(ctx, col, W, H, cur);
  ctx.restore();
}

function drawPlayhead(
  ctx: CanvasRenderingContext2D,
  col: (name: string) => string,
  W: number,
  H: number,
  cur: number,
): void {
  const x = secToX(cur, W);
  if (x < 0 || x > W) return;
  ctx.strokeStyle = col("--knob");
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(x, RULER + 9);
  ctx.lineTo(x, H);
  ctx.stroke();
  ctx.fillStyle = col("--knob");
  ctx.beginPath();
  ctx.moveTo(x - 5, RULER);
  ctx.lineTo(x + 5, RULER);
  ctx.lineTo(x, RULER + 9);
  ctx.closePath();
  ctx.fill();
}

/**
 * Linjalen.
 *
 * Tikk-avstanden er legacys: den minste av de faste avstandene som gir minst
 * 80 piksler mellom to merker. Under ett sekund ville `timecode` rundet to
 * nabomerker til det samme tallet — «0:05 0:05 0:06» — så ett sekund er
 * gulvet.
 */
function drawRuler(
  ctx: CanvasRenderingContext2D,
  col: (name: string) => string,
  W: number,
): void {
  ctx.fillStyle = col("--raised");
  ctx.fillRect(0, 0, W, RULER);
  ctx.strokeStyle = col("--line");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, RULER);
  ctx.lineTo(W, RULER);
  ctx.stroke();

  const span = E.vpEnd - E.vpStart;
  if (span <= 0 || W <= 0) return;
  const raw = (span * 80) / W;
  const step = TICK_INTERVALS.find((v) => v >= raw) ?? 600;
  const first = Math.ceil(E.vpStart / step) * step;

  ctx.font = "500 10px system-ui, -apple-system, sans-serif";
  ctx.textBaseline = "middle";
  for (let s = first; s <= E.vpEnd; s += step) {
    const x = secToX(s, W);
    ctx.strokeStyle = col("--line-2");
    ctx.beginPath();
    ctx.moveTo(x, RULER - 5);
    ctx.lineTo(x, RULER);
    ctx.stroke();
    ctx.fillStyle = col("--ink-3");
    ctx.fillText(rulerLabel(s), x + 3, RULER / 2);
  }
}

/** «0:15» / «1:02:30» — kort form på linjalen, der plassen er trang. */
function rulerLabel(sec: number): string {
  const total = Math.max(0, Math.floor(sec));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
}

// ── Minimapet ───────────────────────────────────────────────────────────────

/**
 * Hele opptaket i én stripe, uansett hvor langt inn man har zoomet.
 *
 * Det er den ene flaten som svarer på «hvor i gudstjenesten er jeg?» når
 * bølgeformen viser to minutter av to timer.
 */
export function drawMinimap(): void {
  const canvas = E.minimap;
  if (!canvas || !E.peaks || E.duration <= 0) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const W = canvas.width / dpr;
  const H = canvas.height / dpr;
  if (W <= 0 || H <= 0) return;

  const col = palette(canvas);
  ctx.save();
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, W, H);
  ctx.fillStyle = col("--bg");
  ctx.fillRect(0, 0, W, H);

  const midY = H / 2;
  const maxBar = Math.max(1, (H - 6) / 2);
  const gold = col("--gold");
  const dim = col("--ink-3");
  for (let px = 0; px < Math.floor(W); px++) {
    const sec = (px / W) * E.duration;
    const index = Math.floor(sec * PEAKS_PER_SEC);
    if (index < 0 || index >= E.peaks.length) continue;
    const barH = Math.min(maxBar, E.peaks[index] * maxBar);
    const inCut = isInCut(sec);
    ctx.fillStyle = inCut ? dim : gold;
    ctx.globalAlpha = 0.55;
    ctx.fillRect(px, midY - barH, 1, barH * 2);
  }
  ctx.globalAlpha = 1;

  // Utsnittet som en ramme — den eneste måten å se at man ER zoomet inn.
  const x1 = (E.vpStart / E.duration) * W;
  const x2 = (E.vpEnd / E.duration) * W;
  ctx.strokeStyle = col("--gold-line");
  ctx.lineWidth = 1.5;
  ctx.strokeRect(x1 + 0.75, 0.75, Math.max(2, x2 - x1) - 1.5, H - 1.5);
  ctx.restore();
}

// ── Predikatene tegningen trenger ───────────────────────────────────────────

export function isInCut(sec: number): boolean {
  return E.cuts.some((c) => sec >= c.start && sec <= c.end);
}

function isInDrag(sec: number): boolean {
  if (!E.isDragging) return false;
  const s = Math.min(E.dragStartSec, E.dragEndSec);
  const e = Math.max(E.dragStartSec, E.dragEndSec);
  return sec >= s && sec <= e;
}
