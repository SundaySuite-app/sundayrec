// Keep docs/SMOKE-TEST.md's VERIFIED-BY pointers honest, and keep the
// UNVERIFIED burndown visible.
//
// The E11 rhythm converts manual smoke claims into automated tests; each
// converted claim is annotated in the runbook as
//
//     VERIFIED-BY: <test file>::<test name>
//
// where <test file> is repo-relative (an e2e spec, a vitest file, or a Rust
// file with #[test] fns). A pointer at a test that no longer exists is worse
// than no pointer — it says "covered" about a claim nothing covers — so this
// check FAILS on any pointer whose file is missing or whose test name is no
// longer found in that file. It also counts the remaining UNVERIFIED markers,
// so the burndown number is printed on every `npm run check` and in CI.
//
// Name matching is by substring on purpose: spec titles live inside
// `test("…")` strings and Rust test names inside `fn <name>()`, and both
// renames and deletions make the substring vanish. False positives (the name
// appearing in a comment) would need the exact title duplicated verbatim,
// which is close enough to "the claim is still asserted somewhere in that
// file" for a tripwire.

import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const DOC = "docs/SMOKE-TEST.md";
const doc = readFileSync(join(root, DOC), "utf8");

// ── 1. The burndown number ──────────────────────────────────────────────────
const unverified = doc.match(/UNVERIFIED/g)?.length ?? 0;

// ── 2. Every pointer resolves ───────────────────────────────────────────────
const pointers = [...doc.matchAll(/VERIFIED-BY:\s*(\S+)::(.+?)\s*$/gm)].map(
  (m) => ({ file: m[1], name: m[2] }),
);

const problems = [];
const fileCache = new Map();
for (const { file, name } of pointers) {
  const abs = join(root, file);
  if (!existsSync(abs)) {
    problems.push(`${file} :: ${name}\n      the file does not exist`);
    continue;
  }
  if (!fileCache.has(abs)) fileCache.set(abs, readFileSync(abs, "utf8"));
  const src = fileCache.get(abs);
  const found = file.endsWith(".rs")
    ? new RegExp(
        `fn\\s+${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\(`,
      ).test(src)
    : src.includes(name);
  if (!found) {
    problems.push(
      `${file} :: ${name}\n      no test by that name in the file (renamed or deleted?)`,
    );
  }
}

if (problems.length) {
  console.error(
    `✗ ${DOC} has ${problems.length} stale VERIFIED-BY pointer(s):`,
  );
  for (const p of problems) console.error(`    ${p}`);
  console.error(
    "  Fix the pointer (or restore the test) — a pointer at nothing claims coverage that does not exist.",
  );
  process.exit(1);
}

console.log(
  `✓ smoke runbook: ${pointers.length} VERIFIED-BY pointer(s) all resolve · ${unverified} UNVERIFIED marker(s) remain`,
);
