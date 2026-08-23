/**
 * Den delte TSX-vandringen for de to nye i18n-gatene i `app/`.
 *
 * ## Hvorfor compiler-API og ikke regex
 *
 * De to GAMLE gatene (`check-i18n-fallbacks.mjs`, `check-i18n-hardcoded.mjs`)
 * leser `legacy/renderer/index.html` med regex, og det holder fordi HTML-en er
 * flat og maskinskrevet. `app/` er TSX: en nøkkel kan stå i en JSX-attributt, i
 * et objektliteral, bak en template-streng, inni en nøstet callback. En regex
 * over det svarer med falske treff der det ikke er noe, og — verre — tier der
 * det er. Derfor vandrer disse gatene TypeScripts egen AST: samme parser som
 * `tsc`, altså nøyaktig den samme forståelsen av filen som byggverket har.
 *
 * ## Hvorfor ÉN modul for to gater
 *
 * Begge gatene stiller spørsmål om de samme nodene i de samme filene. To kopier
 * av vandringen ville vært to steder «hva er et kall til `t()`» kan drifte fra
 * hverandre — nøyaktig skjøtefeil-formen (`reference-seam-bugs`): to lag som
 * hver for seg er riktige og uenige i skjøten. Hver gate har sin egen SELVTEST
 * over denne modulen, så en mutasjon her feller begge.
 */

import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

/**
 * Oversetterfunksjonene `app/` har lov til å kalle. `tDyn` er den ENE
 * sanksjonerte veien til en dynamisk nøkkel (se check-i18n-keys.mjs).
 */
export const I18N_FUNCTIONS = ["t", "tf", "tn", "tArr", "tDyn"];

/**
 * Filer som bærer UI-tekst: TS og TSX, minus testene. Samme regel som
 * `check-i18n-plurals.mjs` bruker — en test asserterer PÅ nøkler og ville
 * rapportert sine egne fixturer som kallsteder.
 */
export function isScannableFile(name) {
  if (/\.test\.tsx?$/.test(name)) return false;
  return /\.tsx?$/.test(name);
}

/** Alle skannbare filer under `dir`, rekursivt. Tom liste for en mappe som
 *  ikke finnes (gaten skal ikke krasje på et halvt sjekket ut tre). */
export function sourceFiles(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(p);
    return e.isFile() && isScannableFile(e.name) ? [p] : [];
  });
}

/** Parse én fil. `.tsx` MÅ parses som TSX — ellers leses `<Foo/>` som en
 *  type-assertion og hele treet blir feil (stille: parseren rapporterer ikke). */
export function parseSource(fileName, source) {
  return ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.ES2022,
    /* setParentNodes */ true,
    fileName.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
}

/** Dybde-først over hele treet. */
export function walk(node, visit) {
  visit(node);
  node.forEachChild((child) => walk(child, visit));
}

/** 1-indeksert linjenummer for en node, slik et menneske teller. */
export function lineOf(sourceFile, node) {
  return (
    ts.getLineAndCharacterOfPosition(sourceFile, node.getStart(sourceFile))
      .line + 1
  );
}

/**
 * Navnet på det som kalles, for `t(…)` OG `ctx.t(…)`.
 *
 * Den kvalifiserte formen finnes i legacy (`n.t(key, fallback)` i api-shim) og
 * kan fint dukke opp i `app/` når en hjelper får i18n injisert. Returnerer
 * `null` for alt annet (`foo()[0]()`, `(cond ? a : b)()`).
 */
export function calleeName(call) {
  const callee = call.expression;
  if (ts.isIdentifier(callee)) return callee.text;
  if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.name)) {
    return callee.name.text;
  }
  return null;
}

/**
 * Teksten i et statisk streng-uttrykk, ellers `null`.
 *
 * Både `'a.b'`, `"a.b"` og `` `a.b` `` (uten interpolasjon) er den samme
 * konstanten skrevet på tre måter. En template MED interpolasjon har ingen
 * statisk kjent verdi og er derfor `null` — det er nettopp det `tDyn` finnes
 * for.
 */
export function staticText(node) {
  if (!node) return null;
  if (ts.isStringLiteral(node)) return node.text;
  if (ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  return null;
}

/**
 * Alle kall til de fem i18n-funksjonene i én fil.
 *
 * `key` er `null` når første argument ikke er en statisk streng — gaten
 * avgjør selv om det er lov (det er det bare for `tDyn`s prefiks-plass, og
 * der er det tvert imot påkrevd at det ER statisk; se gaten).
 */
export function collectI18nCalls(sourceFile) {
  const out = [];
  walk(sourceFile, (node) => {
    if (!ts.isCallExpression(node)) return;
    const fn = calleeName(node);
    if (!fn || !I18N_FUNCTIONS.includes(fn)) return;
    out.push({
      fn,
      key: staticText(node.arguments[0]),
      /** `true` når første argument mangler helt — `t()`. */
      keyMissing: node.arguments.length === 0,
      argCount: node.arguments.length,
      line: lineOf(sourceFile, node),
    });
  });
  return out;
}

/**
 * Alle JSX-tekstnoder i én fil, trimmet og med kollapset mellomrom — samme
 * normalisering som den gamle HTML-skrallen gjør, fordi en JSX-tekst brutt over
 * flere kildelinjer rendres som én linje.
 */
export function collectJsxTexts(sourceFile) {
  const out = [];
  walk(sourceFile, (node) => {
    if (!ts.isJsxText(node)) return;
    const text = node.text.replace(/\s+/g, " ").trim();
    if (!text) return;
    out.push({ text, line: lineOf(sourceFile, node) });
  });
  return out;
}

/**
 * Alle navngitte streng-literaler som kan bære prosa: JSX-attributter OG
 * objekt-egenskaper.
 *
 * Begge, fordi `app/` har begge formene for det samme: `<button title="…">` og
 * `confirmDialog({ title: "…", confirmLabel: "…" })`. En gate som bare så
 * JSX-attributter ville sluppet gjennom hver eneste dialogtekst i appen — og
 * dialogene er akkurat de tekstene en frivillig leser når det står på.
 *
 * `where` er `"jsx"` eller `"prop"` for feilmeldingens skyld.
 */
export function collectNamedStrings(sourceFile) {
  const out = [];
  walk(sourceFile, (node) => {
    if (ts.isJsxAttribute(node)) {
      const name = ts.isIdentifier(node.name)
        ? node.name.text
        : node.name.getText(sourceFile);
      // `title="x"` og `title={"x"}` er samme streng skrevet to måter.
      const init = node.initializer;
      const value =
        init && ts.isJsxExpression(init)
          ? staticText(init.expression)
          : staticText(init);
      out.push({ name, value, where: "jsx", line: lineOf(sourceFile, node) });
      return;
    }
    if (ts.isPropertyAssignment(node)) {
      const name = ts.isIdentifier(node.name)
        ? node.name.text
        : ts.isStringLiteral(node.name)
          ? node.name.text
          : null;
      if (!name) return;
      out.push({
        name,
        value: staticText(node.initializer),
        where: "prop",
        line: lineOf(sourceFile, node),
      });
    }
  });
  return out;
}

/** Hver streng-literal i fila, uansett hvor. Brukes til å lukke rømningsveien
 *  `el.setAttribute("data-i18n", …)` — attributt-vandringen over ser bare JSX. */
export function collectStringLiterals(sourceFile) {
  const out = [];
  walk(sourceFile, (node) => {
    const text = staticText(node);
    if (text === null) return;
    // En JSX-attributtnavn-node er ikke en literal; her er vi bare ute etter
    // verdier og frittstående strenger.
    out.push({ text, line: lineOf(sourceFile, node) });
  });
  return out;
}
