#!/usr/bin/env node
/**
 * Fargegate for `app/`: en farge finnes ÉN gang, i `app/styles/tokens.css`.
 *
 * ## Hvorfor
 *
 * `legacy/renderer/styles.css` HADDE en pen `:root`-blokk med 52 variabler — og
 * over tusen hardkodede farger under den. (Fila er slettet i fase B; grunnen
 * denne gaten finnes for, er ikke.) Hver enkelt var rimelig der og da
 * («nesten den blå, bare litt mørkere»), og til sammen gjør de at eieren ikke
 * kan justere paletten ett sted, fordi de fleste fargene ikke bor der. Det er
 * ikke en stilfeil, det er en eierskapsfeil: fargen ligger i komponenten som
 * tilfeldigvis trengte den først.
 *
 * «Frivilligen først» avtaler ikke dette — den håndhever det. Ett funn er nok
 * til å felle gaten, fra dag én, fordi `app/` ikke har noen gjeld å nedbetale.
 *
 * ## Hva den ser etter
 *
 *   - `#abc`, `#aabbcc`, `#aabbccdd` — heksadesimale farger
 *   - `rgb(…)`, `rgba(…)`, `hsl(…)`, `hsla(…)` — funksjonelle farger
 *
 * Alt annet er lov: `var(--gold)`, `currentColor`, `transparent`, `inherit`,
 * `none`. Navngitte CSS-farger (`red`, `white`) fanges IKKE av et regex som
 * ikke skal ha falske treff på ord i klassenavn og kommentarer — den formen
 * er ikke en fristelse noen har hatt her, og en gate som roper på ordet
 * «white» inne i `--knob-white` ville blitt slått av innen uka.
 *
 * ## Unntaket
 *
 * `app/styles/tokens.css` — ordboken selv. Den er hele poenget.
 *
 * Kjør `node scripts/check-app-css-tokens.mjs --selftest` for gatens egen
 * prøve: en kjent fil med kjente svar, som feiler før gaten får si noe.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const APP_DIR = path.join(ROOT, "app");

/** Den ene fila som får holde literaler: ordboken. */
export const TOKENS_FILE = path.join("app", "styles", "tokens.css");

/**
 * Farge-literaler.
 *
 * Heks krever 3, 4, 6 eller 8 sifre og en grense etterpå, ellers ville
 * `#abcdefghij` (som ikke finnes) og id-selektorer (`#app-root`) truffet.
 * `#` etterfulgt av en bokstav som ikke er heks (`#section`) treffer ikke,
 * fordi hele lengden må være heks.
 */
const HEX = /#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b/g;
const FUNC = /\b(?:rgba?|hsla?)\s*\(/g;

/** Fjern kommentarer før skanning: en literal i en forklaring er ikke en
 *  farge appen bruker, og en gate som roper på egen dokumentasjon blir slått
 *  av. (Denne fila forklarer nettopp `#aabbcc` i sin egen prosa.) */
export function stripComments(css) {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
}

/** Alle funn i én CSS-tekst, som `{ line, text }`. */
export function findColorLiterals(css) {
  const cleaned = stripComments(css);
  const found = [];
  const lines = cleaned.split("\n");
  lines.forEach((line, i) => {
    for (const re of [HEX, FUNC]) {
      re.lastIndex = 0;
      let m;
      while ((m = re.exec(line)) !== null) {
        found.push({ line: i + 1, text: m[0] });
      }
    }
  });
  return found.sort((a, b) => a.line - b.line);
}

/** Hver `.css` under `app/`, som repo-relative stier. */
function cssFiles(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...cssFiles(full));
    else if (entry.name.endsWith(".css")) out.push(path.relative(ROOT, full));
  }
  return out.sort();
}

// ── Selvtest ────────────────────────────────────────────────────────────────

const SELFTEST_CSS = `
/* En kommentar som nevner #aabbcc og rgba(1,2,3,.4) skal IKKE telle. */
.ok {
  color: var(--ink);
  background: transparent;
  border-color: currentColor;
}
.bad-hex { color: #ebb84b; }
.bad-short { color: #fff; }
.bad-rgba { box-shadow: 0 1px 2px rgba(0, 0, 0, 0.5); }
.bad-hsl { color: hsl(210 40% 50%); }
#app-root { color: var(--ink); }
`;

function selfTest() {
  const problems = [];
  const say = (ok, what) => {
    if (!ok) problems.push(what);
  };

  const hits = findColorLiterals(SELFTEST_CSS);
  const texts = hits.map((h) => h.text);

  say(texts.includes("#ebb84b"), "seks-sifret heks ble ikke funnet");
  say(texts.includes("#fff"), "tre-sifret heks ble ikke funnet");
  say(texts.includes("rgba("), "rgba() ble ikke funnet");
  say(texts.includes("hsl("), "hsl() ble ikke funnet");
  say(
    !texts.includes("#aabbcc"),
    "en farge i en KOMMENTAR ble talt — gaten roper på sin egen dokumentasjon",
  );
  say(
    hits.length === 4,
    `forventet nøyaktig 4 funn, fikk ${hits.length}: ${texts.join(", ")}`,
  );
  say(
    findColorLiterals("#app-root { color: var(--gold) }").length === 0,
    "en id-selektor ble lest som en farge",
  );
  say(
    findColorLiterals(".x { border-radius: 4px }").length === 0,
    "en fargefri regel ga funn",
  );

  if (problems.length) {
    console.error("check-app-css-tokens SELVTEST FEILET:");
    for (const p of problems) console.error("  ✗ " + p);
    process.exit(2);
  }
  console.log("check-app-css-tokens selvtest: ok");
}

// ── Gate ────────────────────────────────────────────────────────────────────

function main() {
  if (process.argv.includes("--selftest")) {
    selfTest();
    return;
  }
  selfTest();

  if (!fs.existsSync(APP_DIR)) {
    console.error(`app/ finnes ikke (${APP_DIR})`);
    process.exit(2);
  }

  const files = cssFiles(APP_DIR);
  if (files.length === 0) {
    // En gate som ikke ser noen filer er en gate som alltid er grønn.
    console.error("Fant ingen CSS-filer under app/ — gaten ville vært tom.");
    process.exit(2);
  }

  let total = 0;
  for (const rel of files) {
    if (rel === TOKENS_FILE) continue;
    const hits = findColorLiterals(
      fs.readFileSync(path.join(ROOT, rel), "utf8"),
    );
    if (hits.length === 0) continue;
    total += hits.length;
    console.error(`\n${rel}`);
    for (const h of hits) console.error(`  ${h.line}: ${h.text}`);
  }

  if (total > 0) {
    console.error(
      `\n✗ ${total} fargeliteral(er) i app/-CSS.\n` +
        `  Alt visuelt går gjennom var(--…). Legg fargen i ${TOKENS_FILE}\n` +
        `  og bruk tokenet — ellers kan ikke eieren fargejustere appen ett sted.`,
    );
    process.exit(1);
  }

  console.log(
    `✓ ${files.length} CSS-fil(er) i app/ — ingen fargeliteraler utenfor ${TOKENS_FILE}`,
  );
}

main();
