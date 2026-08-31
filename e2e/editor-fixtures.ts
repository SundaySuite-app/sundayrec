import { BOOT_FIXTURES, fn, VOID, type Fixtures } from "./harness";
import type { EditorSegment } from "../legacy/bindings/EditorSegment";

// The editor's fixtured recording — ONE recipe, read by both shells.
//
// It lived inside `e2e/editor.spec.ts` until P4a needed the same recording in
// `e2e/editor.spec.ts`. A second copy would have been a second thing to
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
    editor_delete_sidecar: fn(`(args) => {
      (window.__E2E_DELETED_SIDECARS__ ||= []).push(args.sidecar);
      return true;
    }`),
    editor_master_presets: [],
    // (V1/PR3 tok `editor_probe_streams`, `editor_probe_peak` og
    // `editor_cleanup_temp_files` ut herfra: kommandoene finnes ikke lenger,
    // og en fixture for en kommando ingen kaller er en stubb som later som
    // den styrer noe. V1-halen tok `editor_detect_chapters` — samme sort, død
    // siden R2 — og med den er hele denne klassen ute av e2e/: et sveip mot
    // kommando-registeret fant ingen flere.)
    // The channel analysis behind the sound step's profiles. Balanced by
    // default — a fixture that "found" a dead channel in every spec would put a
    // repair into every export payload and hide the ones that mean something.
    editor_auto_process: AUTO_PROCESS,
    editor_master_preview: fn(`(args) => {
      (window.__E2E_PREVIEWS__ ||= []).push(args.request);
      return { previewPath: "/tmp/sundayrec-master-preview-e2e.mp3" };
    }`),
    editor_export: EXPORT_OK,
    editor_cancel_export: fn(`() => {
      (window.__E2E_CANCELS__ ||= []).push(1);
      window.__E2E_FINISH_EXPORT__?.("recording error: cancelled");
      return true;
    }`),
    ...over,
  };
}

/** Where a fixtured export lands. The backend picks the name; this is its
 *  answer, and the receipt shows THIS — never the renderer's prediction. */
export const EXPORTED =
  "/Users/test/Opptak/2026-08-02 Gudstjeneste_redigert.mp3";

/** An export that finishes at once. */
export const EXPORT_OK = fn(`(args) => {
  (window.__E2E_EXPORTS__ ||= []).push(args.request);
  return { outputPath: ${JSON.stringify(EXPORTED)} };
}`);

/**
 * An export that HANGS until the spec says otherwise.
 *
 * `window.__E2E_FINISH_EXPORT__(err?)` settles it — the cancel fixture calls it
 * with the backend's own `cancelled` code, which is what a real
 * `editor_cancel_export` causes: the render's ffmpeg is killed and the export
 * rejects. Without a held export there is nothing to cancel, and a "cancel"
 * test would only prove that a button can be clicked.
 */
export const EXPORT_HELD = fn(`(args) => {
  (window.__E2E_EXPORTS__ ||= []).push(args.request);
  return new Promise((resolve, reject) => {
    window.__E2E_FINISH_EXPORT__ = (err) =>
      err ? reject(new Error(err)) : resolve({ outputPath: ${JSON.stringify(EXPORTED)} });
  });
}`);

/** The channel diagnosis `editor_auto_process` answers with. `balanced` +
 *  `mode: "none"` = nothing to repair, which is the ordinary recording. */
export const AUTO_PROCESS = {
  diagnosis: {
    code: "balanced",
    imbalanceDb: 0.2,
    peakLeftDb: -3.1,
    peakRightDb: -3.3,
    recommended: { mode: "none", leftDb: 0, rightDb: 0 },
  },
  vocalChainPreset: "voice-podcast",
  masterPreset: "",
  summary: "Automatisk lydforbedring: kanalbalanse OK, podkast-stemme.",
};

/** A recording whose left channel is dead — the one case where the repair
 *  MUST ride along, or the export is half silent. */
export const AUTO_PROCESS_DEAD_LEFT = {
  ...AUTO_PROCESS,
  diagnosis: {
    ...AUTO_PROCESS.diagnosis,
    code: "dead_left",
    peakLeftDb: -70,
    recommended: { mode: "duplicateRight", leftDb: 0, rightDb: 0 },
  },
};
