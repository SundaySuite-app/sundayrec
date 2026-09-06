/**
 * Utsnittet — zoom, panorering og «vis alt».
 *
 * Portet fra `legacy/renderer/pages/editor/viewport.ts` med kommentaren som
 * betyr noe intakt: MATEMATIKKEN er synkron, bare TEGNINGEN er utsatt. En
 * styreflate sender hjulhendelser raskere enn skjermen oppdaterer seg, og en
 * zoom som er forankret under pekeren må regnes ut mot utsnittet slik det er i
 * det øyeblikket hendelsen kom — ellers driver forankringen.
 */

import { E, syncViewport } from "./model";
import { scheduleDraw } from "./waveform";

/** Korteste utsnitt vi lar noen zoome til. Legacys eget tall. */
const MIN_SPAN_SEC = 0.5;

/** Hele opptaket. */
export function fitAll(): void {
  E.vpStart = 0;
  E.vpEnd = E.duration || 1;
  syncViewport();
  scheduleDraw();
}

/** Zoom om midten. `factor < 1` zoomer inn. */
export function zoomBy(factor: number): void {
  const center = (E.vpStart + E.vpEnd) / 2;
  const half = ((E.vpEnd - E.vpStart) * factor) / 2;
  E.vpStart = Math.max(0, center - half);
  E.vpEnd = Math.min(E.duration, center + half);
  enforceMinSpan();
  syncViewport();
  scheduleDraw();
}

/** Zoom forankret i sekundet under pekeren. */
export function zoomAt(atSec: number, factor: number): void {
  const span = E.vpEnd - E.vpStart;
  if (span <= 0) return;
  const frac = (atSec - E.vpStart) / span;
  const next = span * factor;
  E.vpStart = Math.max(0, atSec - frac * next);
  E.vpEnd = Math.min(E.duration, E.vpStart + next);
  enforceMinSpan();
  syncViewport();
  scheduleDraw();
}

export function panBy(deltaSec: number): void {
  const span = E.vpEnd - E.vpStart;
  E.vpStart = Math.max(0, Math.min(E.duration - span, E.vpStart + deltaSec));
  E.vpEnd = E.vpStart + span;
  syncViewport();
  scheduleDraw();
}

/** Sentrer utsnittet om et sekund — minimapets drag. */
export function centerOn(sec: number): void {
  const half = (E.vpEnd - E.vpStart) / 2;
  E.vpStart = Math.max(0, Math.min(E.duration - half * 2, sec - half));
  E.vpEnd = E.vpStart + half * 2;
  syncViewport();
  scheduleDraw();
}

/** Rull etter spillehodet, men bare når det er i ferd med å forsvinne ut. */
export function followPlayhead(sec: number): void {
  const span = E.vpEnd - E.vpStart;
  if (sec >= E.vpStart && sec <= E.vpEnd - span * 0.1) return;
  E.vpStart = Math.max(0, Math.min(E.duration - span, sec - span * 0.05));
  E.vpEnd = E.vpStart + span;
  syncViewport();
}

function enforceMinSpan(): void {
  if (E.vpEnd - E.vpStart >= MIN_SPAN_SEC) return;
  const mid = (E.vpStart + E.vpEnd) / 2;
  E.vpStart = Math.max(0, mid - MIN_SPAN_SEC / 2);
  E.vpEnd = Math.min(E.duration || MIN_SPAN_SEC, E.vpStart + MIN_SPAN_SEC);
}
