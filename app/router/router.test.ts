import { parseGoto } from "@lib/goto-core";
import { describe, expect, it, vi } from "vitest";

import {
  handleTrayAction,
  navigate,
  pendingAction,
  resolveRoute,
  route,
  type Route,
} from "./router";

/** What a `?goto=` string resolves to, end to end: the shim parses it with
 *  `parseGoto` and hands the result straight to `navigate`. */
function fromGoto(search: string): Route {
  const target = parseGoto(search);
  if (!target) throw new Error(`«${search}» parsed to nothing`);
  return resolveRoute(target.page, { tab: target.tab, highlight: false });
}

describe("?goto= i de fire sidene", () => {
  it.each([
    ["?goto=record", { page: "record" }],
    ["?goto=recordings", { page: "recordings" }],
    // Fanen på OPPTAK er en fane (papirkurven), ikke et anker.
    ["?goto=recordings:trash", { page: "recordings", tab: "trash" }],
    ["?goto=plan", { page: "plan" }],
    // På PLAN og INNSTILLINGER er «fanen» et rullemål.
    [
      "?goto=plan:schedule",
      { page: "plan", anchor: "schedule", highlight: false },
    ],
    ["?goto=settings", { page: "settings" }],
    [
      "?goto=settings:sound",
      { page: "settings", anchor: "sound", highlight: false },
    ],
    // Den fullt kvalifiserte skrivemåten betyr det samme som den bare.
    [
      "?goto=settings:settings-camera",
      { page: "settings", anchor: "camera", highlight: false },
    ],
    // Percent-encoded, som e2e/harness.ts faktisk skriver den.
    [
      "?goto=settings%3Afolder",
      { page: "settings", anchor: "folder", highlight: false },
    ],
  ] as Array<[string, Route]>)("%s", (search, expected) => {
    expect(fromGoto(search)).toEqual(expected);
  });

  it("an empty or absent goto is not this module's problem", () => {
    expect(parseGoto("")).toBeNull();
    expect(parseGoto("?goto=")).toBeNull();
  });
});

describe("resolveRoute", () => {
  it("highlights an anchor by default, and obeys an explicit false", () => {
    expect(resolveRoute("settings", { anchor: "device" })).toEqual({
      page: "settings",
      anchor: "device",
      highlight: true,
    });
    expect(
      resolveRoute("settings", { anchor: "device", highlight: false })
        .highlight,
    ).toBe(false);
  });

  it("lets an explicit anchor win over a goto tab", () => {
    expect(
      resolveRoute("settings", { tab: "settings-camera", anchor: "flip" }),
    ).toEqual({ page: "settings", anchor: "flip", highlight: true });
  });

  it("falls back to TA OPP for an unknown or retired page, and says so", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    for (const old of ["atlantis", "edit", "export", "setup", "library"]) {
      expect(resolveRoute(old)).toEqual({ page: "record" });
    }
    expect(warn).toHaveBeenCalledTimes(5);
    warn.mockRestore();
  });

  it("carries firstRun through", () => {
    expect(resolveRoute("settings", { firstRun: true })).toEqual({
      page: "settings",
      firstRun: true,
    });
  });
});

describe("navigate", () => {
  it("writes the route signal", () => {
    navigate("record");
    expect(route.value).toEqual({ page: "record" });
    navigate("recordings", { tab: "trash" });
    expect(route.value).toEqual({ page: "recordings", tab: "trash" });
    navigate("settings", { anchor: "sound" });
    expect(route.value).toEqual({
      page: "settings",
      anchor: "sound",
      highlight: true,
    });
  });
});

describe("tray actions", () => {
  it("arms the signal AND goes to the page that owns the action", () => {
    for (const [action, page] of [
      ["start-recording", "record"],
      ["stop-recording", "record"],
      ["open-recordings-folder", "recordings"],
    ] as const) {
      expect(handleTrayAction(action)).toBe(true);
      expect(pendingAction.value).toBe(action);
      expect(route.value.page).toBe(page);
    }
    pendingAction.value = null;
  });

  it("«Sjekk oppsettet» lander på lydkortet i INNSTILLINGER uten å vente", () => {
    expect(handleTrayAction("run-preflight")).toBe(true);
    expect(pendingAction.value).toBeNull();
    expect(route.value).toMatchObject({ page: "settings", anchor: "sound" });
  });

  it("ignores an id it does not know instead of throwing inside a listener", () => {
    expect(handleTrayAction("teleport")).toBe(false);
    expect(handleTrayAction(undefined)).toBe(false);
    expect(handleTrayAction({ action: "start-recording" })).toBe(false);
  });
});
