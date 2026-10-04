/**
 * `window.openEditorWithRecording` — den ene globalen Rediger installerer.
 *
 * `e2e/editor.spec.ts` åpner editoren gjennom den, og atlas-scenene gjør det.
 * Den het `openEditorWithFile(filePath, seekToSec?)` til A2: en global som tok
 * en sti og åpnet den var nettopp formen som er lukket — nå tar den en RAD-ID
 * fra historikken (`editor_open_known`), og databasen sier hvilken fil det er.
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
  window.openEditorWithRecording = (
    recordingId: string,
    seekToSec?: number,
  ): void => {
    navigate("edit");
    void openFile(
      { kind: "known", recordingId },
      { seekToSec: typeof seekToSec === "number" ? seekToSec : null },
    );
  };
}

/** Gå til Rediger med et opptak åpent, navngitt ved raden i historikken. Radens
 *  egen dato følger med, fordi editoren ikke kan lese den ut av fila — den er
 *  overskriften. `name` er overskriften mens Rust finner fila.
 *
 *  Uten rad-id (historikken er ikke lest ennå, eller opptaket har ingen rad)
 *  er det ingen fil å åpne: da åpner Rediger med «Kunne ikke åpne opptaket» og
 *  veien til «Åpne fil …», i stedet for at knappen ikke gjør noe. */
export function openInEditor(
  recordingId: string | null | undefined,
  startedAtMs: number | null,
  name?: string,
): void {
  navigate("edit");
  void openFile(
    recordingId
      ? { kind: "known", recordingId, name }
      : { kind: "refused", error: "source_unknown" },
    { startedAtMs },
  );
}
