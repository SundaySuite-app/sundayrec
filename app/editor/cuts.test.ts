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
  clearCuts,
  clearDraft,
  redoCut,
  restoreDraftCuts,
  setKeepWindow,
  undoCut,
} from "./cuts";
import {
  clearDirty,
  cuts,
  dragWindow,
  dirty,
  E,
  resetFileState,
} from "./model";

/** Lappen Rust ga opptaket (A3): det eneste utkastet navngir det med. */
const TOKEN = "11111111-1111-4111-8111-111111111111";
let savedDrafts: Array<{ token: string; list: unknown }>;
let deletedDrafts: string[];

beforeEach(() => {
  (
    globalThis as unknown as { requestAnimationFrame: unknown }
  ).requestAnimationFrame = () => 0;
  (
    globalThis as unknown as { cancelAnimationFrame: unknown }
  ).cancelAnimationFrame = () => {};
  savedDrafts = [];
  deletedDrafts = [];
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      editorSaveCutsDraft: (token: string, list: unknown) => {
        savedDrafts.push({ token, list });
        return Promise.resolve();
      },
      editorDeleteCutsDraft: (token: string) => {
        deletedDrafts.push(token);
        return Promise.resolve();
      },
    },
  };
  vi.useFakeTimers();
  resetFileState();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.sourceToken = TOKEN;
  E.duration = 3600;
  E.cutHistory = [[]];
  E.cutHistoryIdx = 0;
});

afterEach(() => {
  vi.useRealTimers();
  resetFileState();
  delete (globalThis as unknown as { window?: unknown }).window;
});

// ── Utkastet navngir opptaket med lappen (A3) ───────────────────────────────

describe("kutt-utkastet", () => {
  // MUTASJONSPRØVEN: send `E.filePath` i stedet for `E.sourceToken` i
  // `scheduleDraftSave`, og denne blir rød — Rust tar ikke lenger imot en sti.
  it("skrives med opptakets lapp, aldri stien", () => {
    addCut(600, 900);
    vi.advanceTimersByTime(2000);

    expect(savedDrafts).toEqual([
      { token: TOKEN, list: [{ start: 600, end: 900 }] },
    ]);
  });

  it("slettes med lappen etter en eksport", () => {
    clearDraft();

    expect(deletedDrafts).toEqual([TOKEN]);
  });

  it("uten et åpent opptak (ingen lapp) skrives og slettes ingenting", () => {
    E.sourceToken = "";
    addCut(600, 900);
    clearDraft();
    vi.advanceTimersByTime(5000);

    expect(savedDrafts).toEqual([]);
    expect(deletedDrafts).toEqual([]);
  });
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

// ── (b) Et håndtak i bevegelse ──────────────────────────────────────────────

describe("angre mens et vindushåndtak er i bevegelse", () => {
  // Review av #433: ⌘Z midt i et håndtaksdrag ble skrevet over når draget
  // registrerte seg ved slipp.
  it("gjør ingenting så lenge `dragWindow` står", () => {
    addCut(600, 900);
    dragWindow.value = { start: 100, end: 500 };
    undoCut();
    expect(cuts.value).toHaveLength(1);
    dragWindow.value = null;
    undoCut();
    expect(cuts.value).toHaveLength(0);
    dragWindow.value = { start: 100, end: 500 };
    redoCut();
    expect(cuts.value).toHaveLength(0);
    dragWindow.value = null;
  });
});

describe("«Fjern alle kutt»", () => {
  it("tømmer lista, og Angre henter kuttene tilbake", () => {
    addCut(600, 900);
    addCut(1200, 1500);
    clearCuts();
    expect(cuts.value).toHaveLength(0);

    undoCut();
    expect(cuts.value).toHaveLength(2);
  });

  it("gjør ingenting når det ikke er noen kutt", () => {
    clearDirty();
    clearCuts();
    expect(dirty.value).toBe(false);
  });
});

// ── (c) Vinduet blir kutt ───────────────────────────────────────────────────

describe("setKeepWindow", () => {
  it("setter hode og hale, og ett angre-steg tar dem bort igjen", () => {
    setKeepWindow({ start: 600, end: 2400 });
    expect(cuts.value).toEqual([
      { start: 0, end: 600 },
      { start: 2400, end: 3600 },
    ]);

    undoCut();
    expect(cuts.value).toHaveLength(0);
  });

  it("beholder kuttene som lå inne i vinduet", () => {
    addCut(1000, 1100);
    setKeepWindow({ start: 600, end: 2400 });
    expect(cuts.value).toEqual([
      { start: 0, end: 600 },
      { start: 1000, end: 1100 },
      { start: 2400, end: 3600 },
    ]);
  });
});

// ── (d) F2-12: et utkast klemmes til varigheten, som et nytt kutt ──────────

describe("restoreDraftCuts klemmer til E.duration", () => {
  // ⚠️ FUNNET, og det er ekte. `loader.ts` filtrerer utkastet bare på
  // `end > start` før det kalles hit — et kutt som slutter etter varigheten
  // (fila ble kortet ned mellom økten som skrev utkastet og denne) kom rett
  // inn i `E.cuts` uklemt. `kept_duration` regnet da et tall STØRRE enn den
  // ekte varigheten, og framdriften under eksport kunne aldri nå 100 %.
  //
  // MUTASJONSPRØVEN: bytt `addCutToList(restored, c.start, c.end, E.duration)`
  // ut med et rått `restored.push({ start: c.start, end: c.end })`, og den
  // første og tredje testen blir røde.
  it("et kutt som slutter etter varigheten klemmes til den", () => {
    restoreDraftCuts([{ start: 100, end: E.duration + 500 }]);
    expect(cuts.value).toEqual([{ start: 100, end: E.duration }]);
  });

  it("et kutt som BEGYNNER etter varigheten blir tomt og forkastes", () => {
    restoreDraftCuts([{ start: E.duration + 10, end: E.duration + 90 }]);
    expect(cuts.value).toHaveLength(0);
  });

  it("et kutt godt innenfor varigheten kommer tilbake uendret", () => {
    restoreDraftCuts([{ start: 600, end: 900 }]);
    expect(cuts.value).toEqual([{ start: 600, end: 900 }]);
  });

  it("bare det kuttet som stikker ut klemmes — resten av utkastet er urørt", () => {
    restoreDraftCuts([
      { start: 100, end: 200 },
      { start: E.duration - 50, end: E.duration + 500 },
    ]);
    expect(cuts.value).toEqual([
      { start: 100, end: 200 },
      { start: E.duration - 50, end: E.duration },
    ]);
  });
});
