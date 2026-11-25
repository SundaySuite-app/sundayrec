/**
 * «Etter»-lyttingen sender det eksporten sender (M2R #3), nå til avspillingsmotoren (B5b).
 *
 * Her leses det lyttingen faktisk sender (`window.api.editorListenChain`) mot det eksporten
 * ville sendt for det samme valget: bryteren «Podkastklar lyd», kuttene og lengden.
 *
 * MUTASJONSPRØVEN: la `listenRequest` ignorere `podcastAudio` og «av»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { E, resetFileState } from "./model";
import {
  listenRequest,
  listenSide,
  listenStart,
  podcastAudio,
  resetSound,
  toggleListen,
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
  it("på: oppskriften, kuttene og lengden, som eksporten", async () => {
    podcastAudio.value = true;

    await toggleListen();

    expect(engine.chains()).toHaveLength(1);
    const sent = engine.chains()[0];
    expect(sent.podcastAudio).toBe(soundExportFields(true).podcastAudio);
    expect(sent.podcastAudio).toBe(true);
    expect(sent.cutRegions).toEqual([{ start: 10, end: 20 }]);
    expect(sent.fullLengthSec).toBe(3600);
  });

  it("økta åpnes med kuttene og stedet man begynner, og ingen gjengivelse bestilles", async () => {
    podcastAudio.value = true;
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

  it("samme valg settes ikke inn to ganger", async () => {
    podcastAudio.value = true;
    await toggleListen(); // spiller
    await toggleListen(); // pause
    await toggleListen(); // spiller igjen: kjeden er allerede inne
    expect(engine.chains()).toHaveLength(1);
  });

  it("av gir ingenting å høre", () => {
    podcastAudio.value = false;
    expect(listenRequest(0)).toBeNull();
  });
});
