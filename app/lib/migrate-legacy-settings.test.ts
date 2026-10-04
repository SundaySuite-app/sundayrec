// The impure half of the one-shot localStorage hand-over: what it does with the
// answer of `settings_import`. The mapper is `migrate-legacy-settings-core.test.ts`.
//
// Rust counts the hand-over (`legacy_import_done`), so a page can be told «it
// has already happened». That must not leave the blob behind to be retried at
// every boot — a retry could never succeed — and it must not be mistaken for a
// hand-over that just landed (no reschedule).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { migrateLegacySettingsOnce } from "./migrate-legacy-settings";
import {
  LEGACY_MIGRATED_FLAG,
  LEGACY_SETTINGS_KEY,
} from "./migrate-legacy-settings-core";

/** A `Storage` of the three methods the migration uses. */
function fakeStorage(initial: Record<string, string>) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    removeItem: (k: string) => void data.delete(k),
  };
}

const BLOB = JSON.stringify({ churchName: "Domkirken" });

describe("migrateLegacySettingsOnce — the answer of settings_import", () => {
  let storage: ReturnType<typeof fakeStorage>;
  beforeEach(() => {
    storage = fakeStorage({ [LEGACY_SETTINGS_KEY]: BLOB });
    vi.stubGlobal("localStorage", storage);
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("hands the blob over, reschedules, removes the key and sets the flag", async () => {
    const invoke = vi.fn(async () => ({}));
    await migrateLegacySettingsOnce({
      invoke: invoke as never,
      onCorruptBlob: () => {},
    });
    expect(invoke.mock.calls.map((c) => (c as unknown[])[0])).toEqual([
      "settings_import",
      "scheduler_reschedule",
    ]);
    expect(storage.data.has(LEGACY_SETTINGS_KEY)).toBe(false);
    expect(storage.data.get(LEGACY_MIGRATED_FLAG)).toBe("1");
  });

  it("treats «settings_import_done» as done: key removed, flag set, nothing rescheduled", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "settings_import")
        throw {
          code: "validation",
          message:
            "validation: settings_import_done: the settings hand-over from the old installation has already happened",
        };
      return {};
    });
    const onCorruptBlob = vi.fn();
    await migrateLegacySettingsOnce({ invoke: invoke as never, onCorruptBlob });
    expect(invoke.mock.calls.map((c) => (c as unknown[])[0])).toEqual([
      "settings_import",
    ]);
    expect(storage.data.has(LEGACY_SETTINGS_KEY)).toBe(false);
    expect(storage.data.get(LEGACY_MIGRATED_FLAG)).toBe("1");
    expect(onCorruptBlob).not.toHaveBeenCalled();
  });

  it("keeps the blob for the next boot when the import fails for any other reason", async () => {
    const invoke = vi.fn(async () => {
      throw new Error("database error: database is locked");
    });
    await migrateLegacySettingsOnce({
      invoke: invoke as never,
      onCorruptBlob: () => {},
    });
    expect(storage.data.get(LEGACY_SETTINGS_KEY)).toBe(BLOB);
    expect(storage.data.has(LEGACY_MIGRATED_FLAG)).toBe(false);
  });
});
