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
 * F2-T1: kategori-kravet i (2) gjelder alle sju katalogene, MEN med samme
 * unntak som `legacy/locales/parity.test.ts` allerede gir flate nøkler — en
 * gruppe som står i `PAUSED_KEYS` slipper i de fem PAUSET språkene (se
 * `parsePausedLists`/`isPausedException` under). Uten unntaket måtte en ny
 * `tn()`-nøkkel oversettes til sju språk med én gang eller gaten feilet, som
 * er nøyaktig hvorfor nye flater unngikk `tn()` i stedet.
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

// ── Pauset paritet (F2-T1) ───────────────────────────────────────────────────
//
// `legacy/locales/parity.test.ts` lets a flat key skip the five paused
// languages (`PAUSED_LOCALES`) while it is on `PAUSED_KEYS` — «app/ er norsk +
// engelsk til fase B, og de fem andre er PAUSET for nøklene redesignet legger
// til, og BARE for dem» (that file's own words). This gate had no equivalent:
// every plural GROUP was required, correctly shaped, in all seven catalogs
// from the moment it existed — which meant a brand-new `tn()` key needed a
// Polish `few`/`many` before `npm run check` would pass, so new screens simply
// avoided `tn()` rather than pay that tax. The exemption below is the same
// one `parity.test.ts`'s own "plural groups carry exactly the forms their
// language needs" block already grants — this gate was just not reading it.
//
// `PAUSED_LOCALES` / `PAUSED_KEYS` stay canonical in ONE place: this reads
// parity.test.ts's own SOURCE TEXT rather than duplicating the list by hand,
// because a hand-kept copy is a copy that goes stale the day someone edits
// the original and forgets this file exists. It cannot be an ES import —
// parity.test.ts is a vitest suite (imports `vitest` + seven JSON catalogs as
// modules) and this gate runs as a bare Node script outside that loader — so
// it is a bounded-block string-literal scan instead, the same METHOD (not the
// same target) `check-command-reachability.mjs` uses for
// `generate_handler![…]` and `check-i18n-keys.mjs`'s `keysNamedInSharedCore`
// use for the identical reason.
const PARITY_TEST_PATH = path.join(ROOT, "legacy", "locales", "parity.test.ts");

/** One quoted string literal, single/double/backtick — the same tri-quote
 *  shape `QUOTED` below covers for `tn()`/`t()` call sites, kept as its own
 *  constant here rather than a forward reference so a future house-style
 *  change in parity.test.ts (today: single quotes throughout) does not go
 *  blind here too. Comments are stripped by the caller BEFORE this runs —
 *  see `stripLineComments` — so this never has to tell a real key apart from
 *  one a comment merely mentions. */
const PLAIN_QUOTED = /'([^'\n]+)'|"([^"\n]+)"|`([^`\n]+)`/g;
function quotedLiterals(text) {
  return [...text.matchAll(PLAIN_QUOTED)].map((m) => m[1] ?? m[2] ?? m[3]);
}

/**
 * Strip a `//` line comment (and everything after it) from each line.
 *
 * Safe here — UNLIKE a blind strip over arbitrary source, which is exactly
 * what `check-command-reachability.mjs` has a quote-aware state machine to
 * avoid — because every string `PAUSED_LOCALES`/`PAUSED_KEYS` hold is a
 * locale code or a dot-path i18n key, and neither can legally contain `//`.
 * There is nothing here for a naive strip to corrupt.
 */
function stripLineComments(text) {
  return text
    .split("\n")
    .map((line) => line.replace(/\/\/.*$/, ""))
    .join("\n");
}

/**
 * Pull `PAUSED_LOCALES` and `PAUSED_KEYS` out of parity.test.ts's raw text.
 *
 * Pure function of the text (not the filesystem), so the self-test can drive
 * it against a small fixture instead of the real ~670-line list — a parsing
 * regression should fail LOUDLY here, not silently return an empty set that
 * makes every paused-language check strict again by accident.
 */
export function parsePausedLists(source) {
  const localesBlock = source.match(/PAUSED_LOCALES\s*=\s*\[([^\]]*)\]/);
  if (!localesBlock) {
    throw new Error(
      "PAUSED_LOCALES ble ikke funnet i parity.test.ts — er fila omformet?",
    );
  }
  const keysBlock = source.match(
    /PAUSED_KEYS\s*=\s*new Set\(\[([\s\S]*?)\n\]\)/,
  );
  if (!keysBlock) {
    throw new Error(
      "PAUSED_KEYS ble ikke funnet i parity.test.ts — er fila omformet?",
    );
  }
  return {
    pausedLocales: quotedLiterals(stripLineComments(localesBlock[1])),
    pausedKeys: new Set(quotedLiterals(stripLineComments(keysBlock[1]))),
  };
}

/**
 * Is a plural group's absence/shape in `lang` excused by the redesign's
 * pause? Mirrors `parity.test.ts`'s own
 * `PAUSED_LOCALES.includes(lang) && PAUSED_KEYS.has(key)` check — see that
 * file's "plural groups carry exactly the forms their language needs" tests.
 * A NON-paused key, or a NEVER-paused language (no/en), is never excused.
 */
export function isPausedException(lang, key, pausedLocales, pausedKeys) {
  return pausedLocales.includes(lang) && pausedKeys.has(key);
}

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

  // F2-T1: the pause exemption, against FIXTURE lists — not the real ~670-key
  // list, so this asserts the DECISION function, never today's snapshot.
  const FIX_LOCALES = ["xx", "yy"];
  const FIX_KEYS = new Set(["new.group"]);
  say(
    isPausedException("xx", "new.group", FIX_LOCALES, FIX_KEYS),
    "a paused key missing in a paused locale is excused",
  );
  say(
    !isPausedException("no", "new.group", FIX_LOCALES, FIX_KEYS),
    "the SAME group missing in no (never paused) must still fail",
  );
  say(
    !isPausedException("en", "new.group", FIX_LOCALES, FIX_KEYS),
    "…and missing in en (never paused) must still fail",
  );
  say(
    !isPausedException("xx", "old.group", FIX_LOCALES, FIX_KEYS),
    "a NON-paused key missing in a paused locale must still fail",
  );

  // …and the extraction it runs on, against a small fixture TEXT rather than
  // the real file — a broken regex must fail HERE, not the day someone next
  // edits `PAUSED_KEYS` and the pattern quietly stops matching anything.
  const FIXTURE_PARITY_SOURCE = `
some preamble, exactly as unrelated real code would surround it
export const PAUSED_LOCALES = ['xx', 'yy']

export const PAUSED_KEYS = new Set([
  // a comment naming 'not.a.key' must not be picked up
  'app.a',
  'app.b',
])

const somethingAfter = ['should', 'not', 'leak', 'in']
`;
  const parsed = parsePausedLists(FIXTURE_PARITY_SOURCE);
  say(
    parsed.pausedLocales.join(",") === "xx,yy",
    `parsePausedLists reads PAUSED_LOCALES, got [${parsed.pausedLocales}]`,
  );
  say(
    parsed.pausedKeys.size === 2 &&
      parsed.pausedKeys.has("app.a") &&
      parsed.pausedKeys.has("app.b") &&
      !parsed.pausedKeys.has("not.a.key"),
    "parsePausedLists reads PAUSED_KEYS, ignoring comments and the array after it",
  );
  say(
    (() => {
      try {
        parsePausedLists("no PAUSED_LOCALES or PAUSED_KEYS in here at all");
        return false;
      } catch {
        return true;
      }
    })(),
    "parsePausedLists throws (not: silently returns empty) when the markers are gone",
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
  const { pausedLocales, pausedKeys } = parsePausedLists(
    fs.readFileSync(PARITY_TEST_PATH, "utf8"),
  );

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
      // Same pause `parity.test.ts` grants flat keys: a group added for the
      // redesign is not yet expected in a paused language. Checked BEFORE
      // shape/category — a paused key may be entirely ABSENT, not merely
      // short a category.
      if (isPausedException(lang, key, pausedLocales, pausedKeys)) continue;
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
