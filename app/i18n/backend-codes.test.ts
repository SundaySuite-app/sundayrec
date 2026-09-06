/**
 * Skjøten mellom motorens KODER og skallets katalog (F2-I18N-R2).
 *
 * ## Hvorfor denne fila finnes
 *
 * Runden flyttet brukervendt norsk prosa ut av Rust og erstattet den med
 * stabile koder. Det virker bare så lenge appen har en setning for hver kode,
 * på alle sju språk — og de to sidene vet ingenting om hverandre:
 *
 *   • Rust kan legge til en variant uten at noe i TypeScript merker det.
 *   • Katalogen kan miste en nøkkel uten at noe i Rust merker det.
 *
 * Det er skjøtefeilens form (`docs/reference`): begge sider korrekte hver for
 * seg, uenige i skjøten, begge grønne. Symptomet er ikke en feilmelding —
 * `tDyn` kaster bare i DEV; i prod rendrer den TOM TEKST. En kule uten tekst,
 * på flatene som forklarer hvorfor søndagen kanskje ikke blir tatt opp.
 *
 * ## Hvorfor bindingene og ikke `*.rs`
 *
 * `legacy/bindings/*.ts` er GENERERT av ts-rs fra de samme enumene, og
 * `npm run bindings:check` feiler hvis de er foreldet. Å lese dem er derfor
 * Rusts egen liste med ett ledd mindre å ta feil i enn en regex over kilden —
 * og en union kan leses uten en Rust-kompilator i vitest.
 *
 * ## MUTASJONSPRØVEN
 *
 * Legg en variant til i et av enumene i Rust uten å skrive setningen i
 * katalogene, eller slett en nøkkel i ett av de sju språkene: begge blir røde
 * her. Legg en nøkkel til som ingen kode peker på: også rød — en setning ingen
 * ser må likevel oversettes til sju språk og holdes i paritet.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { ALL_LOCALES } from "./index";

const ROOT = join(import.meta.dirname, "../..");

/** Medlemmene i en ts-rs-generert streng-union. */
function unionMembers(file: string): string[] {
  const src = readFileSync(join(ROOT, "legacy/bindings", file), "utf8");
  const decl = /export type \w+ =([^;]+);/.exec(src);
  if (!decl) throw new Error(`fant ingen type-erklæring i ${file}`);
  const members = [...decl[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  if (members.length === 0) throw new Error(`${file} er en tom union`);
  return members;
}

/** Undertreet en `tDyn`-prefiks peker på, i én katalog. */
function group(lang: string, path: string): Record<string, unknown> {
  const cat = JSON.parse(
    readFileSync(join(ROOT, "legacy/locales", `${lang}.json`), "utf8"),
  ) as Record<string, unknown>;
  const node = path
    .split(".")
    .reduce<unknown>(
      (o, k) =>
        o && typeof o === "object"
          ? (o as Record<string, unknown>)[k]
          : undefined,
      cat,
    );
  return (node ?? {}) as Record<string, unknown>;
}

/**
 * Hver rad: bindingsfila med kodene, og `tDyn`-prefikset appen slår dem opp
 * under. Legger en ny kodet flate seg til i Rust, hører den hjemme HER — det
 * er ett sted å huske, ikke ett per flate.
 */
const CONTRACTS: ReadonlyArray<readonly [string, string]> = [
  ["WakeIssue.ts", "app.setup.advanced.wakeIssue"],
  ["WakeRecommendation.ts", "app.setup.advanced.wakeAdvice"],
  ["PreflightCode.ts", "status.preflightCode"],
];

describe.each(CONTRACTS)("%s ⇢ %s", (file, prefix) => {
  const codes = unionMembers(file);

  it.each(ALL_LOCALES)("har en setning per kode i %s.json", (lang) => {
    const tree = group(lang, prefix);
    for (const code of codes) {
      const value = tree[code];
      expect(
        typeof value === "string" && value.length > 0,
        `${lang}.json mangler ${prefix}.${code}`,
      ).toBe(true);
    }
  });

  it.each(ALL_LOCALES)("har ingen nøkler UTOVER kodene i %s.json", (lang) => {
    expect(Object.keys(group(lang, prefix)).sort()).toEqual([...codes].sort());
  });

  it("selve lesingen finner noe — en tom union ville gjort alt over grønt", () => {
    expect(codes.length).toBeGreaterThan(0);
  });
});

/**
 * Plassholderne må overleve oversettelsen.
 *
 * `status.preflightCode.diskLow` bærer `{gb}`, og en oversetter som mister den
 * gir «Bare  GB ledig» — en setning som ser ferdig ut og mangler tallet som er
 * hele poenget. Norsk er fasiten; de seks andre må ha det samme settet.
 */
describe("plassholderparitet", () => {
  const holders = (s: string) =>
    [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

  it.each(CONTRACTS)("%s ⇢ %s", (file, prefix) => {
    const codes = unionMembers(file);
    const no = group("no", prefix);
    for (const lang of ALL_LOCALES) {
      const tree = group(lang, prefix);
      for (const code of codes) {
        expect(
          holders(String(tree[code] ?? "")),
          `${lang}.json / ${prefix}.${code}`,
        ).toEqual(holders(String(no[code] ?? "")));
      }
    }
  });
});
