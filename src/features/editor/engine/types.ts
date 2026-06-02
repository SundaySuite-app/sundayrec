// Editor engine — shared types. Ported faithfully from the Electron renderer
// (`src/renderer/pages/editor/state.ts`). The engine is framework-agnostic; React
// only mounts the canvas and subscribes to snapshots.

/** A cut region the user marked to remove (main-file seconds). Matches the
 *  backend `EditorCutRegion` 1:1 so it serialises straight into `editor_export`. */
export interface Cut {
  start: number;
  end: number;
}

/** An auto-detected segment boundary (speech / music / silence / sermon). */
export interface Suggestion {
  start: number;
  end: number;
  duration: number;
  label: string;
  type: string;
}

export interface HandleDrag {
  cutIdx: number;
  side: "start" | "end";
}

/** Audio formats the browser (Web Audio API) can decode natively. Everything
 *  else falls back to the ffmpeg-extract path (8 kHz mono) via the backend. */
export const WEB_AUDIO_EXTS = new Set([
  ".mp3",
  ".wav",
  ".flac",
  ".aac",
  ".m4a",
  ".m4b",
  ".m4r",
  ".ogg",
  ".oga",
  ".opus",
  ".webm",
]);

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

/** Metadata sidecar shape (title / speaker / description + chapter markers). */
export interface ChapterMark {
  time: number;
  title: string;
}
export interface EditorMeta {
  title: string;
  speaker: string;
  description: string;
  chapters: ChapterMark[];
}

/** The single mutable editor state object — the React-free source of truth the
 *  engine and the pure render/geometry/cut modules read and write. Mirrors the
 *  Electron `E` object so the ported drawing/interaction code transcribes
 *  near-verbatim. */
export interface EditorState {
  filePath: string;
  duration: number;
  peaks: Float32Array | null;

  cuts: Cut[];
  cutHistory: Cut[][];
  cutHistoryIdx: number;
  suggestions: Suggestion[];

  // Intro / outro (audio jingles)
  introBuffer: AudioBuffer | null;
  outroBuffer: AudioBuffer | null;
  introDuration: number;
  outroDuration: number;
  includeIntroOutro: boolean;
  introPeaks: Float32Array | null;
  outroPeaks: Float32Array | null;

  // Analyze-panel display toggles
  showSpeechSegments: boolean;
  showMusicSegments: boolean;
  showSilenceSegments: boolean;

  // Metadata
  meta: EditorMeta;

  // Viewport (seconds visible in the main canvas)
  vpStart: number;
  vpEnd: number;

  // Playback
  audioCtx: AudioContext | null;
  sourceNodes: AudioBufferSourceNode[];
  audioBuffer: AudioBuffer | null;
  playStartCtxTime: number;
  playStartSec: number;
  isPlaying: boolean;
  isPreview: boolean;
  rafId: number;
  loadSeq: number;

  // Interaction
  dragStartSec: number;
  dragEndSec: number;
  isDragging: boolean;
  hoverSec: number;
  minimapDragging: boolean;

  // Clipping detection
  clipTimes: number[];

  // Peak-normalization gain (dB; 0 = none) applied to playback + render + export.
  audioGainDb: number;

  // Loop playback
  isLooping: boolean;
  loopStartSec: number;

  // Cut-handle + playhead dragging
  handleDrag: HandleDrag | null;
  playheadDragging: boolean;
}

export function createEditorState(): EditorState {
  return {
    filePath: "",
    duration: 0,
    peaks: null,
    cuts: [],
    cutHistory: [],
    cutHistoryIdx: -1,
    suggestions: [],
    introBuffer: null,
    outroBuffer: null,
    introDuration: 0,
    outroDuration: 0,
    includeIntroOutro: false,
    introPeaks: null,
    outroPeaks: null,
    showSpeechSegments: true,
    showMusicSegments: true,
    showSilenceSegments: false,
    meta: { title: "", speaker: "", description: "", chapters: [] },
    vpStart: 0,
    vpEnd: 0,
    audioCtx: null,
    sourceNodes: [],
    audioBuffer: null,
    playStartCtxTime: 0,
    playStartSec: 0,
    isPlaying: false,
    isPreview: false,
    rafId: 0,
    loadSeq: 0,
    dragStartSec: -1,
    dragEndSec: -1,
    isDragging: false,
    hoverSec: -99999,
    minimapDragging: false,
    clipTimes: [],
    audioGainDb: 0,
    isLooping: false,
    loopStartSec: 0,
    handleDrag: null,
    playheadDragging: false,
  };
}

/** File extension incl. dot, lower-cased (`"/a/b.MP3"` → `".mp3"`). */
export function extOf(path: string): string {
  return ("." + (path.split(".").pop()?.toLowerCase() ?? "")).toLowerCase();
}

/** Basename of a path, handling both separators. */
export function baseName(path: string): string {
  return path.split(/[/\\]/).pop() ?? path;
}
