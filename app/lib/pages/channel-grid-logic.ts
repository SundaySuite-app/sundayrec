/**
 * Pure decisions for the live channel grid — no DOM, no window.api, fully
 * unit-tested in channel-grid-logic.test.ts.
 */

import type { ChannelMode } from "../../../legacy/types";

/** Which slot a tap fills next. `armed` is the slot the NEXT tap assigns. */
export interface Assignment {
  l: number;
  r: number;
  armed: "l" | "r";
}

/**
 * Tap reducer: assign the armed slot to the tapped channel, then auto-arm the
 * other slot — so the canonical flow is two taps on the two lit columns.
 * Mono modes pin the armed slot (one chip, one tap): monoL writes `l`,
 * monoR writes `r`, monoMix behaves like stereo (both channels feed the mix).
 * Assigning L and R to the same channel is allowed (dual-mono).
 */
export function nextAssignment(
  prev: Assignment,
  tappedCh: number,
  mode: ChannelMode,
): Assignment {
  if (mode === "monoL") return { l: tappedCh, r: prev.r, armed: "l" };
  if (mode === "monoR") return { l: prev.l, r: tappedCh, armed: "r" };
  return prev.armed === "l"
    ? { l: tappedCh, r: prev.r, armed: "r" }
    : { l: prev.l, r: tappedCh, armed: "l" };
}

/** The armed slot a mode change forces (mono modes have exactly one slot). */
export function armedForMode(mode: ChannelMode, prev: "l" | "r"): "l" | "r" {
  if (mode === "monoL") return "l";
  if (mode === "monoR") return "r";
  return prev;
}

/** Signal-presence hysteresis: on above −50 dBFS, off below −55 — a channel
 *  hovering at the threshold must not flicker. */
export function nextSignalState(prev: boolean, db: number): boolean {
  if (db > -50) return true;
  if (db < -55) return false;
  return prev;
}

/** Map a `vu-levels` dB entry to a 0..1 meter fraction. The payload serialises
 *  −∞ as `null`; anything below the −60 dB floor reads as empty. */
export function dbToFraction(db: number | null | undefined): number {
  if (db == null || !isFinite(db)) return 0;
  const floor = -60;
  return Math.min(1, Math.max(0, (db - floor) / -floor));
}
