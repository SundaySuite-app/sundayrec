/**
 * The ONE-SHOT localStorage → sqlite settings migration, impure half (R4).
 *
 * The field-by-field vocabulary mapper and the full reasoning live in
 * `migrate-legacy-settings-core.ts`; this module owns the storage side
 * effects. It is the ONLY renderer code allowed to touch the legacy key —
 * `settings-store-pin.test.ts` holds that boundary.
 *
 * Flag/key discipline:
 *   - success (imported, or nothing worth importing): the legacy key is
 *     REMOVED and the flag set, so the blob can never shadow sqlite again;
 *   - a corrupt blob: mapped to `null` → nothing imported, key still removed
 *     (retrying a blob that cannot parse would fail forever), flag set, and
 *     `onCorruptBlob` fires once so the operator hears the app started from
 *     defaults;
 *   - a FAILED import (backend/db down): key and flag are left untouched, so
 *     the next boot retries with the blob intact.
 *
 * `invoke` is injected by api-shim so the calls ride the E5.1 fixture seam —
 * the migration e2e spec drives this exact code path with a fixtured backend.
 */

import {
  LEGACY_MIGRATED_FLAG,
  LEGACY_SETTINGS_KEY,
  mapLegacyBlob,
} from "./migrate-legacy-settings-core";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export async function migrateLegacySettingsOnce(deps: {
  invoke: InvokeFn;
  /** Called (at most once) when the blob existed but could not be read. */
  onCorruptBlob: () => void;
}): Promise<void> {
  try {
    if (localStorage.getItem(LEGACY_MIGRATED_FLAG)) return;
    const raw = localStorage.getItem(LEGACY_SETTINGS_KEY);
    if (raw !== null) {
      const mapped = mapLegacyBlob(raw);
      if (mapped) {
        await deps.invoke("settings_import", { json: JSON.stringify(mapped) });
        // The imported slots/specials must reach the scheduler now, not at
        // the next save. Best-effort — the supervisor also reads at startup.
        void deps.invoke("scheduler_reschedule").catch(() => {});
      } else {
        deps.onCorruptBlob();
      }
      localStorage.removeItem(LEGACY_SETTINGS_KEY);
    }
    localStorage.setItem(LEGACY_MIGRATED_FLAG, "1");
  } catch (e) {
    // Import failed (backend down / db locked): keep the blob and retry on
    // the next boot rather than half-migrating.
    console.warn("[migrate-legacy-settings] failed — retrying next boot", e);
  }
}
