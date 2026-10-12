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
    patchSettings({ churchName: "Annen kirke" });
    expect(await saveSettingsDebounced(0)).toBe(true);
    expect(saveSettings).toHaveBeenCalledTimes(1);
  });
});

/** Et `saveSettings` som svarer når testen sier fra — for å holde en skriving i lufta. */
function withHeldSaves() {
  const held: Array<(ok: boolean) => void> = [];
  const saveSettings = vi.fn(
    () => new Promise<boolean>((resolve) => held.push(resolve)),
  );
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      getSettings: () => Promise.resolve({ ...SETTINGS_DEFAULTS }),
      saveSettings,
    },
  };
  return { saveSettings, held };
}

const tick = () => new Promise<void>((r) => setTimeout(r, 0));

describe("lagringen sender bare det som er endret", () => {
  it("en utdatert kopi skriver ikke over felt den ikke har rørt", async () => {
    const saveSettings = withApi(() =>
      Promise.resolve({ ...SETTINGS_DEFAULTS, churchName: "Domkirken" }),
    );
    await hydrateSettings();
    patchSettings({ language: "en" });
    expect(await saveSettingsDebounced(0)).toBe(true);
    // Kun det ene feltet krysser — ukeplanen, kirkenavnet og resten blir i
    // basen slik de er der nå, ikke slik denne kopien leste dem.
    expect(saveSettings).toHaveBeenCalledTimes(1);
    expect(saveSettings).toHaveBeenCalledWith({ language: "en" });
  });

  it("ingenting endret → ingen skriving, og «lagret» er sant", async () => {
    const saveSettings = withApi(() =>
      Promise.resolve({ ...SETTINGS_DEFAULTS }),
    );
    await hydrateSettings();
    expect(await saveSettingsDebounced(0)).toBe(true);
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("en feilet skriving husker hva som ikke landet og sender det igjen", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const saveSettings = withApi(() =>
      Promise.resolve({ ...SETTINGS_DEFAULTS }),
    );
    saveSettings.mockRejectedValueOnce(new Error("db locked"));
    await hydrateSettings();
    patchSettings({ churchName: "Domkirken" });
    expect(await saveSettingsDebounced(0)).toBe(false);
    expect(await saveSettingsDebounced(0)).toBe(true);
    expect(saveSettings).toHaveBeenLastCalledWith({ churchName: "Domkirken" });
  });
});

describe("lagringskøen: hver byge får sitt eget svar", () => {
  it("en endring mens en skriving er i lufta låner ikke den skrivingens kvittering", async () => {
    const { saveSettings, held } = withHeldSaves();
    await hydrateSettings();

    patchSettings({ churchName: "Første" });
    const first = saveSettingsDebounced(0);
    await tick();
    expect(saveSettings).toHaveBeenCalledTimes(1);

    patchSettings({ language: "en" });
    let secondDone = false;
    const second = saveSettingsDebounced(0).then((ok) => {
      secondDone = true;
      return ok;
    });
    await tick();
    // Den andre skrivingen venter bak den første: ennå ikke sendt.
    expect(saveSettings).toHaveBeenCalledTimes(1);

    held[0](true);
    expect(await first).toBe(true);
    await tick();
    // Den første er kvittert — den andre er IKKE det. Den skal vente på sin egen.
    expect(secondDone).toBe(false);
    expect(saveSettings).toHaveBeenCalledTimes(2);
    expect(saveSettings).toHaveBeenLastCalledWith({ language: "en" });

    held[1](false);
    expect(await second).toBe(false);
  });
});
