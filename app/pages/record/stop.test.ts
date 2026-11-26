/**
 * `beginStop` / `completeStop` — holdet kan vare lenge nok til at økta tok
 * slutt (eller ble erstattet) før sekundet var omme (audit 2026-10-05: «stopp
 * sjekker ikke tilstanden etter spørsmålet»). Samme vakter som dialogen hadde.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  endSessionLocally,
  enterFinalizing,
  finalizing,
  isRecording,
  markSessionStarted,
} from "../../state/recording";
import { beginStop, completeStop, stopNow } from "./stop";

let stops = 0;
let failStop = false;

beforeEach(() => {
  stops = 0;
  failStop = false;
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      stopRecordingNow: async () => {
        stops++;
        if (failStop) throw new Error("recorder is not running");
      },
    },
  };
  markSessionStarted();
});

afterEach(() => {
  endSessionLocally();
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("completeStop", () => {
  it("stopper når økta fortsatt er den samme", async () => {
    await completeStop(beginStop());
    expect(stops).toBe(1);
    // Skjermen sier «Fullfører» før svaret fra motoren.
    expect(finalizing.peek()).toBe(true);
  });

  it("stopper IKKE hvis opptaket tok slutt mens knappen ble holdt", async () => {
    const ticket = beginStop();
    endSessionLocally();
    await completeStop(ticket);
    expect(isRecording.peek()).toBe(false);
    expect(stops).toBe(0);
  });

  it("stopper IKKE et NYTT opptak som startet mens knappen ble holdt", async () => {
    const ticket = beginStop();
    endSessionLocally();
    // A different take: a later start timestamp.
    const realNow = Date.now;
    Date.now = () => realNow() + 5000;
    try {
      markSessionStarted();
    } finally {
      Date.now = realNow;
    }
    await completeStop(ticket);
    expect(stops).toBe(0);
  });

  it("et andre fullført hold mens motoren skriver ferdig er ingen ny forespørsel", async () => {
    const ticket = beginStop();
    await completeStop(ticket);
    await completeStop(ticket);
    expect(stops).toBe(1);
  });

  it("en feilet forespørsel rydder økta i stedet for å vente ut tidsavbruddet", async () => {
    failStop = true;
    await completeStop(beginStop());
    expect(stops).toBe(1);
    expect(isRecording.peek()).toBe(false);
  });
});

describe("beginStop", () => {
  it("sier nei mens motoren allerede skriver ferdig", () => {
    enterFinalizing();
    expect(beginStop()).toBeNull();
  });

  it("sier nei når det ikke går noe opptak", () => {
    endSessionLocally();
    expect(beginStop()).toBeNull();
  });
});

describe("stopNow (menylinja)", () => {
  it("stopper uten hold, gjennom de samme vaktene", async () => {
    await stopNow();
    expect(stops).toBe(1);
    await stopNow();
    expect(stops).toBe(1);
  });
});
