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
 *     the next boot retries with the blob intact;
 *   - `settings_import_done`: Rust counts the hand-over, not this page (the
 *     localStorage flag is only a courtesy a compromised page can ignore), and
 *     says the one hand-over has already happened. A retry could never
 *     succeed, so the blob is removed and the flag set, with no reschedule.
 *     Normally nothing is lost by that — it happened. But when an EARLIER boot's
 *     import failed ({@link RETRY_MARK} set), Rust closed the hand-over at that
 *     boot's first `settings_get` without it — the blob never arrived, so
 *     `onCorruptBlob` fires: the operator hears it instead of losing the old
 *     settings silently.
 *
 * `invoke` is injected by api-shim so the calls ride the E5.1 fixture seam —
 * the migration e2e spec drives this exact code path with a fixtured backend.
 */

import { errorCode } from "./error-code-core";
import {
  LEGACY_MIGRATED_FLAG,
  LEGACY_SETTINGS_KEY,
  mapLegacyBlob,
} from "./migrate-legacy-settings-core";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** Set when an import failed and the blob was kept for the next boot. */
export const RETRY_MARK = "sundayrec.legacySettingsRetry";

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
        let handedOver = true;
        try {
          await deps.invoke("settings_import", {
            json: JSON.stringify(mapped),
          });
        } catch (e) {
          if (errorCode(e) !== "settings_import_done") throw e;
          handedOver = false;
          if (localStorage.getItem(RETRY_MARK)) deps.onCorruptBlob();
        }
        // The imported slots/specials must reach the scheduler now, not at
        // the next save. Best-effort — the supervisor also reads at startup.
        if (handedOver)
          void deps.invoke("scheduler_reschedule").catch(() => {});
      } else {
        deps.onCorruptBlob();
      }
      localStorage.removeItem(LEGACY_SETTINGS_KEY);
    }
    localStorage.setItem(LEGACY_MIGRATED_FLAG, "1");
    localStorage.removeItem(RETRY_MARK);
  } catch (e) {
    // Import failed (backend down / db locked): keep the blob and retry on
    // the next boot rather than half-migrating.
    console.warn("[migrate-legacy-settings] failed — retrying next boot", e);
    try {
      localStorage.setItem(RETRY_MARK, "1");
    } catch {
      // Storage itself is failing; the next boot retries all the same.
    }
  }
}
