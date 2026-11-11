/**
 * Hurtigtastene i Redigering, formulert for verktøytips. Tabellen som
 * AVGJØR hva tastene gjør er `@lib/shortcuts-core.ts`; denne fila sier bare
 * hvordan de skrives på maskinen brukeren sitter ved (⌘Z på Mac, Ctrl+Z
 * ellers). `aria-keyshortcuts` nevner begge, så skjermleseren har rett på
 * begge plattformer uten å vite hvilken den er på.
 */

import { t, tf } from "../i18n";
import { currentOs } from "../state/platform-core";

export const PLAY_KEYS_ARIA = "Space";
export const UNDO_KEYS_ARIA = "Meta+Z Control+Z";
export const REDO_KEYS_ARIA = "Shift+Meta+Z Control+Shift+Z Control+Y";

export function playShortcutHint(): string {
  return tf("app.editor.shortcutHint", { keys: t("app.editor.keySpace") });
}

export function undoShortcutHint(): string {
  return tf("app.editor.shortcutHint", {
    keys: currentOs() === "mac" ? "⌘Z" : "Ctrl+Z",
  });
}

export function redoShortcutHint(): string {
  return tf("app.editor.shortcutHint", {
    keys: currentOs() === "mac" ? "⇧⌘Z" : "Ctrl+Y",
  });
}
