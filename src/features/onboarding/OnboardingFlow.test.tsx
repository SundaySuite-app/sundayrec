import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { OnboardingFlow, classifySignal } from "./OnboardingFlow";
import type { Settings } from "@/lib/bindings/Settings";
import type { AudioDeviceList } from "@/lib/bindings/AudioDeviceList";
import i18n from "@/i18n";

// --- Tauri bridge mocks -----------------------------------------------------

const h = vi.hoisted(() => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    invoke: vi.fn(),
    handlers,
    listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
      handlers.set(name, cb);
      return Promise.resolve(() => handlers.delete(name));
    }),
    emit(name: string, payload: unknown) {
      handlers.get(name)?.({ payload });
    },
  };
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) =>
    (h.listen as (...a: unknown[]) => unknown)(...args),
}));
const invoke = h.invoke;

const DEVICES: AudioDeviceList = {
  host: "CoreAudio",
  inputs: [
    {
      name: "Built-in Microphone",
      direction: "input",
      channels: 1,
      sample_rates: [48000],
      is_default: true,
    },
    {
      name: "Scarlett 2i2",
      direction: "input",
      channels: 2,
      sample_rates: [48000],
      is_default: false,
    },
  ],
};

function settingsBlob(overrides?: Partial<Settings>): Settings {
  return {
    onboardingDone: false,
    deviceName: null,
    language: "no",
    slots: [],
    specialRecordings: [],
    ...overrides,
  } as unknown as Settings;
}

function routeInvoke(settings: Settings) {
  invoke.mockImplementation((cmd: string, args?: { settings?: Settings }) => {
    switch (cmd) {
      case "settings_get":
        return Promise.resolve(settings);
      case "list_input_devices":
        return Promise.resolve(DEVICES);
      case "settings_save":
        return Promise.resolve(args?.settings ?? settings);
      default:
        return Promise.resolve(undefined);
    }
  });
}

function renderFlow() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <OnboardingFlow />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invoke.mockReset();
  h.handlers.clear();
  h.listen.mockClear();
  routeInvoke(settingsBlob());
  i18n.changeLanguage("no");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("classifySignal", () => {
  it("maps dBFS to a verdict", () => {
    expect(classifySignal(null)).toBe("waiting");
    expect(classifySignal(-70)).toBe("waiting");
    expect(classifySignal(-50)).toBe("weak");
    expect(classifySignal(-20)).toBe("good");
    expect(classifySignal(-6)).toBe("loud");
    expect(classifySignal(-1)).toBe("clip");
  });
});

describe("OnboardingFlow visibility", () => {
  it("does not render when onboarding is already done", async () => {
    routeInvoke(settingsBlob({ onboardingDone: true }));
    renderFlow();
    // Give the settings query a tick to resolve.
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("settings_get"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the welcome step on first run", async () => {
    renderFlow();
    expect(
      await screen.findByText("Velkommen til SundayRec"),
    ).toBeInTheDocument();
  });
});

describe("OnboardingFlow steps + dots", () => {
  it("advances welcome → device → audio → ready with progress dots", async () => {
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    // Dot 1 active.
    expect(screen.getByText("Velkommen til SundayRec")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Kom i gang →"));
    expect(
      await screen.findByText("Hvilken lydenhet bruker dere?"),
    ).toBeInTheDocument();
    // The non-built-in device is auto-selected.
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /Scarlett 2i2/ }),
      ).toHaveAttribute("aria-pressed", "true"),
    );

    fireEvent.click(screen.getByText("Bruk valgt enhet →"));
    expect(
      await screen.findByText("Test at lyden fungerer"),
    ).toBeInTheDocument();
    // Entering the audio step starts the VU engine with the picked device.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_vu", {
        deviceName: "Scarlett 2i2",
      }),
    );

    fireEvent.click(screen.getByText("Lyden fungerer →"));
    expect(await screen.findByText("Alt er klart!")).toBeInTheDocument();
  });

  it("reflects the live VU verdict from vu://levels", async () => {
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    fireEvent.click(screen.getByText("Kom i gang →"));
    await screen.findByText("Hvilken lydenhet bruker dere?");
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /Scarlett 2i2/ }),
      ).toHaveAttribute("aria-pressed", "true"),
    );
    fireEvent.click(screen.getByText("Bruk valgt enhet →"));
    await screen.findByText("Test at lyden fungerer");
    // A healthy signal → "Bra".
    h.emit("vu://levels", { peak_dbfs: [-20], rms_dbfs: [-25] });
    expect(await screen.findByText("Bra")).toBeInTheDocument();
  });

  it("requires a device before leaving the device step", async () => {
    routeInvoke(settingsBlob());
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "settings_get") return Promise.resolve(settingsBlob());
      if (cmd === "list_input_devices")
        return Promise.resolve({ host: "CoreAudio", inputs: [] });
      return Promise.resolve(undefined);
    });
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    fireEvent.click(screen.getByText("Kom i gang →"));
    await screen.findByText("Hvilken lydenhet bruker dere?");
    // No devices → nothing auto-picked → advancing shows the validation error.
    fireEvent.click(screen.getByText("Bruk valgt enhet →"));
    expect(
      await screen.findByText("Velg en lydenhet før du fortsetter"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Test at lyden fungerer"),
    ).not.toBeInTheDocument();
  });
});

describe("OnboardingFlow finish + skip", () => {
  it("persists onboardingDone + picked device on finish", async () => {
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    fireEvent.click(screen.getByText("Kom i gang →"));
    await screen.findByText("Hvilken lydenhet bruker dere?");
    // Wait for the auto-select to settle before advancing.
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /Scarlett 2i2/ }),
      ).toHaveAttribute("aria-pressed", "true"),
    );
    fireEvent.click(screen.getByText("Bruk valgt enhet →"));
    await screen.findByText("Test at lyden fungerer");
    fireEvent.click(screen.getByText("Lyden fungerer →"));
    await screen.findByText("Alt er klart!");
    fireEvent.click(screen.getByText("Åpne SundayRec →"));
    await waitFor(() => {
      const save = invoke.mock.calls.find((c) => c[0] === "settings_save");
      expect(save).toBeTruthy();
      const arg = save?.[1] as { settings: Settings };
      expect(arg.settings.onboardingDone).toBe(true);
      expect(arg.settings.deviceName).toBe("Scarlett 2i2");
    });
    // Wizard dismisses after finishing.
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("skip-all on welcome marks onboarding done and dismisses", async () => {
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    fireEvent.click(screen.getByText("Hopp over — sett opp manuelt"));
    await waitFor(() => {
      const save = invoke.mock.calls.find((c) => c[0] === "settings_save");
      expect(
        (save?.[1] as { settings: Settings })?.settings.onboardingDone,
      ).toBe(true);
    });
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("stops the VU engine when finishing", async () => {
    renderFlow();
    await screen.findByText("Velkommen til SundayRec");
    fireEvent.click(screen.getByText("Hopp over — sett opp manuelt"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_vu"));
  });
});
