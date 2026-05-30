import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { VuMeter } from "./VuMeter";
import type { AudioDeviceList } from "@/lib/bindings/AudioDeviceList";
import type { VuLevels } from "@/lib/bindings/VuLevels";

// --- Tauri bridge mocks -----------------------------------------------------

// `vi.hoisted` lets the mock factories (which are hoisted above imports) share
// state with the test body — the invoke spy and the captured event handler.
const h = vi.hoisted(() => ({
  invoke: vi.fn(),
  vuHandler: null as ((event: { payload: VuLevels }) => void) | null,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));

// Capture the `vu://levels` handler so the test can push fake payloads, mimicking
// the backend event without a real stream.
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: (event: { payload: VuLevels }) => void) => {
    if (name === "vu://levels") h.vuHandler = handler;
    return Promise.resolve(() => {
      h.vuHandler = null;
    });
  },
}));

const invoke = h.invoke;

const DEVICES: AudioDeviceList = {
  host: "CoreAudio",
  inputs: [
    {
      name: "Built-in Microphone",
      direction: "input",
      channels: 1,
      sample_rates: [48_000],
      is_default: true,
    },
    {
      name: "RØDE NT-USB",
      direction: "input",
      channels: 2,
      sample_rates: [44_100, 48_000],
      is_default: false,
    },
  ],
};

function emitLevels(levels: VuLevels) {
  h.vuHandler?.({ payload: levels });
}

beforeEach(() => {
  invoke.mockReset();
  h.vuHandler = null;
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_input_devices":
        return Promise.resolve(DEVICES);
      case "start_vu":
      case "stop_vu":
        return Promise.resolve(null);
      default:
        return Promise.reject(new Error(`unexpected command: ${cmd}`));
    }
  });
});

describe("VuMeter", () => {
  it("loads input devices into the dropdown", async () => {
    render(<VuMeter />);
    await waitFor(() =>
      expect(screen.getByText("CoreAudio")).toBeInTheDocument(),
    );
    expect(
      screen.getByRole("option", { name: /Built-in Microphone/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: /RØDE NT-USB/ }),
    ).toBeInTheDocument();
  });

  it("starts the VU engine and reflects a loud peak vs a silent floor", async () => {
    render(<VuMeter />);
    await waitFor(() =>
      expect(screen.getByText("CoreAudio")).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Start VU" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_vu", { deviceName: null }),
    );

    // A loud, near-clipping peak should fill the meter.
    emitLevels({ peak_dbfs: [-1], rms_dbfs: [-4] });
    await waitFor(() => {
      const meter = screen.getByRole("meter");
      // -1 dB over a -60 floor ≈ 98% full.
      expect(Number(meter.getAttribute("aria-valuenow"))).toBeGreaterThan(90);
    });
    expect(screen.getByText("-1.0 dB")).toBeInTheDocument();

    // Silence (backend sends null for -∞) should empty the meter.
    emitLevels({
      peak_dbfs: [null as unknown as number],
      rms_dbfs: [null as unknown as number],
    });
    await waitFor(() => {
      const meter = screen.getByRole("meter");
      expect(meter.getAttribute("aria-valuenow")).toBe("0");
    });
    expect(screen.getByText("-∞")).toBeInTheDocument();
  });

  it("renders one bar per channel for a stereo device payload", async () => {
    render(<VuMeter />);
    await waitFor(() =>
      expect(screen.getByText("CoreAudio")).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Start VU" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_vu", expect.anything()),
    );

    emitLevels({ peak_dbfs: [-6, -12], rms_dbfs: [-9, -15] });
    await waitFor(() => expect(screen.getAllByRole("meter")).toHaveLength(2));
  });

  it("stops the engine and clears the meter", async () => {
    render(<VuMeter />);
    await waitFor(() =>
      expect(screen.getByText("CoreAudio")).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Start VU" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Stopp" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_vu"));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Start VU" }),
      ).toBeInTheDocument(),
    );
  });
});
