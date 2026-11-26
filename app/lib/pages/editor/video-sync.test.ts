import { describe, expect, it } from "vitest";

import { DRIFT_SEC, SEEK_EPS_SEC, videoSyncAction } from "./video-sync";

describe("videoSyncAction", () => {
  it("står stille: pauser og flytter bildet til spillehodet", () => {
    expect(
      videoSyncAction({
        videoTime: 0,
        target: 30,
        playing: false,
        videoPaused: false,
      }),
    ).toEqual({ seek: 30, play: false, pause: true });
  });

  it("står stille og stemmer: ingenting å gjøre", () => {
    expect(
      videoSyncAction({
        videoTime: 12,
        target: 12 + SEEK_EPS_SEC / 2,
        playing: false,
        videoPaused: true,
      }),
    ).toEqual({ seek: null, play: false, pause: false });
  });

  it("spilles: starter videoen, men hopper ikke når den er på stedet", () => {
    expect(
      videoSyncAction({
        videoTime: 10,
        target: 10.1,
        playing: true,
        videoPaused: true,
      }),
    ).toEqual({ seek: null, play: true, pause: false });
  });

  it("spilles og går: rører ingenting innenfor driftgrensen", () => {
    expect(
      videoSyncAction({
        videoTime: 10,
        target: 10 + DRIFT_SEC,
        playing: true,
        videoPaused: false,
      }),
    ).toEqual({ seek: null, play: false, pause: false });
  });

  it("spilles og har drevet mer enn 0,25 s: rettes til motoren", () => {
    expect(
      videoSyncAction({
        videoTime: 10,
        target: 10 + DRIFT_SEC + 0.01,
        playing: true,
        videoPaused: false,
      }),
    ).toEqual({ seek: 10.26, play: false, pause: false });
  });

  it("hopper over et kutt: målet flyttet seg langt, videoen følger", () => {
    const a = videoSyncAction({
      videoTime: 60,
      target: 95,
      playing: true,
      videoPaused: false,
    });
    expect(a.seek).toBe(95);
  });
});
