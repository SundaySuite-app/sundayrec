/**
 * Å forlate en åpen fil MENS en eksport går — granskningens F2-3.
 *
 * Fram til F2-A-B gjorde «Til biblioteket» / et slipp / «Åpne i Rediger» det
 * samme uansett hva som pågikk: `resetExport()` satte `exporting = false`, og
 * ingen cancel gikk til bakenden. ffmpeg-en fortsatte å male på en fil ingen
 * flate lenger fortalte om — og siden EKSPORTERING samtidig tilbød «Sist
 * redigert · Gjør klar» på den SAMME fila, var neste klikk en andre eksport
 * oppå den første.
 *
 * Rekkefølgen er hele poenget, og derfor er den det denne fila måler: cancel
 * FØR nullstillingen. Snus de to, går beskjeden til en bakende skallet allerede
 * har glemt at det ba om noe av.
 *
 * Node-miljø, ingen DOM: `window.api` er en stubb, og dialogen besvares
 * gjennom køen i `ui/dialog.ts` (`activeDialog` + `resolveDialog`) — den samme
 * veien S1b sin vert bruker.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { activeDialog, cancelAllDialogs, resolveDialog } from "../ui/dialog";
import { confirmAbandonExport, confirmDiscard } from "./discard";
import { exporting, resetExport } from "./export";
import { closeFile } from "./loader";
import { E, loadState, resetFileState } from "./model";

/** Hva stubben har sett. `order` er det testen faktisk er om. */
let cancels: number;
let order: string[];

function installFakeApi(): void {
  cancels = 0;
  order = [];
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      editorCancelExport: () => {
        cancels += 1;
        order.push("cancel");
        return Promise.resolve(true);
      },
      // `closeFile` river avspillingen og sidevognene ned på veien ut; ingen av
      // dem har noe å si her, men de skal ikke kaste.
      editorWriteSidecar: () => Promise.resolve(true),
      editorDeleteCutsDraft: () => Promise.resolve(),
    },
    // Lyttingens avslå-timer (`sound.ts`) rydder seg selv gjennom VERTEN, ikke
    // gjennom den globale. Ingen timer er armet her; stubben er bare det som
    // gjør nedrivningen mulig å kjøre uten en jsdom.
    clearTimeout: () => {},
    setTimeout: () => 0,
  };
}

/** Svar på dialogen som står fremst. Kastes hvis det ikke står noen — en test
 *  som «svarte ja» på ingenting ville vært grønn uten å bevise noe. */
function answerDialog(ok: boolean): void {
  const open = activeDialog.value;
  if (!open) throw new Error("ingen dialog å svare på");
  resolveDialog(open.id, ok);
}

beforeEach(() => {
  // `closeFile` river avspillingen ned på veien ut, og rAF-paret der er de to
  // globalene node-miljøet ikke har. Samme stubb som `playback.test.ts` — to
  // funksjoner, ikke en jsdom. `requestAnimationFrame` svarer aldri, så
  // bølgeformens tegnejobb (som trenger et lerret) kjører aldri.
  (
    globalThis as unknown as { cancelAnimationFrame: unknown }
  ).cancelAnimationFrame = () => {};
  (
    globalThis as unknown as { requestAnimationFrame: unknown }
  ).requestAnimationFrame = () => 0;
  installFakeApi();
  resetExport();
  resetFileState();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.duration = 3600;
  loadState.value = "ready";
});

afterEach(() => {
  cancelAllDialogs();
  resetExport();
  resetFileState();
  E.dirty = false;
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("confirmAbandonExport", () => {
  it("spør ikke når ingenting eksporteres", async () => {
    expect(await confirmAbandonExport()).toBe(true);
    expect(activeDialog.value).toBeNull();
    expect(cancels).toBe(0);
  });

  it("avbryter eksporten når svaret er ja", async () => {
    exporting.value = true;
    const asked = confirmAbandonExport();
    answerDialog(true);
    expect(await asked).toBe(true);
    expect(cancels).toBe(1);
  });

  it("rører ingenting når svaret er nei", async () => {
    exporting.value = true;
    const asked = confirmAbandonExport();
    answerDialog(false);
    expect(await asked).toBe(false);
    // Ikke drept, og ikke glemt: eksporten går fortsatt.
    expect(cancels).toBe(0);
    expect(exporting.value).toBe(true);
  });
});

describe("closeFile med en eksport i gang", () => {
  // MUTASJONSPRØVEN: flytt `closeFileNow()` foran vakten i `closeFile`
  // (`loader.ts`) og denne blir rød — `cancel` kommer da etter `reset`, som er
  // ordrett feilen F2-3 beskriver.
  it("avbryter bakenden FØR tilstanden nullstilles", async () => {
    exporting.value = true;
    // `resetExport` er det nullstillingssteget `closeFile` gjør; abonner på
    // signalet det setter, så rekkefølgen kan observeres uten å stubbe modulen.
    const unsub = exporting.subscribe((on) => {
      if (!on && order[order.length - 1] !== "reset") order.push("reset");
    });

    const closing = closeFile();
    answerDialog(true);
    expect(await closing).toBe(true);

    unsub();
    expect(cancels).toBe(1);
    expect(order).toEqual(["cancel", "reset"]);
    // …og fila er faktisk lukket.
    expect(loadState.value).toBe("idle");
    expect(E.filePath).toBe("");
  });

  it("lar fila stå åpen — og eksporten gå — når svaret er nei", async () => {
    exporting.value = true;
    const closing = closeFile();
    answerDialog(false);

    expect(await closing).toBe(false);
    expect(cancels).toBe(0);
    expect(loadState.value).toBe("ready");
    expect(E.filePath).toBe("/Opptak/2026-08-23.flac");
  });

  it("lukker uten et spørsmål når ingenting eksporteres", async () => {
    expect(await closeFile()).toBe(true);
    expect(activeDialog.value).toBeNull();
    expect(loadState.value).toBe("idle");
  });

  // Å lukke er like mye et generasjonsskifte som å åpne: uten bumpen sto hver
  // `seq !== E.loadSeq`-vakt igjen som sann etter en lukking, og en `runExport`
  // som fortsatt hang i en await fortsatte inn i en tømt `E.filePath`.
  it("bumper loadSeq, så en kjøring som henger vet at fila er borte", async () => {
    const before = E.loadSeq;
    await closeFile();
    expect(E.loadSeq).toBeGreaterThan(before);
  });
});

describe("confirmDiscard mens en eksport går", () => {
  // To dialoger på rad om den samme beslutningen er hvordan folk klikker «Ja»
  // på noe de ikke leste — og eksport-spørsmålet INNEHOLDER kutt-spørsmålet:
  // den som sier ja til å avbryte en eksport har sagt ja til å forlate
  // redigeringen.
  it("stiller ikke sitt eget spørsmål — det sterkere står allerede", async () => {
    E.dirty = true;
    exporting.value = true;
    expect(await confirmDiscard()).toBe(true);
    expect(activeDialog.value).toBeNull();
  });

  it("spør som før når ingen eksport går", async () => {
    E.dirty = true;
    const asked = confirmDiscard();
    expect(activeDialog.value).not.toBeNull();
    answerDialog(false);
    expect(await asked).toBe(false);
  });
});
