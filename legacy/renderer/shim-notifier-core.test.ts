import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  createNotifierSlot,
  type ShimNotifier,
  type ShimToastKind,
} from "./shim-notifier-core";

const HERE = dirname(fileURLToPath(import.meta.url));

/** A recording stand-in for the legacy renderer's toast/navigate/i18n trio. */
function spyNotifier(tag: string) {
  const calls: string[] = [];
  const notifier: ShimNotifier = {
    toast: (kind: ShimToastKind, msg: string) => {
      calls.push(`${tag}:toast:${kind}:${msg}`);
    },
    navigate: (page: string, opts) => {
      calls.push(`${tag}:navigate:${page}:${opts?.tab ?? "-"}`);
    },
    t: (key: string, fallback = "") => {
      calls.push(`${tag}:t:${key}`);
      return `${tag}(${key}|${fallback})`;
    },
  };
  return { calls, notifier };
}

describe("createNotifierSlot", () => {
  it("uses the defaults when nobody injects anything — the legacy path", () => {
    const legacy = spyNotifier("legacy");
    const slot = createNotifierSlot(legacy.notifier);

    slot.current().toast("error", "boom");
    slot.current().navigate("settings", { tab: "settings-audio" });
    expect(slot.current().t("error.ipcFailed", "reserve")).toBe(
      "legacy(error.ipcFailed|reserve)",
    );

    expect(legacy.calls).toEqual([
      "legacy:toast:error:boom",
      "legacy:navigate:settings:settings-audio",
      "legacy:t:error.ipcFailed",
    ]);
  });

  it("routes every service to the host once one is installed", () => {
    const legacy = spyNotifier("legacy");
    const host = spyNotifier("host");
    const slot = createNotifierSlot(legacy.notifier);

    slot.set(host.notifier);
    slot.current().toast("error", "boom");
    slot.current().navigate("home");
    slot.current().t("error.ipcFailed", "reserve");

    expect(host.calls).toEqual([
      "host:toast:error:boom",
      "host:navigate:home:-",
      "host:t:error.ipcFailed",
    ]);
    expect(legacy.calls).toEqual([]);
  });

  it("a PARTIAL override keeps the legacy default for what it leaves out", () => {
    const legacy = spyNotifier("legacy");
    const host = spyNotifier("host");
    const slot = createNotifierSlot(legacy.notifier);

    slot.set({ toast: host.notifier.toast });
    slot.current().toast("error", "boom");
    slot.current().navigate("home");

    expect(host.calls).toEqual(["host:toast:error:boom"]);
    expect(legacy.calls).toEqual(["legacy:navigate:home:-"]);
  });

  it("an explicitly-undefined field does not punch a hole in the defaults", () => {
    // `{ toast: undefined }` is what a host object built from optional config
    // looks like. Spreading it blindly would leave `toast` undefined and the
    // next failure would throw INSIDE the error path — the worst place for it.
    const legacy = spyNotifier("legacy");
    const slot = createNotifierSlot(legacy.notifier);

    slot.set({ toast: undefined, navigate: undefined, t: undefined });
    expect(() => slot.current().toast("error", "boom")).not.toThrow();
    expect(legacy.calls).toEqual(["legacy:toast:error:boom"]);
  });

  it("set(null) restores the defaults", () => {
    const legacy = spyNotifier("legacy");
    const host = spyNotifier("host");
    const slot = createNotifierSlot(legacy.notifier);

    slot.set(host.notifier);
    slot.set(null);
    slot.current().toast("info", "back");

    expect(legacy.calls).toEqual(["legacy:toast:info:back"]);
    expect(host.calls).toEqual([]);
  });

  it("is read per call — a host installed after boot still wins", () => {
    // The shim evaluates at module load; a host renders later. `current()` is
    // read at the CALL, not captured at construction, or the seam would only
    // work for hosts that beat the module graph.
    const legacy = spyNotifier("legacy");
    const host = spyNotifier("host");
    const slot = createNotifierSlot(legacy.notifier);
    const later = () => slot.current().toast("error", "late");

    slot.set(host.notifier);
    later();

    expect(host.calls).toEqual(["host:toast:error:late"]);
  });
});

// ── Source pin ──────────────────────────────────────────────────────────────
//
// The seam is only worth anything if the shim actually goes THROUGH it. A new
// `toast(...)`/`navigateTo(...)`/`t(...)` call added later would compile, pass
// every test, and be invisible to the new shell — a message painted into a DOM
// that is not there. Parsed from source in the house style (settings-store-pin,
// tuning-report), because the property is about what the code says.

/** Source with comments removed — a comment naming `toast()` is documentation,
 *  not a call. */
function codeOf(path: string): string {
  return readFileSync(path, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "")
    .replace(/ \/\/[^"'`\n]*$/gm, "");
}

describe("api-shim goes through the notifier seam", () => {
  const code = codeOf(join(HERE, "api-shim.ts"));

  it("calls no legacy toast/navigate/translate function directly", () => {
    const offenders: string[] = [];
    // A call is the identifier followed by `(` and NOT preceded by `.` or a
    // word character — so `n.toast(` (through the slot) and the bare `toast,`
    // in the defaults object are both fine, and `navigateTo(` is not.
    if (/(?<![A-Za-z0-9_$.])toast\(/.test(code)) offenders.push("toast(");
    if (/(?<![A-Za-z0-9_$.])navigateTo\(/.test(code))
      offenders.push("navigateTo(");
    if (/(?<![A-Za-z0-9_$.])t\(/.test(code)) offenders.push("t(");
    expect(
      offenders,
      "api-shim.ts calls a legacy UI module directly instead of going through " +
        "`notifier.current()` — the second shell (app/) cannot see it. Route it " +
        "through the slot (setShimNotifier) like every other message does.",
    ).toEqual([]);
  });

  it("still exports the setter a host installs itself with", () => {
    expect(code).toContain("export function setShimNotifier");
  });

  it("still defaults to the legacy modules, so an uninjected boot is unchanged", () => {
    expect(code).toMatch(/createNotifierSlot\(\{\s*toast,\s*navigate: navigateTo,\s*t,\s*\}\)/);
  });
});
