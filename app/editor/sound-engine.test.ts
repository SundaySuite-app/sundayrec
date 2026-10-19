/**
 * Lyd-steget på avspillingsmotoren (B5b): Før/Etter er en melding, ikke en gjengivelse; en ny
 * profil midt i avspilling setter inn en ny kjede uten pause; spillehodet er det motoren sier.
 *
 * Motoren er `fakeListen()`: det den ble bedt om, i rekkefølge, og det den ville sagt tilbake.
 *
 * MUTASJONSPRØVEN: la `setListenSide("after")` ringe `installChain()` også når kjeden er inne
 * (altså bestille en ny kjede for hvert bytte), og «bytte er én melding»-testen blir rød. Ta
 * `if (e.gen < maxGen) return` ut av `onListenEvent`, og «en forsinket posisjon»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { isRecording } from "../state/recording";
import { E, resetFileState } from "./model";
import {
  PREPARE_DEBOUNCE_MS,
  listenAt,
  listenError,
  listenPlaying,
  listenPreparing,
  listenSide,
  listenStart,
  resetSound,
  setListenSide,
  soundProfile,
  stopListen,
  toggleListen,
  useMixer,
  watchListenPrepare,
} from "./sound";
import { fakeListen, type FakeListen } from "./test-support/fake-listen-api";

const TOKEN = "33333333-3333-4333-8333-333333333333";

let engine: FakeListen;
let stop: (() => void) | null;

const settle = async (ms = 0): Promise<void> => {
  await vi.advanceTimersByTimeAsync(ms);
};

beforeEach(() => {
  vi.useFakeTimers();
  engine = fakeListen();
  stop = null;
  resetFileState();
  isRecording.value = false;
  const g = globalThis as unknown as Record<string, unknown>;
  g.window = {
    setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms),
    clearTimeout: (id: number) => clearTimeout(id),
    api: {
      ...engine.api,
      editorAutoProcess: () => Promise.resolve({ diagnosis: null }),
      editorTaleSaid: () => Promise.resolve(null),
      prepareListen: () => Promise.resolve(true),
    },
  };
  resetSound();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.sourceToken = TOKEN;
  E.duration = 3600;
  E.cuts = [] as typeof E.cuts;
  listenSide.value = "after";
  listenStart.value = 1800;
  soundProfile.value = "speech";
  useMixer.value = false;
});

afterEach(() => {
  stop?.();
  resetSound();
  resetFileState();
  vi.useRealTimers();
  delete (globalThis as unknown as Record<string, unknown>).window;
});

/** Åpne økta og spill «Etter» til kjeden er inne. */
async function playingAfter(): Promise<void> {
  stop = watchListenPrepare();
  await settle();
  await toggleListen();
  expect(listenPlaying.value).toBe(true);
}

describe("Før og Etter er et bytte", () => {
  it("bytte side mens det spiller er én melding til motoren, uten ny kjede og uten pause", async () => {
    await playingAfter();
    const before = engine.log.length;

    setListenSide("before");
    await settle();
    setListenSide("after");
    await settle();

    // To meldinger, ingenting annet: ikke åpning, ikke kjede (ingenting å måle eller gjengi),
    // ikke pause. Det er derfor det er hørt med en gang.
    expect(engine.log.slice(before)).toEqual([
      { cmd: "transport", action: { kind: "side", after: false } },
      { cmd: "transport", action: { kind: "side", after: true } },
    ]);
    expect(listenPlaying.value).toBe(true);
  });

  it("«Etter» velges i motoren først når kjeden er inne", async () => {
    stop = watchListenPrepare();
    await settle();
    const release = engine.holdChain();
    const playing = toggleListen();
    await settle();
    expect(engine.actions().filter((a) => a.kind === "side")).toEqual([]);
    release(true);
    await playing;
    expect(engine.actions()).toEqual([
      { kind: "side", after: true },
      { kind: "play" },
    ]);
  });

  it("er ikke kjeden inne når man trykker «Etter» under «Før», velges «Etter» når den kommer", async () => {
    listenSide.value = "before";
    stop = watchListenPrepare();
    await settle();
    const release = engine.holdChain();
    await toggleListen(); // spiller «Før» mens kjeden gjøres klar
    expect(listenPlaying.value).toBe(true);

    setListenSide("after");
    await settle();
    expect(listenPreparing.value).toBe(true);
    // Fortsatt «Før» i motoren: kjeden er ikke inne.
    expect(
      engine.actions().filter((a) => a.kind === "side" && a.after === true),
    ).toEqual([]);

    release(true);
    await settle();
    expect(listenPreparing.value).toBe(false);
    expect(engine.actions().at(-1)).toEqual({ kind: "side", after: true });
  });
});

describe("profilbytte midt i avspilling", () => {
  it("setter inn den nye kjeden når den er målt — uten pause, uten ny åpning", async () => {
    await playingAfter();
    const first = engine.chains();
    expect(first).toHaveLength(1);
    expect(first[0].presetId).toBe("speech-clear");
    const before = engine.log.length;

    soundProfile.value = "mixed";
    // Kortet venter på den nye kjeden mens den gamle spiller.
    await settle(1);
    expect(listenPreparing.value).toBe(true);
    await settle(PREPARE_DEBOUNCE_MS);

    const chains = engine.chains();
    expect(chains).toHaveLength(2);
    expect(chains[1].presetId).not.toBe("speech-clear");
    expect(listenPreparing.value).toBe(false);
    const after = engine.log.slice(before);
    expect(after.some((c) => c.cmd === "open" || c.cmd === "close")).toBe(
      false,
    );
    expect(
      after.some((c) => c.cmd === "transport" && c.action.kind === "pause"),
    ).toBe(false);
    expect(listenPlaying.value).toBe(true);
  });

  it("hurtige profilbytter gir én kjede, og en forsinket eldre overstyrer ikke en nyere", async () => {
    await playingAfter();
    const slow = engine.holdChain();
    soundProfile.value = "mixed";
    await settle(PREPARE_DEBOUNCE_MS);
    expect(engine.chains()).toHaveLength(2);

    // Mens den treige måles, velger brukeren en tredje.
    const fast = engine.holdChain();
    soundProfile.value = "speech";
    await settle(PREPARE_DEBOUNCE_MS);
    expect(engine.chains()).toHaveLength(3);
    const sidesBefore = engine
      .actions()
      .filter((a) => a.kind === "side").length;

    fast(true);
    await settle();
    const sidesAfterFast = engine
      .actions()
      .filter((a) => a.kind === "side").length;
    expect(sidesAfterFast).toBe(sidesBefore + 1);

    slow(true); // den eldre blir ferdig etter den nyere
    await settle();
    expect(engine.actions().filter((a) => a.kind === "side").length).toBe(
      sidesAfterFast,
    );
  });

  it("«Ingen» stopper lyttingen: kortet er borte, og lyden med det", async () => {
    await playingAfter();
    soundProfile.value = "none";
    await settle(PREPARE_DEBOUNCE_MS);
    expect(listenPlaying.value).toBe(false);
    expect(engine.log.at(-1)).toEqual({ cmd: "close" });
  });
});

describe("spillehodet er det motoren sier", () => {
  it("følger Position, og en forsinket posisjon fra en eldre generasjon ignoreres", async () => {
    await playingAfter();
    expect(listenAt.value).toBe(1800); // der økta ble åpnet

    engine.emit({ kind: "position", gen: 1, programSecs: 5, sourceSecs: 1805 });
    expect(listenAt.value).toBe(1805);
    engine.emit({ kind: "position", gen: 2, programSecs: 9, sourceSecs: 1900 });
    expect(listenAt.value).toBe(1900);
    // Etter et hopp kommer det én siste fra før hoppet.
    engine.emit({ kind: "position", gen: 1, programSecs: 6, sourceSecs: 1806 });
    expect(listenAt.value).toBe(1900);
  });

  it("når programmet er spilt ut, stopper knappen, og neste spill begynner på nytt", async () => {
    await playingAfter();
    engine.emit({ kind: "ended", gen: 1 });
    expect(listenPlaying.value).toBe(false);

    await toggleListen();
    const actions = engine.actions();
    expect(actions.at(-2)).toEqual({ kind: "seekSource", secs: 1800 });
    expect(actions.at(-1)).toEqual({ kind: "play" });
  });

  it("en motorfeil stopper knappen og sier fra", async () => {
    await playingAfter();
    engine.emit({ kind: "error", message: "the output device failed" });
    expect(listenPlaying.value).toBe(false);
    expect(listenError.value).toBe(true);
  });

  it("pause pauser motoren; fortsett spiller der det stod", async () => {
    await playingAfter();
    await toggleListen();
    expect(listenPlaying.value).toBe(false);
    expect(engine.actions().at(-1)).toEqual({ kind: "pause" });
    await toggleListen();
    expect(engine.actions().at(-1)).toEqual({ kind: "play" });
    expect(engine.actions().some((a) => a.kind === "seekSource")).toBe(false);
  });
});

describe("økta", () => {
  it("åpnes av første spill og lukkes når steget forlates, og hendelsene slutter å komme", async () => {
    stop = watchListenPrepare();
    await settle();
    expect(engine.log.some((c) => c.cmd === "open")).toBe(false);
    await toggleListen();
    expect(engine.listeners()).toBe(1);

    stopListen();
    await settle();
    expect(engine.listeners()).toBe(0);
    expect(engine.log.at(-1)).toEqual({ cmd: "close" });
    expect(listenPlaying.value).toBe(false);
    expect(listenAt.value).toBeNull();
  });

  it("en åpning som mislykkes sier fra og spiller ikke", async () => {
    engine.failOpen();
    await toggleListen();
    expect(listenError.value).toBe(true);
    expect(listenPlaying.value).toBe(false);
    expect(engine.actions()).toEqual([]);
  });

  it("en kjede som ikke kan settes inn sier fra, og «Etter» spilles ikke", async () => {
    stop = watchListenPrepare();
    await settle();
    const release = engine.holdChain();
    const playing = toggleListen();
    await settle();
    release(false);
    await playing;
    expect(listenError.value).toBe(true);
    expect(listenPlaying.value).toBe(false);
  });

  it("forlates steget mens lydkortet åpnes, lukkes det som åpnet", async () => {
    stop = watchListenPrepare();
    await settle();
    const playing = toggleListen();
    stopListen(); // før åpningen er svart
    await playing;
    await settle();
    expect(listenPlaying.value).toBe(false);
    expect(engine.listeners()).toBe(0);
    // Den som åpnet etter at steget var forlatt, lukkes.
    expect(engine.log.filter((c) => c.cmd === "close").length).toBeGreaterThan(
      0,
    );
  });
});
