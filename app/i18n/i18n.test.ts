import { effect } from "@preact/signals";
import { describe, expect, it } from "vitest";

import {
  ACTIVE_LOCALES,
  ALL_LOCALES,
  locale,
  resolveStartupLocale,
  setLocale,
  t,
  tDyn,
  tf,
  tn,
} from "./index";

describe("app i18n", () => {
  // The probe key is one the SHELL renders — the rail's first destination.
  // It was `nav.home` until fase B, which was legacy copy nothing painted any
  // more; the prune that removed 653 such keys took it, and this test went red
  // for the right reason. A probe that outlives the string it probes is a test
  // measuring the catalogue instead of the app.
  it("starts on the catalogue the shell bundles eagerly", () => {
    expect(locale.value).toBe("no");
    expect(t("app.page.record")).toBe("Opptak");
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
      seen.push(t("app.page.record"));
    });
    expect(seen).toEqual(["Opptak"]);

    await setLocale("en");

    expect(locale.value).toBe("en");
    expect(t("app.page.record")).toBe("Record");
    expect(seen, "the effect did not re-run on the language change").toEqual([
      "Opptak",
      "Record",
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
      const heading = t("app.page.record");
      const expected = lang === "en" ? "Record" : "Opptak";
      if (heading !== expected) mismatches.push(`${lang} → ${heading}`);
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
    expect(t("app.page.record")).toBe("Opptak");
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

  // `app.language.<code>` hadde bare de to AKTIVE kodene. Språkvelgeren viser
  // bare aktive språk i dag, så hullet var latent — men `tDyn` KASTER i DEV på
  // en suffiks-bom og rendrer en tom etikett i prod, så den dagen et pauset
  // språk tas i bruk (eller en flate viser navnet på det som står lagret) er
  // det en tom eller krasjende valgboks. Alle sju har et navn nå, i begge
  // katalogene.
  it.each([...ALL_LOCALES])(
    "språkvelgeren har et navn for «%s» — også de fem pausete",
    async (code) => {
      for (const shown of ACTIVE_LOCALES) {
        await setLocale(shown);
        expect(tDyn("app.language", code)).not.toBe("");
      }
      await setLocale("no");
    },
  );

  // Forhåndsbufferen står PÅ som standard (15 s), og det betyr at mikrofonen
  // holdes åpen i bakgrunnen på en fersk installasjon. Eiervalget «pre-roll på
  // og usynlig» står — men da må TEKSTEN si hva det innebærer, ellers er det
  // appen som holder mikrofonen åpen uten at noen sa fra. Rust-doccen advarte
  // ordrett; katalogen sa ingenting.
  it.each([
    ["no", /mikrofonen åpen/i],
    ["en", /microphone open/i],
  ] as Array<[(typeof ACTIVE_LOCALES)[number], RegExp]>)(
    "forhåndsbufferen sier at den holder mikrofonen åpen (%s)",
    async (lang, needle) => {
      await setLocale(lang);
      expect(t("app.setup.advanced.prerollDesc")).toMatch(needle);
      await setLocale("no");
    },
  );

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
