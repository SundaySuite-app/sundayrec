/**
 * Record → edit hand-off for a finished recording.
 *
 * When a recording stops cleanly the backend emits `recording://finished`; the
 * Toast host offers an "Åpne i redigering" action. Clicking it stashes the
 * finished file's path here and navigates to the `editor` view. The mounting
 * (or already-mounted) `EditScreen` reads it once and auto-loads the file
 * through the same path as a manual file-open.
 *
 * Kept in its own module (mirroring {@link ./settingsTab}) so the screen file
 * only exports its component and React Fast Refresh stays happy. A live event
 * ({@link EDITOR_OPEN_FILE_EVENT}) lets an already-mounted editor react when a
 * second recording finishes while the editor is on screen.
 */

/** Fired on `window` when a file should be opened in the editor *now* (the
 *  editor is already mounted). The `detail` is the absolute file path. */
export const EDITOR_OPEN_FILE_EVENT = "editor:open-file";

let pendingEditorFile: string | null = null;

/** Record a file path so a mounting `EditScreen` opens it. Ignores empties. */
export function setPendingEditorFile(path: string): void {
  if (typeof path === "string" && path.length > 0) pendingEditorFile = path;
}

/** Read AND clear the pending file (consumed once, on mount). */
export function consumePendingEditorFile(): string | null {
  const path = pendingEditorFile;
  pendingEditorFile = null;
  return path;
}
