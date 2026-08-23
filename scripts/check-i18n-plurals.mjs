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
/**
 * Every tree that renders UI text. That is `app/` — the shell AND `app/lib/`,
 * the ported inventory fase B PR B moved in under it, whose `*-core` modules
 * still name count-aware keys.
 *
 * Unlike the two AST gates, this one is NOT narrowed to exclude the inventory:
 * it asks a question that is true of a key no matter who reads it (a `tn()` key
 * must be a plural group; a plural group must not be read with `t()`), and it
 * has always covered the port. Narrowing it would drop coverage the move did
 * not touch.
 */
const SOURCE_DIRS = [path.join(ROOT, "app")];
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

/**
 * A quoted key literal, in all three spellings the codebase uses. The legacy
 * renderer is a verbatim Electron port written with single quotes; `api-shim.ts`
 * and everything prettier has touched use double quotes; and a backtick with no
 * interpolation is the same constant written a third way.
 *
 * Until now this matcher saw ONLY single quotes, so every double-quoted call
 * site was invisible to the gate — which is not a smaller gate, it is a gate
 * with a hole in the shape of a whole file's house style.
 *
 * Backticks are captured too, but a template that INTERPOLATES is dropped
 * below: `t(\`x.${y}\`)` has no statically knowable key, so there is nothing to
 * check and pretending otherwise would produce false failures.
 */
const QUOTED = String.raw`(?:'([^'\n]+)'|"([^"\n]+)"|\`([^\`\n]+)\`)`;
const CALL_PREFIX = String.raw`(?:^|[^A-Za-z0-9_$.])(?:[A-Za-z0-9_$]+\.)?`;

/** `tn('a.b'` / `tn("a.b"` / `` tn(`a.b` `` (and `ctx.`-qualified) — the
 *  count-aware call sites. */
const TN_RE = new RegExp(CALL_PREFIX + String.raw`tn\(\s*` + QUOTED, "g");
/** The same for `t(` — but NOT `tn(`, `tf(`, `tArr(`. */
const T_RE = new RegExp(CALL_PREFIX + String.raw`t\(\s*` + QUOTED, "g");

/** Which files carry UI text: TS and TSX, minus their tests (a test asserts ON
 *  keys and would report its own fixtures as call sites). */
export function isScannableFile(name) {
  if (/\.test\.tsx?$/.test(name)) return false;
  return /\.tsx?$/.test(name);
}

function sourceFiles(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(p);
    return e.isFile() && isScannableFile(e.name) ? [p] : [];
  });
}

/** The key out of whichever quote style matched, or `null` for a template with
 *  interpolation in it. */
function keyOf(m) {
  const key = m[1] ?? m[2] ?? m[3];
  return key.includes("${") ? null : key;
}

export function scanKeys(source) {
  const tn = new Set();
  const t = new Set();
  for (const m of source.matchAll(TN_RE)) {
    const key = keyOf(m);
    if (key) tn.add(key);
  }
  for (const m of source.matchAll(T_RE)) {
    const key = keyOf(m);
    if (key) t.add(key);
  }
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

  // The quote styles the matcher was blind to until S0. A regression here is
  // exactly the silent kind: the gate keeps printing OK while it stops looking
  // at half the tree.
  const quoted = scanKeys(
    'tn("q.tn", 1); t("q.t"); tn(`b.tn`, 2); t(`b.t`); ctx.t("q.ctx");',
  );
  say(quoted.tn.has("q.tn"), "double-quoted tn scan");
  say(quoted.t.has("q.t") && quoted.t.has("q.ctx"), "double-quoted t scan");
  say(quoted.tn.has("b.tn"), "backtick tn scan");
  say(quoted.t.has("b.t"), "backtick t scan");

  const interpolated = scanKeys("t(`x.${which}`); tn(`y.${which}`, 1)");
  say(
    interpolated.t.size === 0 && interpolated.tn.size === 0,
    "a template with interpolation has no statically knowable key and must be skipped",
  );

  say(
    isScannableFile("home.ts") &&
      isScannableFile("App.tsx") &&
      !isScannableFile("App.test.tsx") &&
      !isScannableFile("home.test.ts") &&
      !isScannableFile("no.json"),
    "file filter covers .ts AND .tsx, excludes tests",
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
  const files = SOURCE_DIRS.flatMap(sourceFiles);
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
    `i18n-flertallsgate OK — ${groups.length} grupper × ${LANGS.length} språk, ` +
      `kategorier fra Intl.PluralRules; ${files.length} kildefiler skannet i ` +
      `${SOURCE_DIRS.map((d) => path.relative(ROOT, d)).join(" + ")}.`,
  );
}

main();
