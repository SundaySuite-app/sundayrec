#!/usr/bin/env node
/**
 * Feilkode-gate: hver kode motoren sender på den TERMINALE feilkanalen må ha
 * en setning i skallet.
 *
 * ## Hvorfor
 *
 * `emit_error(app, "video_capture_failed", …)` er en helt vanlig linje å
 * skrive. Ingenting i Rust vet at den koden også må stå i `NATIVE_ERRORS` i
 * `app/pages/record/record-core.ts`, og ingenting i TypeScript vet at Rust
 * nettopp fant på en ny. Resultatet er ikke en feilmelding og ikke en tom
 * skjerm — det er `errorUnknown`: «Noe gikk galt under opptak — sjekk at
 * lydenhet og lagringsmappe er klare», vist til en frivillig hvis kamera
 * aldri åpnet. En sann setning som ikke sier noe.
 *
 * Fire koder hadde ligget slik: `start_timeout`, `ffmpeg_exited`,
 * `video_capture_failed`, `mux_failed`. Ingen test kunne se det, fordi ingen
 * av de to sidene er gal alene. Det er skjøten som er gal — samme klasse som
 * `docs/reference`-notatet om skjøtefeil beskriver, og samme svar: en gate som
 * leser BEGGE sidene og spør om de er enige.
 *
 * ## Hva den leser
 *
 *   Rust (`src-tauri/src/recorder/**`):
 *     • `emit_error(<app>, "<kode>", …)`   — argument 2
 *     • `<sink>.error("<kode>", …)`        — argument 1
 *     • armene i `fn error_code_str(…)`    — `Code::X => "<kode>"`
 *   TypeScript:
 *     • nøklene i `const NATIVE_ERRORS = { … }` i `record-core.ts`
 *
 * TS-kilden leses som TEKST, ikke kompilert: gaten skal kunne kjøre alene, før
 * `tsc`, og en typefeil et helt annet sted i skallet skal ikke gjøre denne
 * gaten grønn av forfall.
 *
 * ## Én vei, med vilje
 *
 * Gaten krever at hver EMITTERT kode finnes i tabellen — ikke omvendt.
 * Tabellen dekker også koder som aldri kommer fra `recorder/**`
 * (`no_device`, `already_recording`, `save_folder_permission` … kommer fra
 * kommandolaget og fra `AppError`), og et krav om at hver tabellrad har et
 * emit-sted ville tvunget fram sletting av oversatt tekst som faktisk vises.
 *
 * ## Advarsler er IKKE med
 *
 * `emit_warning`/`sink.warning` står utenfor med vilje. `WARNING_EVENT` river
 * ikke overlegget ned og går ikke til `notify::wire_failure_sources`; skallet
 * viser gjenkoblingsstripa, ikke en kodeoversatt setning. Å kreve en
 * katalognøkkel per advarselskode ville vært å kreve tekst ingen rendrer.
 *
 * Bruk:
 *   node scripts/check-error-codes.mjs          # gate
 *   node scripts/check-error-codes.mjs --list   # vis hver kode og hvor den kom fra
 *
 * Mutasjonsvern: skriptet kjører først seg selv mot en innebygd fasit-fixtur —
 * ekte Rust og ekte TS, med kjent svar og én kjent MANGEL. Slutter en av de tre
 * lesemåtene å finne noe, eller slutter sammenligningen å kunne si nei, feiler
 * selvtesten (exit 2) før gaten får uttale seg. En gate som kan mutere til
 * «alltid grønn» er ingen gate.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const RUST_DIR = path.join(ROOT, "src-tauri", "src", "recorder");
const TABLE_FILE = path.join(ROOT, "app", "pages", "record", "record-core.ts");

// ── Rust-lesing ─────────────────────────────────────────────────────────────

/**
 * Blank ut kommentarer, behold alt annet på PLASS (samme lengde, samme
 * linjeskift) så treff fortsatt har riktig linjenummer.
 *
 * Strengbevisst, og det er ikke pynt: `pub const ERROR_EVENT: &str =
 * "recording://error";` inneholder `//` INNI en streng. En naiv
 * kommentarstripper kutter der, resten av fila blir «inni en streng», og
 * gaten finner null emit-steder — grønn av tomhet, i den ene fila som betyr
 * mest.
 */
export function blankComments(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  while (i < n) {
    const c = src[i];
    const c2 = src[i + 1];
    // Linjekommentar.
    if (c === "/" && c2 === "/") {
      while (i < n && src[i] !== "\n") {
        out += " ";
        i++;
      }
      continue;
    }
    // Blokkkommentar (Rust nester dem).
    if (c === "/" && c2 === "*") {
      let depth = 0;
      while (i < n) {
        if (src[i] === "/" && src[i + 1] === "*") {
          depth++;
          out += "  ";
          i += 2;
          continue;
        }
        if (src[i] === "*" && src[i + 1] === "/") {
          depth--;
          out += "  ";
          i += 2;
          if (depth === 0) break;
          continue;
        }
        out += src[i] === "\n" ? "\n" : " ";
        i++;
      }
      continue;
    }
    // Tegnliteral: `'a'`, `'\n'` — men IKKE en levetid (`&'a str`), som ikke
    // har noen sluttfnutt og ville slukt resten av fila.
    if (c === "'") {
      const m = /^'(\\.|[^\\'])'/.exec(src.slice(i));
      if (m) {
        out += m[0];
        i += m[0].length;
        continue;
      }
      out += c;
      i++;
      continue;
    }
    // Rå streng: r"…", r#"…"#.
    if (c === "r" && (c2 === '"' || c2 === "#")) {
      const m = /^r(#*)"/.exec(src.slice(i));
      if (m) {
        const close = '"' + m[1];
        const end = src.indexOf(close, i + m[0].length);
        const stop = end === -1 ? n : end + close.length;
        out += src.slice(i, stop);
        i = stop;
        continue;
      }
    }
    // Vanlig streng.
    if (c === '"') {
      out += c;
      i++;
      while (i < n) {
        if (src[i] === "\\") {
          out += src.slice(i, i + 2);
          i += 2;
          continue;
        }
        out += src[i];
        i++;
        if (src[i - 1] === '"') break;
      }
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

/**
 * Del argumentlista som starter rett etter `(` på index `open`.
 *
 * Respekterer nesting og strenger, så `&format!("a, b", x)` er ETT argument og
 * ikke to. Returnerer trimmede argumenttekster.
 */
export function splitArgs(src, open) {
  const args = [];
  let depth = 0;
  let cur = "";
  let i = open + 1;
  const n = src.length;
  while (i < n) {
    const c = src[i];
    if (c === '"') {
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") {
          j += 2;
          continue;
        }
        if (src[j] === '"') break;
        j++;
      }
      cur += src.slice(i, j + 1);
      i = j + 1;
      continue;
    }
    if (c === "(" || c === "[" || c === "{") depth++;
    if (c === ")" || c === "]" || c === "}") {
      if (c === ")" && depth === 0) {
        args.push(cur.trim());
        return { args, end: i };
      }
      depth--;
    }
    if (c === "," && depth === 0) {
      args.push(cur.trim());
      cur = "";
      i++;
      continue;
    }
    cur += c;
    i++;
  }
  return { args, end: -1 };
}

/** `"noe"` → `noe`; alt annet (variabel, kall, format!) → `null`. */
export function stringLiteral(text) {
  const m = /^"((?:\\.|[^\\"])*)"$/.exec(text.trim());
  return m ? m[1] : null;
}

const lineOf = (src, index) => src.slice(0, index).split("\n").length;

/**
 * Kodene ÉN Rust-fil sender på den terminale kanalen.
 *
 * `emit_error(` tar koden som argument 2 (argument 1 er `AppHandle`-en),
 * `<sink>.error(` som argument 1. Et kall der koden IKKE er en literal er
 * ikke en glipe: `emit_error(app, error_code_str(code), …)` henter den fra
 * tabellen under, og den leses for seg.
 */
export function collectRustCodes(src, file = "?") {
  const code = blankComments(src);
  const found = [];

  // emit_error(app, "kode", …) — men ikke definisjonen `fn emit_error(`.
  for (const m of code.matchAll(/(?<![A-Za-z0-9_])emit_error\s*\(/g)) {
    const before = code.slice(Math.max(0, m.index - 4), m.index);
    if (/\bfn\s*$/.test(before)) continue;
    const { args } = splitArgs(code, m.index + m[0].length - 1);
    const lit = args.length >= 2 ? stringLiteral(args[1]) : null;
    if (lit) found.push({ code: lit, file, line: lineOf(code, m.index) });
  }

  // <noe>.error("kode", …) — sinken i native-stien. Krever at argument 1 ER en
  // literal, som holder `e.error()`-lignende kall ute uten en navneliste.
  for (const m of code.matchAll(
    /(?<![A-Za-z0-9_])[A-Za-z0-9_]+\s*\.\s*error\s*\(/g,
  )) {
    const { args } = splitArgs(code, m.index + m[0].length - 1);
    if (args.length < 2) continue;
    const lit = stringLiteral(args[0]);
    if (lit) found.push({ code: lit, file, line: lineOf(code, m.index) });
  }

  // Armene i `fn error_code_str(…) -> &'static str { match … }`: den ENE
  // tabellen som oversetter `RecordingErrorCode` til en streng. Kodene her
  // når skallet gjennom hvert `emit_error(app, error_code_str(c), …)`.
  const fnIdx = code.indexOf("fn error_code_str");
  if (fnIdx !== -1) {
    const open = code.indexOf("{", fnIdx);
    if (open !== -1) {
      let depth = 0;
      let end = open;
      for (let i = open; i < code.length; i++) {
        if (code[i] === "{") depth++;
        if (code[i] === "}") {
          depth--;
          if (depth === 0) {
            end = i;
            break;
          }
        }
      }
      const body = code.slice(open, end);
      for (const m of body.matchAll(/=>\s*"((?:\\.|[^\\"])*)"/g)) {
        found.push({
          code: m[1],
          file,
          line: lineOf(code, open + m.index),
          from: "error_code_str",
        });
      }
    }
  }

  return found;
}

// ── TypeScript-lesing ───────────────────────────────────────────────────────

/**
 * Nøklene i `NATIVE_ERRORS`, lest som tekst.
 *
 * Kaster hvis blokka ikke finnes: en gate som stille leser en tom tabell ville
 * feilet på ALT, og en gate som stille leser «ingen tabell» som «tom» ville
 * vært verre — den ville sagt fra én gang og så blitt slått av.
 */
export function collectTableKeys(src) {
  const start = src.indexOf("const NATIVE_ERRORS");
  if (start === -1) {
    throw new Error(
      "fant ikke `const NATIVE_ERRORS` i app/pages/record/record-core.ts — " +
        "ble tabellen omdøpt? Gaten må peke på den nye.",
    );
  }
  const open = src.indexOf("{", start);
  if (open === -1) throw new Error("NATIVE_ERRORS uten `{`");
  let depth = 0;
  let end = open;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    if (src[i] === "}") {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const body = src.slice(open + 1, end);
  const keys = new Set();
  for (const m of body.matchAll(
    /(?:^|\n)\s*(?:"([^"]+)"|'([^']+)'|(\w+))\s*:/g,
  ))
    keys.add(m[1] ?? m[2] ?? m[3]);
  if (keys.size === 0) throw new Error("NATIVE_ERRORS er tom");
  return keys;
}

// ── Filer ───────────────────────────────────────────────────────────────────

function rustFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const ent of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, ent.name);
      if (ent.isDirectory()) walk(p);
      else if (ent.name.endsWith(".rs")) out.push(p);
    }
  };
  walk(dir);
  return out.sort();
}

// ── Selvtest (mutasjonsvern) ────────────────────────────────────────────────

const SELFTEST_RUST = String.raw`
// A comment mentioning emit_error(app, "commented_out", "x") — must be ignored.
pub const ERROR_EVENT: &str = "recording://error";
/* block emit_error(app, "blocked", "x") */
fn emit_error(app: &AppHandle, code: &str, message: &str) { /* … */ }

fn run(app: &AppHandle, sink: &impl EventSink) {
    emit_error(&app, "disk_full", "Lite plass");
    emit_error(
        app,
        "start_timeout",
        &format!("Ingen start på {} s, prøv igjen", 30),
    );
    emit_error(app, error_code_str(code), &line);
    emit_warning(app, "stuck_recording", "kobler til på nytt");
    sink.error("mux_failed", &e.to_string());
    sink.warning("device_disconnected", &reason);
    self.sink.error("orphan_code", "ingen nøkkel for denne");
    let _: &'static str = "levetid ovenfor må ikke slukes";
}

pub(crate) fn error_code_str(code: RecordingErrorCode) -> &'static str {
    match code {
        RecordingErrorCode::DeviceNotFound => "device_not_found",
        RecordingErrorCode::DiskFull => "disk_full",
    }
}
`;

const SELFTEST_TS = `
const NATIVE_ERRORS: Record<string, string> = {
  disk_full: "errorDiskFull",
  start_timeout: "errorStartTimeout",
  mux_failed: "errorMux",
  device_not_found: "errorDeviceNotFound",
};
`;

function selfTest() {
  const problems = [];
  const say = (ok, what) => {
    if (!ok) problems.push(what);
  };

  const found = collectRustCodes(SELFTEST_RUST, "fixture.rs");
  const codes = [...new Set(found.map((f) => f.code))].sort();
  const want = [
    "device_not_found",
    "disk_full",
    "mux_failed",
    "orphan_code",
    "start_timeout",
  ];
  say(
    JSON.stringify(codes) === JSON.stringify(want),
    `Rust-lesingen fant [${codes}], fasit [${want}]`,
  );

  // Hver lesemåte for seg — en mutasjon som sløyfer ÉN av de tre skal felles
  // her og ikke maskeres av at de to andre fortsatt finner noe.
  say(
    found.some((f) => f.code === "start_timeout" && !f.from),
    "flerlinjet emit_error ble ikke funnet",
  );
  say(
    found.some((f) => f.code === "mux_failed" && !f.from),
    "sink.error ble ikke funnet",
  );
  say(
    found.some(
      (f) => f.code === "device_not_found" && f.from === "error_code_str",
    ),
    "error_code_str-armene ble ikke funnet",
  );

  // …og det som IKKE skal telle.
  for (const nope of ["commented_out", "blocked", "stuck_recording"]) {
    say(
      !codes.includes(nope),
      `«${nope}» skulle ikke telt som en terminal feilkode`,
    );
  }
  say(
    !codes.includes("device_disconnected"),
    "sink.warning skulle ikke telt (advarsler er utenfor gaten)",
  );

  // `recording://error` inni en streng må ikke leses som en kommentar.
  say(
    blankComments(SELFTEST_RUST).includes('"recording://error"'),
    "kommentarstripperen spiste en streng med // i seg",
  );

  const keys = collectTableKeys(SELFTEST_TS);
  say(keys.size === 4, `tabell-lesingen fant ${keys.size} nøkler, fasit 4`);
  say(keys.has("start_timeout"), "tabell-lesingen mistet en nøkkel");

  // Og det avgjørende: gaten må kunne SI NEI. `orphan_code` mangler i
  // fixturens tabell, nøyaktig som de fire ekte kodene manglet i den ekte.
  const missing = found.filter((f) => !keys.has(f.code));
  say(
    missing.length === 1 && missing[0].code === "orphan_code",
    `sammenligningen fant ${missing.length} manglende, fasit 1 (orphan_code)`,
  );

  if (problems.length) {
    console.error("check-error-codes SELVTEST FEILET:");
    for (const p of problems) console.error("  ✗ " + p);
    process.exit(2);
  }
}

// ── Gate ────────────────────────────────────────────────────────────────────

function main() {
  selfTest();

  const args = process.argv.slice(2);
  const keys = collectTableKeys(fs.readFileSync(TABLE_FILE, "utf8"));

  const found = [];
  for (const file of rustFiles(RUST_DIR)) {
    const rel = path.relative(ROOT, file);
    found.push(...collectRustCodes(fs.readFileSync(file, "utf8"), rel));
  }

  if (args.includes("--list")) {
    for (const f of found.sort((a, b) => a.code.localeCompare(b.code))) {
      const mark = keys.has(f.code) ? "✓" : "✗";
      console.log(
        `${mark} ${f.code.padEnd(26)} ${f.file}:${f.line}${f.from ? "  (" + f.from + ")" : ""}`,
      );
    }
    return;
  }

  // En gate uten koder å se på er grønn av tomhet.
  if (found.length === 0) {
    console.error(
      "feilkode-gaten FEILET: fant ingen feilkoder i src-tauri/src/recorder/** " +
        "i det hele tatt — enten er lesingen ødelagt, eller så emitterer " +
        "motoren ingen feil lenger.",
    );
    process.exit(1);
  }

  const missing = found.filter((f) => !keys.has(f.code));
  if (missing.length) {
    const seen = new Set();
    console.error(
      "feilkode-gaten FEILET — koder skallet ikke kan oversette:\n",
    );
    for (const f of missing) {
      if (seen.has(f.code)) continue;
      seen.add(f.code);
      console.error(
        `  ✗ "${f.code}"  (${f.file}:${f.line}${f.from ? ", " + f.from : ""})`,
      );
    }
    console.error(
      `\n${seen.size} kode(r) motoren sender på recording://error uten at ` +
        "NATIVE_ERRORS i app/pages/record/record-core.ts kjenner dem.\n" +
        "Brukeren får «errorUnknown» — en sann setning som ikke sier noe.\n\n" +
        "Legg til koden i NATIVE_ERRORS med et `recording.error*`-suffiks, og\n" +
        "skriv setningen i BÅDE legacy/locales/no.json og en.json (de fem\n" +
        "pausede språkene: legg nøkkelen i PAUSED_KEYS i\n" +
        "legacy/locales/parity.test.ts).",
    );
    process.exit(1);
  }

  const distinct = new Set(found.map((f) => f.code)).size;
  console.log(
    `feilkode-gate OK — ${distinct} distinkte koder fra ${found.length} ` +
      `emit-steder i src-tauri/src/recorder/**, alle oversettbare i skallet.`,
  );
}

main();
