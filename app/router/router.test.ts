import { parseGoto } from "@lib/goto-core";
import { describe, expect, it, vi } from "vitest";

import {
  closeGear,
  handleTrayAction,
  navigate,
  panel,
  pendingAction,
  resolveRoute,
  route,
  type Route,
  underlay,
} from "./router";

/** What a `?goto=` string resolves to, end to end: the shim parses it with
 *  `parseGoto` and hands the result straight to `navigate`. */
function fromGoto(search: string): Route {
  const target = parseGoto(search);
  if (!target) throw new Error(`«${search}» parsed to nothing`);
  return resolveRoute(target.page, { tab: target.tab, highlight: false });
}

describe("?goto= i de tre flatene", () => {
  it.each([
    ["?goto=record", { page: "record" }],
    ["?goto=recordings", { page: "recordings" }],
    // Fanen på OPPTAK er en fane (papirkurven), ikke et anker.
    ["?goto=recordings:trash", { page: "recordings", tab: "trash" }],
    // E2: `plan` er ikke lenger en side — den løses til TA OPP + stripa åpen.
    ["?goto=plan", { page: "record", panel: "strip" }],
    [
      "?goto=plan:schedule",
      { page: "record", panel: "strip", anchor: "schedule", highlight: false },
    ],
    ["?goto=settings", { page: "settings" }],
    // Ankrene som flyttet til lydkilde-arket løses til TA OPP + panel.
    [
      "?goto=settings:sound",
      { page: "record", panel: "source", anchor: "sound", highlight: false },
    ],
    [
      "?goto=settings:engine",
      {
        page: "record",
        panel: "source-more",
        anchor: "engine",
        highlight: false,
      },
    ],
    [
      "?goto=settings:autodelete",
      {
        page: "record",
        panel: "source-more",
        anchor: "autodelete",
        highlight: false,
      },
    ],
    // Den fullt kvalifiserte skrivemåten betyr det samme som den bare.
    [
      "?goto=settings:settings-camera",
      { page: "settings", anchor: "camera", highlight: false },
    ],
    // Percent-encoded, som e2e/harness.ts faktisk skriver den.
    [
      "?goto=settings%3Afolder",
      {
        page: "record",
        panel: "source-more",
        anchor: "folder",
        highlight: false,
      },
    ],
    // Varslene bor i ukestripa.
    [
      "?goto=settings:notify",
      { page: "record", panel: "strip", anchor: "notify", highlight: false },
    ],
    // Andre innstillingsankre åpner tannhjulet (INNSTILLINGER).
    [
      "?goto=settings:updates",
      { page: "settings", anchor: "updates", highlight: false },
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

  it("resolves old links to the panel that now owns them", () => {
    expect(resolveRoute("plan")).toEqual({ page: "record", panel: "strip" });
    expect(resolveRoute("settings", { anchor: "folder" })).toEqual({
      page: "record",
      panel: "source-more",
      anchor: "folder",
      highlight: true,
    });
    // En eksplisitt panel-opsjon bæres videre, men bare på TA OPP.
    expect(resolveRoute("record", { panel: "source" })).toEqual({
      page: "record",
      panel: "source",
    });
    expect(resolveRoute("recordings", { panel: "source" })).toEqual({
      page: "recordings",
    });
  });

  it("exposes the panel as a signal", () => {
    navigate("plan");
    expect(panel.value).toBe("strip");
    navigate("recordings");
    expect(panel.value).toBeUndefined();
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

  it("an unknown settings anchor opens the gear and keeps the anchor", () => {
    expect(resolveRoute("settings", { anchor: "licenses" })).toEqual({
      page: "settings",
      anchor: "licenses",
      highlight: true,
    });
  });
});

describe("navigate", () => {
  it("writes the route signal", () => {
    navigate("record");
    expect(route.value).toEqual({ page: "record" });
    navigate("recordings", { tab: "trash" });
    expect(route.value).toEqual({ page: "recordings", tab: "trash" });
    navigate("settings", { anchor: "language" });
    expect(route.value).toEqual({
      page: "settings",
      anchor: "language",
      highlight: true,
    });
  });
});

describe("tannhjulet", () => {
  it("husker siden under, og lukking går dit — uten panel og puls", () => {
    navigate("recordings", { tab: "trash" });
    navigate("settings");
    expect(underlay.value).toEqual({ page: "recordings", tab: "trash" });
    closeGear();
    expect(route.value).toEqual({ page: "recordings", tab: "trash" });
  });

  it("åpnes fra et åpent ark: arket blir ikke stående bak kortet", () => {
    navigate("record", { panel: "source" });
    navigate("settings");
    expect(underlay.value).toEqual({ page: "record" });
    closeGear();
    expect(route.value).toEqual({ page: "record" });
  });

  it("å gå fra tannhjulet til en annen side oppdaterer siden under", () => {
    navigate("record");
    navigate("settings");
    navigate("recordings");
    expect(underlay.value).toEqual({ page: "recordings" });
    closeGear();
    expect(route.value).toEqual({ page: "recordings" });
  });

  it("closeGear er en no-op når tannhjulet ikke er åpent", () => {
    navigate("record", { panel: "strip" });
    closeGear();
    expect(route.value).toEqual({ page: "record", panel: "strip" });
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

  it("«Sjekk oppsettet» åpner lydkilde-arket på TA OPP uten å vente", () => {
    expect(handleTrayAction("run-preflight")).toBe(true);
    expect(pendingAction.value).toBeNull();
    expect(route.value).toMatchObject({
      page: "record",
      panel: "source",
      anchor: "sound",
    });
  });

  it("ignores an id it does not know instead of throwing inside a listener", () => {
    expect(handleTrayAction("teleport")).toBe(false);
    expect(handleTrayAction(undefined)).toBe(false);
    expect(handleTrayAction({ action: "start-recording" })).toBe(false);
  });
});
