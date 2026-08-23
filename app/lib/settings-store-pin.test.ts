// The R4 source-pin: localStorage is DEAD as a settings store.
//
// sqlite (via `settings_get`/`settings_save`) is the single source of truth.
// The failure class this pins shut is the dual-store split itself — renderer
// wrote localStorage, backend read sqlite, and the curated bridge between them
// produced nine silent re-defaulting bugs in two days (#113/#115 and R3's
// reader-by-reader sweep). Every guard those rounds added was instance-level;
// THIS is the class-level end: no renderer code may touch the old key again.
//
// Source-parsed in the read-only-guard house style (tuning-report/display
// ratchet), because the property is about what the code SAYS, not what one
// runtime path happens to do.
//
// The single, explicitly-allowlisted exception: the one-shot migration
// (`migrate-legacy-settings-core.ts` names the key; api-shim's
// `migrateLegacySettingsOnce` reads/removes it via that constant). A future
// release deletes the migration and shrinks the allowlist to nothing.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");
/** The ported inventory — this file's own directory, `app/lib/`. */
const LIB_ROOT = HERE;
/**
 * «Frivilligen først»'s Preact shell. It is covered from the day it exists, not
 * added later: the pin is a CLASS-level promise ("no renderer code may touch
 * the old key again"), and a class-level promise that quietly excludes the tree
 * where all the new code is being written is not one.
 *
 * ONE root since fase B PR B moved the inventory to `app/lib/`: `app/` now
 * contains both trees, and listing them separately would walk half of it twice.
 * The vacuity test below still checks that BOTH halves are in the scan, which
 * is the property that mattered — a single root that silently stopped
 * containing the port would look exactly like green.
 */
const APP_ROOT = join(REPO_ROOT, "app");
const SOURCE_ROOTS = [APP_ROOT];

/** The retired key, assembled so THIS file doesn't trip its own guard. */
const LEGACY_KEY = ["sundayrec", "settings"].join(".");

/**
 * Source with comments removed — the pin is about CODE. A comment saying "the
 * localStorage blob died here" is documentation of the change, not a
 * violation of it. (Line/block comments only; good enough for this tree —
 * none of these sources builds comment markers inside string literals.)
 */
function codeOf(path: string): string {
  return readFileSync(path, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "")
    .replace(/ \/\/[^"'`\n]*$/gm, "");
}

/** Every .ts/.tsx source under `app/` — shell and ported inventory alike
 *  (tests excluded — they assert ON the key; the guard is about production
 *  code). */
function rendererSources(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.tsx?$/.test(p) && !/\.test\.tsx?$/.test(p)) out.push(p);
    }
  };
  for (const root of SOURCE_ROOTS) if (existsSync(root)) walk(root);
  return out;
}

/** Repo-relative, so an offender in either tree is named unambiguously. */
const rel = (p: string): string => relative(REPO_ROOT, p);

/** Files allowed to name the legacy key: the migration, and nothing else. */
const ALLOWLIST = new Set(["app/lib/migrate-legacy-settings-core.ts"]);

describe("settings store pin — sqlite is the ONE store", () => {
  it("covers BOTH the shell and the ported inventory — a pin that scans an empty set proves nothing", () => {
    // The one way this whole file could go silently vacuous: one of the two
    // trees stops being walked (renamed, moved, filter tightened) and every
    // assertion below starts passing for the wrong reason. Both halves are
    // named, not just their common root, precisely because they now share one.
    const scanned = rendererSources().map(rel);
    expect(scanned.some((p) => p.startsWith("app/lib/"))).toBe(true);
    expect(
      scanned.some((p) => p.startsWith("app/") && !p.startsWith("app/lib/")),
    ).toBe(true);
    expect(scanned.some((p) => p.endsWith(".tsx"))).toBe(true);
  });

  it("no renderer source names the legacy localStorage key (migration excepted)", () => {
    const offenders = rendererSources()
      .filter((p) => !ALLOWLIST.has(rel(p)))
      .filter((p) => codeOf(p).includes(LEGACY_KEY))
      .map(rel);
    expect(
      offenders,
      `these files reference "${LEGACY_KEY}" — the localStorage settings store is dead; ` +
        "read settings_get / write settings_save instead (the dual store is the #113 bug class)",
    ).toEqual([]);
  });

  it("api-shim itself touches no localStorage at all", () => {
    // The shim is where the old store lived. After R4 its only localStorage
    // client is the migration MODULE (separate file, allowlisted above) — the
    // shim calling localStorage again would be the split growing back at its
    // root. (Pages may use localStorage for pure UI prefs — remembered tab,
    // preview size — that is not settings state and not covered by this pin.)
    const source = codeOf(join(LIB_ROOT, "api-shim.ts"));
    expect(
      source.includes("localStorage"),
      "api-shim.ts references localStorage — settings live in sqlite; if this is a new UI pref, put it in its own module",
    ).toBe(false);
  });

  it("the old vocabulary lives ONLY in the migration mapper", () => {
    // The retired settings-field names, as bare quoted strings. R4's promise is
    // "no living compat code elsewhere" — the rename table exists in ONE place
    // (mapLegacyBlob), so a stale `'videoSeparate'` in a page is either dead
    // metadata or a bug about to happen. (The chat webhook's `webhookOnWarn`
    // left the vocabulary entirely with the sharing cluster.)
    const OLD_NAMES = ["videoSeparate", "videoKeepAudio"];
    const patterns = OLD_NAMES.flatMap((n) => [`'${n}'`, `"${n}"`, `\`${n}\``]);
    const offenders = rendererSources()
      .filter((p) => !ALLOWLIST.has(rel(p)))
      .filter((p) => {
        const code = codeOf(p);
        return patterns.some((pat) => code.includes(pat));
      })
      .map(rel);
    expect(
      offenders,
      "these files use a retired settings-field name — the unified vocabulary is " +
        "outputMode / keepSeparateAudio; renames live only in mapLegacyBlob",
    ).toEqual([]);
  });

  it("the curated bridge stays dead", () => {
    // `backendRecordingSettings` was the curated-subset bridge — 180 lines of
    // per-field archaeology that existed to compensate for the dual store.
    // Reintroducing anything by that name would mean the split is back.
    const source = codeOf(join(LIB_ROOT, "api-shim.ts"));
    expect(source.includes("backendRecordingSettings")).toBe(false);
  });
});
