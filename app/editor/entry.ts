/**
 * `window.openEditorWithFile` — den ene globalen Rediger installerer.
 *
 * Kontrakten er ikke ny og ikke vår: `e2e/editor.spec.ts` åpner editoren
 * gjennom den, atlas-scenene gjør det, og legacys historikk-rader og
 * «Siste opptak»-kort kaller den. Signaturen er den samme
 * (`(filePath, seekToSec?) => void`) og typen står allerede i `declare global`
 * i `legacy/renderer/main.ts`, så begge skallene svarer på det samme kallet.
 *
 * Skallet installerer ellers bare `window.showPage` (S1a) — ingen
 * `window.loadSettings`, ingen `window.__isRecording`. Denne er unntaket av
 * samme grunn som den: noe UTENFOR treet hviler på den.
 *
 * ## D3: ingen fane lenger
 *
 * Fram til D3 var Rediger en FANE inne i BIBLIOTEK, og begge kallene her
 * navigerte til `library` med `tab: "edit"`. Nå er REDIGERING destinasjonen,
 * og hvilken av dens to visninger som står avgjøres av om det er en fil åpen
 * (`loadState`), ikke av ruten — se `app/Shell.tsx`. Så: naviger dit, og åpne
 * fila. Rekkefølgen er den samme, og den betyr det samme: `openFile` setter
 * `loadState` SYNKRONT, så biblioteket rekker aldri å blinke innom.
 */

import { navigate } from "../router/router";
import { openFile } from "./loader";

export function installEditorEntry(): void {
  window.openEditorWithFile = (path: string, seekToSec?: number): void => {
    navigate("edit");
    void openFile(path, {
      seekToSec: typeof seekToSec === "number" ? seekToSec : null,
    });
  };
}

/** Gå til Rediger med et opptak åpent. Radens egen dato følger med, fordi
 *  editoren ikke kan lese den ut av fila — den er overskriften. */
export function openInEditor(path: string, startedAtMs: number | null): void {
  navigate("edit");
  void openFile(path, { startedAtMs });
}
