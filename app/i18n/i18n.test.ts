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

  it("offers all seven languages, in the ALL_LOCALES order", () => {
    // F2-S6: the translation round filled sv/da/de/fr/pl, `PAUSED_KEYS` is
    // empty, and the picker offers every catalogue that exists. Pinned as a
    // LIST, not as `toEqual(ALL_LOCALES)`: the order is what the picker shows,
    // and "active" is still its own decision — see `ACTIVE_LOCALES`.
    expect([...ACTIVE_LOCALES]).toEqual([
      "no",
      "en",
      "sv",
      "da",
      "de",
      "fr",
      "pl",
    ]);
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

  // `app.language.<code>` hadde bare de to AKTIVE kodene. Hullet var latent
  // mens fem språk stod pauset — men `tDyn` KASTER i DEV på en suffiks-bom og
  // rendrer en tom etikett i prod, så det ville blitt en tom eller krasjende
  // valgboks den dagen de kom i bruk. Den dagen er nå (F2-S6): alle sju står i
  // `ACTIVE_LOCALES`, og hver av dem må ha et navn i hver av de sju
  // katalogene — 49 oppslag, ikke to.
  it.each([...ALL_LOCALES])(
    "språkvelgeren har et navn for «%s», i alle sju katalogene",
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
  //
  // F2-S6: alle sju, ikke to. Setningen er den ene personvernopplysningen i
  // appen som IKKE har en bryter ved siden av seg, og en oversettelse som
  // korter den ned til «lyd fra før du trykket Start» tar den bort for alle som
  // leser appen på det språket — uten at noen gate ser forskjell på en kortere
  // setning og en fattigere.
  it.each([
    ["no", /mikrofonen åpen/i],
    ["en", /microphone open/i],
    ["sv", /mikrofonen öppen/i],
    ["da", /mikrofonen åben/i],
    ["de", /Mikrofon im Hintergrund offen/i],
    ["fr", /microphone ouvert/i],
    ["pl", /mikrofon otwarty/i],
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
    // F2-S6: alle sju er aktive, så et lagret språk brukes som det står. Den
    // gamle nabospråk-mappingen (sv/da → no, resten → en) hørte pausen til, og
    // ville i dag ha vært en app som stille nekter å starte på det språket
    // brukeren faktisk valgte.
    ["svensk", "sv", "sv"],
    ["dansk", "da", "da"],
    ["tysk", "de", "de"],
    ["fransk", "fr", "fr"],
    ["polsk", "pl", "pl"],
    // …men en verdi som ikke er en av de sju er fortsatt ikke et språk.
    // `settings.language` er `string | null` i wire-typen.
    ["noe helt annet", "kv", "no"],
    ["tom streng", "", "no"],
  ])("startup locale for %s", (_name, stored, expected) => {
    expect(resolveStartupLocale(stored)).toBe(expected);
  });
});
