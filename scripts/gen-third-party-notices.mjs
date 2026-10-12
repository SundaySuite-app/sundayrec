#!/usr/bin/env node
// Lager src-tauri/resources/THIRD-PARTY-NOTICES.html (pakke A1b).
//
//   node scripts/gen-third-party-notices.mjs          skriv fila
//   node scripts/gen-third-party-notices.mjs --check  feil hvis den innsjekkede fila er gammel
//
// Rust-delen kommer fra `cargo about` (about.toml + about.hbs), npm-delen fra
// package.json sine `dependencies` (det som havner i frontend-bunten), og
// ffmpeg-/GPL-avsnittet står fast i about.hbs. Fila er innsjekket fordi
// `tauri build` krever at ressursfila finnes, og fordi en endring i
// Cargo.lock/package-lock.json da vises i diffen til PR-en som gjorde den.
import { execFileSync } from "node:child_process";
import {
  readFileSync,
  writeFileSync,
  mkdtempSync,
  rmSync,
  existsSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(root, "src-tauri", "resources", "THIRD-PARTY-NOTICES.html");
const check = process.argv.includes("--check");

const esc = (s) =>
  s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");

function npmSection() {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  // Transitive prod closure, walked through node_modules/<name>/package.json.
  const seen = new Map();
  const queue = Object.keys(pkg.dependencies ?? {});
  while (queue.length) {
    const name = queue.pop();
    if (seen.has(name)) continue;
    const dir = join(root, "node_modules", name);
    const p = join(dir, "package.json");
    if (!existsSync(p))
      throw new Error(`node_modules/${name} mangler — kjør npm ci`);
    const j = JSON.parse(readFileSync(p, "utf8"));
    const file = readdirSync(dir).find((f) =>
      /^(licen[sc]e|copying)(\.|$)/i.test(f),
    );
    seen.set(name, {
      version: j.version,
      license:
        typeof j.license === "string" ? j.license : JSON.stringify(j.license),
      text: file ? readFileSync(join(dir, file), "utf8").trim() : null,
    });
    for (const d of Object.keys(j.dependencies ?? {})) queue.push(d);
  }
  return [...seen.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(
      ([n, v]) =>
        `<h3>${esc(n)} ${esc(v.version)} — ${esc(v.license)}</h3>\n` +
        (v.text
          ? `<pre>${esc(v.text)}</pre>`
          : `<p>License: ${esc(v.license)}.</p>`),
    )
    .join("\n");
}

const tmp = mkdtempSync(join(tmpdir(), "notices-"));
try {
  const rustOut = join(tmp, "rust.html");
  execFileSync(
    "cargo",
    [
      "about",
      "generate",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "-c",
      "about.toml",
      "about.hbs",
      "-o",
      rustOut,
    ],
    { cwd: root, stdio: ["ignore", "inherit", "inherit"] },
  );
  // CRLF -> LF: some crates ship CRLF licence files. Git normalises them on
  // commit, and CI's checkout would then differ from what this wrote on a Mac.
  const html = readFileSync(rustOut, "utf8")
    .replace("<!--NPM-->", () => npmSection())
    .replace(/\r\n/g, "\n");
  if (html.includes("<!--NPM-->"))
    throw new Error("npm-markøren ble ikke erstattet");
  if (check) {
    const have = existsSync(OUT) ? readFileSync(OUT, "utf8") : "";
    if (have !== html) {
      console.error(
        "THIRD-PARTY-NOTICES.html er ikke fersk. Kjør `npm run licenses:notices` og sjekk inn fila.",
      );
      process.exit(1);
    }
    console.log("THIRD-PARTY-NOTICES.html er fersk.");
  } else {
    writeFileSync(OUT, html);
    console.log(`Skrev ${OUT} (${html.length} byte).`);
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
