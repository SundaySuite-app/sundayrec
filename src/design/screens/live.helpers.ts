/**
 * Pure helpers for the redesigned Direkte (live streaming) screen.
 *
 * These mirror the IPC contract that `src/features/streaming/StreamingPanel.tsx`
 * speaks to — the same `stream_status`/`stream_start`/`stream_stop` commands and
 * the same `StreamStatus` shape — so the redesigned screen can be data-driven
 * without re-implementing or diverging from the canonical wiring. Everything
 * here is side-effect-free so it stays trivially testable.
 */
import type { StreamStatus } from "@/lib/bindings/StreamStatus";
import type { StreamResolution } from "@/lib/bindings/StreamResolution";
import type { StreamDestinationView } from "@/lib/bindings/StreamDestinationView";
import type { OverlayConfig } from "@/lib/bindings/OverlayConfig";

/** The resolutions the backend renders, in display order, with the labels the
 *  redesign card shows (matching the original sample copy). */
export const RESOLUTIONS: readonly {
  value: StreamResolution;
  label: string;
  sub: string;
  badge?: string;
}[] = [
  { value: "p480", label: "480p", sub: "1.5 Mbps" },
  { value: "p720", label: "720p", sub: "4.5 Mbps", badge: "Anbefalt" },
  { value: "p1080", label: "1080p", sub: "6 Mbps" },
] as const;

/** Selectable frame rates (matches StreamingPanel's `FRAMERATES`). */
export const FRAMERATES = [25, 30] as const;

/** Format an mm:ss (or hh:mm:ss) uptime from an epoch-ms `startedAt` and the
 *  current epoch-ms. Identical semantics to StreamingPanel's `formatUptime`. */
export function formatUptime(startedAt: bigint | null, nowMs: number): string {
  if (startedAt === null) return "00:00";
  const secs = Math.max(0, Math.floor((nowMs - Number(startedAt)) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** True when the live-streaming push is currently active. */
export function isActive(st: StreamStatus | undefined | null): boolean {
  return st?.active ?? false;
}

/** The fallback destination row the design ships with, shown when the backend
 *  reports no configured destinations (keeps the original mockup visible). */
export const SAMPLE_DEST: StreamDestinationView = {
  id: "sample-youtube",
  name: "YouTube · SundayRec",
  rtmpUrl: "rtmp://a.rtmp.youtube.com/live2",
  enabled: true,
  hasKey: false,
};

/** A live VU channel reading, in dBFS (or `null`/undefined for silence). */
export function channelDbfs(
  peaks: ReadonlyArray<number> | undefined,
  index: number,
): number | null {
  const v = peaks?.[index];
  return v == null || !Number.isFinite(v) ? null : v;
}

/** A destination row as the redesigned screen edits it. Same shape as the
 *  canonical `StreamingPanel` `DestRow`: the on-disk `StreamDestinationView`
 *  plus a transient `keyInput` that's typed and pushed to the keychain via
 *  `stream_set_key` (never persisted in the row itself). */
export type DestRow = StreamDestinationView & { keyInput: string };

let nextDestId = 1;

/** Build a fresh, empty destination row with a unique renderer id. Mirrors the
 *  panel's `dest-N` id scheme so the two never collide visually. */
export function makeDestRow(name: string, rtmpUrl: string): DestRow {
  return {
    id: `dest-${nextDestId++}`,
    name,
    rtmpUrl,
    enabled: true,
    hasKey: false,
    keyInput: "",
  };
}

/** The persistable view of a destination row, dropping the transient key input
 *  exactly as `StreamingPanel` does before passing `destinations` to
 *  `stream_start`. */
export function toDestView(row: DestRow): StreamDestinationView {
  const { keyInput: _keyInput, ...view } = row;
  return view;
}

/** A simple overlay entry as the redesigned screen models it locally before it
 *  is folded into the `stream_start` payload. */
export type OverlayRow = {
  id: string;
  /** Display label shown in the overlays list. */
  label: string;
  /** Lower-third title text rendered over the stream. */
  title: string;
};

let nextOverlayId = 1;

/** Create a minimal text/lower-third overlay row. */
export function makeOverlayRow(label: string, title: string): OverlayRow {
  return { id: `overlay-${nextOverlayId++}`, label, title };
}

/** Convert the screen's local overlay rows into the `OverlayConfig[]` that
 *  `stream_start` accepts. Mirrors the panel's lower-third defaults (bl,
 *  scale 0.3, full opacity) and only emits overlays that carry text. */
export function toOverlayConfigs(
  rows: ReadonlyArray<OverlayRow>,
): OverlayConfig[] {
  return rows
    .filter((r) => r.title.trim().length > 0)
    .map((r) => ({
      id: r.id,
      name: r.label.trim() || "Overlay",
      enabled: true,
      source: { kind: "text", title: r.title.trim(), subtitle: null },
      position: "bl",
      customX: null,
      customY: null,
      scale: 0.3,
      opacity: 1,
    }));
}
