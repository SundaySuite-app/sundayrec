import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { RecorderPanel } from "./RecorderPanel";
import type { RecordingProgress } from "@/lib/bindings/RecordingProgress";
import type { RecordingEvent } from "@/lib/bindings/RecordingEvent";

// --- Tauri bridge mocks -----------------------------------------------------

type Handler = (event: { payload: unknown }) => void;

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
  handlers: new Map<string, Handler>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));

// Capture each `recording://*` handler so the test can push fake events.
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: Handler) => {
    h.handlers.set(name, handler);
    return Promise.resolve(() => h.handlers.delete(name));
  },
}));

const invoke = h.invoke;

function emit(name: string, payload: unknown) {
  h.handlers.get(name)?.({ payload });
}

beforeEach(() => {
  invoke.mockReset();
  h.handlers.clear();
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "start_recording":
      case "stop_recording":
        return Promise.resolve(null);
      default:
        return Promise.reject(new Error(`unexpected command: ${cmd}`));
    }
  });
});

describe("RecorderPanel", () => {
  it("shows the idle hint before starting", () => {
    render(<RecorderPanel />);
    expect(
      screen.getByRole("button", { name: "Start opptak" }),
    ).toBeInTheDocument();
  });

  it("starts a recording and passes RecordingOpts to the backend", async () => {
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "start_recording",
        expect.objectContaining({
          opts: expect.objectContaining({
            audio_device_name: "",
            video_device_name: null,
            stereo: true,
          }),
        }),
      ),
    );
    // The stop button is now shown.
    expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument();
  });

  it("renders progress (MB written) once a progress event arrives", async () => {
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument(),
    );

    emit("recording://started", null);
    // 2 MiB written.
    const progress: RecordingProgress = { bytes_written: 2 * 1024 * 1024 };
    emit("recording://progress", progress);

    await waitFor(() =>
      expect(screen.getByText(/2\.0 MB skrevet/)).toBeInTheDocument(),
    );
    // "started" state replaces the "Starter …" placeholder.
    expect(screen.getByText("● Tar opp")).toBeInTheDocument();
  });

  it("shows a silence warning when a silence event arrives", async () => {
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument(),
    );

    const sil: RecordingEvent = {
      code: "silence_detected",
      message: "Stillhet oppdaget i lydsignalet",
    };
    emit("recording://silence", sil);

    await waitFor(() =>
      expect(
        screen.getByText(/Stillhet oppdaget i lydsignalet/),
      ).toBeInTheDocument(),
    );
  });

  it("shows a classified error when an error event arrives", async () => {
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument(),
    );

    const err: RecordingEvent = {
      code: "device_disconnected",
      message: "Broken pipe while writing",
    };
    emit("recording://error", err);

    await waitFor(() => {
      const alert = screen.getByText(/device_disconnected/);
      expect(alert).toBeInTheDocument();
      expect(alert).toHaveTextContent("Broken pipe while writing");
    });
  });

  it("surfaces a launch failure from start_recording", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "start_recording") {
        return Promise.reject({ message: "no audio device matched ''" });
      }
      return Promise.resolve(null);
    });
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(
        screen.getByText(/Kunne ikke starte: no audio device matched/),
      ).toBeInTheDocument(),
    );
    // Stayed on the start button (never entered running state).
    expect(
      screen.getByRole("button", { name: "Start opptak" }),
    ).toBeInTheDocument();
  });

  it("stops the recording", async () => {
    render(<RecorderPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Start opptak" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Stopp" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Stopp" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("stop_recording"),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Start opptak" }),
      ).toBeInTheDocument(),
    );
  });
});
