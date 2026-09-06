import { parseGoto } from "@lib/goto-core";
import { describe, expect, it } from "vitest";

import { isControlId } from "../pages/record/control-core";
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
    ["?goto=search", { page: "edit" }],
    // D3: Rediger er en DESTINASJON nå, ikke en fane inne i Bibliotek. Lenken
    // lander samme sted som før; den bærer bare ingen fane lenger.
    ["?goto=editor", { page: "edit" }],
    // Skallets egen sideid fram til D3. Den sto i `navigate("library")`-kall og
    // i skjermbildepassene, så den er et alias nå — tabellen utvides, aldri
    // krympes.
    ["?goto=library", { page: "edit" }],
    // D2: hver gammel fane-id er et ANKER i kontrollrommet på OPPTAK. Kortet
    // ankeret navngir foldes ut der; kalenderen og spesialopptakene er fortsatt
    // Avansert, som nå er Innstillinger-flaten.
    ["?goto=schedule", { page: "record", anchor: "auto", highlight: false }],
    ["?goto=settings", { page: "setup" }],
    [
      "?goto=settings:audio",
      { page: "record", anchor: "sound", highlight: false },
    ],
    [
      "?goto=settings:video",
      { page: "record", anchor: "camera", highlight: false },
    ],
    [
      "?goto=settings:files",
      { page: "record", anchor: "folder", highlight: false },
    ],
    // Etter #139 inneholder Deling-fanen BARE «Varsler» — altså spørsmål 5.
    [
      "?goto=settings:sharing",
      { page: "record", anchor: "notify", highlight: false },
    ],
    // Den gamle System-fanen er Innstillinger-flaten: kirkeprofil + Avansert,
    // og ingen fane — flaten er én.
    ["?goto=settings:general", { page: "setup" }],
    // The already-qualified spelling means the same thing as the bare one.
    ["?goto=settings:settings-general", { page: "setup" }],
    // Retired before the 7→5 tab fold; legacy maps them onward, so do we.
    [
      "?goto=settings:notifications",
      { page: "record", anchor: "notify", highlight: false },
    ],
    [
      "?goto=settings:publish",
      { page: "record", anchor: "notify", highlight: false },
    ],
    // Percent-encoded, which is how e2e/harness.ts actually writes it.
    [
      "?goto=settings%3Aaudio",
      { page: "record", anchor: "sound", highlight: false },
    ],
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
      page: "record",
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
    const pages = new Set(["record", "edit", "export", "setup"]);
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
    // Vakten på tabellen. Etter D3 har INGEN rad en fane: de gamle
    // innstillingsfanene er ankre i kontrollrommet på OPPTAK, og `editor` er
    // en destinasjon. Settet er TOMT med vilje — en plassholderfane skal ikke
    // kunne sige inn ubemerket, og den eneste måten et tomt sett kan bli
    // grønt på feil grunnlag er at tabellen selv er tom, som raden over
    // beviser at den ikke er.
    const built = new Set<string>();
    for (const [id, target] of Object.entries(TAB_ALIASES)) {
      if (!target.tab) continue;
      expect(
        built.has(target.tab),
        `«${id}» peker på fanen «${target.tab}», som ingen side rendrer`,
      ).toBe(true);
    }
  });

  it("hvert anker er et kort kontrollrommet faktisk folder ut", () => {
    // Skjøten mellom de to tabellene: ruteren lover et anker, `RecordPage`
    // folder ut kortet med det navnet. Et anker som ikke er en kort-id ville
    // rullet til ingenting og latt kortet stå lukket — en dyplenke som ser ut
    // som om den virket.
    for (const [id, target] of Object.entries(TAB_ALIASES)) {
      if (!target.anchor) continue;
      expect(
        isControlId(target.anchor),
        `«${id}» peker på ankeret «${target.anchor}», som ikke er et kort`,
      ).toBe(true);
      expect(target.page).toBe("record");
    }
  });
});

describe("navigate", () => {
  it("writes the route signal", () => {
    navigate("home");
    expect(route.value).toEqual({ page: "record" });
    navigate("library");
    expect(route.value).toEqual({ page: "edit" });
    navigate("settings", { tab: "settings-audio" });
    expect(route.value).toEqual({
      page: "record",
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
      ["open-recordings-folder", "edit"],
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
