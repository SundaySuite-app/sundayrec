/**
 * F2-9: telling "not there any more" apart from "could not be read", split out
 * of `loader.ts` so the rule is testable without mocking `window.api`/IPC/DOM
 * at all — the whole point of a predicate this small getting its own file.
 */

import { errorCode } from "@lib/error-code-core";

/**
 * Does an IPC failure's message mean the file is GONE — moved to the trash,
 * or removed by hand — rather than present but unreadable (wrong format,
 * genuine corruption)?
 *
 * `checked_input_file` (Rust `path_guard.rs`) runs first in every editor IPC
 * command, and a moved/trashed path fails it with
 * `validation: cannot resolve path <raw>: <os error>` — prose folded in from
 * `canonicalize()`'s own error, not a stable snake_case code. `load_recording`
 * carries its own defense-in-depth existence check too (a narrower race
 * between the guard and the probe), and THAT one does lead with a stable
 * code, `file_not_found`. Same two-step match `exportErrorKey` in
 * `export-core.ts` already uses for the sibling problem on the export path:
 * the stable leading code first, a literal substring second.
 */
export function isMissingFileFailure(message: string): boolean {
  return (
    errorCode(message) === "file_not_found" ||
    message.includes("cannot resolve path")
  );
}
