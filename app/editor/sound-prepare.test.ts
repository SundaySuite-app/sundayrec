/**
 * Lyttingen gjøres klar i bakgrunnen (M2R #3, oppfølger).
 *
 * Første «Etter» venter ellers på et helt kjedepass over programmet. Lyd-steget ber derfor
 * bakenden måle programmet med en gang det åpnes, og på nytt (med ventetid) når valget eller
 * kuttene endres — og aldri under et opptak.
 *
 * MUTASJONSPRØVEN: ta `isRecording.peek()` ut av `prepareNow`, og «under et opptak»-testen
 * blir rød; ta ventetiden ut av `watchListenPrepare` (første argument alltid 0), og
 * «hurtige endringer»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { isRecording } from "../state/recording";
import { E, resetFileState } from "./model";
import {
  PREPARE_DEBOUNCE_MS,
  listenBusy,
  listenPreparing,
  listenSide,
  listenStart,
  mixer,
  resetSound,
  soundProfile,
  toggleListen,
  useMixer,
  watchListenPrepare,
} from "./sound";

const TOKEN = "22222222-2222-4222-8222-222222222222";

type Req = Record<string, unknown>;
let prepareCalls: Req[];
let previewCalls: Req[];
/** Når satt, venter `prepareListen` på at testen åpner porten. */
let gate: Promise<boolean> | null;
let stop: (() => void) | null;

class FakeAudio {
  src = "";
  readyState = 4;
  currentTime = 0;
  preload = "";
  addEventListener(): void {}
  removeAttribute(): void {}
  load(): void {}
  pause(): void {}
  play(): Promise<void> {
    return Promise.resolve();
  }
}

/** La tidtakerne og lovnadene kjøre ferdig; `ms` er fakeklokka, ikke veggklokka. */
const settle = async (ms = 0): Promise<void> => {
  await vi.advanceTimersByTimeAsync(ms);
};

beforeEach(() => {
  vi.useFakeTimers();
  prepareCalls = [];
  previewCalls = [];
  gate = null;
  stop = null;
  resetFileState();
  isRecording.value = false;
  const g = globalThis as unknown as Record<string, unknown>;
  g.Audio = FakeAudio;
  g.window = {
    setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms),
    clearTimeout: (id: number) => clearTimeout(id),
    api: {
      editorAutoProcess: () => Promise.resolve({ diagnosis: null }),
      editorTaleSaid: () => Promise.resolve(null),
      toAssetUrl: (p: string) => p,
      prepareListen: (_token: string, listen: Req) => {
        prepareCalls.push(listen);
        return gate ?? Promise.resolve(true);
      },
      masterPreview: (_token: string, listen: Req) => {
        previewCalls.push(listen);
        return Promise.resolve({ ok: true, previewPath: "/tmp/p.mp3" });
      },
    },
  };
  resetSound();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.sourceToken = TOKEN;
  E.duration = 3600;
  E.cuts = [{ start: 10, end: 20 }] as typeof E.cuts;
  listenSide.value = "after";
  listenStart.value = 1800;
  soundProfile.value = "speech";
  useMixer.value = false;
});

afterEach(() => {
  stop?.();
  resetSound();
  resetFileState();
  isRecording.value = false;
  vi.useRealTimers();
  const g = globalThis as unknown as Record<string, unknown>;
  delete g.window;
  delete g.Audio;
});

describe("lyttingen gjøres klar i bakgrunnen", () => {
  it("ber om forberedelse når steget åpnes, med forespørselen «Etter» sender", async () => {
    stop = watchListenPrepare();
    await settle();

    expect(prepareCalls).toHaveLength(1);
    expect(prepareCalls[0]).toMatchObject({
      presetId: "speech-clear",
      cutRegions: [{ start: 10, end: 20 }],
      fullLengthSec: 3600,
    });
  });

  it("hurtige endringer gir ÉN forberedelse, etter ventetiden", async () => {
    stop = watchListenPrepare();
    await settle();
    expect(prepareCalls).toHaveLength(1);

    useMixer.value = true;
    await settle(PREPARE_DEBOUNCE_MS / 4);
    mixer.value = { ...mixer.value, denoiseEnabled: true };
    await settle(PREPARE_DEBOUNCE_MS / 4);
    mixer.value = { ...mixer.value, gateEnabled: true };
    await settle(PREPARE_DEBOUNCE_MS - 1);
    expect(prepareCalls).toHaveLength(1); // ennå ikke: ventetiden gjelder siste endring

    await settle(1);
    expect(prepareCalls).toHaveLength(2);
    expect(prepareCalls[1].processing).toMatchObject({
      denoiseEnabled: true,
      gateEnabled: true,
    });
    expect(prepareCalls[1].presetId).toBeUndefined();
  });

  it("nye kutt gir ny forberedelse", async () => {
    const { cuts } = await import("./model");
    stop = watchListenPrepare();
    await settle();

    E.cuts = [{ start: 30, end: 90 }] as typeof E.cuts;
    cuts.value = E.cuts.slice();
    await settle(PREPARE_DEBOUNCE_MS);

    expect(prepareCalls).toHaveLength(2);
    expect(prepareCalls[1].cutRegions).toEqual([{ start: 30, end: 90 }]);
  });

  it("under et opptak spørres det ikke — og det tas igjen når opptaket er over", async () => {
    isRecording.value = true;
    stop = watchListenPrepare();
    await settle(PREPARE_DEBOUNCE_MS * 2);
    expect(prepareCalls).toHaveLength(0);

    useMixer.value = true; // en endring under opptaket
    await settle(PREPARE_DEBOUNCE_MS * 2);
    expect(prepareCalls).toHaveLength(0);

    isRecording.value = false;
    await settle(PREPARE_DEBOUNCE_MS);
    expect(prepareCalls).toHaveLength(1);
  });

  it("et program som er målt eller måles spørres ikke om igjen", async () => {
    gate = new Promise(() => {}); // måler fortsatt
    stop = watchListenPrepare();
    await settle();
    // Et annet utsnitt og en ny planlegging av samme program: samme måling.
    listenStart.value = 2400;
    soundProfile.value = "speech";
    useMixer.value = false;
    await settle(PREPARE_DEBOUNCE_MS * 2);
    expect(prepareCalls).toHaveLength(1);
  });

  it("«Ingen» har ikke noe å gjøre klart", async () => {
    soundProfile.value = "none";
    stop = watchListenPrepare();
    await settle(PREPARE_DEBOUNCE_MS * 2);
    expect(prepareCalls).toHaveLength(0);
  });

  it("«Etter» venter på målingen som pågår, og sier hva den venter på", async () => {
    let open!: (ready: boolean) => void;
    gate = new Promise<boolean>((r) => (open = r));
    stop = watchListenPrepare();
    await settle();

    const playing = toggleListen();
    await settle();
    expect(listenBusy.value).toBe(true);
    expect(listenPreparing.value).toBe(true);
    expect(previewCalls).toHaveLength(0); // venter, ber ikke om en gjengivelse ved siden av

    open(true);
    await playing;
    expect(previewCalls).toHaveLength(1);
    expect(listenBusy.value).toBe(false);
    expect(listenPreparing.value).toBe(false);
  });

  it("er programmet allerede målt, sier «Etter» ikke at den gjør klar", async () => {
    stop = watchListenPrepare();
    await settle(); // forberedelsen svarer «klar» med en gang

    const seen: boolean[] = [];
    (
      globalThis as unknown as { window: { api: { masterPreview: unknown } } }
    ).window.api.masterPreview = (_t: string, listen: Req) => {
      seen.push(listenPreparing.value);
      previewCalls.push(listen);
      return Promise.resolve({ ok: true, previewPath: "/tmp/p.mp3" });
    };
    await toggleListen();
    expect(previewCalls).toHaveLength(1);
    expect(seen).toEqual([false]);
  });
});
