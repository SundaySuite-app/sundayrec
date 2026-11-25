/**
 * Varselet om en død kanal: koden fra Rust blir en side, ukjente koder blir ingenting, og et
 * svar som kommer etter et filbytte kastes.
 *
 * MUTASJONSPRØVEN: ta `E.sourceToken === token`-vakten ut av `checkChannels`, og
 * «filbytte»-testen blir rød.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  checkChannels,
  deadChannel,
  deadChannelKey,
  resetChannelCheck,
} from "./channel-check";
import { E, resetFileState } from "./model";

const TOKEN = "44444444-4444-4444-8444-444444444444";

function api(answer: () => Promise<unknown>): void {
  (globalThis as unknown as Record<string, unknown>).window = {
    api: { editorChannelCheck: answer },
  };
}

beforeEach(() => {
  resetFileState();
  resetChannelCheck();
  E.sourceToken = TOKEN;
});

afterEach(() => {
  resetFileState();
  resetChannelCheck();
  delete (globalThis as unknown as Record<string, unknown>).window;
});

describe("deadChannelKey", () => {
  it("kjenner left og right, og ingenting annet", () => {
    expect(deadChannelKey("left")).toBe("left");
    expect(deadChannelKey("right")).toBe("right");
    expect(deadChannelKey("both")).toBeNull();
    expect(deadChannelKey(null)).toBeNull();
    expect(deadChannelKey(undefined)).toBeNull();
  });
});

describe("checkChannels", () => {
  it("setter siden Rust navnga", async () => {
    api(() => Promise.resolve({ deadSide: "right" }));
    await checkChannels(TOKEN);
    expect(deadChannel.value).toBe("right");
  });

  it("et friskt opptak, null og en feil sier ingenting", async () => {
    api(() => Promise.resolve({ deadSide: null }));
    await checkChannels(TOKEN);
    expect(deadChannel.value).toBeNull();
    api(() => Promise.resolve(null));
    await checkChannels(TOKEN);
    expect(deadChannel.value).toBeNull();
    api(() => Promise.reject(new Error("nope")));
    await checkChannels(TOKEN);
    expect(deadChannel.value).toBeNull();
  });

  it("et svar som kommer etter et filbytte gjelder ikke lenger", async () => {
    let answer!: (v: unknown) => void;
    api(() => new Promise((r) => (answer = r)));
    const run = checkChannels(TOKEN);
    E.sourceToken = "55555555-5555-4555-8555-555555555555";
    answer({ deadSide: "left" });
    await run;
    expect(deadChannel.value).toBeNull();
  });
});
