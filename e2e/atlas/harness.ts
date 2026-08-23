import type { Page, ConsoleMessage } from "@playwright/test";
import { boot, type BootOptions } from "../harness";
import type { ConsoleFinding } from "./report";

// What the atlas needs on top of the browser tier's `boot()`.
//
// Two things, both about states a fixture alone cannot reach:
//
//  1. **Backend EVENTS.** Half the app's states are painted by a Tauri event,
//     not by an invoke: the recording overlay's meter (`recording://levels`),
//     the missed-recording card (`scheduler://missed`), the pre-start check
//     (`scheduler://preflight`), the reconnect banner, the global error strip.
//     `e2e/harness.ts` resolves `plugin:event|listen` with an id and then never
//     emits — correct for a test tier ("nothing is emitting" is the truth), but
//     it means those screens can only ever be photographed empty.
//
//     `installEventBridge()` installs a `__TAURI_INTERNALS__` that REMEMBERS
//     which callback id subscribed to which event name, and exposes
//     `window.__ATLAS_EMIT__(event, payload)` to fire them. It must be added
//     BEFORE `boot()` — init scripts run in registration order, and the base
//     harness only installs its own stub `if (!window.__TAURI_INTERNALS__)`.
//
//  2. **A fixed clock.** Every relative label in the app ("om 3 dager",
//     "for 2 timer siden", the recording timer) is derived from `Date.now()`.
//     `page.clock.setFixedTime` pins it so two runs of the atlas produce the
//     same pixels, while leaving timers and rAF running so the UI still paints.

/** The moment every atlas screenshot is taken at: Sunday 23 August 2026, 10:55
 *  — five minutes before a service. Local time, no zone suffix, because the
 *  app's own schedule strings are zone-less local ISO. */
export const ATLAS_NOW = new Date(2026, 7, 23, 10, 55, 0);

/** A recent-but-past Sunday, for recordings that already happened. */
export const LAST_SUNDAY_MS = new Date(2026, 7, 16, 11, 0, 0).getTime();

/** The next scheduled start the scheduler would answer with. Zone-less local
 *  ISO, the shape `scheduler://next` and `scheduler_status.next` both carry. */
export const NEXT_SERVICE_ISO = "2026-08-30T11:00:00";

/**
 * Install the event-capable Tauri stub. Call BEFORE `bootScene`/`boot`.
 */
export async function installEventBridge(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as Record<string, unknown>;
    if (w.__TAURI_INTERNALS__) return;

    let nextId = 1;
    /** event name → the callback ids currently listening to it. */
    const byEvent: Record<string, number[]> = {};
    w.__ATLAS_EVENT_HANDLERS__ = byEvent;

    w.__TAURI_INTERNALS__ = {
      transformCallback(cb: (payload: unknown) => void) {
        const id = nextId++;
        w[`_${id}`] = cb;
        return id;
      },
      unregisterCallback(id: number) {
        delete w[`_${id}`];
      },
      // `listen()` reaches the backend through this same invoke, carrying
      // `{ event, target, handler }` where `handler` is a transformCallback id.
      // Recording that pair is the whole trick.
      invoke(cmd: string, args?: Record<string, unknown>) {
        if (cmd === "plugin:event|listen") {
          const ev = args?.event;
          const handler = args?.handler;
          if (typeof ev === "string" && typeof handler === "number") {
            (byEvent[ev] ??= []).push(handler);
          }
          return Promise.resolve(nextId++);
        }
        if (cmd === "plugin:event|unlisten") return Promise.resolve(undefined);
        return Promise.reject(
          new Error(`no Tauri backend in the browser tier: ${cmd}`),
        );
      },
      convertFileSrc(filePath: string, protocol = "asset") {
        return `${protocol}://localhost/${encodeURIComponent(filePath)}`;
      },
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
    };
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

    w.__ATLAS_EMIT__ = (event: string, payload: unknown): number => {
      const ids = byEvent[event] ?? [];
      for (const id of ids) {
        const cb = w[`_${id}`] as ((e: unknown) => void) | undefined;
        try {
          cb?.({ event, id, payload });
        } catch {
          /* a handler that throws must not stop the other subscribers */
        }
      }
      return ids.length;
    };
  });
}

/** Fire a backend event at every renderer subscriber. Returns how many were
 *  listening — 0 means the screen this scene wanted is NOT driven by that
 *  event name (a finding, not a silent no-op). */
export async function emit(
  page: Page,
  event: string,
  payload: unknown,
): Promise<number> {
  return page.evaluate(
    ([ev, p]) =>
      (
        window as unknown as {
          __ATLAS_EMIT__: (e: string, x: unknown) => number;
        }
      ).__ATLAS_EMIT__(ev as string, p),
    [event, payload] as const,
  );
}

/** One VU packet, per native channel, in dBFS. Steady values on purpose: a
 *  drifting meter is a screenshot that differs every run. */
export function vuPacket(rms = -18, peak = -9, channels = 2): unknown {
  return {
    rms_dbfs: Array.from({ length: channels }, () => rms),
    peak_dbfs: Array.from({ length: channels }, () => peak),
  };
}

/**
 * Push enough identical VU packets that the meter's smoothing has converged,
 * then stop — the bars hold their last painted position because painting is
 * driven by packets.
 */
export async function settleVu(
  page: Page,
  event: "vu://levels" | "recording://levels" = "vu://levels",
  rms = -18,
  peak = -9,
): Promise<number> {
  let delivered = 0;
  for (let i = 0; i < 40; i++) {
    delivered = await emit(page, event, vuPacket(rms, peak));
    if (delivered === 0) break;
    await page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => r(null))),
    );
  }
  return delivered;
}

/** Boot a scene: fixed clock, event bridge, then the ordinary browser-tier
 *  boot with the scene's fixtures and the locale under the `language` key. */
export async function bootScene(
  page: Page,
  locale: string,
  opts: BootOptions,
): Promise<void> {
  await page.clock.setFixedTime(ATLAS_NOW);
  await installEventBridge(page);
  await boot(page, {
    ...opts,
    settings: { ...(opts.settings ?? {}), language: locale },
  });
}

// ── The console guard ────────────────────────────────────────────────────────

/**
 * Collect `console.error` and uncaught page errors for one scene.
 *
 * The atlas runs with NO backend, so a large share of what lands here is the
 * harness telling the truth ("no Tauri backend in the browser tier: …"). Those
 * are classified out in the report; what remains is real debt, and this run is
 * the first time anything has looked.
 */
export function watchConsole(page: Page): ConsoleFinding[] {
  const found: ConsoleFinding[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error")
      found.push({ kind: "console.error", text: m.text() });
  });
  page.on("pageerror", (e: Error) =>
    found.push({ kind: "pageerror", text: e.message }),
  );
  return found;
}
