import type { EditorSegment } from "../../../../legacy/types";

// The editor's shared types. (The mutable `E` state that used to live here, with a
// `<audio>` element and a proxy, went with B5c: playback is the engine in Rust now, and
// the editor's state is `app/editor/model.ts`.)

export interface Cut {
  start: number;
  end: number;
}
/** One analysed segment, exactly as `editor_segments` serialises it.
 *
 *  This is an ALIAS of the generated `EditorSegment` binding, not a re-typing of
 *  it, and that is load-bearing: it used to be a hand-written twin declaring
 *  `type: string`, while the Rust struct was serialising the field as `kind`.
 *  Nothing complained, `s.type` was `undefined` for every segment, and the whole
 *  sermon-detection UI (picker, «Marker preken automatisk», suggestion banner,
 *  timeline layers) was silently dead in every shipped build. Aliasing the
 *  binding turns the next such rename into a typecheck failure at each `.type`
 *  read instead of a feature that quietly stops existing. */
export type Suggestion = EditorSegment;
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
