#!/usr/bin/env node
// Empty the ts-rs OUTPUT directory before regenerating.
//
// Why this exists: `cargo test export_bindings` only ever WRITES files. It has
// no idea a type was deleted, so a binding whose Rust source is gone survives
// in `src/lib/bindings/` forever. That directory is gitignored, so nobody sees
// it — and `sync-bindings.mjs` then treats the stale file as "still generated"
// and leaves its copy in the committed `legacy/bindings/`.
//
// The result is a gate that lies in one direction only: green on a developer's
// machine (stale files present on both sides, no diff), red in CI (clean tree,
// the stale copies get removed, `git status` sees the deletions). That is
// exactly what happened when v0.14's Live removal deleted 17 streaming/NDI/
// overlay types — `npm run check` passed locally and the release PR failed.
//
// Deleting the output directory first makes the generated set a pure function
// of the current Rust source, so local and CI can never disagree again.
//
// ── …and the second half of the same promise ────────────────────────────────
//
// Clearing the output directory only helps for files that land IN it. Until the
// `TS_RS_EXPORT_DIR` pin in `.cargo/config.toml`, ts-rs anchored every relative
// `export_to` at the current directory of whichever test binary happened to
// write it — and because ts-rs exports a type's DEPENDENCIES too, the
// `sundayrec-core` types that `src-tauri` types reference were written with
// core's `../../../…` prefix from src-tauri's directory. They landed OUTSIDE the
// checkout (`<repo>/../src/lib/bindings/`), where this script cannot clear them,
// `.gitignore` cannot hide them and no gate can see them; a stale `ImageFormat.ts`
// had been rotting there since 10 Aug 2026.
//
// The pin makes the base absolute, so the anchor is the same for every package.
// The invariant that keeps it that way is checked here, at the one moment every
// `npm run bindings` (and therefore `bindings:check`, and therefore CI) passes
// through: an `export_to` must be a BARE file name. A path separator in one is
// a relative anchor sneaking back in.
import {
  existsSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const genDir = join(root, "src", "lib", "bindings");

// ── The invariant: `export_to` is a bare file name ──────────────────────────
const EXPORT_TO = /export_to\s*=\s*"([^"]*)"/g;
const offenders = [];

/** Every `.rs` file under `dir`, recursively. */
function rustFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      if (entry === "target" || entry === "node_modules") continue;
      out.push(...rustFiles(path));
    } else if (entry.endsWith(".rs")) {
      out.push(path);
    }
  }
  return out;
}

for (const dir of [join(root, "crates"), join(root, "src-tauri", "src")]) {
  if (!existsSync(dir)) continue;
  for (const file of rustFiles(dir)) {
    const src = readFileSync(file, "utf8");
    for (const [, target] of src.matchAll(EXPORT_TO)) {
      if (target.includes("/") || target.includes("\\")) {
        offenders.push(`${relative(root, file)}: export_to = "${target}"`);
      }
    }
  }
}

if (offenders.length > 0) {
  console.error(
    `✗ ${offenders.length} ts-rs export_to path(s) are not bare file names — they re-anchor the\n` +
      `  export on the writing package's directory and can escape the repo entirely.\n` +
      `  Use just the file name; the directory is TS_RS_EXPORT_DIR in .cargo/config.toml.\n` +
      offenders.map((o) => `    ${o}`).join("\n"),
  );
  process.exit(1);
}

if (existsSync(genDir)) {
  rmSync(genDir, { recursive: true, force: true });
  console.log("clean-bindings: cleared src/lib/bindings (ts-rs never prunes).");
} else {
  console.log("clean-bindings: src/lib/bindings absent, nothing to clear.");
}
