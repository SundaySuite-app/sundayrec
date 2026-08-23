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
const RENDERER_ROOT = HERE;
/**
 * «Frivilligen først»'s Preact shell. It is covered from the day it exists, not
 * added later: the pin is a CLASS-level promise ("no renderer code may touch
 * the old key again"), and a class-level promise that quietly excludes the tree
 * where all the new code is being written is not one.
 */
const APP_ROOT = join(REPO_ROOT, "app");
const SOURCE_ROOTS = [RENDERER_ROOT, APP_ROOT];

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

/** Every .ts/.tsx source under either shell (tests excluded — they assert ON
 *  the key; the guard is about production code). */
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

/** Repo-relative, so an offender in either shell is named unambiguously. */
const rel = (p: string): string => relative(REPO_ROOT, p);

/** Files allowed to name the legacy key: the migration, and nothing else. */
const ALLOWLIST = new Set([
  "legacy/renderer/migrate-legacy-settings-core.ts",
]);

describe("settings store pin — sqlite is the ONE store", () => {
  it("covers BOTH shells — a pin that scans an empty set proves nothing", () => {
    // The one way this whole file could go silently vacuous: the app tree stops
    // being walked (renamed, moved, filter tightened) and every assertion below
    // starts passing for the wrong reason.
    const scanned = rendererSources().map(rel);
    expect(scanned.some((p) => p.startsWith("legacy/renderer/"))).toBe(true);
    expect(scanned.some((p) => p.startsWith("app/"))).toBe(true);
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
    const source = codeOf(join(RENDERER_ROOT, "api-shim.ts"));
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
    const source = codeOf(join(RENDERER_ROOT, "api-shim.ts"));
    expect(source.includes("backendRecordingSettings")).toBe(false);
  });
});
