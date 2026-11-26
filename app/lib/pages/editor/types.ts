// The editor's shared types. (The mutable `E` state that used to live here, with a
// `<audio>` element and a proxy, went with B5c: playback is the engine in Rust now, and
// the editor's state is `app/editor/model.ts`.)

export interface Cut {
  start: number;
  end: number;
}
// ── Immutable format sets ─────────────────────────────────────────────────────
// Formats always routed to the video editor path (HTML video element + ffmpeg peaks)
export const VIDEO_EXTS = new Set([
  ".mp4",
  ".mov",
  ".m4v",
  ".avi",
  ".wmv",
  ".ts",
  ".mts",
  ".m2ts",
  ".flv",
  ".3gp",
  ".asf",
  ".f4v",
]);
