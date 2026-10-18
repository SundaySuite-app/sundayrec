/**
 * «Etter»-lyttingen sender det eksporten sender (M2R #3).
 *
 * Funnet: med mikseren på spilte «Etter» likevel `speech-clear` med «Tale»-rensing,
 * mens eksporten bar mikserkjeden og ingen preset. Og kanalreparasjonen og kuttene
 * nådde aldri forhåndslyttingen. Her leses det forhåndslyttingen faktisk sender
 * (`window.api.masterPreview`) mot det eksporten ville sendt for det samme valget.
 *
 * MUTASJONSPRØVEN: la `listenRequest` lese `specFor(profile).masterPreset` uten å
 * se på `useMixer` (slik det var), og «mikser på»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { E, resetFileState } from "./model";
import {
  channelRepair,
  listenRequest,
  listenSide,
  listenStart,
  mixer,
  mixerProcessing,
  resetSound,
  soundProfile,
  toggleListen,
  useMixer,
} from "./sound";
import { soundExportFields } from "./sound-profiles";

const TOKEN = "11111111-1111-4111-8111-111111111111";

let previewCalls: Array<{ token: string; listen: Record<string, unknown> }>;

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

beforeEach(() => {
  previewCalls = [];
  resetFileState();
  const g = globalThis as unknown as Record<string, unknown>;
  g.Audio = FakeAudio;
  g.window = {
    clearTimeout: () => {},
    setTimeout: () => 0,
    api: {
      // Kanalanalysen: venstre kanal er død.
      editorAutoProcess: () =>
        Promise.resolve({
          diagnosis: {
            code: "dead_left",
            recommended: { mode: "duplicateRight", leftDb: 0, rightDb: 0 },
          },
        }),
      editorTaleSaid: () => Promise.resolve(null),
      toAssetUrl: (p: string) => p,
      masterPreview: (token: string, listen: Record<string, unknown>) => {
        previewCalls.push({ token, listen });
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
});

afterEach(() => {
  resetSound();
  resetFileState();
  const g = globalThis as unknown as Record<string, unknown>;
  delete g.window;
  delete g.Audio;
});

describe("«Etter» sender det eksporten sender", () => {
  it("mikseren på: mikserkjeden og INGEN preset, som eksporten", async () => {
    soundProfile.value = "speech";
    useMixer.value = true;
    mixer.value = { ...mixer.value, denoiseEnabled: true };

    await toggleListen();

    expect(previewCalls).toHaveLength(1);
    const sent = previewCalls[0].listen;
    const exported = soundExportFields({
      profile: "speech",
      useMixer: true,
      processing: mixerProcessing(),
      repair: channelRepair.value,
    });
    expect(sent.presetId).toBeUndefined();
    expect(exported.masterPreset).toBeUndefined();
    expect(sent.processing).toEqual(exported.processing);
    expect(sent.processing).toMatchObject({ denoiseEnabled: true });
  });

  it("uten mikser: profilens preset, kanalreparasjonen, kuttene og lengden", async () => {
    soundProfile.value = "speech";
    useMixer.value = false;

    await toggleListen();

    const sent = previewCalls[0].listen;
    expect(sent.presetId).toBe("speech-clear");
    expect(sent.processing).toBeUndefined();
    expect(sent.channelRepair).toEqual({
      mode: "duplicateRight",
      leftDb: 0,
      rightDb: 0,
    });
    expect(sent.cutRegions).toEqual([{ start: 10, end: 20 }]);
    expect(sent.sourceDurationSec).toBe(3600);
    expect(sent.startSec).toBe(1800);
  });

  it("samme forespørsel gjengis ikke to ganger, men en endret mikser gjør det", async () => {
    soundProfile.value = "speech";
    useMixer.value = true;
    await toggleListen();
    await toggleListen(); // stopper
    await toggleListen(); // samme valg: fra hurtigbufferen
    expect(previewCalls).toHaveLength(1);

    await toggleListen(); // stopper
    mixer.value = { ...mixer.value, gateEnabled: true };
    await toggleListen();
    expect(previewCalls).toHaveLength(2);
  });

  it("«Ingen» gir ingenting å gjengi", () => {
    soundProfile.value = "none";
    expect(listenRequest(0)).toBeNull();
  });
});
