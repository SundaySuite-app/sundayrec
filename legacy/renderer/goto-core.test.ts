import { describe, expect, it } from "vitest";

import { parseGoto, type GotoTarget } from "./goto-core";

// Every `?goto=` form that EXISTS in this repo, taken from the specs that use
// it, plus the shapes the shim's normalisation was written for. If a spec adds
// a new form it belongs in this table — the table is the contract the second
// shell (app/) has to honour too.
const CASES: Array<[string, GotoTarget | null, string]> = [
  // e2e/recorder.spec.ts, e2e/i18n-live-surfaces.spec.ts, telemetry-preview
  ["?goto=home", { page: "home" }, "plain page"],
  // e2e/history.spec.ts — Historikk is `search`, there is no `history` page
  ["?goto=search", { page: "search" }, "plain page (Historikk)"],
  // e2e/editor.spec.ts
  ["?goto=editor", { page: "editor" }, "plain page"],
  // e2e/settings.spec.ts, system-support, no-live-surface, i18n-live-surfaces
  [
    "?goto=settings:audio",
    { page: "settings", tab: "settings-audio" },
    "bare tab id is qualified with the page",
  ],
  [
    "?goto=settings:sharing",
    { page: "settings", tab: "settings-sharing" },
    "bare tab id is qualified with the page",
  ],
  // e2e/auto-update.spec.ts, update-channel, settings-migration, settings-seam
  [
    "?goto=settings:general",
    { page: "settings", tab: "settings-general" },
    "bare tab id is qualified with the page",
  ],
  // e2e/settings.spec.ts's deep-link pin — already qualified, must not double
  [
    "?goto=settings:settings-general",
    { page: "settings", tab: "settings-general" },
    "an already-qualified tab id is passed through",
  ],
  // e2e/settings.spec.ts — retired id from before the 7→5 tab fold; navigateTo's
  // TAB_ALIASES maps it onward, so the parser only has to qualify it.
  [
    "?goto=settings:notifications",
    { page: "settings", tab: "settings-notifications" },
    "retired tab id survives the parse (TAB_ALIASES maps it later)",
  ],
  // e2e/onboarding.spec.ts boots WITHOUT the param on purpose — the wizard is
  // unreachable when `?goto=` is present, because it forces onboardingDone.
  ["", null, "no query string at all"],
  ["?fixtures=1", null, "some other param"],
  // The easy-to-lose case: present but empty was FALSY in the old raw-string
  // code, so it navigated nowhere AND did not skip onboarding.
  ["?goto=", null, "present but empty"],
];

describe("parseGoto", () => {
  for (const [search, expected, why] of CASES) {
    it(`${search || "(empty)"} → ${JSON.stringify(expected)} — ${why}`, () => {
      expect(parseGoto(search)).toEqual(expected);
    });
  }

  it("accepts a search string without the leading ?", () => {
    expect(parseGoto("goto=home")).toEqual({ page: "home" });
  });

  it("decodes percent-encoding — the harness writes encodeURIComponent()", () => {
    // e2e/harness.ts: `/?goto=${encodeURIComponent(opts.goto)}`, which turns
    // `settings:audio` into `settings%3Aaudio`. If the parse ever stopped
    // decoding, EVERY tabbed spec would land on a page called
    // "settings%3Aaudio" and the failure would look like a missing page.
    expect(parseGoto("?goto=settings%3Aaudio")).toEqual({
      page: "settings",
      tab: "settings-audio",
    });
  });

  it("ignores anything after a second colon, as the destructuring always did", () => {
    expect(parseGoto("?goto=settings:audio:extra")).toEqual({
      page: "settings",
      tab: "settings-audio",
    });
  });

  it("treats a trailing colon as no tab", () => {
    expect(parseGoto("?goto=settings:")).toEqual({ page: "settings" });
  });

  it("is pure — no reads of location, no writes anywhere", () => {
    const first = parseGoto("?goto=settings:audio");
    expect(parseGoto("?goto=settings:audio")).toEqual(first);
  });
});
