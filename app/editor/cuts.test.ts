/**
 * Kuttmutasjonene — de tre påstandene granskningen LESTE seg fram til.
 *
 * Den leste koden og pekte på tre steder. Denne fila kjører dem, fordi en
 * lesning er en mistanke og ikke et funn: hvilke av de tre som faktisk biter
 * avhenger av hvilke tilstander som er NÅBARE gjennom knappene i
 * `EditorPage.tsx`, og det ser man ikke i en diff.
 *
 * Node-miljø, ingen DOM: `requestAnimationFrame` og `window.api` er de to
 * eneste tingene modulen strekker seg etter, og begge er en stubb her.
 * Tegningen og IPC-en er ikke det som testes — reglene er.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  addCut,
  applySermon,
  clearCuts,
  deleteCut,
  keepAll,
  redoCut,
  undoCut,
} from "./cuts";
import {
  applied,
  clearDirty,
  cuts,
  dirty,
  dismissed,
  E,
  resetFileState,
} from "./model";

beforeEach(() => {
  (
    globalThis as unknown as { requestAnimationFrame: unknown }
  ).requestAnimationFrame = () => 0;
  (
    globalThis as unknown as { cancelAnimationFrame: unknown }
  ).cancelAnimationFrame = () => {};
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      editorSaveCutsDraft: () => Promise.resolve(),
      editorDeleteCutsDraft: () => Promise.resolve(),
    },
  };
  vi.useFakeTimers();
  resetFileState();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.duration = 3600;
  E.cutHistory = [[]];
  E.cutHistoryIdx = 0;
  // Et forslag å svare på: «vi tror prekenen er her».
  E.suggestion = { start: 600, end: 2400 };
});

afterEach(() => {
  vi.useRealTimers();
  resetFileState();
  delete (globalThis as unknown as { window?: unknown }).window;
});

// ── (a) Angre/gjør om og «ulagrede endringer» ───────────────────────────────

describe("angre/gjør om merker fila som endret", () => {
  // ⚠️ FUNNET, og det er ekte. `clearDirty()` kalles av en VELLYKKET EKSPORT
  // (`editor/export.ts`). Angrestabelen overlever eksporten, så det neste
  // klikket på «Angre» endrer kuttlista på en fil som nettopp ble erklært ren
  // — og prikken «ulagrede endringer» kommer ikke tilbake, spørsmålet ved
  // lukking kommer ikke, og redigeringen forsvinner uten at noen ble spurt.
  // (Utkastet skrives fortsatt, så tapet er «stille» og ikke «totalt» — men
  // «vi spurte ikke» er hele poenget med `dirty`.)
  //
  // MUTASJONSPRØVEN: fjern `markDirty()` fra `undoCut`/`redoCut`, og begge
  // disse blir røde.
  it("ANGRE etter en eksport gjør fila skitten igjen", () => {
    addCut(600, 900);
    expect(cuts.value).toHaveLength(1);
    clearDirty(); // ← det en vellykket eksport gjør
    expect(dirty.value).toBe(false);

    undoCut();
    expect(cuts.value).toHaveLength(0);
    expect(dirty.value).toBe(true);
  });

  it("GJØR OM etter en eksport gjør fila skitten igjen", () => {
    addCut(600, 900);
    undoCut();
    clearDirty();
    expect(dirty.value).toBe(false);

    redoCut();
    expect(cuts.value).toHaveLength(1);
    expect(dirty.value).toBe(true);
  });

  it("…men en angring som ikke endret noe rører ikke flagget", () => {
    // `undoSnapshot` svarer `null` når det ikke er noe å angre. Å markere
    // skittent der ville betydd et spørsmål ved lukking om en endring som
    // aldri skjedde.
    clearDirty();
    undoCut();
    redoCut();
    expect(dirty.value).toBe(false);
  });
});

// ── (b) «Fjern alle kutt» og kortets to svar ────────────────────────────────

describe("clearCuts svarer på HELE forslaget", () => {
  // ⚠️ FUNNET, og det er ekte. `clearCuts` nullstiller `applied` — den er
  // altså allerede i gang med å ta tilbake svaret på forslaget — men lar
  // `dismissed` stå. `undoCut` tilbake til null kutt nullstiller BEGGE, med en
  // kommentar om at «da skal kortet komme tilbake». To veier til nøyaktig
  // samme tilstand, to forskjellige svar.
  //
  // MUTASJONSPRØVEN: fjern `E.dismissed = false` fra `clearCuts`, og denne
  // blir rød.
  it("«Fjern alle kutt» åpner forslaget igjen, akkurat som Angre gjør", () => {
    keepAll();
    expect(dismissed.value).toBe(true);

    addCut(600, 900);
    clearCuts();

    expect(cuts.value).toHaveLength(0);
    expect(applied.value).toBe(false);
    expect(dismissed.value).toBe(false);
  });

  it("Angre tilbake til null kutt gjør det samme — regelen er én", () => {
    keepAll();
    addCut(600, 900);
    undoCut();
    expect(applied.value).toBe(false);
    expect(dismissed.value).toBe(false);
  });
});

// ── (c) «Anvendt» over en kuttliste som er tom ──────────────────────────────

describe("«Behold bare prekenen» er ikke anvendt når ingenting er kuttet", () => {
  // ⚠️ FUNNET, og det er ekte — men det er STØRRE enn `redoCut`. Granskningen
  // pekte på at gjøre-om aldri setter `applied = false` når den lander på null
  // kutt. Riktig; men veien DIT går gjennom vanlig sletting, som har nøyaktig
  // samme hull: `commit()` rører ikke `applied` i det hele tatt. Sletter man
  // kuttene «Behold bare prekenen» la inn, ett for ett, står `applied` igjen
  // som `true` over en fil der ingenting er trimmet — og kortet som ville
  // tilbudt trimmingen på nytt er borte for godt.
  //
  // Invarianten er derfor håndhevet ETT sted (`reconcileApplied` i cuts.ts),
  // og både `commit`, `undoCut` og `redoCut` går gjennom den.
  //
  // MUTASJONSPRØVEN: fjern kallet til `reconcileApplied()` fra `commit`, og
  // den første blir rød; fjern det fra `redoCut`, og den andre blir rød.
  it("sletting av det siste kuttet tar «anvendt» med seg", () => {
    applySermon();
    expect(applied.value).toBe(true);
    const n = cuts.value.length;
    expect(n).toBeGreaterThan(0);

    for (let i = n - 1; i >= 0; i--) deleteCut(i);

    expect(cuts.value).toHaveLength(0);
    expect(applied.value).toBe(false);
  });

  it("gjøre om TIL null kutt tar «anvendt» med seg", () => {
    applySermon();
    const n = cuts.value.length;
    for (let i = n - 1; i >= 0; i--) deleteCut(i);
    // Nå: tom liste i historikken. Angre henter kuttene tilbake …
    undoCut();
    expect(cuts.value.length).toBeGreaterThan(0);
    // … og gjøre om lander på den tomme igjen.
    redoCut();
    expect(cuts.value).toHaveLength(0);
    expect(applied.value).toBe(false);
  });

  it("…og «anvendt» står så lenge det FINNES kutt", () => {
    applySermon();
    expect(applied.value).toBe(true);
    expect(cuts.value.length).toBeGreaterThan(0);
  });
});
