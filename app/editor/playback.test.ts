/**
 * Spillehodet som frøs.
 *
 * `startPlay` melder «spiller» med én gang (knappen skal svare på klikket) og
 * venter på `canplay` for å faktisk starte transporten. Kommer det eventet
 * aldri, kom heller ingenting annet: elementet ble aldri startet, telleren sto
 * stille på `playStartSec`, og rAF-løkka malte det samme tallet for alltid
 * under en knapp som sa «pause».
 *
 * Node-miljø: `<audio>`-elementet er en stubb, og det er nettopp POENGET —
 * stubben er den som aldri fyrer `canplay`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PLAY_READY_TIMEOUT_MS, startPlay, stopPlay } from "./playback";
import { E, playbackSource, playing, resetFileState } from "./model";

/** Et medieelement som aldri blir klart, med mindre testen sier fra. */
function stubEl(): HTMLAudioElement & { fire: (type: string) => void } {
  const listeners = new Map<string, Array<() => void>>();
  const el = {
    readyState: 0,
    volume: 1,
    currentTime: 0,
    getAttribute: (name: string) =>
      name === "src" ? "asset://localhost/opptak.flac" : null,
    removeAttribute: () => {},
    addEventListener: (type: string, fn: () => void) =>
      listeners.set(type, [...(listeners.get(type) ?? []), fn]),
    removeEventListener: (type: string, fn: () => void) =>
      listeners.set(
        type,
        (listeners.get(type) ?? []).filter((f) => f !== fn),
      ),
    play: () => Promise.resolve(),
    pause: () => {},
    load: () => {},
    fire: (type: string) =>
      (listeners.get(type) ?? []).slice().forEach((f) => f()),
  };
  return el as unknown as HTMLAudioElement & { fire: (type: string) => void };
}

beforeEach(() => {
  vi.useFakeTimers();
  (
    globalThis as unknown as { requestAnimationFrame: unknown }
  ).requestAnimationFrame = () => 0;
  (
    globalThis as unknown as { cancelAnimationFrame: unknown }
  ).cancelAnimationFrame = () => {};
  (globalThis as unknown as { performance: unknown }).performance = {
    now: () => 0,
  };
  resetFileState();
  E.duration = 3600;
});

afterEach(() => {
  stopPlay();
  vi.useRealTimers();
  resetFileState();
});

describe("startPlay-vakten", () => {
  // MUTASJONSPRØVEN: fjern `readyTimer`-blokka i `startPlay`, og denne blir
  // rød — «spiller» blir stående for alltid, som den gjorde.
  it("gir opp når `canplay` aldri kommer, i stedet for å påstå at det spilles", () => {
    E.playerEl = stubEl();
    startPlay();
    // Knappen svarer med én gang: det er med vilje.
    expect(playing.value).toBe(true);

    vi.advanceTimersByTime(PLAY_READY_TIMEOUT_MS - 1);
    expect(playing.value).toBe(true);

    vi.advanceTimersByTime(1);
    expect(playing.value).toBe(false);
    expect(E.isPlaying).toBe(false);
    expect(E.mainPlayPending).toBe(false);
    // …og skjermen sier det, med setningen den allerede har for nettopp dette:
    // banneret og den sperrede play-knappen henger på `playbackSource`.
    expect(playbackSource.value).toBe("none");
  });

  it("rører ingenting når `canplay` FAKTISK kommer", () => {
    const el = stubEl();
    E.playerEl = el;
    startPlay();
    el.fire("canplay");
    expect(E.mainPlayPending).toBe(false);

    vi.advanceTimersByTime(PLAY_READY_TIMEOUT_MS * 2);
    expect(playing.value).toBe(true);
    expect(playbackSource.value).toBe("original");
  });

  it("en vakt fra en AVLØST avspilling river ikke den nye ned", () => {
    // Stopp og play innenfor samme sekund er den vanlige måten dette blir
    // subtilt galt på: den gamle timeren løper fortsatt.
    const el = stubEl();
    E.playerEl = el;
    startPlay();
    stopPlay();
    startPlay();
    el.fire("canplay");

    vi.advanceTimersByTime(PLAY_READY_TIMEOUT_MS * 2);
    expect(playing.value).toBe(true);
    expect(playbackSource.value).toBe("original");
  });

  it("uten kilde later den ikke som — knappen er sperret med en grunn over", () => {
    E.playerEl = null;
    startPlay();
    expect(playing.value).toBe(false);
  });
});
