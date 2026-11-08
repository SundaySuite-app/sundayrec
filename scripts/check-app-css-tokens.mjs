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
 * ## Avstander (gap / padding / margin)
 *
 *   Samme eierskapsprinsipp for avstand: en avstand er `var(--sp-*)` eller et
 *   semantisk navn (`--gap-page`, `--pad-card`…) fra `tokens.css`. Gaten avviser
 *   en positiv px-verdi i `gap`, `row-gap`, `column-gap`, `grid-gap` (og -row/-column), `padding*` og
 *   `margin*` — unntatt 0, 1px og 2px (hårlinjer og optiske justeringer).
 *   NEGATIVE verdier (`margin: -18px 0 0 -7px` på redigeringens håndtak) er
 *   geometri, ikke luft, og telles ikke.
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

/** Avstands-egenskaper: gap, padding, margin og langformene. */
const SPACING_DECL =
  /(^|[\s;{])((?:grid-)?(?:(?:row|column)-)?gap|(?:padding|margin)(?:-(?:top|right|bottom|left|inline|block)(?:-(?:start|end))?)?)\s*:\s*([^;}]*)/g;
/** En positiv px-verdi som ikke står etter minus eller i et ord/tall. */
const PX = /(?<![\w.-])(\d*\.?\d+)px\b/g;
const PX_ALLOWED = new Set([0, 1, 2]);

/** px-literaler i avstand, som `{ line, text }`. Skanner HELE teksten, ikke
 *  linje for linje: en verdi kan bryte over flere linjer
 *  (`padding:\n 10px 12px;`), og `[^;}]*` stopper først ved `;` eller `}`. */
export function findSpacingLiterals(css) {
  const cleaned = stripComments(css);
  const found = [];
  const lineAt = (offset) => cleaned.slice(0, offset).split("\n").length;
  SPACING_DECL.lastIndex = 0;
  let d;
  while ((d = SPACING_DECL.exec(cleaned)) !== null) {
    const valueStart = d.index + d[0].length - d[3].length;
    // Verdier inne i var(--x, 12px)-fallback teller også; de skal være tokens.
    PX.lastIndex = 0;
    let m;
    while ((m = PX.exec(d[3])) !== null) {
      if (!PX_ALLOWED.has(Number(m[1]))) {
        found.push({
          line: lineAt(valueStart + m.index),
          text: `${d[2]}: ${m[0]}`,
        });
      }
    }
  }
  return found;
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

  const sp = findSpacingLiterals(`
.a { gap: 10px; }
.b { padding: 0 var(--sp-3) 14px; }
.c { margin-top: 3px; }
.d { margin: 1px 0 2px; padding: 0; gap: var(--sp-2); }
.e { margin: -18px 0 0 -7px; }
.f { font-size: 13px; width: 40px; border-radius: 9px; }
/* gap: 99px */
.g { grid-gap: 10px; grid-row-gap: 12px; grid-column-gap: 6px; }
.h {
  padding:
    10px
    var(--sp-3);
  gap: var(--sp-2)
    14px;
}
`);
  say(
    sp.length === 8,
    `avstandsgaten ventet 8 funn, fikk ${sp.length}: ${sp.map((h) => h.text).join(", ")}`,
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
    const text = fs.readFileSync(path.join(ROOT, rel), "utf8");
    const hits = [
      ...findColorLiterals(text),
      ...findSpacingLiterals(text),
    ].sort((a, b) => a.line - b.line);
    if (hits.length === 0) continue;
    total += hits.length;
    console.error(`\n${rel}`);
    for (const h of hits) console.error(`  ${h.line}: ${h.text}`);
  }

  if (total > 0) {
    console.error(
      `\n✗ ${total} literal(er) (farge eller px-avstand) i app/-CSS.\n` +
        `  Alt visuelt går gjennom var(--…). Legg verdien i ${TOKENS_FILE}\n` +
        `  og bruk tokenet (avstand: var(--sp-*)) — ellers kan ikke eieren justere appen ett sted.`,
    );
    process.exit(1);
  }

  console.log(
    `✓ ${files.length} CSS-fil(er) i app/ — ingen farge- eller avstandsliteraler utenfor ${TOKENS_FILE}`,
  );
}

main();
