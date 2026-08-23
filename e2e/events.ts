import type { Page } from "@playwright/test";

/**
 * Emit backend events into the new shell, from a spec.
 *
 * The harness supplies Tauri's RUNTIME but no backend (see `e2e/harness.ts`):
 * `plugin:event|listen` resolves with an id and then nothing ever fires. That
 * is the truth in a browser — nothing is emitting — and it is exactly what a
 * recording journey needs to be able to fake, because the whole point of
 * `recording://state` is that the UI follows an engine it cannot see.
 *
 * Two routes in, because the app has two kinds of subscription and they are
 * not the same seam:
 *
 *   `__emit(channel, payload)`      — the `window.api.on(...)` channels, i.e.
 *                                     the old Electron names api-shim maps
 *                                     (`recording-overlay-stop` →
 *                                     `recording://state`).
 *   `__emitEvent(event, payload)`   — the backend's OWN event names, which
 *                                     `status/next-recording.ts` subscribes to
 *                                     directly through `@tauri-apps/api/event`
 *                                     (`scheduler://missed` never had an
 *                                     Electron name to map).
 *
 * ⚠️ `__emit` hands the payload to the handler DIRECTLY, so it skips
 * api-shim's `EVENT_ADAPTERS`. Pass the shape the handler reads.
 *
 * Both hooks are installed by intercepting the ASSIGNMENT — `window.api` and
 * `window.__TAURI_INTERNALS__` do not exist yet when an init script runs, so
 * the assignment itself is the only place to get in. Same pattern as
 * `e2e/first-run.spec.ts`'s VU spy and `e2e/auto-update.spec.ts`'s
 * settings spy.
 *
 * Call BEFORE `boot()`: an init script only applies to navigations after it
 * was added.
 */
export async function spyEvents(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as Record<string, unknown>;

    // ── window.api.on ────────────────────────────────────────────────────
    const handlers = new Map<string, Array<(payload: unknown) => void>>();
    let realApi: Record<string, unknown> | undefined;
    Object.defineProperty(window, "api", {
      configurable: true,
      get: () => realApi,
      set: (v: Record<string, unknown>) => {
        realApi = v;
        const origOn = (
          v.on as (c: string, f: (p: unknown) => void) => () => void
        ).bind(v);
        v.on = (channel: string, fn: (p: unknown) => void) => {
          const list = handlers.get(channel) ?? [];
          list.push(fn);
          handlers.set(channel, list);
          const off = origOn(channel, fn);
          return () => {
            const rest = (handlers.get(channel) ?? []).filter((x) => x !== fn);
            handlers.set(channel, rest);
            off();
          };
        };
      },
    });
    w.__emit = (channel: string, payload: unknown): number => {
      const list = handlers.get(channel) ?? [];
      for (const fn of list.slice()) fn(payload);
      return list.length;
    };

    // ── @tauri-apps/api/event listen() ───────────────────────────────────
    //
    // `listen` invokes `plugin:event|listen` with `{ event, handler }`, where
    // `handler` is the id `transformCallback` just minted. Recording that pair
    // is what lets a spec call the callback the way the real backend does.
    const byEvent = new Map<string, number[]>();
    let internals: Record<string, unknown> | undefined;
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      get: () => internals,
      set: (v: Record<string, unknown>) => {
        internals = v;
        const origInvoke = (
          v.invoke as (cmd: string, args?: Record<string, unknown>) => unknown
        ).bind(v);
        v.invoke = (cmd: string, args?: Record<string, unknown>) => {
          if (cmd === "plugin:event|listen" && args) {
            const event = String(args.event);
            const id = Number(args.handler);
            if (Number.isFinite(id)) {
              byEvent.set(event, [...(byEvent.get(event) ?? []), id]);
            }
          }
          return origInvoke(cmd, args);
        };
      },
    });
    w.__emitEvent = (event: string, payload: unknown): number => {
      const ids = byEvent.get(event) ?? [];
      for (const id of ids) {
        const cb = w[`_${id}`] as ((e: unknown) => void) | undefined;
        cb?.({ event, id, payload });
      }
      return ids.length;
    };
  });
}

/** Fire one `window.api.on` channel. Returns how many handlers took it. */
export async function emit(
  page: Page,
  channel: string,
  payload: unknown = null,
): Promise<number> {
  return page.evaluate(
    ([c, p]) =>
      (
        window as unknown as { __emit: (c: string, p: unknown) => number }
      ).__emit(c as string, p),
    [channel, payload] as const,
  );
}

/** Fire one backend event name (the `listen()` route). */
export async function emitEvent(
  page: Page,
  event: string,
  payload: unknown = null,
): Promise<number> {
  return page.evaluate(
    ([e, p]) =>
      (
        window as unknown as { __emitEvent: (e: string, p: unknown) => number }
      ).__emitEvent(e as string, p),
    [event, payload] as const,
  );
}
