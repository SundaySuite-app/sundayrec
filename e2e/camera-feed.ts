import type { Page } from "@playwright/test";

import type { Fixtures } from "./harness";
import { fn } from "./harness";

/**
 * The camera picture, without hardware (C4b).
 *
 * The picture is the backend's: `camera_preview_start` opens ONE camera session
 * and streams JPEGs down a Tauri channel. In a browser there is no backend, so
 * a spec answers the two commands with {@link cameraFixtures} and then plays
 * the part of the camera session itself with {@link pushFrame}: it hands real
 * JPEG bytes to the very channel the app passed in, through the callback Tauri
 * registered for it (`window._<id>`, with the ordered `{ index, message }`
 * envelope the runtime uses) — so the whole renderer path runs, from the
 * channel to the pixels in the `<img>`.
 *
 * No `getUserMedia` stub any more: the webview never opens the camera.
 */

/**
 * Answers for `camera_preview_start` / `camera_preview_stop`.
 *
 * `window.__E2E_CAM__` keeps what the app did: how many times it started and
 * stopped the session (a camera that is closed and reopened is a blink), the
 * order of those calls, and the channel it was handed. `mode` makes the start
 * refuse, with the sentence the backend uses: `denied` is the OS saying no,
 * `failed` is a camera that would not open.
 */
export function cameraFixtures(mode?: "denied" | "failed"): Fixtures {
  const refuse =
    mode === "denied"
      ? `throw { code: "recording", message: "camera_open_failed: camera access is not allowed (Denied)" };`
      : mode === "failed"
        ? `throw { code: "recording", message: "camera_open_timeout" };`
        : "";
  return {
    camera_preview_start: fn(`(args) => {
      const cam = (window.__E2E_CAM__ ||= { starts: 0, stops: 0, log: [], channel: null, index: 0 });
      cam.starts += 1;
      cam.log.push("camera_preview_start " + args.deviceToken);
      cam.channel = args.onFrame;
      cam.index = 0;
      ${refuse}
      return null;
    }`),
    camera_preview_stop: fn(`() => {
      const cam = (window.__E2E_CAM__ ||= { starts: 0, stops: 0, log: [], channel: null, index: 0 });
      cam.stops += 1;
      cam.log.push("camera_preview_stop");
      return null;
    }`),
  };
}

/** What the app did to the camera session so far. */
export async function cameraCalls(
  page: Page,
): Promise<{ starts: number; stops: number; log: string[] }> {
  return page.evaluate(() => {
    const cam = (
      window as unknown as {
        __E2E_CAM__?: { starts: number; stops: number; log: string[] };
      }
    ).__E2E_CAM__;
    return {
      starts: cam?.starts ?? 0,
      stops: cam?.stops ?? 0,
      log: [...(cam?.log ?? [])],
    };
  });
}

/** Deliver one JPEG (base64) down the channel the app gave `camera_preview_start`. */
export async function pushFrame(page: Page, b64: string): Promise<void> {
  await page.evaluate(async (data: string) => {
    const w = window as unknown as {
      __E2E_CAM__?: { channel: { id: number } | null; index: number };
    } & Record<string, unknown>;
    const cam = w.__E2E_CAM__;
    if (!cam?.channel) throw new Error("camera_preview_start was not called");
    const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
    const deliver = w[`_${cam.channel.id}`] as (m: unknown) => void;
    deliver({ index: cam.index++, message: bytes.buffer });
  }, b64);
}

/**
 * Deliver a generated JPEG: a flat picture (640×360 unless `size` says
 * otherwise — a camera that sends its own, larger MJPEG frames) in `color`, made by the
 * browser's own encoder. Two colours give two different, real, decodable
 * fixture pictures; the spec reads the colour back from the `<img>` to prove
 * which picture is on screen.
 */
export async function pushColorFrame(
  page: Page,
  color: string,
  size: [number, number] = [640, 360],
): Promise<void> {
  const b64 = await page.evaluate(
    async ([c, w, h]: [string, number, number]) => {
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const g = canvas.getContext("2d");
      if (!g) throw new Error("no 2d context");
      g.fillStyle = c;
      g.fillRect(0, 0, w, h);
      const blob = await new Promise<Blob | null>((r) =>
        canvas.toBlob(r, "image/jpeg", 0.92),
      );
      if (!blob) throw new Error("no jpeg");
      const bytes = new Uint8Array(await blob.arrayBuffer());
      let s = "";
      for (const b of bytes) s += String.fromCharCode(b);
      return btoa(s);
    },
    [color, size[0], size[1]] as [string, number, number],
  );
  await pushFrame(page, b64);
}

/**
 * A real, tiny 160×90 JPEG (ImageMagick, quality 70).
 *
 * The preview reads the aspect ratio out of the SOF0 header and paints the
 * bytes into an `<img>`, so a header-only stub would measure correctly and then
 * render as a broken image. This one decodes.
 */
export const FRAME_16_9 =
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAoHBwgHBgoICAgLCgoLDhgQDg0NDh0VFhEYIx8lJCIf" +
  "IiEmKzcvJik0KSEiMEExNDk7Pj4+JS5ESUM8SDc9Pjv/2wBDAQoLCw4NDhwQEBw7KCIoOzs7Ozs7" +
  "Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozv/wAARCABaAKADASIA" +
  "AhEBAxEB/8QAFwABAQEBAAAAAAAAAAAAAAAAAAIBB//EABUQAQEAAAAAAAAAAAAAAAAAAAAR/8QA" +
  "FwEBAQEBAAAAAAAAAAAAAAAAAAEEAv/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/" +
  "AOdUqaVuZ1UqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUGCRBQkBQkBQkBQkBQkBQkB" +
  "QkBQkBQkBlKmlcqqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgylTSoqqVNKCqVNKC" +
  "qVNKCqVNKCqVNKCqVNKCqVNKCqVNKCqVNKDKVNKiqpU0oKpU0oKpU0oKpU0oKpU0oKpU0oKpU0oK" +
  "pU0oKpU0oJpWCDaVgDaVgDaVgDaVgDaVgDaVgDaVgDaVgDaVgD//2Q==";

/** The two places the picture is shown. */
export const TILES = [
  "record-camera-preview",
  "overlay-camera-preview",
] as const;
export type Tile = (typeof TILES)[number];

/**
 * Write down every state each camera tile passes through — the proof that the
 * picture never blinks.
 *
 * A MutationObserver (not a timer) so a single committed frame of «Starter
 * kamera…» in the middle of Start cannot slip between two samples. A state is
 * `absent`, or `<phase>` with `+img` when the `<img>` has a `src` and the
 * browser holds a picture for it (a real `naturalWidth` — while a new `src`
 * loads, the previous picture stays current, which is exactly the no-blink
 * property), else `-img`. Call
 * BEFORE `boot()`.
 */
export async function trackTiles(page: Page): Promise<void> {
  await page.addInitScript((tiles: readonly string[]) => {
    const seen: Record<string, string[]> = {};
    for (const t of tiles) seen[t] = [];
    (window as unknown as Record<string, unknown>).__E2E_TILES__ = seen;
    const sample = (): void => {
      for (const t of tiles) {
        const el = document.querySelector<HTMLElement>(`[data-testid="${t}"]`);
        let state = "absent";
        if (el) {
          const img = el.querySelector("img");
          const shown = !!img?.getAttribute("src") && img.naturalWidth > 0;
          state = `${el.dataset.phase}${shown ? "+img" : "-img"}`;
        }
        const list = seen[t];
        if (list[list.length - 1] !== state) list.push(state);
      }
    };
    const start = (): void => {
      new MutationObserver(sample).observe(document, {
        subtree: true,
        childList: true,
        attributes: true,
        attributeFilter: ["data-phase", "src"],
      });
      // A decoded image changes nothing in the DOM: listen for the load too.
      document.addEventListener("load", sample, true);
      sample();
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", start);
    } else {
      start();
    }
  }, TILES);
}

/** The states each tile passed through, in order, repeats collapsed. */
export async function tileStates(page: Page): Promise<Record<Tile, string[]>> {
  return page.evaluate(
    () =>
      (window as unknown as { __E2E_TILES__: Record<Tile, string[]> })
        .__E2E_TILES__,
  );
}

/** The colour of the middle of the picture a tile shows, as [r, g, b]. */
export async function tileColor(
  page: Page,
  tile: Tile,
): Promise<[number, number, number]> {
  return page.evaluate((id: string) => {
    const img = document.querySelector<HTMLImageElement>(
      `[data-testid="${id}"] img`,
    );
    if (!img) throw new Error(`no picture in ${id}`);
    const c = document.createElement("canvas");
    c.width = 8;
    c.height = 8;
    const g = c.getContext("2d");
    if (!g) throw new Error("no 2d context");
    g.drawImage(img, 0, 0, 8, 8);
    const [r, gr, b] = g.getImageData(4, 4, 1, 1).data;
    return [r, gr, b] as [number, number, number];
  }, tile);
}
