import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { HomeScreen } from "./HomeScreen";
import i18n from "@/i18n";

// Mock the Tauri IPC bridge — there's no backend in the jsdom runner. The
// HomeScreen (and its hooks) call: scheduler_status, settings_get,
// settings_save, list_input_devices, list_devices, get_disk_space, plus the
// VU/preview engine start/stop commands. Return sensible values per command
// and resolve `null` for anything unexpected so nothing throws.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown): Promise<unknown> => {
    if (cmd === "scheduler_status") return { next: null, upcoming: [] };
    if (cmd === "settings_get")
      return {
        slots: [],
        specialRecordings: [],
        onboardingDone: true,
        videoEnabled: true,
        videoDeviceName: null,
        videoDeviceIndex: null,
      };
    if (cmd === "settings_save")
      return (args as { settings: unknown })?.settings ?? null;
    if (cmd === "list_input_devices")
      return {
        host: "CoreAudio",
        inputs: [
          {
            name: "Mic",
            direction: "input",
            channels: 2,
            sample_rates: [48000],
            is_default: true,
          },
        ],
      };
    if (cmd === "list_devices") return { video_inputs: [] };
    if (cmd === "get_disk_space") return { freeBytes: 600_000_000_000 };
    return null;
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

function renderHome(props: { onRecord?: (video: boolean) => void } = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <HomeScreen {...props} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  i18n.changeLanguage("no");
});

describe("HomeScreen", () => {
  it("renders the trust banner and the big record button", async () => {
    renderHome();
    // Trust banner: the "ready for recording" label.
    await waitFor(() =>
      expect(screen.getByText("Klar for opptak")).toBeInTheDocument(),
    );
    // The big record button.
    expect(
      screen.getByRole("button", { name: /Start opptak nå/ }),
    ).toBeInTheDocument();
  });

  it("switches from the camera layout to the audio-only meter layout on video toggle", async () => {
    renderHome();

    // Wait for async data to settle (settings seeds the video flag once on
    // mount; toggling before that races the seed effect). The mic name proves
    // the device query resolved.
    await waitFor(() => expect(screen.getByText("Mic")).toBeInTheDocument());

    // Video defaults on → the camera select is shown, audio-only heading absent.
    expect(
      screen.getByRole("combobox", { name: "Velg kamera" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Lydnivå — live")).not.toBeInTheDocument();

    // Flip video off via the toggle button (labelled "Video på" while on).
    fireEvent.click(screen.getByRole("button", { name: /Video på/ }));

    // Audio-only layout appears; the camera select disappears.
    await waitFor(() =>
      expect(screen.getByText("Lydnivå — live")).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("combobox", { name: "Velg kamera" }),
    ).not.toBeInTheDocument();
  });

  it("calls onRecord with the current video flag", async () => {
    const onRecord = vi.fn();
    renderHome({ onRecord });

    // Let settings seed the video flag before interacting (see toggle test).
    await waitFor(() => expect(screen.getByText("Mic")).toBeInTheDocument());

    // With video still on (default), clicking records with video = true.
    fireEvent.click(screen.getByRole("button", { name: /Start opptak nå/ }));
    expect(onRecord).toHaveBeenCalledWith(true);

    // Toggle video off, then record again → records with video = false.
    fireEvent.click(screen.getByRole("button", { name: /Video på/ }));
    await waitFor(() =>
      expect(screen.getByText("Lydnivå — live")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /Start opptak nå/ }));
    expect(onRecord).toHaveBeenLastCalledWith(false);
  });

  it("shows the resolved mic device name from list_input_devices", async () => {
    renderHome();
    await waitFor(() => expect(screen.getByText("Mic")).toBeInTheDocument());
  });
});
