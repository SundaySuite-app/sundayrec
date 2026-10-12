import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AUTO_UPDATE_INTERVAL_MS } from "@lib/pages/auto-update-schedule-core";

import { initAutoUpdate, updatePhase } from "./auto-update";
import { hydrateError, settings } from "./settings";

let dispose: (() => void) | null = null;
let checkForUpdates: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  checkForUpdates = vi.fn(() => Promise.resolve({ available: false }));
  (globalThis as unknown as { window: unknown }).window = {
    api: { checkForUpdates, on: () => () => {} },
  };
});

afterEach(() => {
  dispose?.();
  dispose = null;
  updatePhase.value = { kind: "idle" };
  hydrateError.value = null;
  settings.value = { ...SETTINGS_DEFAULTS };
  delete (globalThis as unknown as { window?: unknown }).window;
  vi.useRealTimers();
});

describe("oppdateringstimeren", () => {
  it("armeres ikke mens innstillingene ikke kunne leses (standardverdien «på» er ikke eierens valg)", () => {
    settings.value = { ...SETTINGS_DEFAULTS, autoUpdate: true };
    hydrateError.value = "settingsLoadFailed";
    dispose = initAutoUpdate();
    vi.advanceTimersByTime(AUTO_UPDATE_INTERVAL_MS * 3);
    expect(checkForUpdates).not.toHaveBeenCalled();

    // «Prøv igjen» lyktes: nå er signalet det som står lagret, og timeren armeres.
    hydrateError.value = null;
    expect(checkForUpdates).toHaveBeenCalledTimes(1);
    expect(checkForUpdates).toHaveBeenCalledWith({ background: true });
  });

  it("hopper over timesjekken mens en nedlasting pågår eller venter på omstart", () => {
    dispose = initAutoUpdate();
    expect(checkForUpdates).toHaveBeenCalledTimes(1);

    updatePhase.value = { kind: "downloading", percent: 40 };
    vi.advanceTimersByTime(AUTO_UPDATE_INTERVAL_MS);
    updatePhase.value = { kind: "ready", version: "9.9.9" };
    vi.advanceTimersByTime(AUTO_UPDATE_INTERVAL_MS);
    expect(checkForUpdates).toHaveBeenCalledTimes(1);
    expect(updatePhase.value.kind).toBe("ready");

    updatePhase.value = { kind: "upToDate" };
    vi.advanceTimersByTime(AUTO_UPDATE_INTERVAL_MS);
    expect(checkForUpdates).toHaveBeenCalledTimes(2);
  });
});
