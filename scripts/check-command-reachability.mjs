// Regression gate for docs/COMMAND_AUDIT_2026-08.md — a command that the audit
// found reachable from the UI must not silently go dark again, and a freshly
// registered command must not sit unclassified (nobody having said "yes, this
// one is meant to be unreachable for now").
//
// WHY THIS EXISTS: the audit's headline number (118 of 178 registered commands
// reachable from the renderer) is a snapshot, not a standing guarantee. Nothing
// stopped a future refactor from deleting the one call site that reached a
// command — the shim would still compile (the fallback/`call()` plumbing
// doesn't care whether anything downstream calls it), so the loss is invisible
// until a feature quietly stops working. This script re-runs the audit's own
// measurement on every `npm run check` and fails on a REGRESSION, not on the
// existing 60-strong backlog of known-unreachable commands (see the audit §4
// for why those are unreached on purpose — pending an owner decision, a
// feature flag, or an SDK).
//
// METHOD — mirrors docs/COMMAND_AUDIT_2026-08.md §1/§7 exactly, because a
// naive `invoke("name")` grep is provably wrong: it missed every call routed
// through the shim's `call()`/`editorCall()` wrappers or through a generic
// type parameter containing its own parens, and undercounted reachable
// commands by 40 (the audit's "78 vs 60" story). The correct method does not
// special-case the wrappers at all — it strips comments, then asks whether the
// command name appears ANYWHERE as a quoted string literal in the renderer
// source. That is deliberately broader than "at an invoke/call/editorCall call
// site": only a couple of files import `invoke` at all, so a string-literal hit
// anywhere in `legacy/`+`src/` (minus generated bindings, locale JSON, and test
// files) is, in practice, always a real call site. Which files those are is not
// assumed here — see `KNOWN_INVOKE_IMPORTERS` below, which fails this gate if
// the set changes.
//
//   node scripts/check-command-reachability.mjs                  check against the baseline
//   node scripts/check-command-reachability.mjs --write-baseline regenerate the baseline from the current tree
//
// FAILS ONLY when:
//   - a command the baseline recorded as REACHABLE is now unreachable (a real
//     regression — something stopped calling it), or
//   - a command was registered since the baseline was generated and is
//     unreachable (an unclassified newcomer — nobody has said whether that is
//     intentional).
// A newly-registered command that IS reachable, or a previously-unreachable
// command that got wired up, both PASS — the second prints a friendly note
// that the baseline is now stale in the boring direction and can be updated.

import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const LIB_RS = join(root, "src-tauri/src/lib.rs");
const BASELINE_PATH = join(root, "scripts/command-reachability-baseline.json");
const WRITE = process.argv.includes("--write-baseline");

// ── 1. Registered commands: the `generate_handler![…]` list ─────────────────

function registeredCommands() {
  const src = readFileSync(LIB_RS, "utf8");
  const block = src.match(/tauri::generate_handler!\[([\s\S]*?)\]/);
  if (!block) {
    throw new Error(`could not find tauri::generate_handler![…] in ${LIB_RS}`);
  }
  // `commands::<area>::<name>` — one per registered command. Comments in this
  // block (there are several, e.g. "// Papirkurv. …") never match this shape,
  // so they fall out for free without a separate strip pass.
  const names = [
    ...block[1].matchAll(/commands::[A-Za-z0-9_]+::([A-Za-z0-9_]+)/g),
  ].map((m) => m[1]);
  const unique = new Set(names);
  if (unique.size !== names.length) {
    throw new Error(
      "duplicate command name in generate_handler![…] — investigate before trusting counts",
    );
  }
  return [...unique].sort();
}

// ── 2. Frontend source: app/ + legacy/ + src/, minus generated/locale/test ──
// (docs/COMMAND_AUDIT_2026-08.md §7, step 2 — src/ is entirely `src/lib/bindings`
// today, which step 3 below excludes anyway, so this reduces in practice to
// `app/` (the shell AND `app/lib/`, where the shim now lives) plus
// legacy/shared + legacy/types.)

// `app` joined the roots the day it was created: «Frivilligen først»'s Preact
// shell is where the call sites were MOVING TO, so a measurement that only
// looked at `legacy/` would report a command as newly unreachable the moment
// its last legacy caller was replaced by an app one — a false alarm — and would
// miss a genuinely dark command whose only caller lived in the new tree.
//
// `legacy` stays a root after PR B carried the inventory into `app/lib/`. What
// is left there is `shared/` and `types/` — no call sites today, but a root
// dropped because it is currently empty of them is a root that is not watched
// when it stops being empty.
const SEARCH_ROOTS = ["legacy", "src", "app"];
const EXCLUDE_DIR_NAMES = new Set(["bindings", "locales", "node_modules"]);
const INCLUDE_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"];

function collectSourceFiles() {
  const files = [];
  const walk = (dir) => {
    for (const ent of readdirSync(dir, { withFileTypes: true })) {
      if (EXCLUDE_DIR_NAMES.has(ent.name)) continue;
      const p = join(dir, ent.name);
      if (ent.isDirectory()) {
        walk(p);
        continue;
      }
      if (!INCLUDE_EXTENSIONS.some((ext) => ent.name.endsWith(ext))) continue;
      if (/\.test\.[jt]sx?$/.test(ent.name)) continue;
      files.push(p);
    }
  };
  for (const r of SEARCH_ROOTS) {
    const dir = join(root, r);
    // `src/` is entirely generated (`/src/lib/bindings/` is gitignored), so on
    // a fresh checkout it does not exist until `npm run bindings` has run. A
    // missing root means "no source there to scan", which is a fact, not a
    // crash — but say so, because a root silently skipped forever would let a
    // future real `src/` file escape the measurement without anyone noticing.
    if (!existsSync(dir)) {
      console.log(
        `  (search root "${r}/" does not exist on this checkout — skipped; ` +
          `it is generated by \`npm run bindings\` and excluded from the scan anyway)`,
      );
      continue;
    }
    walk(dir);
  }
  return files;
}

// ── 2b. The premise this whole measurement rests on ─────────────────────────
//
// A string-literal hit only implies "this command is called" because there is
// no way into the backend EXCEPT the handful of files that import `invoke`. If
// a third file starts calling the backend directly, a name could appear in one
// of them without a matching call — or, worse, a command could be reached from
// a file this script does not even scan.
//
// That premise used to be stated as fact in a comment above, and it was WRONG:
// it named `api-shim.ts` as the only importer while (the since-removed)
// `deeplinks.ts` had been calling `deeplink_confirm_captions` directly since
// the day both landed. The comment was copied into
// `docs/COMMAND_AUDIT_2026-08.md`, so the audit rested on it too. Nothing
// noticed, because nothing checked.
//
// So it is checked now. This is a gate, not a note: if the set changes, the
// measurement's justification has changed with it, and somebody has to re-read
// the audit rather than trust a number computed under an assumption that no
// longer holds.
const KNOWN_INVOKE_IMPORTERS = [
  // The shim every page goes through — `call()`/`editorCall()` wrap `invoke`.
  // It moved from `legacy/renderer/` to `app/lib/` in fase B's PR B; it is the
  // same file and the same single door.
  "app/lib/api-shim.ts",
];

// …and the list itself may never grow an entry from the SHELL. The set-equality
// check below already fails on a new importer, but its remedy is "add the file
// to KNOWN_INVOKE_IMPORTERS" — which for the shell would be the wrong remedy
// dressed as the right one. The shell reaches the backend through `window.api`
// (installed by `@lib/api-shim`) and through nothing else; ESLint's
// no-restricted-imports says the same thing at the call site, and this says it
// where the exemption would have to be written.
//
// «The shell» is `app/` MINUS `app/lib/`, since PR B moved the ported inventory
// — the shim included — inside `app/`. The exclusion is one prefix and not a
// hole: `app/lib/` is covered by the set-equality check below, which is the
// stricter of the two (it fails on ANY change to the set, in either direction),
// and by ESLint's `app/lib/**` block. What is given up here is only the
// friendlier error message, for exactly one file.
const APP_PREFIX = "app/";
const LIB_PREFIX = "app/lib/";
const isShellFile = (f) =>
  f.startsWith(APP_PREFIX) && !f.startsWith(LIB_PREFIX);
function checkNoAppExemptions() {
  const smuggled = KNOWN_INVOKE_IMPORTERS.filter(isShellFile);
  if (smuggled.length === 0) return true;
  console.error(
    "✗ an app/ file was added to KNOWN_INVOKE_IMPORTERS: " +
      smuggled.join(", "),
  );
  console.error(
    "  The new shell has ONE door into the backend — `window.api`, installed by\n" +
      "  @lib/api-shim. A second door is invisible to the fixture seam, to the IPC\n" +
      "  failure ring, and to this measurement. Route the call through the shim\n" +
      "  instead of exempting the file.",
  );
  return false;
}

function checkInvokeImporters(files) {
  const found = files
    .filter((f) => readFileSync(f, "utf8").includes("@tauri-apps/api/core"))
    .map((f) => relative(root, f))
    .sort();
  const fromShell = found.filter(isShellFile);
  if (fromShell.length) {
    console.error(
      `✗ ${fromShell.length} shell file(s) under app/ import @tauri-apps/api/core directly: ` +
        fromShell.join(", "),
    );
    console.error(
      "  The shell talks to the backend through `window.api` only (see @lib/api-shim).\n" +
        "  `app/lib/` is the ported inventory, not the shell — the shim itself lives\n" +
        "  there, and the set-equality check below is what holds it to exactly one file.",
    );
    return false;
  }
  const expected = [...KNOWN_INVOKE_IMPORTERS].sort();
  const added = found.filter((f) => !expected.includes(f));
  const gone = expected.filter((f) => !found.includes(f));
  if (added.length === 0 && gone.length === 0) return true;

  console.error(
    "✗ the set of files that reach the backend directly has changed",
  );
  if (added.length)
    console.error(`  now also importing invoke: ${added.join(", ")}`);
  if (gone.length)
    console.error(`  no longer importing invoke: ${gone.join(", ")}`);
  console.error(
    "  A string-literal hit counts as 'reachable' only because these are the\n" +
      "  only doors into the backend. Update KNOWN_INVOKE_IMPORTERS here AND\n" +
      "  re-check docs/COMMAND_AUDIT_2026-08.md §1, which makes the same claim.",
  );
  return false;
}

// ── 3. Comment-stripping, string/template-literal aware ─────────────────────
//
// A blind `line.replace(/\/\/.*/, "")` corrupts any string containing "//"
// (e.g. `"streaming://stats"`, `"https://…"`), which this codebase has plenty
// of. This is a small hand-rolled scanner, not a JS parser — it tracks
// single/double-quoted strings and template literals (respecting `\`
// escapes) so comment delimiters INSIDE a string are never mistaken for real
// ones. It does not understand regex literals (a `/…\/\/…/` regex containing
// "//" could in principle be mis-tokenized as a comment start); none of the
// files this walks over use one across a command-name string, and getting
// this wrong only makes the script MORE conservative (treats real code as a
// comment → undercounts reachability → a false failure, never a false pass),
// so it is an acceptable, documented gap rather than a silent risk.
function stripComments(src) {
  let out = "";
  let state = "code"; // code | line | block | sq | dq | tpl
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    const c2 = i + 1 < src.length ? src[i + 1] : "";
    switch (state) {
      case "code":
        if (c === "/" && c2 === "/") {
          state = "line";
          i++;
        } else if (c === "/" && c2 === "*") {
          state = "block";
          i++;
        } else if (c === '"' || c === "'" || c === "`") {
          state = { '"': "dq", "'": "sq", "`": "tpl" }[c];
          out += c;
        } else {
          out += c;
        }
        break;
      case "line":
        if (c === "\n") {
          state = "code";
          out += c;
        }
        break;
      case "block":
        if (c === "*" && c2 === "/") {
          state = "code";
          i++;
        } else if (c === "\n") {
          out += c;
        }
        break;
      case "sq":
      case "dq":
      case "tpl": {
        const closer = { sq: "'", dq: '"', tpl: "`" }[state];
        out += c;
        if (c === "\\") {
          out += c2;
          i++;
        } else if (c === closer) {
          state = "code";
        }
        break;
      }
    }
  }
  return out;
}

// ── 4. Reachability: is `name` a quoted string literal anywhere in `source`? ─
// Audit rule (§7, step 4): the name must appear as `"name"`, `'name'` or
// `` `name` `` — the FULL quoted token, not merely a substring of a longer
// string. This is what keeps `settings_get` from being credited by a stray
// `settings_get_all`.
function isReachable(name, source) {
  return (
    source.includes(`"${name}"`) ||
    source.includes(`'${name}'`) ||
    source.includes(`\`${name}\``)
  );
}

// ── run the measurement ───────────────────────────────────────────────────

const commands = registeredCommands();
const files = collectSourceFiles();
const premiseHolds = checkNoAppExemptions() && checkInvokeImporters(files);
const combined = files
  .map((f) => stripComments(readFileSync(f, "utf8")))
  .join("\n");

const reachable = [];
const unreachable = [];
for (const name of commands) {
  (isReachable(name, combined) ? reachable : unreachable).push(name);
}

console.log(
  `command reachability — registered ${commands.length}, reachable ${reachable.length}, unreachable ${unreachable.length}`,
);
console.log(
  `  (scanned ${files.length} files under ${SEARCH_ROOTS.join(", ")}, comments stripped)`,
);

const AUDIT = { registered: 178, reachable: 118, unreachable: 60 };
if (
  commands.length !== AUDIT.registered ||
  reachable.length !== AUDIT.reachable ||
  unreachable.length !== AUDIT.unreachable
) {
  console.log(
    `  ⚠ differs from docs/COMMAND_AUDIT_2026-08.md's ${AUDIT.registered}/${AUDIT.reachable}/${AUDIT.unreachable} ` +
      `— expected: the audit is a snapshot and the tree has moved since. Not a failure by itself.`,
  );
}

// ── write mode ────────────────────────────────────────────────────────────

if (WRITE) {
  const baseline = {
    _comment:
      "Generated by `node scripts/check-command-reachability.mjs --write-baseline`. " +
      "`knownCommands` is every command registered at generation time (reachable or " +
      "not); `unreachable` is the subset that was NOT reachable from the UI then. " +
      "A command leaving `unreachable` (i.e. it got wired up) is fine — no action " +
      "needed, though re-running --write-baseline keeps this file current. The check " +
      "script FAILS when a command NOT in `unreachable` (i.e. it was reachable, or " +
      "did not exist yet at generation time) shows up unreachable now.",
    generatedAt: new Date().toISOString(),
    knownCommands: commands,
    unreachable: unreachable,
  };
  writeFileSync(BASELINE_PATH, JSON.stringify(baseline, null, 2) + "\n");
  console.log(`✓ wrote baseline: ${relative(root, BASELINE_PATH)}`);
  process.exit(0);
}

// ── check mode ────────────────────────────────────────────────────────────

let baseline;
try {
  baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
} catch (e) {
  console.error(
    `✗ could not read baseline at ${relative(root, BASELINE_PATH)}: ${e.message}`,
  );
  console.error(
    "  run `node scripts/check-command-reachability.mjs --write-baseline` to create it.",
  );
  process.exit(1);
}

const knownSet = new Set(baseline.knownCommands ?? []);
const baselineUnreachableSet = new Set(baseline.unreachable ?? []);

const regressions = []; // was reachable (known, not in baseline.unreachable) → now unreachable
const unclassifiedNew = []; // not known at baseline time, and unreachable now
const newlyWired = []; // was in baseline.unreachable → now reachable
const newlyRegisteredReachable = []; // not known at baseline time, reachable now (fine, just informational)

for (const name of unreachable) {
  if (!knownSet.has(name)) {
    unclassifiedNew.push(name);
  } else if (!baselineUnreachableSet.has(name)) {
    regressions.push(name);
  }
  // else: known AND already classified unreachable — expected, silent.
}
for (const name of reachable) {
  if (!knownSet.has(name)) {
    newlyRegisteredReachable.push(name);
  } else if (baselineUnreachableSet.has(name)) {
    newlyWired.push(name);
  }
}

const removed = [...knownSet].filter((n) => !commands.includes(n));

let failed = false;

if (regressions.length > 0) {
  failed = true;
  console.error(
    `\n✗ ${regressions.length} command(s) that were reachable are now unreachable:`,
  );
  for (const n of regressions.sort()) console.error(`    ${n}`);
  console.error(
    "  Something stopped calling this command. If the removal was intentional,",
  );
  console.error(
    "  regenerate the baseline: node scripts/check-command-reachability.mjs --write-baseline",
  );
}

if (unclassifiedNew.length > 0) {
  failed = true;
  console.error(
    `\n✗ ${unclassifiedNew.length} newly-registered command(s) are unreachable and unclassified:`,
  );
  for (const n of unclassifiedNew.sort()) console.error(`    ${n}`);
  console.error(
    "  A brand-new command with no UI path needs an explicit decision (see the audit's",
  );
  console.error(
    "  §4 keep/wire/cut framing), not silence. Wire it up, or accept it as intentionally",
  );
  console.error("  unreachable for now by regenerating the baseline:");
  console.error(
    "    node scripts/check-command-reachability.mjs --write-baseline",
  );
}

if (newlyWired.length > 0) {
  console.log(
    `\nℹ ${newlyWired.length} command(s) moved from unreachable → reachable since the baseline:`,
  );
  for (const n of newlyWired.sort()) console.log(`    ${n}`);
  console.log(
    "  Nice — no action required. The baseline can be refreshed to reflect it:",
  );
  console.log(
    "    node scripts/check-command-reachability.mjs --write-baseline",
  );
}

if (newlyRegisteredReachable.length > 0) {
  console.log(
    `\nℹ ${newlyRegisteredReachable.length} newly-registered command(s) are already reachable — fine, no classification needed:`,
  );
  for (const n of newlyRegisteredReachable.sort()) console.log(`    ${n}`);
}

if (removed.length > 0) {
  console.log(
    `\nℹ ${removed.length} command(s) in the baseline no longer exist in lib.rs (removed) — harmless.`,
  );
}

if (!failed && premiseHolds) {
  console.log("\n✓ no reachability regressions, no unclassified new commands");
}

process.exit(failed || !premiseHolds ? 1 : 0);
