// Kilde-vakten bak `useReceipt`: kvitteringen har ÉN nedtelling.
//
// ## Feilklassen dette lukker
//
// «Lagret ✓» er en KVITTERING, ikke en tilstand. `useSetting` har alltid ryddet
// den opp igjen etter `SAVED_CHIP_MS`. De tolv flatene som ikke gikk gjennom
// `useSetting` — enhetsvalget, mappevalget, kameravelgeren, de to skjemaene med
// Lagre/Avbryt, SMTP-passordet, telemetriraden, motorvalget, de to tallbryterne
// — skrev hver sin `useState<Receipt>` og satte `"saved"` uten noen nedtelling.
// Resultatet var et «Lagret ✓» som ble stående til siden ble forlatt, på tolv
// skjermer samtidig, og som derfor sluttet å bety «det du nettopp gjorde
// landet».
//
// Å fikse tolv kallsteder er en instans-fiks; den trettende skrives i morgen.
// Dette er klasse-fiksen: kvitteringens tilstand og timer bor i `useReceipt`,
// og ingen annen produksjonsfil under `app/` får holde sin egen.
//
// Kilde-parset, i husets vakt-stil (`settings-store-pin.test.ts`,
// tuning-report-ratsjetten): egenskapen handler om hva koden SIER, ikke om hva
// én kjøring tilfeldigvis gjør — og en oppførselstest kan uansett ikke se en
// timer som aldri ble armet.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");
const APP_ROOT = join(REPO_ROOT, "app");
/** Det delte inventaret. `SAVED_CHIP_MS` er DEFINERT der (`ui/bind-setting-core`),
 *  så det er ikke et kallsted og teller ikke som en andre nedtelling. */
const LIB_PREFIX = "app/lib/";

/** Kilden uten kommentarer — vakten handler om KODE. En kommentar som forklarer
 *  hvorfor kvitteringen flyttet er dokumentasjon av endringen, ikke et brudd. */
function codeOf(path: string): string {
  return readFileSync(path, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "")
    .replace(/ \/\/[^"'`\n]*$/gm, "");
}

/** Hver .ts/.tsx under `app/`, uten tester (en test får gjerne nevne det den
 *  tester). */
function appSources(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.tsx?$/.test(p) && !/\.test\.tsx?$/.test(p)) out.push(p);
    }
  };
  if (existsSync(APP_ROOT)) walk(APP_ROOT);
  return out;
}

const rel = (p: string): string => relative(REPO_ROOT, p);

/** Den ene fila som har lov: hjelperen selv. */
const HELPER = "app/settings/use-receipt.ts";

describe("kvitteringen har én nedtelling", () => {
  it("vakten ser faktisk på både skjermene og innstillingslaget", () => {
    // Den ene måten hele fila kan bli grønn av tomhet: treet slutter å bli
    // gått, og alt under begynner å passere av feil grunn.
    const scanned = appSources().map(rel);
    expect(scanned).toContain(HELPER);
    expect(scanned.some((p) => p.endsWith(".tsx"))).toBe(true);
    expect(scanned.some((p) => p.startsWith("app/pages/setup/"))).toBe(true);
  });

  it("ingen annen kilde holder sin egen kvitterings-tilstand", () => {
    // `useState<Receipt…>` er formen hver av de tolv hadde. Finner du den
    // utenfor hjelperen, er det en trettende kvittering som blir stående.
    const offenders = appSources()
      .filter((p) => rel(p) !== HELPER)
      .filter((p) => /useState<\s*Receipt/.test(codeOf(p)))
      .map(rel);
    expect(
      offenders,
      "disse holder sin egen «Lagret ✓»-tilstand — bruk useReceipt(), " +
        "ellers blir kvitteringen stående til siden forlates",
    ).toEqual([]);
  });

  it("nedtellingens varighet leses ETT sted", () => {
    // `SAVED_CHIP_MS` er definert i det delte inventaret og skal ha nøyaktig
    // én leser i skallet. En andre leser er en andre timer, og to timere over
    // det samme ordet er hvordan «forbigående» blir «noen ganger».
    const readers = appSources()
      .filter((p) => !rel(p).startsWith(LIB_PREFIX))
      .filter((p) => codeOf(p).includes("SAVED_CHIP_MS"))
      .map(rel);
    expect(readers).toEqual([HELPER]);
  });
});
