/**
 * The tray → renderer dispatcher.
 *
 * The Rust tray emits ONE event, `tray://action`, whose payload is a stable
 * action-id string (`src-tauri/src/tray/mod.rs` `action_id`). The renderer used
 * to listen for Electron-era channel names instead — `tray-start-recording`,
 * `tray-stop-recording`, `tray-run-preflight` — none of which any Rust code has
 * ever emitted. Dead listeners, dead menu items.
 *
 * This module is the missing adapter, and it is deliberately split in two:
 *
 *   - `createTrayDispatcher(handlers)` is PURE: an id → handler lookup with no
 *     DOM, no IPC and no globals, so every routing decision (including "an id we
 *     don't know must not throw") is unit-testable.
 *   - `initTrayActions(handlers)` is the thin shell that subscribes to the real
 *     Tauri event and feeds it to the dispatcher.
 *
 * Like `status/next-recording`, this listens DIRECTLY via `@tauri-apps/api/event`
 * rather than through `window.api.on`: EVENT_MAP is the compatibility layer for
 * old Electron channel names, and `tray://action` never had one.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Every action-id the Rust tray can send that the RENDERER has to act on.
 *  `open-window`, `show-on-error`, `quit` and the disabled info rows are handled
 *  entirely in Rust (`emit_action`) and never reach us; `stop-recording` DOES
 *  reach us — Rust stops the engine itself, and the event is the cue for the UI
 *  to follow. */
export type TrayActionId =
  | "start-recording"
  | "stop-recording"
  | "open-recordings-folder"
  | "run-preflight"
  | "run-diagnostics";

/** What the shell must be able to do for each id. Every handler is optional so a
 *  caller can wire a subset (tests, or a page that only cares about one). */
export interface TrayActionHandlers {
  startRecording?: () => void;
  stopRecording?: () => void;
  openRecordingsFolder?: () => void;
  runPreflight?: () => void;
  runDiagnostics?: () => void;
}

/** Which handler each id resolves to. One table, so the mapping is data rather
 *  than a switch buried in a listener. */
const ROUTES: Record<TrayActionId, keyof TrayActionHandlers> = {
  "start-recording": "startRecording",
  "stop-recording": "stopRecording",
  "open-recordings-folder": "openRecordingsFolder",
  "run-preflight": "runPreflight",
  "run-diagnostics": "runDiagnostics",
};

/** The ids this renderer knows about — exported so a test can assert the table
 *  and the Rust `action_id` list have not drifted apart. */
export const TRAY_ACTION_IDS = Object.keys(ROUTES) as TrayActionId[];

/**
 * Build the dispatcher. The returned function takes a raw event payload and
 * returns whether it was routed — `false` for anything unknown, so a tray from a
 * newer build (or a malformed payload) is ignored rather than throwing inside an
 * event callback, where nothing would catch it.
 *
 * A handler that throws is logged and swallowed for the same reason.
 */
export function createTrayDispatcher(
  handlers: TrayActionHandlers,
): (payload: unknown) => boolean {
  return (payload: unknown): boolean => {
    if (typeof payload !== "string") return false;
    const key = ROUTES[payload as TrayActionId];
    if (!key) return false;
    const fn = handlers[key];
    if (!fn) return false;
    try {
      fn();
    } catch (err) {
      console.error("[tray] handler for", payload, "failed:", err);
    }
    return true;
  };
}

let unlisten: UnlistenFn | null = null;

/**
 * Subscribe to `tray://action`. Idempotent — a second call replaces the first
 * subscription rather than stacking a second handler on the same event.
 */
export function initTrayActions(handlers: TrayActionHandlers): void {
  const dispatch = createTrayDispatcher(handlers);
  unlisten?.();
  unlisten = null;
  listen<string>("tray://action", (e) => {
    dispatch(e.payload);
  })
    .then((u) => {
      unlisten = u;
    })
    .catch((err) => console.warn("[tray] listen failed:", err));

  window.addEventListener("beforeunload", () => {
    try {
      unlisten?.();
    } catch {
      /* teardown is best-effort */
    }
    unlisten = null;
  });
}
