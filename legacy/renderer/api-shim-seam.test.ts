// The renderer→sqlite settings seam, held key-by-key against the OTHER side.
//
// e2e/settings-seam.spec.ts proves the renderer SENDS the curated keys. This
// test closes the half that spec cannot see: Rust's `settings_save`
// deserialises with `#[serde(default)]` and IGNORES unknown keys, so a curated
// key whose name does not exactly match a `Settings` field is not an error
// anywhere — the renderer happily sends it, Rust silently drops it, and the
// field re-defaults on every save. That is the #113 failure shape with a typo
// as the cause instead of an omission.
//
// The two vocabularies compared here are both SOURCE artifacts in this repo:
//
//   - the object literal `backendRecordingSettings` returns (api-shim.ts) —
//     the exact keys that cross the seam;
//   - the ts-rs-generated `Settings` binding (legacy/bindings/Settings.ts) —
//     the serde-visible camelCase field names of the Rust struct, regenerated
//     from `crates/sundayrec-core/src/settings.rs` by `npm run bindings:check`
//     (so drift between the binding and the struct is already fatal in CI).
//
// Parsed from source in the display_ratchet house style, because the function
// is deliberately not exported and the binding is types-only (erased at
// runtime) — source is the only place both vocabularies exist.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

/** The top-level keys of the object literal `backendRecordingSettings` returns. */
function curatedKeys(): string[] {
  const source = readFileSync(join(HERE, "api-shim.ts"), "utf8");
  const fnStart = source.indexOf("function backendRecordingSettings");
  expect(fnStart, "backendRecordingSettings not found in api-shim.ts").toBeGreaterThan(-1);
  const retStart = source.indexOf("return {", fnStart);
  expect(retStart, "its return literal not found").toBeGreaterThan(-1);
  // The literal closes at the function's 2-space-indented `};`.
  const retEnd = source.indexOf("\n  };", retStart);
  expect(retEnd, "its closing `};` not found").toBeGreaterThan(-1);
  const body = source.slice(retStart, retEnd);
  // Top-level keys sit at exactly 4-space indent; ternary continuations and
  // nested call arguments are deeper. `? (…` / `: (…` branch lines never match
  // the identifier-colon shape at that indent.
  const keys = [...body.matchAll(/^ {4}([A-Za-z0-9_]+):/gm)].map((m) => m[1]);
  expect(keys.length, "parsed suspiciously few curated keys").toBeGreaterThan(30);
  return keys;
}

/** The field names of the generated `Settings` binding. */
function settingsFields(): Set<string> {
  const source = readFileSync(join(HERE, "..", "bindings", "Settings.ts"), "utf8");
  const typeStart = source.indexOf("export type Settings = {");
  expect(typeStart, "Settings type not found in binding").toBeGreaterThan(-1);
  // ts-rs emits each field at column 0 inside the braces: `name: type, `.
  const fields = [...source.slice(typeStart).matchAll(/^([A-Za-z0-9_]+)\??:/gm)].map(
    (m) => m[1],
  );
  expect(fields.length, "parsed suspiciously few Settings fields").toBeGreaterThan(60);
  return new Set(fields);
}

describe("backendRecordingSettings ↔ Settings binding", () => {
  it("every curated key names a real Settings field — an unknown key is silently dropped by serde", () => {
    const fields = settingsFields();
    const unknown = curatedKeys().filter((k) => !fields.has(k));
    expect(
      unknown,
      "these curated keys match no Settings field, so Rust drops them and the " +
        "field re-defaults on every save (the #113 shape, caused by a typo or " +
        "a renamed/removed Rust field)",
    ).toEqual([]);
  });

  it("the five #113-generalisation keys are in the curated set", () => {
    // Presence pinned here as well as in the e2e spec, so removing one fails
    // the fast unit gate too, with this file's rationale attached.
    const keys = curatedKeys();
    for (const k of [
      "updateChannel",
      "reminderMinutes",
      "localAdaptivity",
      "autoUpdate",
      "autoDeleteDays",
      "showLiveLevels",
      "inputVolume",
      "trimSilence",
      "launchAtLogin",
    ]) {
      expect(keys, `curated set lost ${k}`).toContain(k);
    }
  });
});
