/**
 * «Legg ut» — kanalnavnet, lenkesjekken og beskrivelsesmalen.
 *
 * Lenkesjekken leser de SAMME vektorene som `custom_upload_url` i kjernen
 * (`crates/sundayrec-core/tests/fixtures/custom-upload-url.json`). Går denne
 * rød og ikke den, har Oppsett begynt å kalle en lenke god som bakenden
 * nekter å åpne — eller omvendt.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  channelName,
  customUrlProblem,
  PUBLISH_TARGETS,
  renderDescription,
  type DescriptionFields,
} from "./publish-core";

describe("kanalene", () => {
  it("SoundCloud står først — det er standarden", () => {
    expect(PUBLISH_TARGETS[0]).toBe("soundcloud");
    expect(PUBLISH_TARGETS).toContain("none");
  });

  it("produktnavnene er de samme på alle språk, ordene kommer fra katalogen", () => {
    expect(channelName("soundcloud")).toBe("SoundCloud");
    expect(channelName("youtube")).toBe("YouTube");
    expect(channelName("spotify")).toBe("Spotify");
    expect(channelName("custom")).toBeNull();
    expect(channelName("none")).toBeNull();
  });
});

describe("den egne lenken speiler kjernen", () => {
  const vectors = JSON.parse(
    readFileSync(
      join(
        import.meta.dirname,
        "../../crates/sundayrec-core/tests/fixtures/custom-upload-url.json",
      ),
      "utf8",
    ),
  ) as Array<{ url: string; ok: boolean }>;

  it("fixturen har vektorene sine", () => {
    expect(vectors.length).toBeGreaterThanOrEqual(15);
  });

  for (const v of vectors) {
    it(`${JSON.stringify(v.url)} → ${v.ok ? "godtatt" : "avvist"}`, () => {
      expect(customUrlProblem(v.url) === null).toBe(v.ok);
    });
  }

  it("grunnen skiller «ikke https» fra «ikke en adresse»", () => {
    expect(customUrlProblem("")).toBe("empty");
    expect(customUrlProblem("http://kirken.no/")).toBe("notHttps");
    expect(customUrlProblem("kirken.no")).toBe("notHttps");
    expect(customUrlProblem("https://soundcloud.com@example.net/")).toBe(
      "invalid",
    );
    expect(customUrlProblem("https://kirken.no/a b")).toBe("invalid");
  });
});

describe("beskrivelsesmalen", () => {
  const base: DescriptionFields = {
    title: "Den gode hyrde",
    speaker: "Kari Nordmann",
    date: "2026-09-27",
    church: "Sentrumskirken",
    locale: "no",
  };

  it("fyller inn de fire feltene, på norsk og engelsk", () => {
    expect(
      renderDescription("«{tittel}» — {taler}, {kirke}, {dato}.", base),
    ).toBe(
      "«Den gode hyrde» — Kari Nordmann, Sentrumskirken, 27. september 2026.",
    );
    expect(
      renderDescription("{title} by {speaker} ({church}, {date})", {
        ...base,
        locale: "en",
      }),
    ).toBe(
      "Den gode hyrde by Kari Nordmann (Sentrumskirken, September 27, 2026)",
    );
  });

  it("store og små bokstaver teller ikke", () => {
    expect(renderDescription("{Taler} / {KIRKE}", base)).toBe(
      "Kari Nordmann / Sentrumskirken",
    );
  });

  it("et felt vi ikke kjenner, blir stående — så skrivefeilen synes", () => {
    expect(renderDescription("{bibeltekst}: {tittel}", base)).toBe(
      "{bibeltekst}: Den gode hyrde",
    );
  });

  it("et tomt felt etterlater ikke doble mellomrom, og linjeskift beholdes", () => {
    expect(
      renderDescription("Preken  {taler}  i {kirke}\nMer på kirken.no  ", {
        ...base,
        speaker: "",
      }),
    ).toBe("Preken i Sentrumskirken\nMer på kirken.no");
  });

  it("en ukjent dato blir tom, ikke «Invalid Date»", () => {
    expect(renderDescription("Dato: {dato}", { ...base, date: null })).toBe(
      "Dato:",
    );
    expect(renderDescription("Dato: {dato}", { ...base, date: "tull" })).toBe(
      "Dato:",
    );
  });

  it("datoen er opptaksdagen i lokal tid, ikke UTC-gårsdagen", () => {
    // `new Date("2026-01-01")` er midnatt UTC — 31. desember vest for
    // Greenwich. Malen skal si 1. januar uansett hvor maskinen står.
    expect(
      renderDescription("{dato}", {
        ...base,
        date: "2026-01-01",
        locale: "no",
      }),
    ).toBe("1. januar 2026");
  });
});
