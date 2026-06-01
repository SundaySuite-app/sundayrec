import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { LiveScreen } from "./LiveScreen";
import i18n from "@/i18n";

// Mock the Tauri IPC bridge — there's no backend in the jsdom test runner. The
// LiveScreen polls `stream_status` (the shared STREAM_STATUS_KEY), starts/stops
// the push via `stream_start`/`stream_stop`, manages keys via
// `stream_set_key`/`stream_delete_key`, and drives the VU meters which call
// `start_vu`/`stop_vu`. Stub them all to the "ready/off" state.
const invokeMock = vi.fn(async (cmd: string): Promise<unknown> => {
  if (cmd === "stream_status")
    return {
      active: false,
      startedAt: null,
      bitrateKbps: 0,
      fps: 0,
      dropped: 0,
      lastLine: "",
    };
  if (cmd === "stream_start") return null;
  if (cmd === "stream_stop") return null;
  if (cmd === "stream_set_key") return null;
  if (cmd === "stream_delete_key") return null;
  // VU engine started by useVuLevels(true) while mounted.
  if (cmd === "start_vu") return null;
  if (cmd === "stop_vu") return null;
  return null;
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) =>
    invokeMock(...(args as Parameters<typeof invokeMock>)),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

beforeEach(() => {
  i18n.changeLanguage("no");
  invokeMock.mockClear();
});

function renderLive() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <LiveScreen />
    </QueryClientProvider>,
  );
}

describe("LiveScreen", () => {
  it("renders the page title, the Statistikk card with its stat labels, and the start buttons", () => {
    renderLive();

    // Page title.
    expect(screen.getByText("Direkte sending")).toBeInTheDocument();

    // Statistikk card + its four stat labels.
    expect(screen.getByText("Statistikk")).toBeInTheDocument();
    expect(screen.getByText("Tid")).toBeInTheDocument();
    expect(screen.getByText("Bitrate")).toBeInTheDocument();
    expect(screen.getByText("FPS")).toBeInTheDocument();
    expect(screen.getByText("Tapte rammer")).toBeInTheDocument();

    // The two start buttons (idle state).
    expect(
      screen.getByText("Start direktesending + opptak"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Bare direktesending (uten lokal opptak)"),
    ).toBeInTheDocument();
  });

  it("renders the Strøm-kvalitet resolution options and selects one on click", () => {
    renderLive();

    expect(screen.getByText("Strøm-kvalitet")).toBeInTheDocument();

    // The three resolution labels render.
    const opt480 = screen.getByText("480p");
    const opt720 = screen.getByText("720p");
    const opt1080 = screen.getByText("1080p");
    expect(opt480).toBeInTheDocument();
    expect(opt720).toBeInTheDocument();
    expect(opt1080).toBeInTheDocument();

    // The SegOpt root carries the `sel` class on the selected option. Default
    // selection is 720p (the recommended/default resolution).
    const segOf = (label: HTMLElement) => label.closest(".sr-seg-opt")!;
    expect(segOf(opt720)).toHaveClass("sel");
    expect(segOf(opt1080)).not.toHaveClass("sel");

    // Clicking 1080p moves the selection there.
    fireEvent.click(opt1080);
    expect(segOf(opt1080)).toHaveClass("sel");
    expect(segOf(opt720)).not.toHaveClass("sel");
  });

  it("calls stream_start when the primary start button is clicked", async () => {
    renderLive();

    fireEvent.click(screen.getByText("Start direktesending + opptak"));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "stream_start",
        expect.any(Object),
      ),
    );
  });

  it("opens the inline add-destination form when the + action is used", () => {
    renderLive();

    // No add-form name input until the editor is opened.
    expect(
      screen.queryByPlaceholderText("Navn (f.eks. YouTube)"),
    ).not.toBeInTheDocument();

    // The "→ Konfigurer destinasjoner" link opens the same inline editor as +.
    fireEvent.click(screen.getByText("→ Konfigurer destinasjoner"));

    expect(
      screen.getByPlaceholderText("Navn (f.eks. YouTube)"),
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText("rtmp://…")).toBeInTheDocument();
  });
});
