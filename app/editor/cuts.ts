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
  dragWindow,
  E,
  markDirty,
  syncCuts,
  type Cut,
  type Range,
} from "./model";
import { windowToCuts } from "./editor-core";
import { scheduleDraw } from "./waveform";

/** Debouncevinduet for utkast-skrivingen. Legacys eget tall: langt nok til at
 *  en dra-operasjon ikke spammer IPC, kort nok til at en krasj koster et par
 *  sekunders redigering. */
const DRAFT_SAVE_DEBOUNCE_MS = 2000;

export const canUndo = signal(false);
export const canRedo = signal(false);

const draftSaver = createDraftScheduler<Cut[]>({
  delayMs: DRAFT_SAVE_DEBOUNCE_MS,
  // Skrivingen navngir opptaket med lappen den ble armert med (A3) — ikke
  // stien, som Rust ikke tar imot, og ikke lappen `E` har om to sekunder.
  save: (token, list) => {
    void window.api.editorSaveCutsDraft(token, list).catch(() => {});
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
  if (E.sourceToken) {
    void window.api.editorDeleteCutsDraft(E.sourceToken).catch(() => {});
  }
}

function scheduleDraftSave(): void {
  if (!E.sourceToken) return;
  // Øyeblikksbilde: den levende lista endres videre (håndtaksdrag redigerer
  // elementene in place), og skrivingen skal beskrive tilstanden slik den var
  // da den ble bedt om.
  draftSaver.schedule(
    E.sourceToken,
    E.cuts.map((c) => ({ ...c })),
  );
}

function syncHistory(): void {
  canUndo.value =
    E.cutHistoryIdx > 0 || (E.cuts.length > 0 && !E.cutHistoryTruncated);
  canRedo.value = E.cutHistoryIdx < E.cutHistory.length - 1;
}

/** Registrer et øyeblikksbilde ETTER en mutasjon. */
function pushCutHistory(): void {
  const next = pushSnapshot(
    {
      history: E.cutHistory,
      idx: E.cutHistoryIdx,
      truncated: E.cutHistoryTruncated,
    },
    E.cuts,
  );
  E.cutHistoryTruncated = next.truncated === true;
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

/** «Fjern alle kutt». */
export function clearCuts(): void {
  if (E.cuts.length === 0) return;
  commit([]);
}

/**
 * Angre.
 *
 * ## ⚠️ `markDirty()`, som manglet
 *
 * Angre og gjør om ENDRER kuttlista, men var de eneste mutasjonene som ikke
 * gikk gjennom `commit()` og derfor de eneste som ikke merket fila som endret.
 * Det så harmløst ut, fordi `dirty` uansett var satt av kuttet man angret på —
 * helt til man husker at `clearDirty()` finnes, og hvem som kaller den: en
 * VELLYKKET EKSPORT (`editor/export.ts`). Angrestabelen overlever eksporten.
 * Så «eksporter, så angre» endret kuttene på en fil skallet nettopp hadde
 * erklært ren: ingen prikk, og ingen «vil du forkaste endringene?» ved
 * lukking. Redigeringen forsvant uten at noen ble spurt.
 *
 * Merkingen står ETTER `if (!r) return`: en angring som ikke hadde noe å angre
 * skal ikke gi et spørsmål ved lukking om en endring som aldri skjedde.
 */
export function undoCut(): void {
  // Aldri bytt ut lista under et pågående drag — det foreldreløser dragets
  // endringer og korrumperer historikken ved museslipp. Legacys egen regel.
  // Et vindushåndtak i bevegelse (mus eller tastatur) går gjennom
  // `dragWindow` og registrerer seg når det slippes — et angre midt i ville
  // blitt skrevet over av den registreringen (review av #433: ⌘Z under drag).
  if (E.handleDrag || E.isDragging || dragWindow.peek() !== null) return;
  const r = undoSnapshot(
    {
      history: E.cutHistory,
      idx: E.cutHistoryIdx,
      truncated: E.cutHistoryTruncated,
    },
    E.cuts.length,
  );
  if (!r) return;
  E.cutHistoryIdx = r.idx;
  E.cuts = r.cuts;
  markDirty();
  syncHistory();
  syncCuts();
  scheduleDraftSave();
  scheduleDraw();
}

/** Gjør om. Samme `markDirty`-historie som `undoCut` — se den. */
export function redoCut(): void {
  // Et vindushåndtak i bevegelse (mus eller tastatur) går gjennom
  // `dragWindow` og registrerer seg når det slippes — et angre midt i ville
  // blitt skrevet over av den registreringen (review av #433: ⌘Z under drag).
  if (E.handleDrag || E.isDragging || dragWindow.peek() !== null) return;
  const r = redoSnapshot({ history: E.cutHistory, idx: E.cutHistoryIdx });
  if (!r) return;
  E.cutHistoryIdx = r.idx;
  E.cuts = r.cuts;
  markDirty();
  syncHistory();
  syncCuts();
  scheduleDraftSave();
  scheduleDraw();
}

/**
 * Et håndtak ble sluppet.
 *
 * Det flytter kuttgrensene, og alt som lå inne i vinduet blir med videre.
 */
export function setKeepWindow(next: Range): void {
  commit(windowToCuts(next, E.duration, E.cuts));
}

/**
 * Legg tilbake kuttene fra en økt som ble avbrutt. Ingen `markDirty` her —
 * de var allerede lagret som et utkast, og et opptak som ÅPNER som «ulagret»
 * ville spurt om bekreftelse ved lukking uten at noen hadde gjort noe.
 *
 * ## F2-12: klemmes til `E.duration`, akkurat som et nytt kutt
 *
 * `loader.ts` filtrerer utkastet bare på `end > start` — et kutt som sluttet
 * etter varigheten (fila ble kortet ned, eller lest med feil probe, mellom
 * økten som skrev utkastet og denne) kom rett inn i `E.cuts` uklemt. Kuttet
 * telte likevel med i `keptDuration`, som da tegnet et framdriftstall som
 * aldri kunne nå 100 og drev opp den anslåtte gjenværende tiden.
 *
 * Fiksen går gjennom `addCutToList` — SAMME funksjon `addCut` bruker for et
 * kutt fra en dra-operasjon — i stedet for en egen kopi av klemme-regelen:
 * den klemmer til `[0, E.duration]` OG forkaster det som blir kortere enn
 * `MIN_CUT_LENGTH_SEC` etter klemmingen, og fletter som den går. Rekkefølgen
 * spiller ingen rolle — `mergeCuts` sorterer uansett — så resultatet er
 * identisk med å klemme hele lista og flette den én gang.
 */
export function restoreDraftCuts(list: Cut[]): void {
  let restored: Cut[] = [];
  for (const c of list) {
    restored = addCutToList(restored, c.start, c.end, E.duration) ?? restored;
  }
  E.cuts = restored;
  E.cutHistory = [E.cuts.map((c) => ({ ...c }))];
  E.cutHistoryIdx = 0;
  E.cutHistoryTruncated = false;
  syncHistory();
  syncCuts();
}

/** Nullstill angrestabelens speil. Kalles når en ny fil åpnes. */
export function resetHistoryMirror(): void {
  canUndo.value = false;
  canRedo.value = false;
}
