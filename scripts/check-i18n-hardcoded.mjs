#!/usr/bin/env node
/**
 * i18n-skralle (ratchet-lite): teller norske prosa-tekstnoder i
 * legacy/renderer/index.html som IKKE dekkes av data-i18n, og feiler hvis
 * tallet ØKER over den innsjekkede baselinen. Retningen er én vei: ny UI-tekst
 * skal fødes med nøkkel; gammel gjeld betales ned når man likevel er innom.
 *
 * Hva som telles som «prosa»: en tekstnode med et ord på ≥3 bokstaver hvorav
 * minst én liten bokstav — dvs. setninger og etiketter, ikke «—», tall,
 * enheter eller enkeltglyfer. Noder inni <svg>/<script>/<style> og noder der
 * noden selv ELLER en forelder bærer data-i18n (applyTranslations overskriver
 * hele elementets textContent) telles ikke.
 *
 * Bruk:
 *   node scripts/check-i18n-hardcoded.mjs                  # gate
 *   node scripts/check-i18n-hardcoded.mjs --list           # vis nodene
 *   node scripts/check-i18n-hardcoded.mjs --write-baseline # senk baselinen
 *
 * Mutasjonsvern: skriptet kjører først seg selv mot en innebygd fixture med
 * fasit. Endrer noen telle-logikken slik at den slutter å se hardkodet tekst,
 * feiler selvtesten før gaten i det hele tatt får uttale seg — en skralle som
 * kan mutere til «alltid 0» er ingen skralle.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// ── Baseline ────────────────────────────────────────────────────────────────
// Oppdateres KUN med --write-baseline (og bare nedover uten --force-up).
const BASELINE = 131;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const HTML_PATH = path.join(
  __dirname,
  "..",
  "legacy",
  "renderer",
  "index.html",
);

/** Elementer uten lukketagg (HTML void elements) — de skal aldri på stakken. */
const VOID = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

/** Subtrær som aldri inneholder oversettbar prosa. */
const SKIP_SUBTREES = new Set(["svg", "script", "style"]);

/** Minst ett ord på ≥3 bokstaver med minst én liten bokstav. */
const PROSE =
  /[A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]*[a-zæøåäöüéè][A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]{2,}|[A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]{2,}[a-zæøåäöüéè]/;

/**
 * Tell udekkede prosa-tekstnoder i en HTML-streng.
 * Returnerer { count, nodes } der nodes er tekstene (trimmet, forkortet).
 */
export function countHardcoded(html) {
  // Kommentarer er ikke tekstnoder.
  const src = html.replace(/<!--[\s\S]*?-->/g, "");
  const tagRe = /<\/?([a-zA-Z][a-zA-Z0-9-]*)((?:"[^"]*"|'[^']*'|[^'">])*)>/g;
  /** stack av { name, i18n } for åpne elementer */
  const stack = [];
  const nodes = [];
  let last = 0;
  let m;
  const skipDepth = () => stack.findIndex((e) => SKIP_SUBTREES.has(e.name));
  while ((m = tagRe.exec(src)) !== null) {
    const text = src.slice(last, m.index);
    last = tagRe.lastIndex;
    const inSkip = skipDepth() !== -1;
    const covered = stack.some((e) => e.i18n);
    const trimmed = text.replace(/\s+/g, " ").trim();
    if (!inSkip && !covered && trimmed && PROSE.test(trimmed)) {
      nodes.push(trimmed.length > 90 ? trimmed.slice(0, 90) + "…" : trimmed);
    }
    const [, rawName, attrs] = m;
    const name = rawName.toLowerCase();
    const isClose = m[0].startsWith("</");
    const selfClose = /\/\s*>$/.test(m[0]);
    if (isClose) {
      // Lukk til og med nærmeste element med samme navn (tåler slurvete nesting).
      const at = stack.map((e) => e.name).lastIndexOf(name);
      if (at !== -1) stack.length = at;
    } else if (!VOID.has(name) && !selfClose) {
      stack.push({ name, i18n: /\bdata-i18n\s*=/.test(attrs) });
    }
  }
  // Halen etter siste tagg er aldri prosa i et velformet dokument.
  return { count: nodes.length, nodes };
}

// ── Mutasjonsvern: innebygd fixture med fasit ───────────────────────────────
function selfTest() {
  const fixture = `
    <div>
      <span data-i18n="a.b">Dekket tekst</span>
      <span>Udekket prosa her</span>
      <div data-i18n="c.d"><em>Barn av dekket element</em></div>
      <svg><text>Aldri talt</text></svg>
      <span>—</span><span>42</span><span>OK?</span>
      <button>Lagre alt</button>
      <p>Setning nummer to.</p>
    </div>`;
  const { count, nodes } = countHardcoded(fixture);
  // Fasit: «Udekket prosa her», «Lagre alt», «Setning nummer to.» = 3.
  if (count !== 3) {
    console.error(
      "SELVTEST FEILET: telle-logikken fant",
      count,
      "noder i fixturen (fasit 3):",
      nodes,
    );
    console.error(
      "Skrallen kan ikke stoles på — fiks countHardcoded før du rører baselinen.",
    );
    process.exit(2);
  }
}

// ── Gate ────────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
selfTest();

const html = fs.readFileSync(HTML_PATH, "utf8");
const { count, nodes } = countHardcoded(html);

if (args.includes("--list")) {
  for (const n of nodes) console.log("·", n);
  console.log(`\n${count} udekkede prosa-tekstnoder (baseline ${BASELINE})`);
  process.exit(0);
}

if (args.includes("--write-baseline")) {
  if (count > BASELINE && !args.includes("--force-up")) {
    console.error(
      `Nekter å HEVE baselinen (${BASELINE} → ${count}) uten --force-up — skrallen går én vei.`,
    );
    process.exit(1);
  }
  const self = fs.readFileSync(fileURLToPath(import.meta.url), "utf8");
  fs.writeFileSync(
    fileURLToPath(import.meta.url),
    self.replace(/const BASELINE = \d+/, `const BASELINE = ${count}`),
  );
  console.log(`Baseline oppdatert: ${BASELINE} → ${count}`);
  process.exit(0);
}

if (count > BASELINE) {
  console.error(
    `✕ i18n-skralle: ${count} udekkede prosa-tekstnoder i index.html — baselinen er ${BASELINE} (+${count - BASELINE}).`,
  );
  console.error(
    "  Ny UI-tekst skal ha data-i18n-nøkkel (×7 locales). Kjør med --list for å se nodene.",
  );
  process.exit(1);
}
if (count < BASELINE) {
  console.log(
    `✓ i18n-skralle: ${count}/${BASELINE} — ${BASELINE - count} under baselinen. Stram til: node scripts/check-i18n-hardcoded.mjs --write-baseline`,
  );
} else {
  console.log(
    `✓ i18n-skralle: ${count} udekkede prosa-tekstnoder (== baseline)`,
  );
}
