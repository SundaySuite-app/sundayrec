import { describe, expect, it } from "vitest";

import {
  createCameraFeed,
  type FeedApi,
  type FeedState,
  type FeedSubscriber,
  type FeedTimers,
  type StartResult,
} from "./camera-feed-core";
import { classifyStartError } from "./camera-preview-core";

/** Falske timere: ingenting skjer før testen sier det. */
function fakeTimers() {
  let next = 1;
  const pending = new Map<number, () => void>();
  const timers: FeedTimers = {
    set: (fn) => {
      pending.set(next, fn);
      return next++;
    },
    clear: (h) => void pending.delete(h as number),
  };
  return {
    timers,
    fire: () => {
      const fns = [...pending.values()];
      pending.clear();
      fns.forEach((f) => f());
    },
    count: () => pending.size,
  };
}

/** En bakende som skriver opp hva den ble bedt om. */
function fakeApi(result: StartResult = { ok: true }) {
  const log: string[] = [];
  const channels: Array<(jpeg: ArrayBuffer) => void> = [];
  const api: FeedApi = {
    start: async (token, onFrame) => {
      log.push(`start ${token}`);
      channels.push(onFrame);
      return result;
    },
    stop: async () => {
      log.push("stop");
    },
  };
  return { api, log, channels };
}

function sub() {
  const frames: number[] = [];
  const states: FeedState[] = [];
  const s: FeedSubscriber = {
    onFrame: (b) => frames.push(b.byteLength),
    onState: (st) => states.push(st),
  };
  return { s, frames, states };
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

describe("createCameraFeed", () => {
  it("den første visningen starter økta, og bildene deles ut til alle", async () => {
    const { api, log, channels } = fakeApi();
    const t = fakeTimers();
    const feed = createCameraFeed(api, classifyStartError, t.timers);
    const a = sub();
    const b = sub();
    feed.acquire("BRIO", a.s);
    feed.acquire("BRIO", b.s);
    await flush();
    expect(log).toEqual(["start BRIO"]);
    channels[0](new ArrayBuffer(7));
    expect(a.frames).toEqual([7]);
    expect(b.frames).toEqual([7]);
    expect(feed.state()).toBe("live");
    expect(a.states).toEqual(["idle", "starting", "live"]);
    // Den andre fikk tilstanden den kom til, ikke en ny start.
    expect(b.states[0]).toBe("starting");
  });

  it("økta lever så lenge EN visning ser på, og stoppes først etter fristen", async () => {
    const { api, log } = fakeApi();
    const t = fakeTimers();
    const feed = createCameraFeed(api, classifyStartError, t.timers);
    const a = sub();
    const b = sub();
    const releaseA = feed.acquire("BRIO", a.s);
    const releaseB = feed.acquire("BRIO", b.s);
    await flush();
    releaseA();
    expect(t.count()).toBe(0);
    releaseB();
    expect(t.count()).toBe(1);
    await flush();
    expect(log).toEqual(["start BRIO"]);
    t.fire();
    await flush();
    expect(log).toEqual(["start BRIO", "stop"]);
    expect(feed.state()).toBe("idle");
  });

  it("en visning som tar over innenfor fristen berører ikke bakenden (sideskifte)", async () => {
    const { api, log, channels } = fakeApi();
    const t = fakeTimers();
    const feed = createCameraFeed(api, classifyStartError, t.timers);
    const a = sub();
    const release = feed.acquire("BRIO", a.s);
    await flush();
    channels[0](new ArrayBuffer(1));
    release();
    const b = sub();
    feed.acquire("BRIO", b.s);
    t.fire();
    await flush();
    expect(log).toEqual(["start BRIO"]);
    expect(feed.state()).toBe("live");
    // …og den nye visningen får siste bilde med én gang: ingen tom flate.
    expect(b.states).toEqual(["live"]);
    expect(b.frames).toEqual([1]);
  });

  it("et annet kamera starter økta på nytt, og bilder fra den gamle kanalen slippes", async () => {
    const { api, log, channels } = fakeApi();
    const t = fakeTimers();
    const feed = createCameraFeed(api, classifyStartError, t.timers);
    const a = sub();
    feed.acquire("BRIO", a.s);
    await flush();
    feed.acquire("FaceTime", sub().s);
    await flush();
    expect(log).toEqual(["start BRIO", "start FaceTime"]);
    channels[0](new ArrayBuffer(3)); // gammel kanal
    channels[1](new ArrayBuffer(5));
    expect(a.frames).toEqual([5]);
  });

  it("nektet tilgang og annen feil er to tilstander, og en ny visning prøver igjen", async () => {
    const t = fakeTimers();
    const denied = fakeApi({
      ok: false,
      error: "camera access is not allowed (Denied)",
    });
    const feed = createCameraFeed(denied.api, classifyStartError, t.timers);
    const a = sub();
    feed.acquire("BRIO", a.s);
    await flush();
    expect(feed.state()).toBe("denied");

    const other = fakeApi({ ok: false, error: "camera_open_timeout" });
    const feed2 = createCameraFeed(other.api, classifyStartError, t.timers);
    feed2.acquire("BRIO", sub().s);
    await flush();
    expect(feed2.state()).toBe("failed");
    feed2.acquire("BRIO", sub().s);
    await flush();
    expect(other.log).toEqual(["start BRIO", "start BRIO"]);
  });

  it("stopp venter på et start som er på vei, aldri forbi det", async () => {
    const log: string[] = [];
    let finish: (r: StartResult) => void = () => undefined;
    const api: FeedApi = {
      start: (token) => {
        log.push(`start ${token}`);
        return new Promise<StartResult>((resolve) => (finish = resolve));
      },
      stop: async () => {
        log.push("stop");
      },
    };
    const t = fakeTimers();
    const feed = createCameraFeed(api, classifyStartError, t.timers);
    const release = feed.acquire("BRIO", sub().s);
    await flush(); // start er på vei
    release();
    t.fire();
    await flush();
    expect(log).toEqual(["start BRIO"]);
    finish({ ok: true });
    await flush();
    expect(log).toEqual(["start BRIO", "stop"]);
  });
});
