/**
 * Musa og styreflaten på bølgeformen.
 *
 * Portet fra `legacy/renderer/pages/editor/canvas-input.ts`. Algoritmene er
 * legacys — måle-cachen, snappingen mot analysens grenser, hjul-zoomens
 * forankring — og de tre tingene som er byttet ut er oppkoblingen:
 *
 *   1. **Peker-hendelser, ikke muse-hendelser.** Et drag som forlater lerretet
 *      må fortsatt følges, og legacy løser det med `mouseleave` som AVSLUTTER
 *      draget. Her fanges pekeren i stedet, så et drag som går tre piksler
 *      utenfor kanten fortsetter i stedet for å bli avbrutt midt i.
 *   2. **Lytterne monteres og demonteres av `WaveformHost`.** Legacy binder
 *      dem én gang for appens levetid og har en egen kommentar om hvordan man
 *      unngår å lekke dem ved gjentatte oppsett. Her rydder unmount.
 *   3. **Kuttene går gjennom `cuts.ts`**, som speiler signalet. Legacy tegner
 *      selv i hver håndterer.
 *
 * ## Måle-cachen er ikke mikro-optimering
 *
 * `getBoundingClientRect()` tvinger en stil- og layout-gjennomgang, og
 * `pointermove` fyrer så fort pekeren rapporterer (125 Hz på mye maskinvare),
 * med hjulhendelser fra en styreflate enda tettere. Rektangelet endrer seg
 * bare når lerretet flytter eller endrer størrelse, så det måles av
 * `ResizeObserver`-en `WaveformHost` uansett har, og av scroll/resize.
 */

import { limitPulse, resetHaptics, snapPulse } from "@lib/pages/editor/haptics";

import { addCut, commitCutEdges, setSermonWindow } from "./cuts";
import { dragHandle, sermonWindow } from "./editor-core";
import { clampToFile, grabThreshold, secToX, xToSec } from "./geometry";
import { dragWindow, E, syncCuts, type Range } from "./model";
import { seekTo, stopPlay } from "./playback";
import { centerOn, panBy, zoomAt } from "./viewport";
import { scheduleDraw } from "./waveform";

/** Lerretets rektangel, målt høyst én gang per layout-endring. */
let cachedRect: DOMRect | null = null;

export function invalidateCanvasRect(): void {
  cachedRect = null;
}

function rect(el: HTMLElement): DOMRect {
  if (!cachedRect) cachedRect = el.getBoundingClientRect();
  return cachedRect;
}

/** Hvor høyt oppe i lerretet linjalstripa er «spillehodets håndtak». */
const RULER_GRAB_PX = 28;

// ── Bølgeformen ─────────────────────────────────────────────────────────────

export function onCanvasPointerDown(event: PointerEvent): void {
  const canvas = E.canvas;
  if (!canvas || !E.peaks || event.button !== 0) return;
  // Én fersk måling per interaksjon: et drag som starter fra et foreldet
  // rektangel søker til feil sekund, og det er ikke en kostnad verdt å spare.
  invalidateCanvasRect();
  resetHaptics();
  const box = rect(canvas);
  const x = event.clientX - box.left;
  const y = event.clientY - box.top;
  const sec = xToSec(x, box.width);
  const threshold = grabThreshold(box.width);

  // En kuttgrense under pekeren → dra den.
  for (let i = 0; i < E.cuts.length; i++) {
    if (Math.abs(sec - E.cuts[i].start) < threshold) {
      startCutEdgeDrag(canvas, event, i, "start");
      return;
    }
    if (Math.abs(sec - E.cuts[i].end) < threshold) {
      startCutEdgeDrag(canvas, event, i, "end");
      return;
    }
  }

  // Spillehodets trekant i linjalen → dra den.
  if (
    y < RULER_GRAB_PX &&
    Math.abs(x - secToX(E.playStartSec, box.width)) < 12
  ) {
    stopPlay();
    E.playheadDragging = true;
    capture(canvas, event, onPlayheadMove, onPlayheadUp);
    return;
  }

  // Ellers: marker en region som skal bort.
  E.dragStartSec = clampToFile(sec);
  E.dragEndSec = E.dragStartSec;
  E.isDragging = true;
  capture(canvas, event, onSelectMove, onSelectUp);
  scheduleDraw();
}

export function onCanvasPointerMove(event: PointerEvent): void {
  const canvas = E.canvas;
  if (!canvas || !E.peaks || E.isDragging || E.playheadDragging) return;
  const box = rect(canvas);
  E.hoverSec = xToSec(event.clientX - box.left, box.width);
  scheduleDraw();
}

export function onCanvasPointerLeave(): void {
  if (E.hoverSec === -99999) return;
  E.hoverSec = -99999;
  scheduleDraw();
}

/**
 * Hjulet: zoom med modifikator, panorer uten.
 *
 * Matematikken er synkron per hendelse — det er dét som holder zoomen forankret
 * i sekundet under pekeren. Bare tegningen er utsatt (`scheduleDraw`).
 */
export function onCanvasWheel(event: WheelEvent): void {
  const canvas = E.canvas;
  if (!canvas || !E.peaks) return;
  event.preventDefault();
  const box = rect(canvas);
  if (event.ctrlKey || event.metaKey) {
    const at = xToSec(event.clientX - box.left, box.width);
    zoomAt(at, event.deltaY > 0 ? 1.25 : 0.75);
    return;
  }
  panBy((event.deltaY * (E.vpEnd - E.vpStart)) / 800);
}

// ── Minimapet ───────────────────────────────────────────────────────────────

export function onMinimapPointerDown(event: PointerEvent): void {
  const map = E.minimap;
  if (!map || E.duration <= 0 || event.button !== 0) return;
  E.minimapDragging = true;
  const jump = (e: PointerEvent): void => {
    const box = map.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - box.left) / box.width));
    centerOn(frac * E.duration);
  };
  jump(event);
  capture(map, event, jump, () => {
    E.minimapDragging = false;
  });
}

// ── Håndtakene på prekenvinduet ─────────────────────────────────────────────

/**
 * Et gullhåndtak ble tatt tak i.
 *
 * Kalles fra `WaveformHost` — håndtakene er ekte DOM-knapper og ikke tegnede
 * piksler, slik at en tastaturbruker også kan flytte dem.
 */
export function beginWindowHandleDrag(
  side: "start" | "end",
  event: PointerEvent,
  applied: boolean,
): void {
  const canvas = E.canvas;
  if (!canvas || E.duration <= 0) return;
  const target = event.currentTarget as HTMLElement | null;
  if (!target) return;
  invalidateCanvasRect();
  resetHaptics();
  const box = rect(canvas);
  let window_ = sermonWindow({
    cuts: E.cuts,
    duration: E.duration,
    suggestion: E.suggestion,
    applied,
  });
  if (!window_) return;

  const move = (e: PointerEvent): void => {
    const raw = xToSec(e.clientX - box.left, box.width);
    const snapped = e.shiftKey ? raw : snapToSegmentBoundary(raw, box.width);
    snapPulse(raw, snapped);
    const next = dragHandle(window_ as Range, side, snapped, E.duration);
    if (next[side] !== snapped) limitPulse();
    window_ = next;
    // Under draget skrives bare DRAG-speilet, ikke kuttlista: et kuttflett per
    // pekerhendelse ville lagt et øyeblikksbilde i angrestabelen 125 ganger
    // i sekundet. Speilet er det som gjør at gullvinduet følger fingeren.
    dragWindow.value = next;
    scheduleDraw();
  };

  const up = (): void => {
    dragWindow.value = null;
    if (window_) setSermonWindow(window_, applied);
  };

  capture(target, event, move, up);
}

// ── Dragene ─────────────────────────────────────────────────────────────────

function startCutEdgeDrag(
  canvas: HTMLCanvasElement,
  event: PointerEvent,
  index: number,
  side: "start" | "end",
): void {
  E.handleDrag = side;
  const box = rect(canvas);
  const move = (e: PointerEvent): void => {
    const cut = E.cuts[index];
    if (!cut) return;
    const raw = xToSec(e.clientX - box.left, box.width);
    const snapped = e.shiftKey ? raw : snapToSegmentBoundary(raw, box.width);
    snapPulse(raw, snapped);
    const applied =
      side === "start"
        ? Math.max(0, Math.min(cut.end - 0.1, snapped))
        : Math.min(E.duration, Math.max(cut.start + 0.1, snapped));
    // Pinnet mot den andre kanten eller mot minstebredden: et hakk, ikke et
    // snapp. Et annet mønster, fordi det betyr «du kommer ikke lenger».
    if (applied !== snapped) limitPulse();
    cut[side] = applied;
    syncCuts();
    scheduleDraw();
  };
  const up = (): void => {
    E.handleDrag = null;
    // Sorteringen og flettet skjer her, ikke per bevegelse: et drag som drar
    // én grense forbi en annen skal ende opp med ETT kutt, men bare når
    // brukeren slipper.
    commitCutEdges();
  };
  capture(canvas, event, move, up);
}

function onSelectMove(event: PointerEvent): void {
  const canvas = E.canvas;
  if (!canvas) return;
  const box = rect(canvas);
  E.dragEndSec = clampToFile(xToSec(event.clientX - box.left, box.width));
  scheduleDraw();
}

function onSelectUp(event: PointerEvent): void {
  const canvas = E.canvas;
  if (!canvas) return;
  const box = rect(canvas);
  const at = clampToFile(xToSec(event.clientX - box.left, box.width));
  const from = E.dragStartSec;
  E.isDragging = false;
  E.dragStartSec = -1;
  E.dragEndSec = -1;

  if (Math.abs(at - from) > 0.1) {
    const s = event.shiftKey ? from : snapToSegmentBoundary(from, box.width);
    const e = event.shiftKey ? at : snapToSegmentBoundary(at, box.width);
    snapPulse(at, e);
    addCut(s, e);
    return;
  }
  // Et klikk, ikke et drag: flytt spillehodet dit.
  seekTo(at);
}

function onPlayheadMove(event: PointerEvent): void {
  const canvas = E.canvas;
  if (!canvas) return;
  const box = rect(canvas);
  E.playStartSec = clampToFile(xToSec(event.clientX - box.left, box.width));
  scheduleDraw();
}

function onPlayheadUp(): void {
  E.playheadDragging = false;
  seekTo(E.playStartSec);
}

/**
 * Fang pekeren for hele dragets levetid.
 *
 * `setPointerCapture` gjør at bevegelser utenfor elementet fortsatt kommer
 * hit, og `pointercancel` er med fordi et systemvindu eller en gest kan ta
 * pekeren fra oss midt i — uten den ville draget blitt hengende igjen som
 * «pågår» for alltid.
 */
function capture(
  el: HTMLElement,
  event: PointerEvent,
  move: (e: PointerEvent) => void,
  up: (e: PointerEvent) => void,
): void {
  const id = event.pointerId;
  try {
    el.setPointerCapture(id);
  } catch {
    /* noen nettlesere nekter for en peker som allerede er sluppet */
  }
  const onMove = (e: PointerEvent): void => {
    if (e.pointerId !== id) return;
    move(e);
  };
  const finish = (e: PointerEvent): void => {
    if (e.pointerId !== id) return;
    el.removeEventListener("pointermove", onMove);
    el.removeEventListener("pointerup", finish);
    el.removeEventListener("pointercancel", finish);
    try {
      el.releasePointerCapture(id);
    } catch {
      /* allerede sluppet */
    }
    up(e);
    scheduleDraw();
  };
  el.addEventListener("pointermove", onMove);
  el.addEventListener("pointerup", finish);
  el.addEventListener("pointercancel", finish);
}

/**
 * Snapp et sekundtall til nærmeste grense analysen fant.
 *
 * Legacys egen, med legacys terskel: den skalerer med zoomen (~8 piksler), så
 * den er stram når man er langt inne og romslig når man er langt ute, og et
 * grovt drag finner grensen likevel.
 */
export function snapToSegmentBoundary(sec: number, W: number): number {
  if (E.segments.length === 0) return sec;
  const threshold = Math.max(
    0.15,
    ((E.vpEnd - E.vpStart) / Math.max(1, W)) * 8,
  );
  let closest = sec;
  let best = threshold;
  for (const seg of E.segments) {
    for (const at of [seg.start, seg.end]) {
      const d = Math.abs(sec - at);
      if (d < best) {
        best = d;
        closest = at;
      }
    }
  }
  return closest;
}
