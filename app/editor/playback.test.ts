/**
 * Avspillingen i Klipp-steget på motoren (B5c): knappene er meldinger, spillehodet er det
 * motoren sier, og kuttene er en del av programmet den spiller.
 *
 * Motoren er `fakeListen()`: det den ble bedt om, i rekkefølge, og det den ville sagt tilbake.
 *
 * MUTASJONSPRØVEN: la `onEngineEvent` hoppe over `position`-grenen, og «spillehodet følger
 * motoren»-testen blir rød. Ta `cancelCutsTimer()` ut av `watchCuts`, og «nye kutt samles»-
 * testen får to `cuts`-meldinger. La `startPlay` sende `play` uten å sjekke `gen`, og «stopp
 * mens lydkortet åpner»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  cuts,
  E,
  playbackSource,
  playheadSec,
  playing,
  resetFileState,
} from "./model";
import {
  CUTS_DEBOUNCE_MS,
  seekTo,
  startPlay,
  stopPlay,
  teardownPlayback,
  togglePlay,
} from "./playback";
import { resetSound } from "./sound";
import { fakeListen, type FakeListen } from "./test-support/fake-listen-api";

const TOKEN = "44444444-4444-4444-8444-444444444444";

let engine: FakeListen;

const settle = async (ms = 0): Promise<void> => {
  await vi.advanceTimersByTimeAsync(ms);
};

const position = (sourceSecs: number, gen = 1) =>
  engine.emit({ kind: "position", gen, programSecs: sourceSecs, sourceSecs });

const kinds = (): string[] => engine.actions().map((a) => String(a.kind));

function setCuts(list: Array<{ start: number; end: number }>): void {
  E.cuts = list as typeof E.cuts;
  cuts.value = list.slice() as typeof cuts.value;
}

beforeEach(() => {
  vi.useFakeTimers();
  engine = fakeListen();
  resetFileState();
  const g = globalThis as unknown as Record<string, unknown>;
  g.requestAnimationFrame = () => 0;
  g.cancelAnimationFrame = () => {};
  g.performance = { now: () => Date.now() };
  g.window = {
    setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms),
    clearTimeout: (id: number) => clearTimeout(id),
    api: { ...engine.api },
  };
  resetSound();
  E.sourceToken = TOKEN;
  E.filePath = "/Opptak/2026-08-23.flac";
  E.duration = 3600;
});

afterEach(() => {
  teardownPlayback();
  resetSound();
  vi.useRealTimers();
  resetFileState();
  cuts.value = [];
  delete (globalThis as unknown as Record<string, unknown>).window;
});

describe("spill og pause", () => {
  it("åpner motoren der spillehodet står, med kuttene, og sier spill", async () => {
    setCuts([{ start: 10, end: 20 }]);
    E.playStartSec = 125;
    startPlay();
    // Knappen svarer med én gang: det er med vilje.
    expect(playing.value).toBe(true);
    await settle();

    const open = engine.log.find((c) => c.cmd === "open");
    expect(open).toMatchObject({
      token: TOKEN,
      listen: {
        startSec: 125,
        cutRegions: [{ start: 10, end: 20 }],
        fullLengthSec: 3600,
      },
    });
    expect(kinds()).toEqual(["play"]);
    expect(playbackSource.value).toBe("original");
  });

  it("spillehodet følger motoren, og ingenting annet", async () => {
    E.playStartSec = 100;
    startPlay();
    await settle();
    position(100.5);
    expect(E.playStartSec).toBe(100.5);
    await settle(100);
    position(131.25);
    expect(E.playStartSec).toBe(131.25);
    expect(playheadSec.value).toBe(131.25);
    // Veggklokka flytter ikke noe: lydkortet er klokka.
    await settle(5000);
    expect(E.playStartSec).toBe(131.25);
  });

  it("pause lar spillehodet stå der motoren sist var, og holder økta åpen", async () => {
    startPlay();
    await settle();
    position(42);
    stopPlay();
    expect(playing.value).toBe(false);
    expect(E.playStartSec).toBe(42);
    expect(kinds()).toEqual(["play", "pause"]);
    expect(engine.log.some((c) => c.cmd === "close")).toBe(false);
    // En posisjon som kommer etter pausen flytter ingenting.
    position(43);
    expect(E.playStartSec).toBe(42);
  });

  it("andre trykk fortsetter uten å åpne på nytt", async () => {
    startPlay();
    await settle();
    position(42);
    togglePlay();
    togglePlay();
    await settle();
    expect(engine.log.filter((c) => c.cmd === "open")).toHaveLength(1);
    expect(kinds()).toEqual(["play", "pause", "play"]);
  });

  it("stopp mens lydkortet åpner: ingen «spill» sendes", async () => {
    startPlay();
    stopPlay();
    await settle();
    expect(kinds()).not.toContain("play");
  });

  it("en åpning som mislykkes sier det, og neste trykk prøver igjen", async () => {
    engine.failOpen();
    startPlay();
    await settle();
    expect(playing.value).toBe(false);
    expect(playbackSource.value).toBe("none");
    expect(kinds()).not.toContain("play");
    // Knappen er ikke sperret: et nytt forsøk er ett trykk.
    startPlay();
    await settle();
    expect(engine.log.filter((c) => c.cmd === "open")).toHaveLength(2);
  });

  it("programmet spilt ut: spillehodet står ved slutten, og neste spill begynner på nytt", async () => {
    E.playStartSec = 3590;
    startPlay();
    await settle();
    position(3599.9, 1);
    engine.emit({ kind: "ended", gen: 1 });
    expect(playing.value).toBe(false);
    expect(E.playStartSec).toBe(3600);

    startPlay();
    await settle();
    // Åpen økta flyttes til begynnelsen før den spiller.
    expect(engine.actions().at(-2)).toEqual({ kind: "seekSource", secs: 0 });
    expect(engine.actions().at(-1)).toEqual({ kind: "play" });
  });

  it("et «ferdig» fra en eldre generasjon stopper ikke det nye programmet", async () => {
    startPlay();
    await settle();
    position(10, 2);
    engine.emit({ kind: "ended", gen: 1 });
    expect(playing.value).toBe(true);
  });

  it("en feil fra motoren stopper og sier det", async () => {
    startPlay();
    await settle();
    engine.emit({ kind: "error", message: "the output device failed" });
    expect(playing.value).toBe(false);
    expect(playbackSource.value).toBe("none");
  });

  it("et opptak som starter stopper avspillingen: spillehodet blir stående (M4R #8)", async () => {
    startPlay();
    await settle();
    position(42);
    engine.emit({ kind: "stopped" });
    expect(playing.value).toBe(false);
    expect(E.isPlaying).toBe(false);
    expect(E.playStartSec).toBeCloseTo(42, 3);
    // Ikke en feil: motoren er ikke «borte», bare lukket for opptaket.
    expect(playbackSource.value).not.toBe("none");
  });

  it("uten fil gjør spill ingenting", async () => {
    E.sourceToken = "";
    startPlay();
    await settle();
    expect(playing.value).toBe(false);
    expect(engine.log).toHaveLength(0);
  });
});

describe("hopp", () => {
  it("et klikk mens det spiller stopper, flytter spillehodet og flytter motoren", async () => {
    startPlay();
    await settle();
    position(50);
    seekTo(900);
    expect(playing.value).toBe(false);
    expect(E.playStartSec).toBe(900);
    expect(engine.actions().slice(-2)).toEqual([
      { kind: "pause" },
      { kind: "seekSource", secs: 900 },
    ]);
  });

  it("et klikk inne i et kutt lander etter det", async () => {
    setCuts([{ start: 1000, end: 1100 }]);
    startPlay();
    await settle();
    seekTo(1050);
    expect(E.playStartSec).toBe(1100);
    expect(engine.actions().at(-1)).toEqual({ kind: "seekSource", secs: 1100 });
  });

  it("uten åpen økte flyttes bare spillehodet; økta åpnes der det står ved spill", async () => {
    seekTo(700);
    expect(engine.log).toHaveLength(0);
    startPlay();
    await settle();
    expect(engine.log.find((c) => c.cmd === "open")).toMatchObject({
      listen: { startSec: 700 },
    });
  });
});

describe("nye kutt", () => {
  it("mens det spiller: motoren får kuttene og fortsetter fra samme sted", async () => {
    startPlay();
    await settle();
    position(300);
    setCuts([{ start: 400, end: 500 }]);
    await settle(CUTS_DEBOUNCE_MS);
    expect(engine.actions().at(-1)).toEqual({
      kind: "cuts",
      cutRegions: [{ start: 400, end: 500 }],
      fullLengthSec: 3600,
      atSecs: 300,
    });
    expect(playing.value).toBe(true);
  });

  it("endringer samles: tre raske angre gir én melding", async () => {
    startPlay();
    await settle();
    position(300);
    setCuts([{ start: 400, end: 500 }]);
    await settle(CUTS_DEBOUNCE_MS / 2);
    setCuts([]);
    await settle(CUTS_DEBOUNCE_MS / 2);
    setCuts([{ start: 410, end: 500 }]);
    await settle(CUTS_DEBOUNCE_MS);
    expect(kinds().filter((k) => k === "cuts")).toHaveLength(1);
    expect(engine.actions().at(-1)).toMatchObject({
      cutRegions: [{ start: 410, end: 500 }],
    });
  });

  it("pauset: et kutt over spillehodet flytter det ut av kuttet", async () => {
    startPlay();
    await settle();
    position(450);
    stopPlay();
    setCuts([{ start: 400, end: 500 }]);
    await settle(CUTS_DEBOUNCE_MS);
    expect(E.playStartSec).toBe(500);
    expect(engine.actions().at(-1)).toMatchObject({
      kind: "cuts",
      atSecs: 500,
    });
  });

  it("alt klippet bort mens det spiller: stopp", async () => {
    startPlay();
    await settle();
    position(10);
    setCuts([{ start: 0, end: 3600 }]);
    await settle(CUTS_DEBOUNCE_MS);
    expect(playing.value).toBe(false);
    expect(kinds()).not.toContain("cuts");
  });

  it("uten åpen økte sendes ingenting", async () => {
    setCuts([{ start: 400, end: 500 }]);
    await settle(CUTS_DEBOUNCE_MS);
    expect(engine.log).toHaveLength(0);
  });
});

describe("lukking", () => {
  it("å forlate steget stopper og slipper lydkortet", async () => {
    startPlay();
    await settle();
    teardownPlayback();
    await settle();
    expect(kinds()).toContain("pause");
    expect(engine.log.at(-1)).toEqual({ cmd: "close" });
    expect(engine.listeners()).toBe(0);
  });
});
