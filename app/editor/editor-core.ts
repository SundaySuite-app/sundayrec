/**
 * Editorens avgjørelser — en tabell, ikke betingelser inne i en JSX-linje.
 *
 * Samme grense som `record-core.ts` og `library-core.ts` trekker: alt som er
 * ren aritmetikk står her og er node-testet, alt som trenger `Intl` eller et
 * lerret gjør det ikke.
 *
 * ## Preken-vinduet er ÉN idé, ikke to
 *
 * Canvasens 4.1 har to gullhåndtak i bølgeformen, og de gjør to forskjellige
 * ting avhengig av når man drar i dem: FØR «Behold bare prekenen» justerer de
 * FORSLAGET, etterpå justerer de KUTTGRENSENE. Det er én idé — «hvor begynner
 * og slutter prekenen» — med to representasjoner under seg, og hvis de to
 * representasjonene får hver sin kode kommer de til å bli uenige.
 *
 * Så: `sermonWindow()` svarer alltid med det samme vinduet uansett hvilken
 * side av «anvend» man står på, og `windowToCuts()` er den ene veien tilbake.
 *
 * ## Kuttene rundt prekenen er IKKE bare hodet og halen
 *
 * `sermonCutRegions` er ordrett legacys `applySermonTrim` (som selv speiler
 * `sundayrec_core::editor::sermon_cut_regions`): alt før prekenen, alt etter,
 * OG musikken som ligger INNE i den. Auto-pekingen kan spenne en sang mellom
 * to taleblokker, og den som ber om «bare prekenen» vil ha sangen bort. Indre
 * STILLHET beholdes — det er naturlige pauser, og å klippe dem hakker opp
 * talen.
 */

import { computeKeepSegs } from "@lib/pages/editor/keep-segments";
import { mergeCuts } from "@lib/pages/editor/cut-ops";

import type { Cut, Range, Segment } from "./model";

/**
 * Hvor nær en filkant en grense må være for at kuttet skal droppes.
 *
 * Legacys eget tall (`applySermonTrim`), og grunnen er legacys egen: et kutt
 * på under et halvsekund i hver ende er ikke et kutt, det er avrunding.
 */
export const EDGE_EPSILON_SEC = 0.5;

/** Korteste vindu vi lar et håndtak dra til. Under dette er det ikke en
 *  preken lenger, det er et uhell. */
export const MIN_WINDOW_SEC = 1;

// ── Forslaget ───────────────────────────────────────────────────────────────

/** Blokken analysen mener er prekenen, eller `null` når den ikke fant noen. */
export function sermonSegment(
  segments: readonly Segment[],
): Segment | undefined {
  return segments.find((s) => s.type === "sermon");
}

/** Forslaget som et vindu. `null` når analysen ikke fant noen preken — og da
 *  vises INGENTING, ikke et tomt kort. */
export function suggestionRange(segments: readonly Segment[]): Range | null {
  const sermon = sermonSegment(segments);
  if (!sermon) return null;
  return { start: sermon.start, end: sermon.end };
}

/**
 * Er det noe å trimme i det hele tatt?
 *
 * Legacys `showSuggestionBanner`-regel: hvis både hodet og halen er under et
 * halvsekund er «behold bare prekenen» det samme som «behold alt», og et kort
 * som tilbyr et valg uten forskjell er verre enn ingen kort.
 */
export function suggestionIsWorthOffering(
  range: Range | null,
  durationSec: number,
): boolean {
  if (!range || durationSec <= 0) return false;
  const head = range.start;
  const tail = durationSec - range.end;
  return head > EDGE_EPSILON_SEC || tail > EDGE_EPSILON_SEC;
}

// ── Preken-vinduet ──────────────────────────────────────────────────────────

export interface WindowState {
  cuts: readonly Cut[];
  duration: number;
  suggestion: Range | null;
  applied: boolean;
}

/**
 * Vinduet håndtakene står på.
 *
 * Før «Behold bare prekenen»: forslaget. Etterpå: den ytre grensen av det som
 * er igjen — altså slutten på ledende kutt og starten på det avsluttende. Ett
 * svar, uansett hvilken side av knappen man står på.
 *
 * `null` når det ikke finnes noe vindu å tegne: verken et forslag å justere
 * eller et kutt å flytte.
 */
export function sermonWindow(state: WindowState): Range | null {
  if (!state.applied) {
    return state.suggestion ? { ...state.suggestion } : null;
  }
  const keeps = computeKeepSegs([...state.cuts], state.duration);
  if (keeps.length === 0) return null;
  return { start: keeps[0].start, end: keeps[keeps.length - 1].end };
}

/**
 * Vinduet → kuttlista.
 *
 * Hodet og halen settes av vinduet; alt som allerede lå INNE i vinduet blir
 * med videre, klemt inn i de nye grensene. Det er det som gjør at å dra
 * venstre håndtak ikke sletter musikk-kuttene «Behold bare prekenen» la inn.
 */
export function windowToCuts(
  window: Range,
  durationSec: number,
  existing: readonly Cut[] = [],
): Cut[] {
  const start = clamp(window.start, 0, durationSec);
  const end = clamp(window.end, 0, durationSec);
  const next: Cut[] = [];
  if (start > EDGE_EPSILON_SEC) next.push({ start: 0, end: start });
  if (end < durationSec - EDGE_EPSILON_SEC)
    next.push({ start: end, end: durationSec });
  for (const cut of existing) {
    const s = Math.max(cut.start, start);
    const e = Math.min(cut.end, end);
    if (e > s + EDGE_EPSILON_SEC) next.push({ start: s, end: e });
  }
  return mergeCuts(next);
}

/**
 * Hvor et håndtak faktisk havner.
 *
 * Håndtakene kan ikke krysse hverandre og kan ikke ut av opptaket. Grensa er
 * `MIN_WINDOW_SEC`, ikke null: et vindu på null sekunder ville vært et opptak
 * uten innhold, presentert som et gyldig valg.
 */
export function dragHandle(
  window: Range,
  side: "start" | "end",
  toSec: number,
  durationSec: number,
): Range {
  if (side === "start") {
    const start = clamp(toSec, 0, Math.max(0, window.end - MIN_WINDOW_SEC));
    return { start, end: window.end };
  }
  const end = clamp(
    toSec,
    Math.min(durationSec, window.start + MIN_WINDOW_SEC),
    durationSec,
  );
  return { start: window.start, end };
}

/**
 * Kuttene «Behold bare prekenen» legger inn.
 *
 * Ordrett legacys `applySermonTrim`, som selv speiler Rustens
 * `sermon_cut_regions`: hodet, halen, og musikken inne i prekenen. Indre
 * stillhet beholdes.
 */
export function sermonCutRegions(
  sermon: Range,
  segments: readonly Segment[],
  durationSec: number,
): Cut[] {
  const cuts: Cut[] = [];
  if (sermon.start > EDGE_EPSILON_SEC) {
    cuts.push({ start: 0, end: Math.min(sermon.start, durationSec) });
  }
  if (sermon.end < durationSec - EDGE_EPSILON_SEC) {
    cuts.push({ start: Math.max(0, sermon.end), end: durationSec });
  }
  for (const s of segments) {
    if (s.type !== "music") continue;
    const start = Math.max(s.start, sermon.start);
    const end = Math.min(s.end, sermon.end);
    if (end > start + EDGE_EPSILON_SEC) cuts.push({ start, end });
  }
  return cuts.sort((a, b) => a.start - b.start);
}

// ── Resultatet ──────────────────────────────────────────────────────────────

/** Sekundene som blir igjen etter kuttene. */
export function keptSeconds(
  cutList: readonly Cut[],
  durationSec: number,
): number {
  return computeKeepSegs([...cutList], durationSec).reduce(
    (sum, s) => sum + (s.end - s.start),
    0,
  );
}

/**
 * Formen resultatlinja skal ha.
 *
 * Canvasens «28 min 10 s» trenger sekunder, som `spanOfSeconds` runder bort —
 * den er laget for «hvor lenge varte gudstjenesten», og dette er «hva blir
 * igjen etter klippet», der ti sekunder er forskjellen mellom å ha med
 * velsignelsen og ikke.
 *
 * Tre former, og ingen `tn()`: «min», «s» og «t» er invariante forkortelser i
 * hele tallområdet de vises for — samme mønster som `app.setup.auto.minutes`
 * og `spanOfMinutes` i `record-core`.
 */
export type ExactSpanKind = "seconds" | "minutesSeconds" | "hoursMinutes";

export interface ExactSpan {
  kind: ExactSpanKind;
  hours: number;
  minutes: number;
  seconds: number;
}

export function exactSpan(totalSec: number): ExactSpan {
  const total =
    Number.isFinite(totalSec) && totalSec > 0 ? Math.round(totalSec) : 0;
  if (total < 60) {
    return { kind: "seconds", hours: 0, minutes: 0, seconds: total };
  }
  if (total < 3600) {
    return {
      kind: "minutesSeconds",
      hours: 0,
      minutes: Math.floor(total / 60),
      seconds: total % 60,
    };
  }
  return {
    kind: "hoursMinutes",
    hours: Math.floor(total / 3600),
    minutes: Math.floor((total % 3600) / 60),
    seconds: 0,
  };
}

/**
 * «0:21:08» — klokkeslettformen transporten viser.
 *
 * Egen, og ikke `formatTime` fra `@lib/…/format`: den dropper timetallet under
 * en time, så en gudstjeneste på 1 t 2 min ville hoppet fra «59:59» til
 * «1:00:00» og gjort talltegnene ustabile midt i avspillingen. Her er formen
 * den samme hele veien.
 */
export function timecode(sec: number): string {
  const total = Number.isFinite(sec) && sec > 0 ? Math.floor(sec) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return `${h}:${pad(m)}:${pad(s)}`;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}
