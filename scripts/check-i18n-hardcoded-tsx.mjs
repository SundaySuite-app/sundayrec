#!/usr/bin/env node
/**
 * i18n-hardkodingsgate for `app/` — baseline 0 fra dag én.
 *
 * ## Forskjellen fra den gamle skrallen
 *
 * `check-i18n-hardcoded.mjs` VAR en SKRALLE med baseline 73: `index.html` var en
 * verbatim port av et Electron-skall med gammel gjeld, og den gjelda ble betalt
 * ned én runde om gangen. `app/` har ingen gjeld. Hver eneste streng en
 * frivillig leser skrives NÅ, av oss, og da er den riktige baselinen 0 og den
 * riktige tiden å innføre den er før den første komponenten finnes. En skralle
 * som starter på 0 er en gate.
 *
 * ## Hva som telles
 *
 *   1. JSX-tekst som matcher PROSE-regexen — ARVET fra
 *      `scripts/check-i18n-hardcoded.mjs` (se PROSE under). Én definisjon av
 *      «prosa» i repoet: to ville vært to steder å drifte fra hverandre, og en
 *      tekst som er prosa i det gamle skallet er prosa i det nye.
 *   2. Streng-literal i en PROSA-ATTRIBUTT — både JSX-attributter
 *      (`<button title="…">`) og objekt-egenskaper
 *      (`confirmDialog({ title: "…", confirmLabel: "…" })`). Begge former, fordi
 *      `app/` bruker begge for det samme, og fordi dialogtekstene ER
 *      objekt-egenskaper: en gate som bare så JSX ville sluppet gjennom hver
 *      eneste tekst en frivillig leser når det står på.
 *   3. Ethvert `data-i18n*`-attributt, og enhver streng som nevner `data-i18n`.
 *      Det er legacy-skallets mekanisme (`applyTranslations()` overskriver
 *      textContent fra katalogen ved språkbytte) og den er FORBUDT her: `app/`
 *      er reaktivt, `t()` leser `locale`-signalet, og en DOM-overskriving
 *      utenfor Preact ville blitt strøket ved neste render — stille, og bare
 *      noen ganger.
 *
 * Bruk:
 *   node scripts/check-i18n-hardcoded-tsx.mjs           # gate (baseline 0)
 *   node scripts/check-i18n-hardcoded-tsx.mjs --list    # vis funnene
 *
 * Mutasjonsvern: innebygd selvtest med TSX-fixtur og fasit — én kjent
 * hardkoding per klasse, pluss korrekte naboer som må forbli stille. Exit 2.
 *
 * ## `app/lib/` er MED (F1-I18N-T, W3)
 *
 * Fram til nå gikk vandringen forbi `app/lib/` — det porterte inventaret fase
 * B flyttet inn under `app/`. Det unntaket hørte hjemme i `check-i18n-keys.mjs`
 * (den gaten håndhever fallback-forbudet, og inventaret har legacy-signaturen
 * `t(key, fallback)` — se den gatens egen kommentar om hvorfor DEN fortsatt
 * hopper over `app/lib/`). Denne gaten teller ikke argumenter, bare hardkodet
 * prosa, og prosa som aldri når en frivillig er like hardkodet i `app/lib/`
 * som i `app/` selv — kopiert unntak, ikke samme begrunnelse. Baseline gikk
 * fra «umålt» til 0 i samme PR som slettet `mixer.ts`s døde DOM-rendering,
 * som var den suverent største kilden til akkurat denne gjelda (et helt
 * hardkodet mikserpanel ingen kalte lenger — se den fila).
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  collectJsxTexts,
  collectNamedStrings,
  collectStringLiterals,
  parseSource,
  sourceFiles,
} from "./lib/tsx-i18n-scan.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const APP_DIR = path.join(ROOT, "app");

/**
 * «Minst ett ord på ≥3 bokstaver med minst én liten bokstav.»
 *
 * ⚠️ Denne regexen var KOPIERT VERBATIM fra `scripts/check-i18n-hardcoded.mjs`,
 * og den gaten ble slettet i fase B sammen med `index.html`-en den målte. Kopien
 * er dermed ORIGINALEN nå: det er ikke lenger to steder som kan drive fra
 * hverandre, det er ett. Endres den, endres definisjonen av «prosa» i dette
 * repoet.
 */
const PROSE =
  /[A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]*[a-zæøåäöüéè][A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]{2,}|[A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]{2,}[a-zæøåäöüéè]/;

/**
 * Attributt-/egenskapsnavn som bærer tekst et menneske leser.
 *
 * Listen er lukket med vilje: den dekker det `app/`s komponentbibliotek og
 * dialog/toast-API faktisk tar imot. Et nytt navn her er en bevisst utvidelse,
 * ikke noe som siger inn.
 */
const PROSE_ATTRS = new Set([
  "title",
  "placeholder",
  "aria-label",
  "aria-description",
  "alt",
  "label",
  "description",
  "hint",
  "message",
  "confirmLabel",
  "cancelLabel",
  "heading",
  "subtitle",
  "tooltip",
]);

/**
 * Ord som ER prosa etter regexen, men som ikke skal oversettes: produktnavn,
 * verktøynavn, enheter og OS-navn. Et tekstnode er unntatt bare når ALLE ordene
 * i den står her — «kHz» slipper gjennom, «Vis i kHz» gjør det ikke.
 *
 * Den gamle skrallen har INGEN slik liste (den ser bare på HTML, der disse
 * ordene alltid står inne i en setning som uansett har en nøkkel). Denne er
 * derfor ny, og holdes ærlig av en selvtest: hver oppføring MÅ være noe
 * PROSE-regexen faktisk flagger. Ord som «MP3», «WAV», «FLAC», «LUFS» og «Hz»
 * er rene versaler eller for korte og treffes ikke av regexen i det hele tatt —
 * de står derfor bevisst IKKE her, for en liste med død vekt er en liste ingen
 * tør å rydde i.
 */
const EXEMPT_TEXT = new Set([
  "SundayRec",
  "SundaySuite",
  "ffmpeg",
  "ffprobe",
  "kHz",
  "dBFS",
  "macOS",
  "Windows",
  "Mac",
]);

/**
 * Bokstavordene i en tekst. Tall og skilletegn faller ut med vilje: de er
 * aldri prosa i seg selv, så «44,1 kHz» skal veie akkurat like mye som «kHz».
 */
function words(text) {
  return text.split(/[^A-ZÆØÅa-zæøåÄÖÜäöüéÉèÈ]+/).filter(Boolean);
}

/** Er hele teksten unntatt? Tom ordliste (rene tall/symboler) er ikke prosa
 *  og kommer aldri hit. */
export function isExempt(text) {
  const w = words(text);
  return w.length > 0 && w.every((x) => EXEMPT_TEXT.has(x));
}

/** Prosa som ikke er unntatt — den ene avgjørelsen begge klasser deler. */
export function isHardcodedProse(text) {
  return PROSE.test(text) && !isExempt(text);
}

/** Alle funn i én parset fil. */
export function findHardcoded(sourceFile) {
  const found = [];

  for (const node of collectJsxTexts(sourceFile)) {
    if (isHardcodedProse(node.text)) {
      found.push({
        kind: "jsx-text",
        line: node.line,
        text: node.text,
        why: "Hardkodet tekst i JSX — bruk {t('nøkkel')}.",
      });
    }
  }

  for (const attr of collectNamedStrings(sourceFile)) {
    if (attr.name.startsWith("data-i18n")) {
      found.push({
        kind: "data-i18n",
        line: attr.line,
        text: attr.name,
        why: "data-i18n hører til legacy-skallets applyTranslations() — app/ er reaktivt (locale-signalet).",
      });
      continue;
    }
    if (!PROSE_ATTRS.has(attr.name)) continue;
    if (attr.value === null) continue;
    if (!isHardcodedProse(attr.value)) continue;
    found.push({
      kind: attr.where === "jsx" ? "jsx-attr" : "prop",
      line: attr.line,
      text: `${attr.name}=${JSON.stringify(attr.value)}`,
      why: "Tekst en frivillig leser — bruk t('nøkkel').",
    });
  }

  // Rømningsveien attributt-vandringen ikke ser: `setAttribute("data-i18n", …)`,
  // `querySelectorAll("[data-i18n]")`, en klasse bygget som streng.
  for (const lit of collectStringLiterals(sourceFile)) {
    if (!lit.text.includes("data-i18n")) continue;
    if (found.some((f) => f.kind === "data-i18n" && f.line === lit.line))
      continue;
    found.push({
      kind: "data-i18n",
      line: lit.line,
      text: lit.text,
      why: "data-i18n hører til legacy-skallet — app/ oversetter gjennom t() og locale-signalet.",
    });
  }

  return found;
}

// ── Selvtest (mutasjonsvern) ────────────────────────────────────────────────

const SELFTEST_TSX = `
import { t } from "./i18n";
export function Fixture() {
  return (
    <div data-i18n="a.b">
      {t("ok.key")}
      <span>Udekket prosa her</span>
      <span>—</span>
      <span>42</span>
      <span>kHz</span>
      <span>SundayRec</span>
      <button title="Lagre alt" aria-label={t("ok.aria")}>
        {t("ok.save")}
      </button>
      <input placeholder={t("ok.placeholder")} />
      <Row label={t("ok.label")} hint="Dette er en forklaring" />
    </div>
  );
}
export const dialog = {
  title: "Er du sikker?",
  message: t("ok.message"),
  confirmLabel: "Ja, gjør det",
  cancelLabel: t("ok.cancel"),
  id: "some-id",
};
export const legacyish = document.querySelector("[data-i18n]");
`;

function selfTest() {
  const problems = [];
  const say = (ok, what) => {
    if (!ok) problems.push(what);
  };

  const sf = parseSource("selftest.tsx", SELFTEST_TSX);
  const found = findHardcoded(sf);
  const kinds = found.map((f) => f.kind).sort();
  const texts = found.map((f) => f.text);

  // Fasit: «Udekket prosa her» (jsx-text), title=«Lagre alt» (jsx-attr),
  // hint=«Dette er en forklaring» (jsx-attr), title/confirmLabel i
  // objektliteralet (prop ×2), data-i18n-attributtet + "[data-i18n]"-strengen.
  say(
    texts.some((x) => x === "Udekket prosa her"),
    "klassen «JSX-tekst» ble ikke fanget",
  );
  say(
    texts.some((x) => x.startsWith('title="Lagre alt"')),
    "klassen «prosa-attributt i JSX» ble ikke fanget",
  );
  say(
    texts.some((x) => x.startsWith('hint="Dette er en forklaring"')),
    "hint-attributtet ble ikke fanget",
  );
  say(
    texts.some((x) => x.startsWith('title="Er du sikker?"')),
    "klassen «prosa i objekt-egenskap» ble ikke fanget (dialogtekster)",
  );
  say(
    texts.some((x) => x.startsWith('confirmLabel="Ja, gjør det"')),
    "confirmLabel i objektliteral ble ikke fanget",
  );
  say(
    kinds.filter((k) => k === "data-i18n").length === 2,
    `klassen «data-i18n» ble ikke fanget to ganger (attributt + streng), fikk ${kinds.filter((k) => k === "data-i18n").length}`,
  );
  say(
    found.length === 7,
    `fasit er 7 funn, fikk ${found.length}: ${texts.join(" | ")}`,
  );

  // Naboene som må forbli STILLE. En gate som roper på korrekt kode blir slått
  // av, og da beskytter den ingenting.
  for (const quiet of ["—", "42", "kHz", "SundayRec", "some-id"]) {
    say(!texts.includes(quiet), `«${quiet}» skulle ikke vært et funn`);
  }

  // Unntakslista holdes ærlig: hver oppføring må være noe regexen FAKTISK
  // flagger, ellers er den død vekt som skjuler hva lista egentlig gjør.
  for (const word of EXEMPT_TEXT) {
    say(
      PROSE.test(word),
      `EXEMPT_TEXT «${word}» treffes ikke av PROSE-regexen — den er overflødig`,
    );
  }
  // …og et unntatt ord inne i en setning er fortsatt prosa.
  say(
    isHardcodedProse("Krever ffmpeg for å eksportere"),
    "et unntatt ord midt i en setning må fortsatt telle",
  );
  say(!isHardcodedProse("ffmpeg"), "et frittstående unntatt ord må ikke telle");
  say(!isHardcodedProse("44,1 kHz"), "tall + enhet må ikke telle");

  if (problems.length) {
    console.error("check-i18n-hardcoded-tsx SELVTEST FEILET:");
    for (const p of problems) console.error("  ✗ " + p);
    process.exit(2);
  }
}

// ── Gate ────────────────────────────────────────────────────────────────────

function main() {
  selfTest();

  const args = process.argv.slice(2);
  // F1-I18N-T: used to be `sourceFiles(APP_DIR, [LIB_DIR])` — see "`app/lib/`
  // er MED" in the file header for why the exclusion existed and why it no
  // longer does.
  const files = sourceFiles(APP_DIR);
  const findings = [];

  for (const file of files) {
    const rel = path.relative(ROOT, file);
    const sf = parseSource(file, fs.readFileSync(file, "utf8"));
    for (const f of findHardcoded(sf)) findings.push({ ...f, rel });
  }

  if (args.includes("--list")) {
    for (const f of findings) {
      console.log(`· ${f.rel}:${f.line} [${f.kind}] ${f.text}`);
    }
    console.log(`\n${findings.length} funn (baseline 0)`);
    return;
  }

  if (findings.length) {
    console.error(
      `✕ i18n-hardkodingsgate (app/): ${findings.length} hardkodet tekst — baselinen er 0.\n`,
    );
    for (const f of findings) {
      console.error(`  ✗ ${f.rel}:${f.line} [${f.kind}] ${f.text}`);
      console.error(`      ${f.why}`);
    }
    process.exit(1);
  }

  console.log(
    `✓ i18n-hardkodingsgate (app/): 0 hardkodet tekst i ${files.length} filer (baseline 0)`,
  );
}

main();
