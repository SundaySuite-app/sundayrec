import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FRAME_POLL_MS, startFramePoll } from "./frame-poll-core";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

/** La mikrotasks (promise-kjeden inne i et tikk) få kjøre ferdig. */
async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

describe("startFramePoll", () => {
  it("henter en frame per tikk og gir den videre", async () => {
    const onFrame = vi.fn();
    const stop = startFramePoll({
      fetchFrame: () => Promise.resolve("AAAA"),
      onFrame,
    });
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS * 3);
    stop();
    expect(onFrame).toHaveBeenCalledTimes(3);
    expect(onFrame).toHaveBeenCalledWith("AAAA");
  });

  it("⚠️ IN-FLIGHT-VAKTEN: en treg bakende gir ikke en kø av kall", async () => {
    // Dette er testen som blir RØD hvis `if (busy) return` fjernes fra
    // `frame-poll-core.ts`. Uten den ville et kall som henger i ti tikk gitt
    // ti overlappende IPC-rundturer, alle om å skrive den samme `img.src`.
    let inFlight = 0;
    let peak = 0;
    let release: (() => void) | null = null;
    const stop = startFramePoll({
      fetchFrame: () =>
        new Promise<string | null>((resolve) => {
          inFlight++;
          peak = Math.max(peak, inFlight);
          release = () => {
            inFlight--;
            resolve("AAAA");
          };
        }),
      onFrame: () => {},
    });

    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS * 10);
    expect(peak).toBe(1);

    // Og når den endelig svarer, tar løkka opp igjen.
    release!();
    await settle();
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS);
    expect(peak).toBe(1);
    expect(inFlight).toBe(1);
    stop();
  });

  it("hopper over tomme svar i stedet for å male et tomt bilde", async () => {
    const onFrame = vi.fn();
    const stop = startFramePoll({
      fetchFrame: () => Promise.resolve(null),
      onFrame,
    });
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS * 4);
    stop();
    expect(onFrame).not.toHaveBeenCalled();
  });

  it("en feilet lesning er stille, og løkka lever videre", async () => {
    const onFrame = vi.fn();
    let n = 0;
    const stop = startFramePoll({
      fetchFrame: () =>
        ++n === 1
          ? Promise.reject(new Error("IPC nede"))
          : Promise.resolve("A"),
      onFrame,
    });
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS * 3);
    stop();
    expect(onFrame).toHaveBeenCalledTimes(2);
  });

  it("stopper for godt, og tåler å bli stoppet to ganger", async () => {
    const onFrame = vi.fn();
    const fetchFrame = vi.fn(() => Promise.resolve("A"));
    const stop = startFramePoll({ fetchFrame, onFrame });
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS);
    stop();
    stop();
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS * 5);
    expect(fetchFrame).toHaveBeenCalledTimes(1);
  });

  it("en frame som lander ETTER stopp males ikke", async () => {
    // Overlegget rives ned i det opptaket slutter; en frame fra en lesning som
    // allerede var i lufta skal ikke treffe et element ingen ser.
    const onFrame = vi.fn();
    let release: ((v: string) => void) | null = null;
    const stop = startFramePoll({
      fetchFrame: () =>
        new Promise<string | null>((resolve) => {
          release = resolve;
        }),
      onFrame,
    });
    await vi.advanceTimersByTimeAsync(FRAME_POLL_MS);
    stop();
    release!("AAAA");
    await settle();
    expect(onFrame).not.toHaveBeenCalled();
  });
});
