/**
 * Editorens modell — `E`, uendret i form, og speilene ved siden av.
 *
 * ## Hvorfor `E` fortsatt er ett muterbart objekt
 *
 * Legacy-editoren deler ett `E` mellom alle `editor/`-modulene, og det er ikke
 * gammel gjeld — det er den ene hete stien i hele appen. Tegneløkka leser
 * `cuts`, `peaks`, `vpStart`, `playStartSec` opptil seksti ganger i sekundet.
 * Et signal per felt ville betydd seksti sporede lesninger per frame og en
 * re-render av treet hver gang spillehodet flyttet seg en piksel.
 *
 * Så: `E` er PORTET, ikke oversatt. Samme objekt, samme felter, samme
 * betydning — bare uten de feltene P4a ikke bygger (mikser, mastring, video,
 * jingler, metadata). Hvert felt som ER her betyr nøyaktig det det betyr i
 * `legacy/renderer/pages/editor/state.ts`.
 *
 * ## Speilene, og regelen som holder dem sanne
 *
 * Treet kan ikke lese `E` — det er ikke reaktivt. Så hvert felt en KOMPONENT
 * viser har et signal ved siden av seg, og regelen er én linje lang:
 *
 *     E.cuts = neste          // sannheten, for tegneløkka
 *     cuts.value = E.cuts     // speilet, for treet
 *
 * Legacy gjør det samme paret allerede, bare med `drawWaveform()` som andre
 * halvdel («anvend, så tegn»). Her er andre halvdel et signal, og tegningen
 * abonnerer på det.
 *
 * Speilet er ALDRI kilden. Ingen skriver `cuts.value` uten å ha skrevet
 * `E.cuts` først, og ingenting inne i en rAF-løkke leser et signal — det er
 * `peek()` eller `E` der. To skrivere på én sannhet er skjøten hele dette
 * skallet er skrevet for å slippe.
 *
 * ## Spillehodet er frame-gatet
 *
 * `playheadSec` oppdateres på husets tegne-kadens (`@lib/ui/frame-gate`), ikke
 * per rAF: tallet «0:21:08» endrer seg ett sekund om gangen, og en re-render av
 * transportlinja seksti ganger i sekundet for å vise det samme tallet er den
 * jank-en canvasen ellers ville fått skylda for.
 */

import { computed, signal } from "@preact/signals";
import type { EditorSegment } from "@lib/../bindings/EditorSegment";
import type { Cut } from "@lib/pages/editor/state";

export type { Cut };

/** Ett analysert segment, slik `editor_segments` sender det. */
export type Segment = EditorSegment;

/** Et tidsvindu i opptaket. Brukes for forslaget og for preken-vinduet. */
export interface Range {
  start: number;
  end: number;
}

/** Hvor langt åpningen av en fil er kommet. */
export type LoadState = "idle" | "loading" | "ready" | "error";

/** Stegene i Rediger. Bare `cut` er bygget i P4a; P4b legger til de to andre. */
export type Step = "cut";

/**
 * Hva avspillingen kan si om seg selv.
 *
 *   `original` — webviewen strømmer opptaket rett fra disk. Ingen beskjed.
 *   `proxy`    — den kunne ikke, så vi laget en mellomfil. Beskjed, dempet.
 *   `none`     — heller ikke det gikk. Beskjed, og knappen er sperret med grunn.
 */
export type PlaybackSource = "original" | "proxy" | "none";

// ── Den muterbare sannheten ─────────────────────────────────────────────────

export const E = {
  /** Stien til opptaket som er åpent. Tom = ingen. */
  filePath: "",
  /** Filnavnet, avledet én gang ved åpning. */
  fileName: "",
  /** Millisekundet opptaket startet, når biblioteket kjente raden. */
  startedAtMs: null as number | null,
  /** Lengden på opptaket i sekunder. 0 = ikke lest ennå. */
  duration: 0,
  /** 100 topper i sekundet, fra bakenden. `null` = ingen bølgeform ennå. */
  peaks: null as Float32Array | null,

  /** Delene som skal BORT. Alltid sortert og flettet — se `@lib/…/cut-ops`. */
  cuts: [] as Cut[],
  /** Angrestabelen. Rene øyeblikksbilder; `@lib/…/cut-history` eier reglene. */
  cutHistory: [] as Cut[][],
  cutHistoryIdx: -1,

  /** Analysens segmenter, slik brukeren ser dem. */
  segments: [] as Segment[],
  /**
   * Indeksen DETEKTOREN selv pekte på, før en lagret korreksjon ble lagt
   * oppå. Det er grunnlinja hver korreksjon registreres mot (E8-kontrakten).
   */
  autoSermonIndex: null as number | null,

  /** Vinduet vi tror er prekenen, før noen har svart på det. */
  suggestion: null as Range | null,
  /** Har «Behold bare prekenen» blitt brukt? */
  applied: false,
  /** Har «Behold alt» lagt kortet bort? */
  dismissed: false,

  /** Utsnittet bølgeformen viser, i sekunder. */
  vpStart: 0,
  vpEnd: 0,

  /**
   * `<audio>`-elementet som strømmer opptaket. EIES AV MODELLEN, ikke av
   * JSX: et element Preact rendrer ville mistet `src`-en sin ved neste
   * render av noe over det, og en avspilling som stopper uten at noe feiler
   * er den verste formen for feil. Legacy gjør det samme, av samme grunn.
   */
  playerEl: null as HTMLAudioElement | null,
  playbackSource: "original" as PlaybackSource,
  /** Der spillehodet står NÅ, i sekunder fra opptakets start. */
  playStartSec: 0,
  isPlaying: false,
  /** Play er bedt om, men elementet har ikke startet ennå. Se `playback.ts`. */
  mainPlayPending: false,
  rafId: 0,

  /**
   * Lastesekvensen. Hver `await` sjekker den på nytt, så en bruker som åpner
   * fil nummer to midt i lastingen av den første aldri får den førstes
   * varighet, topper eller transport.
   */
  loadSeq: 0,

  /** Ulagrede endringer — kutt som ikke er eksportert. */
  dirty: false,

  // ── Interaksjon (rene arbeidsfelter for canvas-input) ────────────────────
  isDragging: false,
  dragStartSec: -1,
  dragEndSec: -1,
  handleDrag: null as "start" | "end" | null,
  playheadDragging: false,
  minimapDragging: false,
  /** Spøkelseslinja der musa er. −99999 = ingen. Legacys egen sentinel. */
  hoverSec: -99999,

  /** De to lerretene. Satt av `WaveformHost` ved montering, aldri byttet. */
  canvas: null as HTMLCanvasElement | null,
  minimap: null as HTMLCanvasElement | null,
};

// ── Speilene ────────────────────────────────────────────────────────────────

export const filePath = signal("");
export const fileName = signal("");
export const startedAtMs = signal<number | null>(null);
export const duration = signal(0);
export const cuts = signal<readonly Cut[]>([]);
export const segments = signal<readonly Segment[]>([]);
export const suggestion = signal<Range | null>(null);
export const applied = signal(false);
export const dismissed = signal(false);
export const dirty = signal(false);
export const playing = signal(false);
export const playheadSec = signal(0);
export const viewport = signal<Range>({ start: 0, end: 0 });
export const loadState = signal<LoadState>("idle");
/** Hva lastingen holder på med akkurat nå, som en katalognøkkel-suffiks. */
export const loadPhase = signal<string | null>(null);
/** Hvor langt lastefasen er kommet (0–1), eller `null` for ubestemt. */
export const loadProgress = signal<number | null>(null);
/** Feilteksten når `loadState` er `error`. En nøkkel, ikke prosa. */
export const loadError = signal<string | null>(null);
export const playbackSource = signal<PlaybackSource>("original");
/** Steget som vises. P4b legger til `sound` og `export`. */
export const activeStep = signal<Step>("cut");
/** Er kuttverktøyene avslørt? «Klipp manuelt» slår dem på. */
export const manualMode = signal(false);
/** Kjører analysen fortsatt i bakgrunnen? */
export const analyzing = signal(false);

/**
 * Vinduet mens et håndtak er i bevegelse, eller `null`.
 *
 * Uten det står gullvinduet stille til fingeren slippes: overlegget leser enten
 * forslaget eller kuttlista, og ingen av dem skrives per pekerhendelse (et
 * kuttflett per bevegelse ville lagt et øyeblikksbilde i angrestabelen 125
 * ganger i sekundet). Så draget får sitt eget speil, og det er det eneste som
 * lever mellom `pointerdown` og `pointerup`.
 */
export const dragWindow = signal<Range | null>(null);

/** Er det noe åpent i det hele tatt? */
export const hasFile = computed(() => filePath.value !== "");

// ── Synkroniseringen: ett par, ett sted ─────────────────────────────────────

/** Speil kuttlista. `slice()` fordi dra-operasjoner endrer elementene i
 *  lista IN PLACE — et speil som pekte på den samme arrayen ville aldri
 *  meldt fra om at noe hadde endret seg. */
export function syncCuts(): void {
  cuts.value = E.cuts.slice();
}

/** Speil segmentene. Kopier av samme grunn: `promoteSermon` bytter `type` på
 *  de samme objektene. */
export function syncSegments(): void {
  segments.value = E.segments.map((s) => ({ ...s }));
}

/** Speil forslaget og de to svarene på det. */
export function syncSuggestion(): void {
  suggestion.value = E.suggestion ? { ...E.suggestion } : null;
  applied.value = E.applied;
  dismissed.value = E.dismissed;
}

/** Speil utsnittet. */
export function syncViewport(): void {
  viewport.value = { start: E.vpStart, end: E.vpEnd };
}

export function markDirty(): void {
  if (E.dirty) return;
  E.dirty = true;
  dirty.value = true;
}

export function clearDirty(): void {
  E.dirty = false;
  dirty.value = false;
}

/**
 * Nullstill alt som hører til ÉN åpen fil.
 *
 * Kalles ved starten av en ny åpning og når fila lukkes. Det som IKKE
 * nullstilles er `playerEl` — elementet gjenbrukes, for et nytt `new Audio()`
 * per fil lekker en dekoder og et åpent filhåndtak hver gang (legacys
 * `persistentPlayerEl`, samme grunn).
 */
export function resetFileState(): void {
  E.filePath = "";
  E.fileName = "";
  E.startedAtMs = null;
  E.duration = 0;
  E.peaks = null;
  E.cuts = [];
  E.cutHistory = [];
  E.cutHistoryIdx = -1;
  E.segments = [];
  E.autoSermonIndex = null;
  E.suggestion = null;
  E.applied = false;
  E.dismissed = false;
  E.vpStart = 0;
  E.vpEnd = 0;
  E.playStartSec = 0;
  E.isPlaying = false;
  E.mainPlayPending = false;
  E.playbackSource = "original";
  E.isDragging = false;
  E.dragStartSec = -1;
  E.dragEndSec = -1;
  E.handleDrag = null;
  E.playheadDragging = false;
  E.minimapDragging = false;
  E.hoverSec = -99999;

  filePath.value = "";
  fileName.value = "";
  startedAtMs.value = null;
  duration.value = 0;
  playheadSec.value = 0;
  playing.value = false;
  playbackSource.value = "original";
  manualMode.value = false;
  analyzing.value = false;
  activeStep.value = "cut";
  loadPhase.value = null;
  loadProgress.value = null;
  loadError.value = null;
  dragWindow.value = null;
  syncCuts();
  syncSegments();
  syncSuggestion();
  syncViewport();
  clearDirty();
}
