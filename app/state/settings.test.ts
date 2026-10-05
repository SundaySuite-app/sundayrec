import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelSavePending,
  hydrateError,
  hydrateSettings,
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "./settings";

/** Et minimalt `window.api`: lesingen svarer som bestilt, skrivingen telles. */
function withApi(getSettings: () => Promise<unknown>) {
  const saveSettings = vi.fn(() => Promise.resolve(true));
  (globalThis as unknown as { window: unknown }).window = {
    api: { getSettings, saveSettings },
  };
  return saveSettings;
}

afterEach(() => {
  cancelSavePending();
  hydrateError.value = null;
  settings.value = { ...SETTINGS_DEFAULTS };
  delete (globalThis as unknown as { window?: unknown }).window;
  vi.restoreAllMocks();
});

describe("innstillinger er skrivebeskyttet mens lesingen står som feilet", () => {
  it("en feilet hydrering → lagring kalles ALDRI, og svaret er false", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const saveSettings = withApi(() => Promise.reject(new Error("db locked")));
    await hydrateSettings();
    expect(hydrateError.value).toBe("settingsLoadFailed");

    // Brukeren flipper en bryter: i minnet er det standardverdier + endringen.
    patchSettings({ churchName: "Domkirken" });
    expect(await saveSettingsDebounced(0)).toBe(false);
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("en skriving som alt var armert FØR feilen, nektes også (write-vakten)", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const saveSettings = withApi(() => Promise.reject(new Error("db locked")));
    const queued = saveSettingsDebounced(5); // armert mens alt var friskt
    await hydrateSettings(); // …så feiler en omlesing
    expect(await queued).toBe(false);
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("«Prøv igjen»: en vellykket omlesing opphever vernet, og lagring går gjennom", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    let fail = true;
    const saveSettings = withApi(() =>
      fail
        ? Promise.reject(new Error("db locked"))
        : Promise.resolve({ ...SETTINGS_DEFAULTS, churchName: "Domkirken" }),
    );
    await hydrateSettings();
    expect(hydrateError.value).toBe("settingsLoadFailed");

    fail = false;
    await hydrateSettings();
    expect(hydrateError.value).toBeNull();
    expect(settings.value.churchName).toBe("Domkirken");
    expect(await saveSettingsDebounced(0)).toBe(true);
    expect(saveSettings).toHaveBeenCalledTimes(1);
  });
});
