/**
 * «Vi tror prekenen er her» — analysen bak forslaget, og korreksjonen.
 *
 * ## Analysen kjører alltid ved åpning
 *
 * Legacy gjør det (`runDetection(true)` når bølgeformen er klar), og
 * canvasens 4.1 hviler på det: kortet er det FØRSTE en frivillig ser, og et
 * kort som først dukker opp etter at hun har trykket på noe er ikke et forslag,
 * det er en funksjon hun må finne. Bakenden svarer fra sin egen
 * `<stem>.segments.json`-cache på gjenåpning, så det er en filesing andre gang.
 *
 * `force: false` er ikke et detalj: en automatisk kjøring skal gjerne ta
 * gårsdagens svar, mens en bruker som ber om analysen på nytt ber om ARBEIDET.
 * P4a har ingen «Analyser på nytt»-knapp, så den er alltid usann her.
 *
 * ## Korreksjonen bygges FØR forfremmelsen
 *
 * Hele E8-kontrakten hviler på det, og `@lib/pages/editor/sermon-feedback` sier
 * hvorfor med egne ord: en nyttelast satt sammen ETTER at de to `type`-feltene
 * har byttet plass beskriver en verden der detektoren pekte på blokken
 * mennesket pekte på, og hver eneste rapport skrevet fra den sier at detektoren
 * hadde rett. Så funksjonen er importert, ikke gjenskapt, og den kalles først.
 *
 * Hva som TELLER som en korreksjon — om den erstatter den forrige, hvilken
 * blokk den betyr etter en ny analyse — er bakendens dom
 * (`sundayrec_core::feedback`). Her rapporteres hendelsen, den bedømmes ikke.
 */

import {
  sermonCandidates,
  type SermonCandidate,
} from "@lib/pages/editor/sermon-candidates";
import {
  autoSermonIndex as detectorPick,
  buildSermonPickRequest,
} from "@lib/pages/editor/sermon-feedback";

import { suggestionRange } from "./editor-core";
import {
  analyzing,
  E,
  manualMode,
  syncSegments,
  syncSuggestion,
  type Segment,
} from "./model";
import { scheduleDraw } from "./waveform";

export type { SermonCandidate };

/** Blokkene «Er ikke dette prekenen?» får tilby, i tidsrekkefølge. */
export function candidatesFor(segments: readonly Segment[]): SermonCandidate[] {
  return sermonCandidates(segments);
}

/**
 * Kjør analysen for den åpne fila.
 *
 * `seq` er `E.loadSeq` slik den var da åpningen startet, og den sjekkes på
 * nytt etter hver `await` — en bruker som bytter opptak midt i analysen skal
 * aldri få det forrige opptakets segmenter tegnet oppå det nye.
 */
export async function runAnalysis(seq: number): Promise<void> {
  const filePath = E.filePath;
  if (!filePath) return;
  analyzing.value = true;
  let raw: Segment[];
  try {
    raw = await window.api.editorDetectSegments(filePath, false);
  } catch {
    raw = [];
  }
  if (seq !== E.loadSeq) return;

  E.segments = raw;
  // Detektorens EGET svar, lest før noe forfremmes oppå det. Det er
  // grunnlinja hver korreksjon registreres mot, og den er bare synlig i dette
  // ene øyeblikket.
  E.autoSermonIndex = detectorPick(raw);

  await applyStoredPick(seq);
  if (seq !== E.loadSeq) return;

  E.suggestion = suggestionRange(E.segments);
  analyzing.value = false;
  // Ingen preken funnet: da er kuttverktøyene den eneste veien videre, og et
  // klikk for å avsløre den eneste veien videre er et klikk for ingenting.
  if (!E.suggestion) manualMode.value = true;
  syncSegments();
  syncSuggestion();
  scheduleDraw();
}

/**
 * Legg tilbake blokken mennesket rettet oss til sist.
 *
 * Porten hele E8 finnes for: rett opp valget, lukk editoren, åpne igjen — og
 * se DIN blokk, ikke detektorens. Bakenden matcher på tidspunkter, så en ny
 * analyse som har omnummerert lista lander likevel riktig, og den svarer
 * `null` når opptaket ikke inneholder blokken lenger.
 */
async function applyStoredPick(seq: number): Promise<void> {
  let stored: number | null;
  try {
    stored =
      (await window.api.editorSermonPick(E.filePath, E.segments)) ?? null;
  } catch {
    stored = null;
  }
  if (seq !== E.loadSeq) return;
  if (stored === null || stored === E.autoSermonIndex) return;
  // Ingen ny korreksjon registreres: å spille av et lagret svar er ikke et
  // nytt svar.
  promote(stored);
}

/**
 * Mennesket pekte på en annen blokk.
 *
 * Registrerer korreksjonen (fire-and-forget — et svar som ikke lar seg lagre
 * skal aldri blokkere redigeringen hun faktisk holder på med), forfremmer
 * blokken og flytter forslaget dit.
 */
export function chooseSermon(index: number): void {
  if (!E.segments[index]) return;
  const request =
    E.filePath && E.duration > 0
      ? buildSermonPickRequest(E.segments, E.autoSermonIndex, index, E.duration)
      : null;
  const filePath = E.filePath;
  if (!promote(index)) return;
  if (request) void window.api.editorRecordSermonPick(filePath, request);
}

/**
 * Flytt «preken»-merket til `index`, og degrader den forrige tilbake til
 * vanlig tale. Rent i minnet, og bevisst taus om det: gjenopprettingsstien
 * over forfremmer UTEN å registrere noe.
 */
function promote(index: number): boolean {
  const target = E.segments[index];
  if (!target) return false;
  for (const s of E.segments) {
    if (s.type === "sermon") s.type = "speech";
  }
  target.type = "sermon";
  E.suggestion = suggestionRange(E.segments);
  // En ny blokk betyr et nytt forslag: det som var anvendt gjaldt den forrige.
  E.applied = false;
  E.dismissed = false;
  syncSegments();
  syncSuggestion();
  scheduleDraw();
  return true;
}
