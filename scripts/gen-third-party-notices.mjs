#!/usr/bin/env node
// Lager src-tauri/resources/THIRD-PARTY-NOTICES.html (pakke A1b).
//
//   node scripts/gen-third-party-notices.mjs          skriv fila
//   node scripts/gen-third-party-notices.mjs --check  feil hvis den innsjekkede fila er gammel
//
// Rust-delen kommer fra `cargo about --format json` (about.toml), npm-delen fra
// package.json sine `dependencies` (det som havner i frontend-bunten), og
// ffmpeg-/GPL-avsnittet står fast her.
//
// HTML-en bygges HER og ikke med en cargo-about-mal, fordi malens rekkefølge
// følger katalogrekkefølgen i crate-kildene, og den er ulik på APFS og ext4
// (første CI-kjøring: ring-lisensene kom i annen rekkefølge enn på Macen).
// Sortert på (lisens, tekst) og (crate, versjon) er fila lik overalt. Fila er innsjekket fordi
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

const FFMPEG = `<h2 id="ffmpeg">FFmpeg (GPL)</h2>
<p>The installers include the <code>ffmpeg</code> and <code>ffprobe</code>
programs, which SundayRec runs as separate processes. They are built with
<code>--enable-gpl --enable-version3</code> and are licensed under the
<strong>GNU General Public License, version 3 or later</strong>. The GPL applies
to those two programs only, not to the rest of SundayRec. Nothing is built with
<code>--enable-nonfree</code>.</p>
<p>You are entitled to the corresponding source code of the FFmpeg version we
ship (see <code>docs/DISTRIBUTION.md</code> in the SundayRec repository for the
exact version and build):</p>
<ul>
<li>FFmpeg source releases: <code>https://ffmpeg.org/download.html</code></li>
<li>macOS builds (Martin Riedl): <code>https://ffmpeg.martin-riedl.de</code></li>
<li>Windows builds (gyan.dev): <code>https://www.gyan.dev/ffmpeg/builds/</code></li>
<li>SundayRec source: <code>https://github.com/SundaySuite-app/sundayrec</code></li>
</ul>
<p>The GPL v3 text: <code>https://www.gnu.org/licenses/gpl-3.0.txt</code>.
FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.</p>`;

const HEAD = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>SundayRec - third-party notices</title>
<style>
  body { font: 15px/1.5 system-ui, sans-serif; max-width: 52rem; margin: 2rem auto; padding: 0 1rem; color: #1a1a1a; background: #fff; }
  h1 { font-size: 1.6rem; } h2 { margin-top: 2.5rem; font-size: 1.2rem; border-bottom: 1px solid #ccc; padding-bottom: .25rem; }
  pre { white-space: pre-wrap; background: #f5f5f5; padding: .75rem; border-radius: 6px; font-size: 12px; }
  ul.used-by { columns: 2; font-size: 13px; } code { font-size: 13px; }
  @media (prefers-color-scheme: dark) { body { color: #e6e6e6; background: #161616; } pre { background: #222; } h2 { border-color: #444; } }
</style>
</head>
<body>
<h1>Third-party notices</h1>
<p>SundayRec is MIT-licensed. It is built from, and distributed together with,
the software listed below. Each component keeps its own license; the notices
are reproduced here as those licenses require.</p>
`;

const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

function rustSection(json) {
  // Same licence text under the same id = one block, with the union of users.
  const groups = new Map();
  for (const l of json.licenses) {
    const text = l.text.replace(/\r\n/g, "\n").trim();
    const key = `${l.id}\u0000${text}`;
    let g = groups.get(key);
    if (!g)
      groups.set(key, (g = { name: l.name, id: l.id, text, users: new Set() }));
    for (const u of l.used_by) {
      g.users.add(
        `${esc(u.crate.name)} ${esc(u.crate.version)}` +
          (u.crate.repository
            ? ` - <code>${esc(u.crate.repository)}</code>`
            : ""),
      );
    }
  }
  return [...groups.values()]
    .sort((a, b) => cmp(a.id, b.id) || cmp(a.text, b.text))
    .map(
      (g) =>
        `<h3>${esc(g.name)}</h3>\n<ul class="used-by">\n` +
        [...g.users]
          .sort(cmp)
          .map((u) => `<li>${u}</li>`)
          .join("\n") +
        `\n</ul>\n<pre>${esc(g.text)}</pre>`,
    )
    .join("\n");
}

const tmp = mkdtempSync(join(tmpdir(), "notices-"));
try {
  const rustOut = join(tmp, "rust.json");
  execFileSync(
    "cargo",
    [
      "about",
      "generate",
      "--format",
      "json",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "-c",
      "about.toml",
      "-o",
      rustOut,
    ],
    { cwd: root, stdio: ["ignore", "inherit", "inherit"] },
  );
  const json = JSON.parse(readFileSync(rustOut, "utf8"));
  const html =
    HEAD +
    FFMPEG +
    `\n<h2 id="frontend">User interface libraries (npm)</h2>\n${npmSection()}\n` +
    `<h2 id="rust">Rust libraries</h2>\n${rustSection(json)}\n</body>\n</html>\n`;
  if (check) {
    const have = existsSync(OUT) ? readFileSync(OUT, "utf8") : "";
    // Sammenlignes på INNHOLDET det handler om, ikke byte for byte: delen før
    // Rust-seksjonen (ffmpeg + npm, med lisensteksten) må være lik, og Rust-
    // seksjonen må liste nøyaktig de samme crate+versjon-ene. Lisenstekstene
    // cargo-about finner INNE i et crate (kildefiler med lisenshode, som i
    // `ring`) er ikke den samme mengden på Mac og Linux, og en sjekk som
    // flakker blir slått av. Den sjekken skal fange «Cargo.lock endret seg».
    const split = (h) => {
      const k = h.indexOf('<h2 id="rust">');
      return {
        head: k < 0 ? h : h.slice(0, k),
        crates: new Set(
          h
            .slice(Math.max(k, 0))
            .split("\n")
            .filter((l) => l.startsWith("<li>")),
        ),
      };
    };
    const a = split(have);
    const b = split(html);
    const missing = [...b.crates].filter((c) => !a.crates.has(c));
    const extra = [...a.crates].filter((c) => !b.crates.has(c));
    if (a.head !== b.head || missing.length > 0 || extra.length > 0) {
      if (a.head !== b.head) console.error("Forskjell i ffmpeg-/npm-delen.");
      for (const c of missing) console.error(`  mangler i fila: ${c}`);
      for (const c of extra) console.error(`  burde ikke stå i fila: ${c}`);
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
