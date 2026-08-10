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
import { existsSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const genDir = join(root, "src", "lib", "bindings");

if (existsSync(genDir)) {
  rmSync(genDir, { recursive: true, force: true });
  console.log("clean-bindings: cleared src/lib/bindings (ts-rs never prunes).");
} else {
  console.log("clean-bindings: src/lib/bindings absent, nothing to clear.");
}
