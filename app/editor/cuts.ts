/**
 * Kuttene — mutasjonene, angrestabelen og utkast-sidevogna.
 *
 * Alle REGLENE er importert, ikke skrevet på nytt:
 *
 *   `@lib/pages/editor/cut-ops`        klemming, minstelengde og fletting
 *   `@lib/pages/editor/cut-history`    angre/gjør om som en ren tilstandsmaskin
 *   `@lib/pages/editor/keep-segments`  hva som blir igjen
 *   `@lib/pages/editor/draft-scheduler` debouncen som ikke kan skrive til feil fil
 *
 * De fire er allerede enhetstestet i legacy, og de er de fire stedene en feil
 * ville vært stille: et galt flett kombinerer eller splitter brukerens kutt
 * uten at noe sier fra, og en utkast-skriving som lander på NESTE fil tar med
 * seg kuttene hun holdt på med. Å skrive dem på nytt her ville vært to steder
 * å ta feil.
 *
 * Det denne fila legger til er ett par per mutasjon: skriv `E`, speil
 * signalet. Ingenting annet.
 */

import { addCutToList, mergeCuts } from "@lib/pages/editor/cut-ops";
import {
  pushSnapshot,
  redoSnapshot,
  undoSnapshot,
} from "@lib/pages/editor/cut-history";
import { createDraftScheduler } from "@lib/pages/editor/draft-scheduler";
import { signal } from "@preact/signals";

import {
  E,
  manualMode,
  markDirty,
  syncCuts,
  syncSuggestion,
  type Cut,
  type Range,
} from "./model";
import { sermonCutRegions, windowToCuts } from "./editor-core";
import { scheduleDraw } from "./waveform";

/** Debouncevinduet for utkast-skrivingen. Legacys eget tall: langt nok til at
 *  en dra-operasjon ikke spammer IPC, kort nok til at en krasj koster et par
 *  sekunders redigering. */
const DRAFT_SAVE_DEBOUNCE_MS = 2000;

export const canUndo = signal(false);
export const canRedo = signal(false);

const draftSaver = createDraftScheduler<Cut[]>({
  delayMs: DRAFT_SAVE_DEBOUNCE_MS,
  save: (fp, list) => {
    void window.api.editorSaveCutsDraft(fp, list).catch(() => {});
  },
});

/** Slå av en ventende utkast-skriving. Kalles øverst i `openFile` og ved
 *  lukking: fra det øyeblikket vi river den åpne fila ned er en køet skriving
 *  i beste fall bortkastet og i verste fall en skriving til NESTE fil. */
export function cancelDraftSave(): void {
  draftSaver.cancel();
}

/**
 * Slett kutt-utkastets sidevogn. Kalles når en eksport lyktes.
 *
 * Utkastet finnes for å overleve en krasj midt i en redigering. Etter en
 * vellykket eksport er redigeringen ute av huset, og et utkast som blir
 * liggende ville lagt de samme kuttene tilbake ved neste åpning — som om
 * eksporten ikke hadde skjedd. Legacys `clearEditorDraft`, med det samme
 * paret: avbryt en ventende skriving FØR sletting, ellers skriver timeren
 * utkastet tilbake to sekunder senere.
 */
export function clearDraft(): void {
  draftSaver.cancel();
  if (E.filePath) {
    void window.api.editorDeleteCutsDraft(E.filePath).catch(() => {});
  }
}

function scheduleDraftSave(): void {
  if (!E.filePath) return;
  // Øyeblikksbilde: den levende lista endres videre (håndtaksdrag redigerer
  // elementene in place), og skrivingen skal beskrive tilstanden slik den var
  // da den ble bedt om.
  draftSaver.schedule(
    E.filePath,
    E.cuts.map((c) => ({ ...c })),
  );
}

function syncHistory(): void {
  canUndo.value = E.cutHistoryIdx > 0 || E.cuts.length > 0;
  canRedo.value = E.cutHistoryIdx < E.cutHistory.length - 1;
}

/** Registrer et øyeblikksbilde ETTER en mutasjon. */
export function pushCutHistory(): void {
  const next = pushSnapshot(
    { history: E.cutHistory, idx: E.cutHistoryIdx },
    E.cuts,
  );
  E.cutHistory = next.history;
  E.cutHistoryIdx = next.idx;
  syncHistory();
  scheduleDraftSave();
}

/** Sett kuttlista, registrer den og speil den. Den ENE veien inn. */
function commit(next: Cut[]): void {
  E.cuts = mergeCuts(next);
  pushCutHistory();
  markDirty();
  syncCuts();
  scheduleDraw();
}

/** Legg til ett kutt fra en dra-operasjon. `null` fra kjernen = for kort. */
export function addCut(from: number, to: number): void {
  const next = addCutToList(E.cuts, from, to, E.duration);
  if (!next) return;
  commit(next);
}

/**
 * Registrer kuttlista etter at en GRENSE er dratt.
 *
 * Draget redigerer elementene in place (det er dét som gjør at bølgeformen
 * følger fingeren uten et flett per pekerhendelse), så sorteringen, flettet,
 * angre-øyeblikksbildet og utkast-skrivingen skjer ÉN gang — her, ved
 * museslipp. Legacy gjør det samme i `onCanvasUp`.
 */
export function commitCutEdges(): void {
  commit(
    E.cuts.map((c) => ({
      start: Math.min(c.start, c.end),
      end: Math.max(c.start, c.end),
    })),
  );
}

export function deleteCut(index: number): void {
  if (index < 0 || index >= E.cuts.length) return;
  commit(E.cuts.filter((_, i) => i !== index));
}

export function clearCuts(): void {
  if (E.cuts.length === 0) return;
  E.applied = false;
  syncSuggestion();
  commit([]);
}

export function undoCut(): void {
  // Aldri bytt ut lista under et pågående drag — det foreldreløser dragets
  // endringer og korrumperer historikken ved museslipp. Legacys egen regel.
  if (E.handleDrag || E.isDragging) return;
  const r = undoSnapshot(
    { history: E.cutHistory, idx: E.cutHistoryIdx },
    E.cuts.length,
  );
  if (!r) return;
  E.cutHistoryIdx = r.idx;
  E.cuts = r.cuts;
  // Angre TILBAKE til ingen kutt betyr at forslaget ikke er anvendt lenger, og
  // da skal kortet komme tilbake — det er dét «angre» betyr her.
  if (E.cuts.length === 0) {
    E.applied = false;
    E.dismissed = false;
    syncSuggestion();
  }
  syncHistory();
  syncCuts();
  scheduleDraftSave();
  scheduleDraw();
}

export function redoCut(): void {
  if (E.handleDrag || E.isDragging) return;
  const r = redoSnapshot({ history: E.cutHistory, idx: E.cutHistoryIdx });
  if (!r) return;
  E.cutHistoryIdx = r.idx;
  E.cuts = r.cuts;
  if (E.cuts.length > 0 && E.suggestion) E.applied = true;
  syncSuggestion();
  syncHistory();
  syncCuts();
  scheduleDraftSave();
  scheduleDraw();
}

/**
 * «Behold bare prekenen» — ETT klikk.
 *
 * Kuttene er `sermonCutRegions` sine, altså legacys `applySermonTrim`, altså
 * Rustens `sermon_cut_regions`. Angre gjenoppretter, fordi mutasjonen går
 * gjennom `commit` som alle andre.
 */
export function applySermon(): void {
  if (!E.suggestion || E.duration <= 0) return;
  E.applied = true;
  E.dismissed = false;
  syncSuggestion();
  commit(sermonCutRegions(E.suggestion, E.segments, E.duration));
  // Kuttlista åpnes: det man nettopp fjernet skal være synlig, og ANGRE skal
  // være innen rekkevidde uten å måtte lete etter «Klipp manuelt» først. Ett
  // klikk skal kunne tas tilbake med ett klikk.
  manualMode.value = true;
}

/** «Behold alt» — legg kortet bort. Ingen kutt, ingen endring å angre. */
export function keepAll(): void {
  E.dismissed = true;
  syncSuggestion();
}

/**
 * Et håndtak ble sluppet.
 *
 * Før «Behold bare prekenen» flytter det FORSLAGET og rører ingen kutt. Etterpå
 * flytter det kuttgrensene, og alt som lå inne i vinduet blir med videre.
 */
export function setSermonWindow(next: Range, applied: boolean): void {
  E.suggestion = { ...next };
  if (!applied) {
    syncSuggestion();
    return;
  }
  commit(windowToCuts(next, E.duration, E.cuts));
  syncSuggestion();
}

/** Legg tilbake kuttene fra en økt som ble avbrutt. Ingen `markDirty` her —
 *  de var allerede lagret som et utkast, og et opptak som ÅPNER som «ulagret»
 *  ville spurt om bekreftelse ved lukking uten at noen hadde gjort noe. */
export function restoreDraftCuts(list: Cut[]): void {
  E.cuts = mergeCuts(list);
  E.cutHistory = [E.cuts.map((c) => ({ ...c }))];
  E.cutHistoryIdx = 0;
  E.applied = E.cuts.length > 0;
  // Og de VISES: hele poenget med gjenopprettingen er at man ser hva forrige
  // økt rakk før den ble avbrutt. Legacy gjorde det stille (den gamle «Fant
  // lagrede kutt»-stripa er borte), og stille er greit — usynlig er det ikke.
  if (E.cuts.length > 0) manualMode.value = true;
  syncHistory();
  syncCuts();
  syncSuggestion();
}

/** Nullstill angrestabelens speil. Kalles når en ny fil åpnes. */
export function resetHistoryMirror(): void {
  canUndo.value = false;
  canRedo.value = false;
}
