/**
 * Pure rules for the VU meter's draw loop — kept out of `VuMeter.tsx` so the
 * node test gate can pin them (there is no DOM there).
 */

import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";

/**
 * How long a feed may be silent (no packet at all) before the meter stops
 * believing its last level. Packets arrive ~30 per second, so a full second
 * without one means the stream has stopped, not that the room went quiet.
 */
export const STALE_MS = 1000;

/** A dB step below this is invisible at bar width — not worth a repaint. */
export const REPAINT_EPSILON_DB = 0.05;

/**
 * The level the bars should chase. While packets flow it is the last packet's
 * level; once the stream has been silent for `STALE_MS` it is the floor, so the
 * needle falls instead of freezing on whatever the last packet said (a stalled
 * or failed feed otherwise looks like a loud, live signal).
 */
export function effectiveLevel(
  levelDb: number,
  lastPacketAtMs: number | null,
  nowMs: number,
): number {
  if (lastPacketAtMs === null) return levelDb;
  return nowMs - lastPacketAtMs > STALE_MS ? VU_FLOOR_DB : levelDb;
}

/** Did anything the canvas shows change since the last paint? */
export function needsRepaint(
  prev: { l: number; r: number; mono: boolean } | null,
  next: { l: number; r: number; mono: boolean },
  dirty: boolean,
): boolean {
  if (dirty || prev === null) return true;
  return (
    prev.mono !== next.mono ||
    Math.abs(prev.l - next.l) >= REPAINT_EPSILON_DB ||
    Math.abs(prev.r - next.r) >= REPAINT_EPSILON_DB
  );
}
