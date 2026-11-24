/**
 * Forhåndssjekk-raden på OPPTAK: hvilken lydenhet sier den mangler?
 *
 * Funnet er skrevet slik motoren sender det (`serde_json`, camelCase).
 *
 * ## MUTASJONSPRØVEN
 *
 * La `preflightText` returnere `f.message` uansett (den ENGELSKE reserven):
 * «viser katalogens setning» blir rød. Slett `deviceMissing` fra én av de sju
 * katalogene: `app/i18n/backend-codes.test.ts` og sjuspråkstesten under blir røde.
 */

import { afterEach, describe, expect, it } from "vitest";

import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";

import { ALL_LOCALES, setLocale, type Locale } from "../../i18n";
import { preflightText } from "./preflight-text";

/** Lydenheten fra innstillingene mangler — koden uten parametre. */
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
  it("enheten som mangler beholder akkurat den teksten den alltid har hatt", () => {
    expect(preflightText(GLOBAL_MISSING)).toBe(
      "Lydkilden som er valgt i innstillingene er ikke tilkoblet.",
    );
  });

  it("viser katalogens setning, ikke motorens engelske reserve", () => {
    // `message` er reserven for et funn UTEN kode. Et funn med kode skal aldri
    // vise den — det er hele poenget med at oppslaget skjer ved render.
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
    "sier det på brukerens språk: %s har ingen ufylt plassholder",
    async (lang: Locale) => {
      await setLocale(lang);
      const global = preflightText(GLOBAL_MISSING);
      expect(global.trim().length).toBeGreaterThan(0);
      expect(global).not.toMatch(/\{\w+\}/);
    },
  );
});
