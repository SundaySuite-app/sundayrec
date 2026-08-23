/**
 * Kjernens mastring-preset-id-er, lest ut av Rust-fila.
 *
 * `speech-clear` og `music-speech` er STRENGER som krysser IPC. På den andre
 * siden slås de opp med `get_preset_by_id`, og et navn som ikke finnes der blir
 * `unknown_preset` og en eksport som stopper — ikke en typefeil, ikke en
 * kompileringsfeil, ingenting før en frivillig trykker Eksporter.
 *
 * Ingen bindings dekker dette: `EditorMasterPreset` er en struct med et
 * `id: String`, og ts-rs sier ingenting om hvilke id-er som finnes. Så
 * TypeScript-siden leser den ene sannheten der den bor. Bytter noen navnet i
 * Rust, går `sound-profiles.test.ts` rød i det samme commitet.
 *
 * Bare for tester. Fila kjøres aldri i appen — den leser fra disk.
 */

import fs from "node:fs";
import path from "node:path";

const MASTERING_RS = path.resolve(
  import.meta.dirname,
  "../../../crates/sundayrec-core/src/mastering.rs",
);

/** Hver `id: "…".into()` inne i `master_presets()`, i rekkefølge. */
export function masterPresetIds(): string[] {
  const src = fs.readFileSync(MASTERING_RS, "utf8");
  const body = src.slice(src.indexOf("pub fn master_presets()"));
  const end = body.indexOf("\npub fn ", 1);
  const table = end > 0 ? body.slice(0, end) : body;
  return [...table.matchAll(/\bid:\s*"([^"]+)"\.into\(\)/g)].map((m) => m[1]);
}
