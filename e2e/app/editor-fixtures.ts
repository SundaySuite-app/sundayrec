import { BOOT_FIXTURES, fn, VOID, type Fixtures } from "../harness";
import type { EditorSegment } from "../../legacy/bindings/EditorSegment";

// The editor's fixtured recording — ONE recipe, read by both shells.
//
// It lived inside `e2e/editor.spec.ts` until P4a needed the same recording in
// `e2e/app/editor.spec.ts`. A second copy would have been a second thing to
// drift: the whole point of these fixtures is that they stand in for what the
// BACKEND puts on the wire, so the two shells must be fed the same wire.

export const FILE = "/Users/test/Opptak/2026-08-02 Gudstjeneste.mp3";
export const DURATION = 600;

/**
 * A recording whose timeline has THREE plausible sermon candidates, all ≥ 60 s
 * and already in time order — which is what makes the picker offer a real
 * choice.
 *
 * Typed as the GENERATED `EditorSegment` binding on purpose: these fixtures
 * stand in for what `editor_segments` puts on the wire, so a Rust-side field
 * rename must fail `npm run typecheck` here rather than quietly making the whole
 * sermon-detection UI inert (which is exactly what a `kind` / `type` split did
 * to every shipped build until this was fixed).
 */
export const SEGMENTS: EditorSegment[] = [
  { start: 0, end: 30, duration: 30, label: "Stillhet", type: "silence" },
  { start: 30, end: 180, duration: 150, label: "Tale", type: "speech" },
  { start: 180, end: 210, duration: 30, label: "Musikk", type: "music" },
  { start: 210, end: 420, duration: 210, label: "Preken", type: "sermon" },
  { start: 420, end: 600, duration: 180, label: "Tale", type: "speech" },
];

export function editorFixtures(over: Fixtures = {}): Fixtures {
  return {
    ...BOOT_FIXTURES,
    editor_probe_streams: { hasVideo: false, hasAudio: true },
    editor_load_recording: {
      durationSec: DURATION,
      hasVideo: false,
      hasAudio: true,
      channels: 2,
      sampleFmt: "s16",
      sampleRate: 48_000,
    },
    editor_allow_asset_path: VOID,
    // ~100 buckets/sec. Generated in the page rather than shipped as a 60 000
    // element literal across the init-script boundary.
    editor_peaks: fn(`() => {
      (window.__E2E_CALLS__ ||= {}).editor_peaks = ((window.__E2E_CALLS__.editor_peaks || 0) + 1);
      return { peaks: Array.from({ length: ${DURATION} * 100 }, (_, i) => Math.abs(Math.sin(i / 137))), sampleRate: 8000 };
    }`),
    editor_segments: fn(`() => {
      (window.__E2E_CALLS__ ||= {}).editor_segments = ((window.__E2E_CALLS__.editor_segments || 0) + 1);
      return ${JSON.stringify(SEGMENTS)};
    }`),
    editor_read_sidecar: null,
    editor_write_sidecar: true,
    editor_delete_sidecar: true,
    editor_master_presets: [],
    editor_probe_peak: -3,
    editor_detect_chapters: [],
    editor_cleanup_temp_files: 0,
    ...over,
  };
}
