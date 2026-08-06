import { E } from './state'
import { scheduleViewportDraw } from './waveform'

// ── Viewport (zoom / pan / fit) ─────────────────────────────────────────────
//
// The viewport MATHS is synchronous — vpStart/vpEnd are the truth the moment an
// input arrives, so anchoring and clamping compose exactly however many events
// land in one frame. Only the two repaints are deferred, coalesced into a
// single rAF (scheduleViewportDraw). A trackpad fires wheel events faster than
// the screen refreshes; drawing per event just meant painting frames nobody
// ever saw, at the price of the ones they did.

export function fitAll(): void {
  E.vpStart = 0
  E.vpEnd   = E.duration || 1
}

export function zoomBy(factor: number): void {
  const center = (E.vpStart + E.vpEnd) / 2
  const half   = ((E.vpEnd - E.vpStart) * factor) / 2
  E.vpStart = Math.max(0, center - half)
  E.vpEnd   = Math.min(E.duration, center + half)
  const minSpan = 0.5
  if (E.vpEnd - E.vpStart < minSpan) {
    const mid = (E.vpStart + E.vpEnd) / 2
    E.vpStart = Math.max(0, mid - minSpan / 2)
    E.vpEnd   = Math.min(E.duration, E.vpStart + minSpan)
  }
  scheduleViewportDraw()
}

export function panBy(deltaSecs: number): void {
  const span = E.vpEnd - E.vpStart
  E.vpStart = Math.max(0, Math.min(E.duration - span, E.vpStart + deltaSecs))
  E.vpEnd   = E.vpStart + span
  scheduleViewportDraw()
}
