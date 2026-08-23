import { parseGoto } from "@lib/goto-core";
import { describe, expect, it } from "vitest";

import {
  handleTrayAction,
  navigate,
  PAGE_ALIASES,
  pendingAction,
  resolveRoute,
  route,
  TAB_ALIASES,
  type Route,
} from "./router";

/** What a `?goto=` string resolves to, end to end: the shim parses it with
 *  `parseGoto` and hands the result straight to `navigate`. */
function fromGoto(search: string): Route {
  const target = parseGoto(search);
  if (!target) throw new Error(`«${search}» parsed to nothing`);
  return resolveRoute(target.page, { tab: target.tab, highlight: false });
}

describe("every ?goto= form that exists in this repo", () => {
  // Collected from e2e/*.spec.ts, the screenshot passes and the retired ids
  // legacy/renderer/ui/navigate.ts still maps. A deep link that silently opens
  // the wrong screen is worse than one that fails loudly, so each one is a row.
  it.each([
    ["?goto=home", { page: "record" }],
    ["?goto=search", { page: "library" }],
    ["?goto=editor", { page: "library", tab: "edit" }],
    // Tidsplanen er tillegget «Ta opp automatisk» på NIVÅ 1 nå — et anker, ikke
    // en fane. Kalenderen og spesialopptakene er Avansert (P1b).
    ["?goto=schedule", { page: "setup", anchor: "auto", highlight: false }],
    ["?goto=settings", { page: "setup" }],
    ["?goto=settings:audio", { page: "setup", tab: "sound" }],
    // Kameraet er også et tillegg på nivå 1.
    [
      "?goto=settings:video",
      { page: "setup", anchor: "camera", highlight: false },
    ],
    ["?goto=settings:files", { page: "setup", tab: "folder" }],
    // Etter #139 inneholder Deling-fanen BARE «Varsler» — altså spørsmål 5.
    ["?goto=settings:sharing", { page: "setup", tab: "notify" }],
    // ⚠️ `advanced` bygges i P1b. Fram til da rendrer SetupPage nivå 1 for den;
    // `data-tab` står likevel på <main>, så lenken er ikke tapt.
    ["?goto=settings:general", { page: "setup", tab: "advanced" }],
    // The already-qualified spelling means the same thing as the bare one.
    ["?goto=settings:settings-general", { page: "setup", tab: "advanced" }],
    // Retired before the 7→5 tab fold; legacy maps them onward, so do we.
    ["?goto=settings:notifications", { page: "setup", tab: "notify" }],
    ["?goto=settings:publish", { page: "setup", tab: "notify" }],
    // Percent-encoded, which is how e2e/harness.ts actually writes it.
    ["?goto=settings%3Aaudio", { page: "setup", tab: "sound" }],
  ] as Array<[string, Route]>)("%s", (search, expected) => {
    expect(fromGoto(search)).toEqual(expected);
  });

  it("an empty or absent goto is not this module's problem", () => {
    // `parseGoto` answers null for both; the router is never asked.
    expect(parseGoto("")).toBeNull();
    expect(parseGoto("?goto=")).toBeNull();
  });
});

describe("resolveRoute", () => {
  it("passes a NEW tab id through untouched", () => {
    // S1b's own ids are already in the new namespace; only the old ones need
    // translating.
    expect(resolveRoute("setup", { tab: "sound" })).toEqual({
      page: "setup",
      tab: "sound",
    });
  });

  it("highlights an anchor by default, and obeys an explicit false", () => {
    expect(resolveRoute("setup", { anchor: "device" })).toEqual({
      page: "setup",
      anchor: "device",
      highlight: true,
    });
    expect(
      resolveRoute("setup", { anchor: "device", highlight: false }).highlight,
    ).toBe(false);
  });

  it("lets an explicit anchor win over the alias's default", () => {
    expect(
      resolveRoute("settings", { tab: "settings-video", anchor: "flip" }),
    ).toEqual({
      page: "setup",
      anchor: "flip",
      highlight: true,
    });
  });

  it("falls back to TA OPP for an unknown page rather than showing nothing", () => {
    expect(resolveRoute("atlantis")).toEqual({ page: "record" });
  });

  it("has no alias that points at a page that does not exist", () => {
    // A guard on the tables themselves: a typo here is a deep link that lands
    // on the fallback and looks like the user mis-clicked.
    const pages = new Set(["record", "library", "setup"]);
    for (const page of Object.values(PAGE_ALIASES))
      expect(pages).toContain(page);
    for (const target of Object.values(TAB_ALIASES)) {
      expect(pages).toContain(target.page);
    }
  });

  it("maps every settings tab the legacy shell has", () => {
    // index.html's data-tab values, plus the two retired ids.
    for (const tab of [
      "settings-audio",
      "settings-video",
      "settings-files",
      "settings-sharing",
      "settings-general",
      "settings-publish",
      "settings-notifications",
    ]) {
      expect(TAB_ALIASES[tab], `${tab} has nowhere to go`).toBeDefined();
    }
  });

  it("peker bare på faner som FINNES i det nye skallet", () => {
    // Vakten på tabellen etter P1a: de fem spørsmålene er bygget, `advanced`
    // er P1b sin og er den ENESTE som får peke på noe som ikke finnes ennå.
    // En sjette plassholder skal ikke kunne sige inn ubemerket.
    const built = new Set(["sound", "folder", "quality", "church", "notify"]);
    const notBuiltYet = new Set(["advanced", "edit"]);
    for (const [id, target] of Object.entries(TAB_ALIASES)) {
      if (!target.tab) continue;
      expect(
        built.has(target.tab) || notBuiltYet.has(target.tab),
        `«${id}» peker på fanen «${target.tab}», som ingen side rendrer`,
      ).toBe(true);
    }
  });
});

describe("navigate", () => {
  it("writes the route signal", () => {
    navigate("home");
    expect(route.value).toEqual({ page: "record" });
    navigate("settings", { tab: "settings-audio" });
    expect(route.value).toEqual({ page: "setup", tab: "sound" });
  });
});

describe("tray actions", () => {
  it("arms the signal AND goes to the page that owns the action", () => {
    for (const [action, page] of [
      ["start-recording", "record"],
      ["stop-recording", "record"],
      ["open-recordings-folder", "library"],
      ["run-preflight", "record"],
      ["run-diagnostics", "setup"],
    ] as const) {
      expect(handleTrayAction(action)).toBe(true);
      expect(pendingAction.value).toBe(action);
      expect(route.value.page).toBe(page);
    }
    pendingAction.value = null;
  });

  it("ignores an id it does not know instead of throwing inside a listener", () => {
    // A tray from a newer build must not take the renderer down.
    expect(handleTrayAction("teleport")).toBe(false);
    expect(handleTrayAction(undefined)).toBe(false);
    expect(handleTrayAction({ action: "start-recording" })).toBe(false);
  });
});
