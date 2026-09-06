import { describe, expect, it } from "vitest";

import { dateTimeTitle, longDateTitle } from "./date-title";
import { DOT } from "./dot";

// Søndag 2. august 2026, kl. 11 — samme dato `ExportPage`s gamle filhode brukte
// som eksempel. Midt på dagen, ikke ved midnatt: en test som formaterer med
// `Intl` skal ikke kunne velte over til nabodøgnet bare fordi CI-maskinen står
// i en annen tidssone enn den som skrev testen. Bygget av lokale komponenter
// (ikke en ISO-streng med `Z`) av samme grunn `parseLocalIso` gjør det i
// `next-recording-core.ts`: en gudstjeneste har ikke en tidssone, den har et
// klokkeslett.
const SUNDAY = new Date(2026, 7, 2, 11, 0, 0).getTime();

describe("longDateTitle", () => {
  it("er Intls lange dato, med stor forbokstav på ukedagen", () => {
    // Sammenlignet mot RÅ `Intl`-utdata, ikke en hardkodet streng: det som
    // testes er at kapitaliseringen faktisk skjer, ikke hvilken ICU-versjon
    // eller tidssone maskinen som kjører testen har. `Intl` gir «søndag …»
    // med liten forbokstav — det er nøyaktig løgnen en overskrift ikke skal
    // videreføre.
    const raw = new Date(SUNDAY).toLocaleDateString("no", {
      weekday: "long",
      day: "numeric",
      month: "long",
      year: "numeric",
    });
    expect(raw[0]).not.toBe(raw[0].toLocaleUpperCase("no"));
    expect(longDateTitle(SUNDAY, "no")).toBe(
      raw[0].toLocaleUpperCase("no") + raw.slice(1),
    );
  });

  it("bruker språket som er sendt inn, ikke et fast ett", () => {
    const no = longDateTitle(SUNDAY, "no");
    const en = longDateTitle(SUNDAY, "en");
    expect(no).not.toBe(en);
    const rawEn = new Date(SUNDAY).toLocaleDateString("en", {
      weekday: "long",
      day: "numeric",
      month: "long",
      year: "numeric",
    });
    expect(en).toBe(rawEn[0].toLocaleUpperCase("en") + rawEn.slice(1));
  });
});

describe("dateTimeTitle", () => {
  it("er longDateTitle + DOT + klokkeslettet", () => {
    const time = new Date(SUNDAY).toLocaleTimeString("no", {
      hour: "2-digit",
      minute: "2-digit",
    });
    expect(dateTimeTitle(SUNDAY, "no")).toBe(
      `${longDateTitle(SUNDAY, "no")}${DOT}${time}`,
    );
  });

  it("bærer samme kapitalisering som longDateTitle alene", () => {
    // De tre gamle kopiene (ExportPage, LibraryPage) gjorde kapitaliseringen
    // og klokkeslett-sammenslåingen i ÉN funksjon hver; denne testen er
    // skjøten mellom de to som nå er delt.
    expect(
      dateTimeTitle(SUNDAY, "en").startsWith(longDateTitle(SUNDAY, "en")),
    ).toBe(true);
  });
});
