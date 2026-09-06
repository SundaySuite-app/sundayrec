// Invarianten `VuMeter` hviler på, skrevet der den gjelder.
//
// ## Regelen
//
// Rust EIER lydenheten. Når `start_recording` åpner den, stopper backenden
// VU-strømmen selv — og en `start_vu` som kommer etterpå ber om nøyaktig den
// enheten opptaket holder, midt i en gudstjeneste. Derfor: en måler som leser
// den DELTE strømmen (`acquireVuFeed`, altså uten `source`) må kunne slås av,
// og den må slås av mens det tas opp.
//
// `use-vu-word.ts` har regelen skrevet ned og bruker den (`active`);
// `RecordPage` bruker den (`off={source.kind === "no-source" || live || …}`).
// `SoundPage` — den ENE skjermen som finnes nettopp for å lytte på enheten —
// hadde den ikke, så måleren der ba om enheten mens opptaket gikk. Det er
// samme klasse som Qu-5-hendelsen 2026-07-31, bare fra den andre siden.
//
// ## Hvorfor en KILDE-vakt og ikke en oppførselstest
//
// Egenskapen er «hvert kallsted har tatt stilling til dette», og det er en
// egenskap ved koden, ikke ved én kjøring: en oppførselstest kan bare dekke de
// målerne den vet om, og det er nøyaktig den trettende som glemmer prop-en.
// Node-gaten har heller ingen DOM, så en montert måler finnes ikke å måle.
//
// En måler med `source` er UNNTAKET og skal ikke ha `off`: opptaksoverlegget
// leser motorens egen telemetri (`recording://levels`) og skal nettopp vise
// noe MENS det tas opp. Vakten spør derfor bare de som deler strømmen.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..", "..");
const APP_ROOT = join(REPO_ROOT, "app");

/** Hver .tsx under `app/`, uten tester. */
function componentFiles(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.tsx$/.test(p) && !/\.test\.tsx$/.test(p)) out.push(p);
    }
  };
  if (existsSync(APP_ROOT)) walk(APP_ROOT);
  return out;
}

/** Hvert `<VuMeter …/>`-element i en kilde, som råtekst. */
function vuMeterElements(source: string): string[] {
  return [...source.matchAll(/<VuMeter\b[\s\S]*?\/>/g)].map((m) => m[0]);
}

const rel = (p: string): string => relative(REPO_ROOT, p);

describe("VuMeter — hvert kallsted har tatt stilling til `off`", () => {
  it("vakten finner faktisk målerne", () => {
    // En vakt som ikke ser noen målere er grønn av tomhet, og ville forblitt
    // grønn gjennom hele den refaktoreringen som fjernet prop-en.
    const found = componentFiles().flatMap((p) =>
      vuMeterElements(readFileSync(p, "utf8")),
    );
    expect(found.length).toBeGreaterThanOrEqual(3);
  });

  it("en måler på den DELTE strømmen bærer alltid `off`", () => {
    const offenders: string[] = [];
    for (const file of componentFiles()) {
      for (const element of vuMeterElements(readFileSync(file, "utf8"))) {
        // `source=` er unntaket: den måleren leser opptaksmotorens egen
        // telemetri og skal vise noe mens det tas opp.
        if (/\bsource=/.test(element)) continue;
        if (!/\boff=/.test(element)) offenders.push(rel(file));
      }
    }
    expect(
      offenders,
      "disse monterer en VuMeter på den delte VU-strømmen uten `off` — " +
        "den ber da om lydenheten mens opptaket holder den " +
        "(se toppen av denne fila, og `use-vu-word.ts`)",
    ).toEqual([]);
  });
});
