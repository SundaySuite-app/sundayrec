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
//
// ── ⚠️ THE FAILURE THIS FILE WAS REOPENED FOR ────────────────────────────────
//
// Everything above only ever looked at lines the POINTER REGEX matched. A line
// that says VERIFIED-BY but does not parse — a single `:` instead of `::`, a
// missing name, a stray line break after the file — simply fell out of
// `matchAll` and was never counted, never checked, never mentioned. The gate
// then printed a smaller, entirely truthful-sounding number and exited 0.
//
// Three broken separators took the count from 43 to 38 and nothing went red.
// That is worse than a stale pointer: a stale pointer at least claims something
// checkable, while a malformed one silently converts "this claim is covered"
// into "this claim was never here". The runbook keeps reading as if it were
// covered, because the sentence is still on the page for a human.
//
// Two rules close it, and they are the whole of §1 and §4 below:
//
//   1. A CLAIM LINE — VERIFIED-BY at the start of its own line, bullet or not —
//      must parse. If it does not, that is an error, with the line quoted.
//      A prose mention MID-SENTENCE is not a claim (this file's own prose, and
//      the runbook's, both do it), and that is the discriminator: a pointer is
//      a claim on its own line; prose mentions it inside a sentence.
//   2. The pointer count is a RATCHET, pinned at MIN_POINTERS. Pointers may be
//      added freely; losing one has to be a deliberate edit of this constant,
//      with the reason in the commit. Deletion by typo is exactly what rule 1
//      catches, and this is the belt under it: it also catches deletion by
//      DELETION — a pointer line removed wholesale leaves no malformed line
//      behind to notice.
//
// Both rules, and the resolution rules above them, are exercised by a SELF-TEST
// against an embedded fixture with a known answer — one known failure per
// class — that runs before the gate is allowed to speak. A gate that can mutate
// into "always green" is not a gate. (Same house pattern as
// scripts/check-i18n-keys.mjs.)

import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const DOC = "docs/SMOKE-TEST.md";

/**
 * The floor the pointer count may not fall below.
 *
 * Not a target and not a snapshot of "how many there happen to be": a ratchet.
 * Raising it when pointers are added is optional; LOWERING it is the deliberate
 * act this constant exists to force, because every step down means a claim in
 * the runbook stopped being covered by anything.
 */
// 43 → 47 in D2/PR2: §4 «Camera» went from «there is no live picture anywhere»
// to four covered claims (the live frame and its badge, the two named failure
// states, the hand-over to the recorder, the overlay's polled frames). A
// ratchet that is not raised when pointers are added leaves the new ones
// unprotected — which is the deletion-by-deletion hole rule 2 exists to close.
//
// 47 → 51 in D2/PR3: the control room. The runbook's navigation note now claims
// four things about it — a card edits in place, an old deep link folds the right
// card out, the frame comes back on Innstillinger, and a recording start pulls
// BOTH meters out of the tree — and each of the four is a mutation-proof for a
// guard that would otherwise fail silently.
//
// 51 → 52 in D2/PR4: the prose sweep found §9 telling a rig tester to click a
// tray row — «Sjekk systemet» — that the core deliberately does not build. The
// correction is only worth as much as the thing that keeps it true, so the
// runbook now points at the named Rust test that forbids the row.
//
// 52 → 59 in V1/PR2: the Diagnose screen came back, and with it seven claims
// the runbook could not point at anything for while the surface was missing —
// the five status rows, the code→catalogue translation AND its fallback, the
// clipboard copy, the honest failure, the recording guard on the test capture,
// the tray action that used to dead-end, and the probe's three-state. One of
// those pointers (§«Claims this runbook used to point at a test for») replaces
// a pointer that was DELETED in fase B, which is the shape a ratchet is meant
// to make visible: the claim was uncovered for a whole phase.
const MIN_POINTERS = 59;

// ── 1. Claim lines, and which of them parse ─────────────────────────────────

/**
 * A line that CLAIMS coverage: `VERIFIED-BY` opening the line, allowing a
 * markdown bullet and/or a code-span backtick in front of it. Every real
 * pointer in the runbook is written that way — as its own bullet — while the
 * prose that talks ABOUT the convention mentions it mid-sentence.
 */
const CLAIM_LINE = /^\s*(?:[-*+]\s+)?`?VERIFIED-BY\b/;

/** A well-formed pointer: `VERIFIED-BY: <file>::<name>` to end of line. */
const POINTER = /^\s*(?:[-*+]\s+)?`?VERIFIED-BY:\s*(\S+)::(.+?)`?\s*$/;

/**
 * Split the runbook into the claims it makes and the ones that are broken.
 *
 * Pure, and given the doc as a string, so the self-test can hand it a fixture.
 */
function collectPointers(doc) {
  const pointers = [];
  const malformed = [];
  const lines = doc.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!CLAIM_LINE.test(line)) continue;
    const m = POINTER.exec(line);
    if (!m) {
      malformed.push({ line: i + 1, text: line.trim() });
      continue;
    }
    pointers.push({ file: m[1], name: m[2].trim(), line: i + 1 });
  }
  return { pointers, malformed };
}

// ── 2. Titles a test file actually declares ─────────────────────────────────

/**
 * Every title declared by a `test` / `it` / `describe` call in one file.
 *
 * Deliberately syntactic rather than semantic: the argument has to be a plain
 * double- or single-quoted literal, which every spec and vitest file in this
 * repo writes. A title built from a template or a variable would not be found —
 * and that is the right answer, because a VERIFIED-BY pointer names a title
 * somebody can read in the runbook and then grep for.
 */
function testTitles(src) {
  const titles = new Set();
  const CALL =
    /\b(?:test|it|describe)(?:\.(?:only|skip|todo|fails|concurrent|sequential|each|describe))*\s*\(\s*(["'])((?:[^\\]|\\.)*?)\1/g;
  for (const m of src.matchAll(CALL)) {
    titles.add(m[2].replace(/\\(["'\\])/g, "$1"));
  }
  return titles;
}

/** Does `file` declare a test called `name`? Rust is `fn <name>(`. */
function declares(file, src, name) {
  return file.endsWith(".rs")
    ? new RegExp(
        `fn\\s+${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\(`,
      ).test(src)
    : testTitles(src).has(name);
}

// ── 3. The audit, over an injected file reader ──────────────────────────────

/**
 * Audit one runbook.
 *
 * `read(file)` answers the file's source, or `null` when it does not exist —
 * injected so the self-test never touches the disk and the gate never grows a
 * second way of asking the same question.
 */
function auditDoc(doc, read, minPointers = MIN_POINTERS) {
  const { pointers, malformed } = collectPointers(doc);
  const problems = [];

  for (const { line, text } of malformed) {
    problems.push(
      `${DOC}:${line}\n      ${text}\n      ` +
        "this line claims coverage but is not a `VERIFIED-BY: <file>::<test name>` " +
        "pointer — a malformed pointer used to be skipped in silence, which turned " +
        "a covered claim into no claim at all",
    );
  }

  const cache = new Map();
  for (const { file, name, line } of pointers) {
    if (!cache.has(file)) cache.set(file, read(file));
    const src = cache.get(file);
    if (src === null || src === undefined) {
      problems.push(
        `${DOC}:${line}  ${file} :: ${name}\n      the file does not exist`,
      );
      continue;
    }
    if (!declares(file, src, name)) {
      problems.push(
        `${DOC}:${line}  ${file} :: ${name}\n      no test by that name in the file ` +
          "(renamed, deleted — or only quoted in a comment?)",
      );
    }
  }

  if (pointers.length < minPointers) {
    problems.push(
      `the runbook is down to ${pointers.length} VERIFIED-BY pointer(s), ` +
        `below the ratchet of ${minPointers}\n      ` +
        "a pointer that disappears takes a covered claim with it. If the loss is " +
        "deliberate (the claim itself was retired), lower MIN_POINTERS in " +
        "scripts/check-smoke-verified.mjs in the same commit and say why.",
    );
  }

  const unverified = doc.match(/UNVERIFIED/g)?.length ?? 0;
  return { pointers, malformed, problems, unverified };
}

// ── 4. Self-test: one known failure per class ───────────────────────────────

function selfTest() {
  const spec = [
    "// a title only QUOTED in a comment: 'ghost claim'",
    'test("a real spec title", async () => {})',
    'describe.only("a described group", () => {})',
  ].join("\n");
  const rust = "#[test]\nfn live_probe_is_callable() { }";
  const read = (f) =>
    f === "e2e/x.spec.ts" ? spec : f === "src-tauri/src/y.rs" ? rust : null;

  const fixture = [
    "Prose that mentions VERIFIED-BY mid-sentence must NOT count as a claim.",
    "- VERIFIED-BY: e2e/x.spec.ts::a real spec title",
    "   - VERIFIED-BY: e2e/x.spec.ts::a described group",
    "- VERIFIED-BY: src-tauri/src/y.rs::live_probe_is_callable",
    "- VERIFIED-BY: e2e/x.spec.ts::ghost claim",
    "- VERIFIED-BY: e2e/gone.spec.ts::anything at all",
    "- VERIFIED-BY: e2e/x.spec.ts:a real spec title",
    "- VERIFIED-BY: e2e/x.spec.ts::",
    "Still UNVERIFIED, and one more UNVERIFIED for the count.",
  ].join("\n");

  const { pointers, malformed, problems, unverified } = auditDoc(
    fixture,
    read,
    3,
  );
  const fail = [];
  const want = (cond, why) => {
    if (!cond) fail.push(why);
  };

  want(pointers.length === 5, `parsed ${pointers.length} pointers, wanted 5`);
  // The two broken separators — a single `:` and an empty name — are the whole
  // point: neither parses, and neither may be skipped.
  want(
    malformed.length === 2,
    `flagged ${malformed.length} malformed claim line(s), wanted 2`,
  );
  want(
    malformed.every((m) => /VERIFIED-BY/.test(m.text)),
    "a malformed claim is reported without its own line",
  );
  // Prose is not a claim; if it were, this fixture's first line would be a
  // third malformed entry.
  want(
    !malformed.some((m) => m.text.startsWith("Prose")),
    "a mid-sentence mention of VERIFIED-BY was mistaken for a pointer",
  );
  // 2 malformed + 1 ghost (comment-only title) + 1 missing file = 4.
  want(problems.length === 4, `reported ${problems.length} problems, wanted 4`);
  want(
    problems.some((p) => p.includes("does not exist")),
    "a pointer at a missing file was not caught",
  );
  want(
    problems.some((p) => p.includes("no test by that name")),
    "a title that only appears in a comment was credited as coverage",
  );
  want(unverified === 2, `counted ${unverified} UNVERIFIED, wanted 2`);

  // …and the ratchet itself, with the same fixture held one notch higher.
  const ratcheted = auditDoc(fixture, read, 6);
  want(
    ratcheted.problems.some((p) => p.includes("below the ratchet")),
    "the pointer-count ratchet did not fire when the count fell below it",
  );

  if (fail.length) {
    console.error("check-smoke-verified SELVTEST FEILET:");
    for (const f of fail) console.error("  ✗ " + f);
    process.exit(2);
  }
}

// ── 5. Gate ─────────────────────────────────────────────────────────────────

function main() {
  selfTest();

  const doc = readFileSync(join(root, DOC), "utf8");
  const read = (file) => {
    const abs = join(root, file);
    return existsSync(abs) ? readFileSync(abs, "utf8") : null;
  };

  const { pointers, problems, unverified } = auditDoc(doc, read);

  if (problems.length) {
    console.error(`✗ ${DOC} has ${problems.length} problem(s):`);
    for (const p of problems) console.error(`    ${p}`);
    console.error(
      "  Fix the pointer (or restore the test) — a pointer at nothing, and a " +
        "pointer that does not parse, both claim coverage that does not exist.",
    );
    process.exit(1);
  }

  console.log(
    `✓ smoke runbook: ${pointers.length} VERIFIED-BY pointer(s) all parse and ` +
      `resolve (ratchet ${MIN_POINTERS}) · ${unverified} UNVERIFIED marker(s) remain`,
  );
}

main();
