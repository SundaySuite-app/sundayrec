#!/usr/bin/env node
/**
 * i18n-flertallsgate: holder `tn()`-nøkler, katalogene og CLDR-kategoriene i takt.
 *
 * Count-avhengige nøkler er ikke lenger flate strenger, men CLDR-grupper:
 *
 *     "trash.moved": { "one": "…", "other": "…" }                    // no/en/sv/da/de/fr
 *     "trash.moved": { "one": …, "few": …, "many": …, "other": … }   // pl
 *
 * Tre måter det kan gå stille galt på, og som denne gaten gjør høylytt:
 *
 *   1. `tn('x.y', n)` på en nøkkel som IKKE er en gruppe → `tn` faller tilbake
 *      til den flate strengen og alle språk får entallsformen for alt.
 *   2. En gruppe som mangler en kategori i ETT språk → `tn` faller tilbake til
 *      `other`, og polske brukere leser feil substantivform for 2–4 og 22–24.
 *      Ingen test krasjer; teksten er bare gal.
 *   3. En gruppe lest med `t()` i stedet for `tn()` → objektet er ikke en
 *      streng, så brukeren får reservestrengen (norsk) uansett språk.
 *
 * Påkrevde kategorier per språk regnes ut med `Intl.PluralRules`, ikke listet
 * for hånd: hver kategori et HELTALL kan treffe, pluss `other` som `tn`s
 * universelle reserve. Fransk `many` (n ≥ 1e6) og polsk `other` (brøk) er
 * derfor ikke påkrevd — ingen telling i denne appen kommer dit.
 *
 * Bruk:
 *   node scripts/check-i18n-plurals.mjs          # gate
 *   node scripts/check-i18n-plurals.mjs --list   # vis gruppene
 *
 * Mutasjonsvern: skriptet kjører først seg selv mot en innebygd fixture med
 * fasit. Sløyfer noen ut kategorisjekken, feiler selvtesten før gaten får
 * uttale seg — en gate som kan mutere til «alltid grønn» er ingen gate.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const LOCALE_DIR = path.join(ROOT, "legacy", "locales");
const RENDERER_DIR = path.join(ROOT, "legacy", "renderer");
const LANGS = ["no", "en", "sv", "da", "de", "fr", "pl"];
/** BCP-47 for plural data — mirrors i18n.ts `localeTag()`. */
const TAG = (lang) => (lang === "no" ? "nb-NO" : lang);

const CLDR = new Set(["zero", "one", "two", "few", "many", "other"]);

/** A plural group: keyed only by CLDR categories, and carrying `other`. */
export function isPluralGroup(v) {
  if (!v || typeof v !== "object" || Array.isArray(v)) return false;
  const keys = Object.keys(v);
  return (
    keys.length > 0 &&
    keys.every((k) => CLDR.has(k)) &&
    typeof v.other === "string"
  );
}

export function pluralGroupKeys(tree, prefix = "") {
  return Object.entries(tree).flatMap(([k, v]) => {
    if (isPluralGroup(v)) return [prefix + k];
    if (v && typeof v === "object" && !Array.isArray(v)) {
      return pluralGroupKeys(v, prefix + k + ".");
    }
    return [];
  });
}

export function requiredCategories(lang) {
  const rules = new Intl.PluralRules(TAG(lang));
  const cats = new Set(["other"]);
  for (let n = 0; n <= 1000; n++) cats.add(rules.select(n));
  return cats;
}

const lookup = (tree, key) =>
  key.split(".").reduce((o, k) => (o == null ? undefined : o[k]), tree);

// ── Kildeskanning ───────────────────────────────────────────────────────────

/** `tn('a.b'` and `ctx.tn('a.b'` — the count-aware call sites. */
const TN_RE = /(?:^|[^A-Za-z0-9_$.])(?:[A-Za-z0-9_$]+\.)?tn\(\s*'([^']+)'/g;
/** `t('a.b'` and `ctx.t('a.b'` — but NOT `tn(`, `tf(`, `tArr(`. */
const T_RE = /(?:^|[^A-Za-z0-9_$.])(?:[A-Za-z0-9_$]+\.)?t\(\s*'([^']+)'/g;

function sourceFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(p);
    return e.isFile() && p.endsWith(".ts") && !p.endsWith(".test.ts")
      ? [p]
      : [];
  });
}

export function scanKeys(source) {
  const tn = new Set();
  const t = new Set();
  for (const m of source.matchAll(TN_RE)) tn.add(m[1]);
  for (const m of source.matchAll(T_RE)) t.add(m[1]);
  return { tn, t };
}

// ── Selvtest (mutasjonsvern) ────────────────────────────────────────────────

function selfTest() {
  const problems = [];
  const say = (ok, what) => {
    if (!ok) problems.push(what);
  };

  say(isPluralGroup({ one: "a", other: "b" }), "group detection");
  say(!isPluralGroup({ one: "a" }), "group without `other` must not count");
  say(
    !isPluralGroup({ title: "a", other: "b" }),
    "non-CLDR key must disqualify",
  );
  say(!isPluralGroup("flat"), "a string is not a group");

  const pl = requiredCategories("pl");
  say(
    pl.has("few") && pl.has("many") && pl.has("one") && pl.has("other"),
    "Polish needs one/few/many/other",
  );
  const fr = requiredCategories("fr");
  say(!fr.has("many"), "French `many` is unreachable from an integer count");
  const nb = requiredCategories("no");
  say(
    nb.size === 2 && nb.has("one") && nb.has("other"),
    "Norwegian needs one/other",
  );

  const scanned = scanKeys(
    "tn('a.b', 1); ctx.tn('c.d', 2); t('e.f'); ctx.t('g.h'); tf('i.j', {}); tArr('k.l', [])",
  );
  say(scanned.tn.has("a.b") && scanned.tn.has("c.d"), "tn scan");
  say(scanned.t.has("e.f") && scanned.t.has("g.h"), "t scan");
  say(
    !scanned.t.has("a.b") && !scanned.t.has("i.j") && !scanned.t.has("k.l"),
    "t scan must not swallow tn/tf/tArr",
  );

  say(
    pluralGroupKeys({ a: { b: { one: "x", other: "y" } }, c: "z" }).join() ===
      "a.b",
    "nested group discovery",
  );

  if (problems.length) {
    console.error("check-i18n-plurals SELVTEST FEILET:");
    for (const p of problems) console.error("  ✗ " + p);
    process.exit(2);
  }
}

// ── Gate ────────────────────────────────────────────────────────────────────

function main() {
  selfTest();

  const trees = Object.fromEntries(
    LANGS.map((l) => [
      l,
      JSON.parse(fs.readFileSync(path.join(LOCALE_DIR, l + ".json"), "utf8")),
    ]),
  );
  const groups = pluralGroupKeys(trees.no).sort();

  if (process.argv.includes("--list")) {
    for (const g of groups) console.log(g);
    console.log(`\n${groups.length} flertallsgrupper`);
    return;
  }

  const errors = [];

  if (groups.length === 0) {
    errors.push("no.json har ingen flertallsgrupper — gaten ville vært tom.");
  }

  // 1. Hver gruppe finnes i alle språk, med nøyaktig de kategoriene språket
  //    trenger. Én manglende `few` i pl.json = feil substantivform for 2–4.
  for (const lang of LANGS) {
    const want = [...requiredCategories(lang)].sort();
    for (const key of groups) {
      const node = lookup(trees[lang], key);
      if (!isPluralGroup(node)) {
        errors.push(`${lang}.json: «${key}» er ikke en flertallsgruppe`);
        continue;
      }
      const have = Object.keys(node).sort();
      if (have.join() !== want.join()) {
        errors.push(`${lang}.json: «${key}» har [${have}], skal ha [${want}]`);
      }
    }
  }

  // 2. Hver `tn('…')` peker på en gruppe; ingen gruppe leses med `t('…')`.
  const files = sourceFiles(RENDERER_DIR);
  const groupSet = new Set(groups);
  for (const file of files) {
    const rel = path.relative(ROOT, file);
    const { tn, t } = scanKeys(fs.readFileSync(file, "utf8"));
    for (const key of tn) {
      if (!groupSet.has(key)) {
        errors.push(
          `${rel}: tn('${key}') — nøkkelen er ingen flertallsgruppe i no.json`,
        );
      }
    }
    for (const key of t) {
      if (groupSet.has(key)) {
        errors.push(
          `${rel}: t('${key}') — dette er en flertallsgruppe, bruk tn()`,
        );
      }
    }
  }

  if (errors.length) {
    console.error("i18n-flertallsgate FEILET:\n");
    for (const e of errors) console.error("  ✗ " + e);
    console.error(`\n${errors.length} problem(er).`);
    process.exit(1);
  }

  console.log(
    `i18n-flertallsgate OK — ${groups.length} grupper × ${LANGS.length} språk, kategorier fra Intl.PluralRules.`,
  );
}

main();
