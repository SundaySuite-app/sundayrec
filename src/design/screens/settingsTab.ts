/**
 * Deep-link hand-off for the settings sub-tab (WS-6).
 *
 * When a navigation targets the `settings` view with a specific tab (e.g. the
 * Home format card's "Endre" → `"filer"`), `MainLayout` stashes the tab here
 * before switching views. The mounting `SettingsScreen` reads it once to open
 * that tab instead of the default `lydkilde`. Kept in its own module (not
 * `SettingsScreen.tsx`) so the screen file only exports its component and React
 * Fast Refresh stays happy.
 */

/** The seven settings tab ids, in display order. The source of truth for both
 *  the tab bar and the deep-link validation. */
export const SETTINGS_TAB_IDS = [
  "lydkilde",
  "video",
  "filer",
  "publisering",
  "varsler",
  "system",
  "suite",
] as const;

export type SettingsTabId = (typeof SETTINGS_TAB_IDS)[number];

/** True when `id` is a real settings tab id. */
export function isSettingsTabId(id: unknown): id is SettingsTabId {
  return (
    typeof id === "string" &&
    (SETTINGS_TAB_IDS as readonly string[]).includes(id)
  );
}

let pendingSettingsTab: SettingsTabId | null = null;

/** Record a deep-link target tab so a mounting `SettingsScreen` opens it.
 *  Ignores anything that isn't a real tab id. */
export function setPendingSettingsTab(tab: string): void {
  if (isSettingsTabId(tab)) pendingSettingsTab = tab;
}

/** Read AND clear the pending tab (consumed once, on mount). */
export function consumePendingSettingsTab(): SettingsTabId | null {
  const tab = pendingSettingsTab;
  pendingSettingsTab = null;
  return tab;
}

/**
 * Deep-link target *anchor* — a specific setting within a tab (e.g. the Home
 * audio card sends `"device"`). The mounting `SettingsScreen` consumes it,
 * scrolls the matching `[data-sr-anchor]` element to the centre of the window
 * and flashes it for ~1.3 s so the user sees exactly what to change.
 */
let pendingSettingsAnchor: string | null = null;

/** Record a deep-link target anchor (alongside its tab). Empty → cleared. */
export function setPendingSettingsAnchor(anchor: string | null): void {
  pendingSettingsAnchor = anchor && anchor.length > 0 ? anchor : null;
}

/** Read AND clear the pending anchor (consumed once, on mount). */
export function consumePendingSettingsAnchor(): string | null {
  const anchor = pendingSettingsAnchor;
  pendingSettingsAnchor = null;
  return anchor;
}
