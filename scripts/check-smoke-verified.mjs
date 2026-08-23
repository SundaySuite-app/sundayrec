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
// Name matching is by TEST, not by substring.
//
// ⚠️ It used to be a plain `src.includes(name)`, with the reasoning that a
// false positive "would need the exact title duplicated verbatim, which is
// close enough to 'the claim is still asserted somewhere in that file' for a
// tripwire". Fase B is where that premise died: the new shell's specs were
// written as re-pointed copies of the old ones and each carries a comment
// listing, VERBATIM, the legacy titles it deliberately did NOT carry over —
// which is the exact shape the substring test cannot tell from a real test.
// Three pointers went on reading green over screens nobody had built, and only
// the fourth (whose title appeared nowhere at all) failed.
//
// So a pointer must now resolve to a title passed to `test(…)`, `it(…)` or
// `describe(…)` — including their `.only`/`.skip`/`.describe` forms — which is
// what a VERIFIED-BY pointer always claimed. Rust is unchanged: `fn <name>(`.
// A renamed or deleted test still makes it vanish; a title quoted in a comment
// no longer counts as coverage.

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

/**
 * Every title declared by a `test` / `it` / `describe` call in one file.
 *
 * Deliberately syntactic rather than semantic: the argument has to be a plain
 * double- or single-quoted literal, which every spec and vitest file in this
 * repo writes. A title built from a template or a variable would not be found —
 * and that is the right answer, because a VERIFIED-BY pointer names a title
 * somebody can read in the runbook and then grep for.
 */
const titleCache = new Map();
function testTitles(abs, src) {
  if (titleCache.has(abs)) return titleCache.get(abs);
  const titles = new Set();
  const CALL =
    /\b(?:test|it|describe)(?:\.(?:only|skip|todo|fails|concurrent|sequential|each|describe))*\s*\(\s*(["'])((?:[^\\]|\\.)*?)\1/g;
  for (const m of src.matchAll(CALL)) {
    titles.add(m[2].replace(/\\(["'\\])/g, "$1"));
  }
  titleCache.set(abs, titles);
  return titles;
}

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
    : testTitles(abs, src).has(name);
  if (!found) {
    problems.push(
      `${file} :: ${name}\n      no test by that name in the file (renamed, deleted — or only quoted in a comment?)`,
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
