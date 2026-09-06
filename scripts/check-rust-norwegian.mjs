#!/usr/bin/env node
/**
 * Norsk-skralle for Rust: motoren skal ikke lære seg nye norske setninger.
 *
 * ## Hvorfor
 *
 * Hele appen er oversatt til sju språk — bortsett fra de to kanalene som når en
 * frivillig som IKKE ser på skjermen: OS-varselet og linja i varsel-e-posten.
 * De var skrevet i norsk, som literaler, på stedet der det gikk galt:
 *
 *     dispatch_scheduler_failure(app, "scheduled_start_timeout",
 *         "Planlagt opptak startet ikke (tidsavbrudd) — sjekk kamera/mikrofon.")
 *
 * En polsk frivillig fikk polsk grensesnitt, polsk e-post-emne — og DEN
 * setningen på norsk. Det var funn A8. `sundayrec_core::alerts` flyttet
 * setningene inn i en katalog der kompilatoren krever alle sju.
 *
 * Denne gaten er den andre halvdelen (funn D5): den holder på flyttingen.
 * Ingen enkelt test kan se at noen skriver en NY norsk streng inn i motoren i
 * morgen — men en skralle kan: den teller `æøåÆØÅ` i strengliteraler per fil og
 * sammenligner mot en fasit. Tallet kan gå NED, aldri opp.
 *
 * ## Hva den teller
 *
 *   • strengliteraler (`"…"`, `r"…"`, `r#"…"#`) i `src-tauri/src` og i
 *     `crates/<krate>/src`, rekursivt
 *   • IKKE kommentarer eller doc-kommentarer — de er for den som leser koden,
 *     og norsk der er ikke en brukervendt streng
 *   • IKKE `#[cfg(test)]`-regioner eller `mod tests { … }` — en test som
 *     sjekker at «Planlagt opptak startet.» faktisk står i katalogen MÅ kunne
 *     skrive setningen
 *
 * ## Hvorfor en skralle og ikke et forbud
 *
 * Fordi noe norsk i Rust er riktig, og noe er gjeld:
 *
 *   • RIKTIG: helligdagsnavnene i `church_calendar.rs` er DATA — «Skjærtorsdag»
 *     er ikke en oversettbar setning, det er navnet på en dag i den norske
 *     kirkeåret-tabellen. Og SR-kodenes norske reserve i `diagnostics.rs` er en
 *     dokumentert post på språkrunden, ikke en glipp.
 *     Begge står i `allowlist` i fasit-fila, med grunnen skrevet ut.
 *   • GJELD: alt annet. Toast-tekstene (`notify::warn`) har en KODE som
 *     skallet oversetter, så den norske strengen er en reserve-detalj —
 *     tellende, men ikke noe denne PR-en river ut. Fasiten er derfor et tall
 *     som skal gå nedover, ikke null.
 *
 * Bruk:
 *   node scripts/check-rust-norwegian.mjs                    # gate
 *   node scripts/check-rust-norwegian.mjs --list             # per fil, med linjer
 *   node scripts/check-rust-norwegian.mjs --write-baseline   # etter en flytting
 *
 * Mutasjonsvern: skriptet kjører først seg selv mot innebygde fixturer med
 * kjent svar — ekte Rust, kjente tall, og ett kjent BRUDD. Slutter lesingen å
 * finne strenger, eller slutter sammenligningen å kunne si nei, feiler
 * selvtesten (exit 2) før gaten får uttale seg. En skralle som kan mutere til
 * «alltid grønn» er ingen skralle.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const BASELINE_FILE = path.join(__dirname, "rust-norwegian-baseline.json");

/** De norske tegnene. Ikke `é`, ikke `ü` — de finnes i tysk og fransk òg. */
const NORWEGIAN = /[æøåÆØÅ]/g;

// ── Lesing ──────────────────────────────────────────────────────────────────

/**
 * Gå gjennom Rust-kilden ÉN gang og skill kode, kommentarer og strenger.
 *
 * Returnerer `{ blanked, literals }`:
 *   • `blanked` har samme lengde og samme linjeskift som kilden, med
 *     kommentarer og STRENGINNHOLD erstattet av mellomrom. Den brukes til å
 *     finne `#[cfg(test)]` og til å telle klammer — begge deler ville vært
 *     usikre i rå kilde, der `"{"` er en klamme inni en streng.
 *   • `literals` er hver streng med tekst, startindeks og linjenummer.
 *
 * Én gjennomgang, ikke to regexer: `// ikke en kommentar hvis den står inni en
 * streng`, og `"/*"` er ikke starten på en blokk. To uavhengige passeringer får
 * dette galt hver gang; en tokenizer får det riktig av konstruksjon.
 */
export function scanRust(src) {
  const n = src.length;
  const out = new Array(n);
  const literals = [];
  let line = 1;
  let i = 0;

  // Blank ut `count` tegn fra `from`, behold linjeskift.
  const blank = (from, to) => {
    for (let k = from; k < to; k++) out[k] = src[k] === "\n" ? "\n" : " ";
  };
  const keep = (from, to) => {
    for (let k = from; k < to; k++) out[k] = src[k];
  };
  const countLines = (from, to) => {
    for (let k = from; k < to; k++) if (src[k] === "\n") line++;
  };

  while (i < n) {
    const c = src[i];
    const c2 = src[i + 1];

    // Linjekommentar (også `///` og `//!`).
    if (c === "/" && c2 === "/") {
      let j = i;
      while (j < n && src[j] !== "\n") j++;
      blank(i, j);
      i = j;
      continue;
    }

    // Blokk-kommentar. Rust nester dem, så vi teller dybde.
    if (c === "/" && c2 === "*") {
      let j = i;
      let depth = 0;
      while (j < n) {
        if (src[j] === "/" && src[j + 1] === "*") {
          depth++;
          j += 2;
          continue;
        }
        if (src[j] === "*" && src[j + 1] === "/") {
          depth--;
          j += 2;
          if (depth === 0) break;
          continue;
        }
        j++;
      }
      blank(i, j);
      countLines(i, j);
      i = j;
      continue;
    }

    // Tegnliteral (`'a'`, `'\n'`) — men IKKE en levetid (`&'a str`), som ikke
    // har noen sluttfnutt og ville slukt resten av fila.
    if (c === "'") {
      const m = /^'(\\.|[^\\'])'/.exec(src.slice(i, i + 12));
      if (m) {
        keep(i, i + m[0].length);
        i += m[0].length;
        continue;
      }
      out[i] = c;
      i++;
      continue;
    }

    // Rå streng: `r"…"`, `r#"…"#`, `br#"…"#`.
    const rawM = /^b?r(#*)"/.exec(src.slice(i, i + 40));
    if (rawM && (c === "r" || (c === "b" && c2 === "r"))) {
      const close = '"' + rawM[1];
      const bodyStart = i + rawM[0].length;
      let end = src.indexOf(close, bodyStart);
      if (end === -1) end = n;
      const value = src.slice(bodyStart, end);
      literals.push({ value, start: i, line });
      keep(i, bodyStart);
      blank(bodyStart, end);
      keep(end, Math.min(end + close.length, n));
      countLines(bodyStart, end);
      i = Math.min(end + close.length, n);
      continue;
    }

    // Vanlig streng (`"…"`, `b"…"`).
    if (c === '"') {
      const bodyStart = i + 1;
      let j = bodyStart;
      while (j < n) {
        if (src[j] === "\\") {
          j += 2;
          continue;
        }
        if (src[j] === '"') break;
        j++;
      }
      const value = src.slice(bodyStart, Math.min(j, n));
      literals.push({ value, start: i, line });
      out[i] = '"';
      blank(bodyStart, j);
      if (j < n) out[j] = '"';
      countLines(bodyStart, j);
      i = Math.min(j + 1, n);
      continue;
    }

    out[i] = c;
    if (c === "\n") line++;
    i++;
  }

  return { blanked: out.join(""), literals };
}

/**
 * Intervallene som er TESTKODE: alt under `#[cfg(test)]` og alt inne i en
 * `mod tests { … }` / `mod test { … }`.
 *
 * Leses fra den utblankede kilden, så en klamme inni en streng ikke kan flytte
 * grensa. Et `#[cfg(test)]` foran noe uten kropp (`use …;`) hopper fram til
 * semikolonet i stedet.
 */
export function testRegions(blanked) {
  const regions = [];
  const add = (from) => {
    // Fram til første `{` eller `;`, det som kommer først.
    let j = from;
    while (j < blanked.length && blanked[j] !== "{" && blanked[j] !== ";") j++;
    if (j >= blanked.length) return;
    if (blanked[j] === ";") {
      regions.push([from, j + 1]);
      return;
    }
    let depth = 0;
    let k = j;
    for (; k < blanked.length; k++) {
      if (blanked[k] === "{") depth++;
      else if (blanked[k] === "}") {
        depth--;
        if (depth === 0) {
          k++;
          break;
        }
      }
    }
    regions.push([from, k]);
  };

  for (const m of blanked.matchAll(/#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]/g))
    add(m.index);
  for (const m of blanked.matchAll(/\bmod\s+tests?\s*\{/g)) add(m.index);
  return regions;
}

const inRegions = (pos, regions) =>
  regions.some(([a, b]) => pos >= a && pos < b);

/**
 * Norske tegn i én fils PRODUKSJONS-strengliteraler.
 *
 * `{ count, hits }` — `count` er antall tegn (det skralla sammenligner),
 * `hits` er literalene de kom fra, for `--list` og for feilmeldingen.
 */
export function norwegianInFile(src) {
  const { blanked, literals } = scanRust(src);
  const regions = testRegions(blanked);
  let count = 0;
  const hits = [];
  for (const lit of literals) {
    if (inRegions(lit.start, regions)) continue;
    const m = lit.value.match(NORWEGIAN);
    if (!m) continue;
    count += m.length;
    hits.push({ line: lit.line, chars: m.length, text: lit.value });
  }
  return { count, hits };
}

// ── Filer ───────────────────────────────────────────────────────────────────

/** Hver `.rs` under `src-tauri/src` og `crates/<krate>/src`, repo-relativt. */
export function rustSources(root = ROOT) {
  const roots = [path.join(root, "src-tauri", "src")];
  const cratesDir = path.join(root, "crates");
  if (fs.existsSync(cratesDir)) {
    for (const ent of fs.readdirSync(cratesDir, { withFileTypes: true })) {
      if (!ent.isDirectory()) continue;
      const p = path.join(cratesDir, ent.name, "src");
      if (fs.existsSync(p)) roots.push(p);
    }
  }
  const out = [];
  const walk = (d) => {
    for (const ent of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, ent.name);
      if (ent.isDirectory()) walk(p);
      else if (ent.name.endsWith(".rs")) out.push(path.relative(root, p));
    }
  };
  for (const r of roots) if (fs.existsSync(r)) walk(r);
  return out.sort();
}

function readBaseline() {
  if (!fs.existsSync(BASELINE_FILE))
    throw new Error(
      `fant ikke ${path.relative(ROOT, BASELINE_FILE)} — kjør ` +
        "`node scripts/check-rust-norwegian.mjs --write-baseline`",
    );
  const b = JSON.parse(fs.readFileSync(BASELINE_FILE, "utf8"));
  if (!b.counts || typeof b.counts !== "object")
    throw new Error("fasit-fila mangler `counts`");
  if (!b.allowlist || typeof b.allowlist !== "object")
    throw new Error("fasit-fila mangler `allowlist`");
  return b;
}

/** Alle filers tall, allowlisten holdt utenfor. */
export function measure(files, allowlist, root = ROOT) {
  const counts = {};
  let total = 0;
  const hitsByFile = {};
  for (const rel of files) {
    if (allowlist[rel] !== undefined) continue;
    const { count, hits } = norwegianInFile(
      fs.readFileSync(path.join(root, rel), "utf8"),
    );
    if (count > 0) {
      counts[rel] = count;
      hitsByFile[rel] = hits;
      total += count;
    }
  }
  return { counts, total, hitsByFile };
}

// ── Selvtest (mutasjonsvern) ────────────────────────────────────────────────

const FIXTURE_OK = String.raw`
//! Doc-kommentar med æøå — teller IKKE.
// Linjekommentar med øø — teller IKKE.
/* Blokk med å /* nestet med å */ fortsatt kommentar med å */
pub const EVENT: &str = "recording://error";      // ingen norske tegn
fn body() -> &'static str {
    let a = "Planlagt opptak startet.";            // 0
    let b = "Opptaket ble tomt eller skadet — ingen fil ble lagret."; // 0
    let c = "gå glipp av så mange";                // å, å, å = 3
    let d = r#"rå streng med ø"#;                  // å, ø = 2
    let e = "delt over \
             to linjer med æ";                    // æ = 1
    let _lifetime: &'a str = "";
    let _ch = '/';
    a
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert_eq!(body(), "en norsk setning med ø og å i en TEST");
    }
}
`;

// Samme fil, pluss ÉN ny norsk literal i produksjonskode. Skralla må se den.
const FIXTURE_REGRESSED = FIXTURE_OK.replace(
  `    a
}`,
  `    let f = "én ny setning på norsk";
    a
}`,
);

function selfTest() {
  const problems = [];
  const say = (ok, what) => {
    if (!ok) problems.push(what);
  };

  const ok = norwegianInFile(FIXTURE_OK);
  // c=3 (gå, så → å,å + glipp? nei: "gå glipp av så mange" har å og å = 2)
  // Regn etter: "gå glipp av så mange" → å (gå) + å (så) = 2.
  // d: "rå streng med ø" → å + ø = 2. e: "…med æ" → 1. Sum = 5.
  say(ok.count === 5, `fixturen ga ${ok.count} norske tegn, fasit 5`);
  say(
    ok.hits.length === 3,
    `fixturen ga ${ok.hits.length} treff-literaler, fasit 3`,
  );

  // Hver lesemåte for seg — en mutasjon som sløyfer ÉN skal felles her.
  say(
    ok.hits.some((h) => h.text.includes("rå streng")),
    'rå strenger (`r#"…"#`) leses ikke lenger',
  );
  say(
    ok.hits.some((h) => h.text.includes("to linjer")),
    "strenger med linjefortsettelse (`\\` + linjeskift) leses ikke lenger",
  );
  say(
    !ok.hits.some((h) => h.text.includes("i en TEST")),
    "en literal inne i `#[cfg(test)] mod tests` ble talt",
  );
  say(
    !ok.hits.some((h) => h.text.includes("kommentar")),
    "en kommentar ble talt som streng",
  );

  // `//` inni en streng må ikke lese resten av fila som kommentar.
  const { blanked } = scanRust(FIXTURE_OK);
  say(
    blanked.includes('pub const EVENT: &str = "') &&
      blanked.includes("fn body"),
    "kommentarstripperen spiste kode etter en streng med // i seg",
  );
  say(
    blanked.length === FIXTURE_OK.length,
    "den utblankede kilden har ikke samme lengde som kilden",
  );

  // …og det avgjørende: skralla må kunne SI NEI.
  const worse = norwegianInFile(FIXTURE_REGRESSED);
  say(
    worse.count > ok.count,
    `en ny norsk literal hevet ikke tallet (${worse.count} vs ${ok.count})`,
  );
  say(
    compare({ "f.rs": ok.count }, { "f.rs": worse.count }).length === 1,
    "sammenligningen godtok en fil som gikk OPP",
  );
  say(
    compare({ "f.rs": worse.count }, { "f.rs": ok.count }).length === 0,
    "sammenligningen avviste en fil som gikk NED",
  );
  say(
    compare({}, { "ny.rs": 1 }).length === 1,
    "sammenligningen godtok en HELT NY fil med norsk i seg",
  );

  if (problems.length) {
    console.error("check-rust-norwegian SELVTEST FEILET:");
    for (const p of problems) console.error("  ✗ " + p);
    process.exit(2);
  }
}

/**
 * Filene som gikk OPP (eller er nye med norsk i seg). Ren funksjon, så
 * selvtesten kan bevise at den kan si nei uten å røre disken.
 */
export function compare(baselineCounts, actualCounts) {
  const bad = [];
  for (const [file, now] of Object.entries(actualCounts)) {
    const allowed = baselineCounts[file] ?? 0;
    if (now > allowed) bad.push({ file, now, allowed });
  }
  return bad;
}

// ── Gate ────────────────────────────────────────────────────────────────────

function main() {
  selfTest();

  const args = process.argv.slice(2);
  const files = rustSources();
  if (files.length === 0) {
    console.error(
      "norsk-skralla FEILET: fant ingen .rs-filer i src-tauri/src/** eller " +
        "crates/**/src/** — lesingen er ødelagt, ikke koden.",
    );
    process.exit(1);
  }

  if (args.includes("--write-baseline")) {
    const prev = fs.existsSync(BASELINE_FILE) ? readBaseline() : null;
    const allowlist = prev?.allowlist ?? {};
    const { counts, total } = measure(files, allowlist);
    const body = {
      _comment:
        "Generert av `node scripts/check-rust-norwegian.mjs --write-baseline`. " +
        "`counts` er antall norske tegn (æøåÆØÅ) i PRODUKSJONS-strengliteraler " +
        "per fil — kommentarer og #[cfg(test)] holdes utenfor. Skralla feiler " +
        "når en fil går OPP, eller når en fil som ikke står her får norsk i " +
        "seg. Ned er alltid greit; kjør --write-baseline etter en flytting så " +
        "tallet ikke kan sprette tilbake. `allowlist` er filene der norsk er " +
        "RIKTIG, med grunnen skrevet ut — ikke et sted å gjemme gjeld.",
      generatedAt: new Date().toISOString(),
      allowlist,
      total,
      counts,
    };
    fs.writeFileSync(BASELINE_FILE, JSON.stringify(body, null, 2) + "\n");
    console.log(
      `skrev fasit: ${Object.keys(counts).length} filer, ${total} norske tegn ` +
        `(+ ${Object.keys(allowlist).length} på allowlisten).`,
    );
    return;
  }

  const baseline = readBaseline();
  const { counts, total, hitsByFile } = measure(files, baseline.allowlist);

  if (args.includes("--list")) {
    const rows = Object.entries(counts).sort((a, b) => b[1] - a[1]);
    for (const [file, n] of rows) {
      const allowed = baseline.counts[file] ?? 0;
      const mark = n > allowed ? "✗" : n < allowed ? "↓" : "·";
      console.log(
        `${mark} ${String(n).padStart(4)}  (fasit ${allowed})  ${file}`,
      );
      for (const h of hitsByFile[file])
        console.log(
          `        :${h.line} ${h.text.replace(/\s+/g, " ").slice(0, 90)}`,
        );
    }
    // Allowlisten teller ikke, men den SKJULES heller ikke: tallene står her
    // så «unntatt» aldri blir det samme som «usynlig».
    for (const [file, why] of Object.entries(baseline.allowlist)) {
      const abs = path.join(ROOT, file);
      const n = fs.existsSync(abs)
        ? norwegianInFile(fs.readFileSync(abs, "utf8")).count
        : 0;
      console.log(`~ ${String(n).padStart(4)}  (allowlist)         ${file}`);
      console.log(`        ${why}`);
    }
    console.log(`\n${total} norske tegn i ${rows.length} tellende filer.`);
    return;
  }

  // En skralle uten noe å telle er grønn av tomhet: fasiten har et tall, og
  // hvis lesingen slutter å finne NOE i det hele tatt er det lesingen som er
  // ødelagt — ikke en jubeldag.
  if (total === 0 && baseline.total > 0) {
    console.error(
      "norsk-skralla FEILET: fant 0 norske tegn i hele treet, mens fasiten " +
        `sier ${baseline.total}. Enten ble alt oversatt i én commit (kjør ` +
        "--write-baseline), eller så leser skriptet ingenting.",
    );
    process.exit(1);
  }

  const bad = compare(baseline.counts, counts);
  if (bad.length) {
    console.error("norsk-skralla FEILET — norsk i strengliteraler økte:\n");
    for (const { file, now, allowed } of bad) {
      console.error(`  ✗ ${file}: ${now} norske tegn, fasit ${allowed}`);
      for (const h of hitsByFile[file])
        console.error(
          `      :${h.line} ${h.text.replace(/\s+/g, " ").slice(0, 90)}`,
        );
    }
    console.error(
      "\nEn brukervendt setning hører hjemme i `sundayrec_core::alerts`\n" +
        "(`AlertText`-varianten + alle sju språk — kompilatoren krever dem).\n" +
        "En loggtekst eller en diagnose hører hjemme på ENGELSK.\n" +
        "Er norsken faktisk riktig her (data, et navn), sett fila i\n" +
        "`allowlist` i scripts/rust-norwegian-baseline.json MED en grunn.",
    );
    process.exit(1);
  }

  const dropped = baseline.total - total;
  const suffix =
    dropped > 0
      ? ` — ${dropped} færre enn fasiten; kjør --write-baseline så tallet ikke spretter tilbake.`
      : ".";
  console.log(
    `norsk-skralle OK — ${total} norske tegn i ${Object.keys(counts).length} ` +
      `filer, fasit ${baseline.total}${suffix}`,
  );
}

main();
