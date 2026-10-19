/**
 * «Etter»-lyttingen sender det eksporten sender (M2R #3), nå til avspillingsmotoren (B5b).
 *
 * Funnet: med mikseren på spilte «Etter» likevel `speech-clear` med «Tale»-rensing,
 * mens eksporten bar mikserkjeden og ingen preset. Og kanalreparasjonen og kuttene
 * nådde aldri forhåndslyttingen. Her leses det lyttingen faktisk sender
 * (`window.api.editorListenChain`) mot det eksporten ville sendt for det samme valget.
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
import { fakeListen, type FakeListen } from "./test-support/fake-listen-api";

const TOKEN = "11111111-1111-4111-8111-111111111111";

let engine: FakeListen;

beforeEach(() => {
  engine = fakeListen();
  resetFileState();
  const g = globalThis as unknown as Record<string, unknown>;
  g.window = {
    clearTimeout: () => {},
    setTimeout: () => 0,
    api: {
      ...engine.api,
      // Kanalanalysen: venstre kanal er død.
      editorAutoProcess: () =>
        Promise.resolve({
          diagnosis: {
            code: "dead_left",
            recommended: { mode: "duplicateRight", leftDb: 0, rightDb: 0 },
          },
        }),
      editorTaleSaid: () => Promise.resolve(null),
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
});

describe("«Etter» sender det eksporten sender", () => {
  it("mikseren på: mikserkjeden og INGEN preset, som eksporten", async () => {
    soundProfile.value = "speech";
    useMixer.value = true;
    mixer.value = { ...mixer.value, denoiseEnabled: true };

    await toggleListen();

    expect(engine.chains()).toHaveLength(1);
    const sent = engine.chains()[0];
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

    const sent = engine.chains()[0];
    expect(sent.presetId).toBe("speech-clear");
    expect(sent.processing).toBeUndefined();
    expect(sent.channelRepair).toEqual({
      mode: "duplicateRight",
      leftDb: 0,
      rightDb: 0,
    });
    expect(sent.cutRegions).toEqual([{ start: 10, end: 20 }]);
    expect(sent.fullLengthSec).toBe(3600);
  });

  it("økta åpnes med kuttene og stedet man begynner, og ingen gjengivelse bestilles", async () => {
    soundProfile.value = "speech";
    await toggleListen();

    const open = engine.log.find((c) => c.cmd === "open");
    expect(open).toMatchObject({
      cmd: "open",
      token: TOKEN,
      listen: {
        startSec: 1800,
        cutRegions: [{ start: 10, end: 20 }],
        fullLengthSec: 3600,
      },
    });
    // Ingen `masterPreview`: det finnes ikke engang på api-et i denne testen.
    expect(
      (globalThis as unknown as { window: { api: Record<string, unknown> } })
        .window.api.masterPreview,
    ).toBeUndefined();
  });

  it("samme valg settes ikke inn to ganger, men en endret mikser gjør det", async () => {
    soundProfile.value = "speech";
    useMixer.value = true;
    await toggleListen(); // spiller
    await toggleListen(); // pause
    await toggleListen(); // spiller igjen: kjeden er allerede inne
    expect(engine.chains()).toHaveLength(1);

    await toggleListen(); // pause
    mixer.value = { ...mixer.value, gateEnabled: true };
    await toggleListen();
    expect(engine.chains()).toHaveLength(2);
  });

  it("«Ingen» gir ingenting å høre", () => {
    soundProfile.value = "none";
    expect(listenRequest(0)).toBeNull();
  });
});
