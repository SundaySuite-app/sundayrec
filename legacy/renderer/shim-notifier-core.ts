// The host-injection seam for the three RENDERER services `api-shim.ts` uses —
// pure, no DOM, no Tauri.
//
// ## The problem this exists for
//
// `api-shim.ts` is the ONE door into the backend: every page, every command,
// every fixture-seam hop goes through it. It is also, today, hard-wired to the
// legacy renderer's own UI: it imports `ui/toast`, `ui/navigate` and `i18n`
// so that a failed invoke can say so, and `?goto=` can navigate.
//
// Those three imports are the only thing in the shim that assumes WHICH shell
// is on top of it. «Frivilligen først» builds a second shell (`app/`, Preact)
// beside the legacy one, and it has its own toast surface, its own router and
// its own translator. Without a seam the new shell would either inherit the old
// renderer's DOM-mutating toast stack (which paints into an element tree that
// does not exist there) or fork the shim — and a forked shim is two IPC layers
// that drift, which is the exact failure class `reference-seam-bugs` is about.
//
// ## The shape
//
// A SLOT holding one `ShimNotifier`. The shim creates it with the legacy
// modules as the defaults, so with nobody calling the setter the behaviour is
// byte-identical to what it was before this file existed. A host that wants its
// own surfaces calls `setShimNotifier({ … })` before the first failure can fire.
//
// The contract is exactly as wide as the shim's actual call sites and no wider:
// three `toast("error", …)` calls, the `t(key, fallback)` those three read their
// copy from, and the ONE `navigateTo(page, { tab, highlight })` behind `?goto=`.
// Anything else the shim might one day want belongs in a new field with its own
// default, not in a "notifier" that has quietly become a second window object.
//
// The override is a PARTIAL: a host that only has a toast keeps the legacy
// navigate and translator rather than being forced to stub them. `set(null)`
// restores the defaults — which is what makes this testable at all, and what a
// teardown wants.

/** The toast kinds `ui/toast` accepts. Mirrored (not imported) so this module
 *  stays free of the renderer's DOM module graph. */
export type ShimToastKind = "info" | "success" | "warn" | "error";

/** Options the shim's one `navigate` call site passes. */
export interface ShimNavigateOpts {
  tab?: string;
  highlight?: boolean;
}

/** The three host services `api-shim.ts` reaches for. */
export interface ShimNotifier {
  /** Surface a message. The shim only ever sends `"error"`. */
  toast(kind: ShimToastKind, msg: string): void;
  /** Go to a page (optionally an inner tab). Only used by the `?goto=` hook. */
  navigate(page: string, opts?: ShimNavigateOpts): void;
  /** Translate a key, falling back to the given literal. */
  t(key: string, fallback?: string): string;
}

/** A live, replaceable `ShimNotifier`. */
export interface NotifierSlot {
  /** The notifier in force right now. Read per call — a host may install its
   *  own after the shim module has already evaluated. */
  current(): ShimNotifier;
  /** Install an override (merged over the defaults), or `null` to restore
   *  the defaults. */
  set(override: Partial<ShimNotifier> | null): void;
}

/** Drop explicitly-`undefined` fields so `{ toast: undefined }` does not
 *  clobber the default with a hole that then throws at the call site. */
function defined(override: Partial<ShimNotifier>): Partial<ShimNotifier> {
  const out: Partial<ShimNotifier> = {};
  if (typeof override.toast === "function") out.toast = override.toast;
  if (typeof override.navigate === "function") out.navigate = override.navigate;
  if (typeof override.t === "function") out.t = override.t;
  return out;
}

/**
 * Create the slot. `defaults` is what the shim behaves like when nobody has
 * injected anything — i.e. the legacy renderer's own toast/navigate/i18n.
 */
export function createNotifierSlot(defaults: ShimNotifier): NotifierSlot {
  let active: ShimNotifier = defaults;
  return {
    current: () => active,
    set(override) {
      active = override ? { ...defaults, ...defined(override) } : defaults;
    },
  };
}
