/**
 * Draw pacing — how to run at ~30 fps inside a 60 Hz rAF loop without judder.
 *
 * The obvious version is a boundary check: `if (now - last >= 33) { last = now;
 * draw() }`. It looks right and is not: `last` is set to the timestamp of the
 * frame that happened to cross the line, so the phase drifts with the jitter of
 * every frame. rAF ticks at 16.7 ms, so the gate alternates between admitting a
 * draw after 33.4 ms and after 50.1 ms — a 33/50 sawtooth that reads as a
 * visible wobble in anything that moves at a constant speed (a scrolling
 * waveform, a playhead).
 *
 * The accumulator advances the gate by exactly ONE interval per admitted draw,
 * so the cadence stays even no matter when in the frame the check runs. Written
 * for the recording overlay's waveform in v0.5.0; the editor's playback loop
 * had the naive version until Phase 5.
 */

/** Draw-gate interval — every other 60 Hz frame. */
export const DRAW_INTERVAL_MS = 33.4

/**
 * Returns the NEW gate timestamp when a draw is due (advanced by exactly one
 * interval so the cadence stays even), or the old one when not — so a caller
 * draws precisely when `nextDrawGate(gate, now) !== gate`.
 *
 * A stall longer than two intervals resyncs to `now` instead of replaying every
 * missed draw: after a long task, catching up costs more than it buys.
 */
export function nextDrawGate(gate: number, now: number): number {
  if (now - gate < DRAW_INTERVAL_MS) return gate
  if (now - gate > DRAW_INTERVAL_MS * 2) return now
  return gate + DRAW_INTERVAL_MS
}
