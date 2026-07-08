// Sync the freshly generated ts-rs bindings from the transient regen target
// (`src/lib/bindings/`, gitignored) into the committed, consumed location
// (`legacy/bindings/`). Runs as the last step of `npm run bindings`.
//
// `legacy/bindings/` is inside the Vite root's import graph (tsconfig includes
// `legacy`), so `legacy/types/index.ts` re-exports generated types from there —
// backend contract changes surface as tsc errors instead of silent drift. CI
// regenerates and diffs this directory to catch stale bindings.
//
// The sync is a full mirror: stale files in legacy/bindings that the regen no
// longer produces are removed.

import {
  readdirSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  rmSync,
} from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = join(root, "src", "lib", "bindings");
const outDir = join(root, "legacy", "bindings");

let generated;
try {
  generated = readdirSync(srcDir).filter((f) => f.endsWith(".ts"));
} catch {
  console.error(
    `✗ ${srcDir} not found — run \`npm run bindings\` (which regenerates it) instead of calling this script directly.`,
  );
  process.exit(1);
}
if (generated.length === 0) {
  console.error(`✗ ${srcDir} contains no .ts bindings — regen failed?`);
  process.exit(1);
}

mkdirSync(outDir, { recursive: true });
const existing = readdirSync(outDir).filter((f) => f.endsWith(".ts"));

let wrote = 0;
for (const file of generated) {
  const next = readFileSync(join(srcDir, file), "utf8");
  let prev = null;
  try {
    prev = readFileSync(join(outDir, file), "utf8");
  } catch {
    /* new file */
  }
  if (prev !== next) {
    writeFileSync(join(outDir, file), next);
    wrote++;
  }
}

let removed = 0;
for (const file of existing) {
  if (!generated.includes(file)) {
    rmSync(join(outDir, file));
    removed++;
  }
}

console.log(
  `sync-bindings: ${generated.length} bindings → legacy/bindings (${wrote} updated, ${removed} removed).`,
);
