/**
 * Forhåndssjekk-raden på OPPTAK: hvilken lydenhet sier den mangler?
 *
 * Gapet (review av #303): planleggeren sjekket spesialopptakets egen lydenhet og
 * NAVNGA den i OS-varselet og i banneret — men raden på OPPTAK slo opp
 * `deviceMissing` og sa «Lydenheten som er valgt i innstillingene er ikke
 * tilkoblet», om en enhet ingen hadde valgt der. Funnet bærer nå en egen kode
 * (`specialDeviceMissing`) med navnet som DATA i `params.device`.
 *
 * Funnene under er skrevet slik motoren sender dem (`serde_json`, camelCase) —
 * `sundayrec-core`s `the_named_finding_serialises_its_code_and_device_for_the_renderer`
 * står for den andre halvdelen av skjøten.
 *
 * ## MUTASJONSPRØVEN
 *
 * La `preflightText` returnere `f.message` uansett (den ENGELSKE reserven), eller
 * slå opp `deviceMissing` for begge kodene: «navngir enheten» og «sier det på
 * brukerens språk» blir røde. Slett `specialDeviceMissing` fra én av de sju
 * katalogene: `app/i18n/backend-codes.test.ts` og sjuspråkstesten under blir røde.
 */

import { afterEach, describe, expect, it } from "vitest";

import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";

import { ALL_LOCALES, setLocale, type Locale } from "../../i18n";
import { preflightText } from "./preflight-text";

/** Spesialopptakets lydenhet mangler — som `scheduler://preflight` sender den. */
const SPECIAL_MISSING: PreflightFinding = {
  severity: "error",
  category: "device",
  code: "specialDeviceMissing",
  message:
    'The audio device "Zoom H6" for the one-off recording is not connected. If it is not connected before the start, the usual audio device is used.',
  params: { device: "Zoom H6" },
};

/** Den GLOBALE lydenheten mangler — det en ukentlig slot alltid har sendt: koden
 *  uten parametre. */
const GLOBAL_MISSING: PreflightFinding = {
  severity: "error",
  category: "device",
  code: "deviceMissing",
  message: "The audio device selected in settings is not connected.",
  params: {},
};

afterEach(async () => {
  await setLocale("no");
});

describe("forhåndssjekk-radens setning for en manglende lydenhet", () => {
  it("navngir spesialopptakets enhet, med samme ord som OS-varselet", () => {
    expect(preflightText(SPECIAL_MISSING)).toBe(
      "Lydenheten «Zoom H6» for spesialopptaket er ikke tilkoblet. " +
        "Kobles den ikke til før start, tas opptaket fra den vanlige lydenheten.",
    );
  });

  it("den globale enheten beholder akkurat den teksten den alltid har hatt", () => {
    expect(preflightText(GLOBAL_MISSING)).toBe(
      "Lydenheten som er valgt i innstillingene er ikke tilkoblet.",
    );
  });

  it("en ukentlig slot er byte-lik: ingen enhetsnavn lekker inn i den globale setningen", () => {
    // Samme funn som før — `params` er tom, og setningen har ingen plassholder.
    const text = preflightText({ ...GLOBAL_MISSING, params: {} });
    expect(text).not.toContain("{");
    expect(text).not.toContain("spesialopptak");
  });

  it("viser katalogens setning, ikke motorens engelske reserve", () => {
    // `message` er reserven for et funn UTEN kode. Et funn med kode skal aldri
    // vise den — det er hele poenget med at oppslaget skjer ved render.
    expect(preflightText(SPECIAL_MISSING)).not.toContain("one-off recording");
    expect(preflightText(GLOBAL_MISSING)).not.toContain("selected in settings");
  });

  it("et funn uten kode viser fortsatt sin egen tekst", () => {
    // De tre `buildHealthFindings` lager selv — allerede på brukerens språk.
    expect(
      preflightText({
        severity: "warn",
        category: "device",
        code: null,
        message: "Mikrofontilgang er avslått",
        params: {},
      }),
    ).toBe("Mikrofontilgang er avslått");
  });

  it.each(ALL_LOCALES)(
    "sier det på brukerens språk: %s navngir enheten og har ingen ufylt plassholder",
    async (lang: Locale) => {
      await setLocale(lang);
      const special = preflightText(SPECIAL_MISSING);
      const global = preflightText(GLOBAL_MISSING);

      expect(special, `${lang}: navnet står i setningen`).toContain("Zoom H6");
      expect(special, `${lang}: {device} ble ikke fylt`).not.toMatch(/\{\w+\}/);
      // De to setningene sier ikke det samme — ellers er koden bare et navn.
      expect(special).not.toBe(global);
      // Den globale setningen har intet navn å fylle, og har aldri hatt et.
      expect(global).not.toContain("Zoom H6");
      expect(global).not.toMatch(/\{\w+\}/);
    },
  );

  it("navnet kommer fra `params.device`, ikke fra `message`", () => {
    // Lik `message`, forskjellig `params`: bare data kan avgjøre hva som står.
    const other = preflightText({
      ...SPECIAL_MISSING,
      params: { device: "USB-mikrofon" },
    });
    expect(other).toContain("«USB-mikrofon»");
    expect(other).not.toContain("Zoom H6");
  });
});
