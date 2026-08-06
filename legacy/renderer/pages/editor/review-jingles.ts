/**
 * The intro/outro dropdowns in review mode: value → stored path, and back.
 *
 * Small, but the two directions were previously written three times between
 * `loadAndUpdateReviewBanner` and the two `change` handlers, and the third copy
 * (the picker-cancelled restore) had it wrong: it read `path ? 'custom' :
 * 'default'`, so cancelling out of the file picker on an episode with NO jingle
 * moved the control to «Standard» — a choice the user never made and, once the
 * pushes became real, one that would have been saved.
 *
 * Pure: no DOM, no IPC, no settings singleton. The picker and the invoke stay
 * at the call site.
 */

/** The three values the `<select>` carries. */
export type JingleSelect = 'default' | 'none' | 'custom'

/** Ask the caller to open a file picker; anything else is a path (or `null`). */
export const PICK = 'pick' as const

/**
 * What a dropdown value means for the path we store.
 *
 *   'none'    → `null`, the explicit "no jingle" (NOT "unchanged")
 *   'default' → the configured default, or `null` when none is configured
 *   'custom'  → [`PICK`]: the caller must ask the user for a file first
 */
export function jinglePathFor(
  value: string,
  defaultPath: string | null | undefined,
): string | null | typeof PICK {
  if (value === 'none') return null
  if (value === 'default') return defaultPath ?? null
  return PICK
}

/**
 * Which dropdown value reflects a stored path — the inverse, used both to paint
 * the banner and to put the control back after a cancelled picker or a push
 * that did not land. A path equal to the configured default reads as
 * «Standard», so changing the default later moves the control with it.
 */
export function jingleValueFor(
  path: string | null | undefined,
  defaultPath: string | null | undefined,
): JingleSelect {
  if (path == null || path === '') return 'none'
  return path === defaultPath ? 'default' : 'custom'
}
