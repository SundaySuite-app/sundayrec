// Fail when a `data-i18n` fallback in index.html disagrees with no.json.
//
// Every localizable element in `legacy/renderer/index.html` is written twice:
//
//     <span data-i18n="editor.save">Lagre redigert fil</span>
//                     ^ the key            ^ the fallback
//
// `applyTranslations()` overwrites the text with the locale value at startup, so
// the fallback is what the user sees only in the first frame and when a key is
// missing. That makes drift invisible in the running app and very easy to
// introduce: edit no.json, forget the HTML, and the source now describes a UI
// that does not exist. Fase 7 found 32 of them — `editor.save` claimed «Lagre
// redigert fil» while the button had read «Eksporter» for weeks.
//
//   node scripts/check-i18n-fallbacks.mjs        report drift, exit 1 if any
//   node scripts/check-i18n-fallbacks.mjs --fix  rewrite fallbacks := no.json
//
// Two classes of difference are NOT drift and are never rewritten, because both
// render identically in a browser:
//   - whitespace-only  — HTML collapses runs of whitespace, so a fallback broken
//                        across source lines matches a single-line locale value
//   - case-only        — these labels sit under `text-transform: uppercase`

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const HTML = join(root, "legacy/renderer/index.html");
const NO = join(root, "legacy/locales/no.json");
const FIX = process.argv.includes("--fix");

const html = readFileSync(HTML, "utf8");
const no = JSON.parse(readFileSync(NO, "utf8"));

const lookup = (key) =>
  key.split(".").reduce((o, k) => (o == null ? undefined : o[k]), no);

const escText = (s) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const unesc = (s) =>
  s
    .replace(/&nbsp;/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");

const norm = (s) => s.replace(/\s+/g, " ").trim();
/** 'ws' and 'case' render identically; only 'drift' is a real mismatch. */
function classify(cur, want) {
  if (norm(cur) === norm(want)) return "ws";
  if (norm(cur).toLocaleLowerCase("no") === norm(want).toLocaleLowerCase("no"))
    return "case";
  return "drift";
}

const drift = [];
const missing = [];
let out = html;

// ── Attribute-backed fallbacks: title / placeholder / aria-label ─────────────
// NB: the plain attribute must be preceded by whitespace. `\btitle=` also
// matches the tail of `data-i18n-title=`, because '-' is a non-word character.
for (const [dataAttr, plainAttr] of [
  ["data-i18n-title", "title"],
  ["data-i18n-placeholder", "placeholder"],
  ["data-i18n-aria-label", "aria-label"],
]) {
  const tagRe = new RegExp(`<[^>]*\\b${dataAttr}="([^"]+)"[^>]*>`, "g");
  for (const m of [...html.matchAll(tagRe)]) {
    const [tag, key] = m;
    const want = lookup(key);
    if (typeof want !== "string") {
      missing.push(`${dataAttr}="${key}"`);
      continue;
    }
    const am = tag.match(new RegExp(`\\s${plainAttr}="([^"]*)"`));
    if (!am) continue; // no plain attribute to compare against
    const cur = unesc(am[1]);
    if (cur === want || classify(cur, want) !== "drift") continue;
    drift.push({ key, kind: dataAttr, cur, want });
    if (FIX) {
      out = out.replace(
        tag,
        tag.replace(
          new RegExp(`(\\s${plainAttr}=")[^"]*(")`),
          (_x, a, b) => a + escText(want).replace(/"/g, "&quot;") + b,
        ),
      );
    }
  }
}

// ── textContent fallbacks: data-i18n ────────────────────────────────────────
const textRe = /<(\w+)([^>]*\bdata-i18n="([^"]+)"[^>]*)>([^<]*)</g;
for (const m of [...html.matchAll(textRe)]) {
  const [full, , , key, inner] = m;
  const want = lookup(key);
  if (typeof want !== "string") {
    missing.push(`data-i18n="${key}"`);
    continue;
  }
  // Empty inner text means the element wraps child elements, not a text node.
  if (inner.trim() === "") continue;
  const cur = unesc(inner).trim();
  if (cur === want || classify(cur, want) !== "drift") continue;
  drift.push({ key, kind: "data-i18n", cur, want });
  if (FIX) {
    out = out.replace(
      full,
      full.replace(/>([^<]*)</, (_x, t) => {
        const lead = t.match(/^\s*/)[0];
        const tail = t.match(/\s*$/)[0];
        return ">" + lead + escText(want) + tail + "<";
      }),
    );
  }
}

if (missing.length) {
  console.error(`✗ ${missing.length} data-i18n key(s) missing from no.json:`);
  for (const k of missing) console.error(`    ${k}`);
  process.exit(1);
}

if (!drift.length) {
  console.log("✓ index.html fallbacks match legacy/locales/no.json");
  process.exit(0);
}

if (FIX) {
  writeFileSync(HTML, out);
  console.log(`✓ rewrote ${drift.length} fallback(s) from no.json:`);
  for (const d of drift) console.log(`    ${d.key}`);
  process.exit(0);
}

console.error(
  `✗ ${drift.length} data-i18n fallback(s) disagree with no.json — the HTML\n` +
    `  describes a UI the user never sees. Run:\n` +
    `      node scripts/check-i18n-fallbacks.mjs --fix\n` +
    `  and commit index.html (or fix no.json if the FALLBACK is the correct copy).\n`,
);
for (const d of drift) {
  console.error(`  [${d.kind}] ${d.key}`);
  console.error(`      html: ${JSON.stringify(d.cur)}`);
  console.error(`      json: ${JSON.stringify(d.want)}`);
}
process.exit(1);
