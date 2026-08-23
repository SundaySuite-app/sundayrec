import { effect } from "@preact/signals";
import { describe, expect, it } from "vitest";

import {
  ACTIVE_LOCALES,
  locale,
  resolveStartupLocale,
  setLocale,
  t,
  tDyn,
  tf,
  tn,
} from "./index";

describe("app i18n", () => {
  it("starts on the catalogue the legacy shell bundles eagerly", () => {
    expect(locale.value).toBe("no");
    expect(t("nav.home")).toBe("Hjem");
  });

  it("offers only the two languages the redesign keeps translated", () => {
    // The other five are PAUSED, not gone — see legacy/locales/parity.test.ts.
    expect([...ACTIVE_LOCALES]).toEqual(["no", "en"]);
  });

  it("a signal change gives t() the new text, and wakes a subscriber", async () => {
    // The whole point of the signal: a reader that never mentions the locale
    // still re-runs when it changes. This is what a component gets for free.
    const seen: string[] = [];
    const dispose = effect(() => {
      seen.push(t("nav.home"));
    });
    expect(seen).toEqual(["Hjem"]);

    await setLocale("en");

    expect(locale.value).toBe("en");
    expect(t("nav.home")).toBe("Home");
    expect(seen, "the effect did not re-run on the language change").toEqual([
      "Hjem",
      "Home",
    ]);
    dispose();
  });

  it("never renders the new language with the old catalogue", async () => {
    // The ordering invariant, asserted the only way it can be: whatever the
    // signal says at the moment a subscriber runs, the catalogue must already
    // agree with it. A `setLocale` that flipped the signal first would show
    // one frame of Norwegian text under an English locale.
    const mismatches: string[] = [];
    const dispose = effect(() => {
      const lang = locale.value;
      const home = t("nav.home");
      const expected = lang === "en" ? "Home" : "Hjem";
      if (home !== expected) mismatches.push(`${lang} → ${home}`);
    });
    await setLocale("no");
    await setLocale("en");
    await setLocale("no");
    expect(mismatches).toEqual([]);
    dispose();
  });

  it("falls back to Norwegian for an unknown language, and says so", async () => {
    await setLocale("kv" as never);
    expect(locale.value).toBe("no");
    expect(t("nav.home")).toBe("Hjem");
  });

  it("tf interpolates and tn picks the count-aware form", async () => {
    await setLocale("no");
    expect(tf("guard.title", { what: "Bytte lydenhet" })).toBe(
      "Bytte lydenhet nå?",
    );
    expect(tn("guard.beforeRecording", 1)).toContain("1 minutt.");
    expect(tn("guard.beforeRecording", 4)).toContain("4 minutter.");
  });

  it("tDyn resolves a dynamic suffix under a static prefix", async () => {
    await setLocale("no");
    expect(tDyn("app.page", "record")).toBe("Opptak");
    await setLocale("en");
    expect(tDyn("app.page", "record")).toBe("Record");
    await setLocale("no");
  });

  it("tDyn throws in dev when the suffix misses", () => {
    // Loud beats a blank label: an empty heading survives a whole test round
    // because it looks like "that one is just empty".
    expect(() => tDyn("app.page", "nowhere")).toThrow(/finnes ikke/);
    expect(() => tDyn("app.nothing", "record")).toThrow(/finnes ikke/);
  });

  it.each([
    ["nothing stored", null, "no"],
    ["norsk", "no", "no"],
    ["engelsk", "en", "en"],
    // Paused languages pick the NEAREST active one rather than rendering the
    // redesigned strings as empty text.
    ["svensk", "sv", "no"],
    ["dansk", "da", "no"],
    ["tysk", "de", "en"],
    ["fransk", "fr", "en"],
    ["polsk", "pl", "en"],
    ["noe helt annet", "kv", "en"],
  ])("startup locale for %s", (_name, stored, expected) => {
    expect(resolveStartupLocale(stored)).toBe(expected);
  });
});
