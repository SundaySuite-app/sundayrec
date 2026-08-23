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
 */

import { navigate } from "../router/router";
import { openFile } from "./loader";

/**
 * Fanen Rediger bor i.
 *
 * BIBLIOTEK er destinasjonen — å finne opptaket igjen og å redigere det er
 * samme sted i den nye arkitekturen (canvasens sett 3 og 4). `TAB_ALIASES`
 * oversetter allerede den gamle `editor`-siden hit, og navnet her er den
 * andre halvdelen av den samme raden.
 */
export const EDIT_TAB = "edit";

export function installEditorEntry(): void {
  window.openEditorWithFile = (path: string, seekToSec?: number): void => {
    navigate("library", { tab: EDIT_TAB });
    void openFile(path, {
      seekToSec: typeof seekToSec === "number" ? seekToSec : null,
    });
  };
}

/** Gå til Rediger med et opptak åpent. Radens egen dato følger med, fordi
 *  editoren ikke kan lese den ut av fila — den er overskriften. */
export function openInEditor(path: string, startedAtMs: number | null): void {
  navigate("library", { tab: EDIT_TAB });
  void openFile(path, { startedAtMs });
}
