// Normalise ts-rs cross-crate imports in the generated bindings.
//
// ts-rs computes the relative import between two types from their `export_to`
// paths. Our types live in TWO crates (`src-tauri` and `crates/sundayrec-core`)
// whose `export_to` USED TO be relative paths anchored at different depths, so
// when a `src-tauri` type imported a `sundayrec-core` type ts-rs emitted a path
// that escaped the repo, e.g.
//   import type { ChannelMode } from "../../../../src/lib/bindings/ChannelMode";
// That resolves to `<repo>/../src/lib/bindings/...` (outside the checkout), so it
// built locally (where a stray copy existed) but failed on a clean CI checkout
// with TS2307.
//
// Since the `TS_RS_EXPORT_DIR` pin in `.cargo/config.toml`, every `export_to` is
// a BARE file name against one absolute directory, so ts-rs already emits
// `./Name` and this script normally reports "normalised 0 file(s)". It stays as
// the belt to that braces: every binding lives in this one directory, so an
// import that walks out of it is wrong however it got there. Rewrite any
// `(…/)*src/lib/bindings/Name` import to `./Name`. Idempotent; run as the last
// step of `npm run bindings`.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const dir = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "src",
  "lib",
  "bindings",
);
// `from "../../.../src/lib/bindings/Name"` (one or more `../`) → `from "./Name"`.
const BAD = /from "(?:\.\.\/)+src\/lib\/bindings\/([A-Za-z0-9_]+)"/g;

let fixed = 0;
for (const file of readdirSync(dir)) {
  if (!file.endsWith(".ts")) continue;
  const path = join(dir, file);
  const src = readFileSync(path, "utf8");
  const out = src.replace(BAD, 'from "./$1"');
  if (out !== src) {
    writeFileSync(path, out);
    fixed++;
  }
}
console.log(`fix-bindings-imports: normalised ${fixed} file(s).`);
