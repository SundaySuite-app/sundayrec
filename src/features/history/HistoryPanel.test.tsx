import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { HistoryPanel } from "./HistoryPanel";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";
import i18n from "@/i18n";

// --- Tauri bridge + opener plugin mocks -------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn(), reveal: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (...args: unknown[]) => h.reveal(...args),
}));

const invoke = h.invoke;
const reveal = h.reveal;

const ROWS: RecordingRow[] = [
  {
    id: "r1",
    file_path: "/recordings/2026-05-30.mp3",
    device_name: "USB Mixer",
    started_at: 1_700_000_000_000,
    duration_ms: 3_661_000,
    byte_size: 5_242_880,
    created_at: 1_700_000_100_000,
    note: "kun preken",
  },
  {
    id: "r2",
    file_path: "/recordings/2026-05-23.wav",
    device_name: null,
    started_at: 1_699_000_000_000,
    duration_ms: null,
    byte_size: null,
    created_at: 1_699_000_100_000,
    note: null,
  },
];

function renderPanel() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <HistoryPanel />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("no");
  invoke.mockReset();
  reveal.mockReset();
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "recordings_list":
        return Promise.resolve(ROWS);
      case "recordings_delete":
      case "recordings_clear":
      case "recording_update_note":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unexpected command: ${cmd}`));
    }
  });
  reveal.mockResolvedValue(undefined);
  // Confirm dialogs default to "yes" for the destructive-action tests.
  vi.spyOn(window, "confirm").mockReturnValue(true);
});

afterEach(async () => {
  vi.restoreAllMocks();
  await i18n.changeLanguage("no");
});

describe("HistoryPanel", () => {
  it("lists recordings newest-first with formatted duration and size", async () => {
    renderPanel();
    await waitFor(() =>
      expect(screen.getByText("2026-05-30.mp3")).toBeInTheDocument(),
    );
    expect(screen.getByText("2026-05-23.wav")).toBeInTheDocument();
    // 3_661_000 ms → 1:01:01, 5_242_880 bytes → 5.0 MB.
    expect(document.body.textContent).toContain("1:01:01");
    expect(document.body.textContent).toContain("5.0 MB");
    // The second row has no duration/size → em-dashes.
    expect(document.body.textContent).toContain("—");
  });

  it("shows the empty state when there are no recordings", async () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "recordings_list"
        ? Promise.resolve([])
        : Promise.reject(new Error(cmd)),
    );
    renderPanel();
    await waitFor(() =>
      expect(screen.getByText("Ingen opptak ennå")).toBeInTheDocument(),
    );
  });

  it("deletes a row after confirmation", async () => {
    renderPanel();
    await waitFor(() =>
      expect(screen.getByText("2026-05-30.mp3")).toBeInTheDocument(),
    );
    const deleteButtons = screen.getAllByRole("button", {
      name: "Slett oppføring",
    });
    fireEvent.click(deleteButtons[0]);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("recordings_delete", { id: "r1" }),
    );
  });

  it("clears the whole history after confirmation", async () => {
    renderPanel();
    const clearBtn = await screen.findByRole("button", { name: "Slett alle" });
    fireEvent.click(clearBtn);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("recordings_clear"),
    );
  });

  it("debounce-saves an edited note (empty string → null)", async () => {
    vi.useFakeTimers();
    try {
      renderPanel();
      await vi.waitFor(() =>
        expect(screen.getByText("2026-05-30.mp3")).toBeInTheDocument(),
      );
      const noteInputs = screen.getAllByLabelText("Rediger notat");
      fireEvent.change(noteInputs[0], { target: { value: "ny tekst" } });

      expect(invoke).not.toHaveBeenCalledWith(
        "recording_update_note",
        expect.anything(),
      );
      await vi.advanceTimersByTimeAsync(700);
      expect(invoke).toHaveBeenCalledWith("recording_update_note", {
        id: "r1",
        note: "ny tekst",
      });

      // Clearing the field sends null, not an empty string.
      fireEvent.change(noteInputs[0], { target: { value: "" } });
      await vi.advanceTimersByTimeAsync(700);
      expect(invoke).toHaveBeenCalledWith("recording_update_note", {
        id: "r1",
        note: null,
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("reveals a file in its folder via the opener plugin", async () => {
    renderPanel();
    await waitFor(() =>
      expect(screen.getByText("2026-05-30.mp3")).toBeInTheDocument(),
    );
    const revealButtons = screen.getAllByRole("button", {
      name: "Vis i mappe",
    });
    fireEvent.click(revealButtons[0]);
    await waitFor(() =>
      expect(reveal).toHaveBeenCalledWith("/recordings/2026-05-30.mp3"),
    );
  });

  it("shows a friendly error when reveal fails", async () => {
    reveal.mockRejectedValue(new Error("no such path"));
    renderPanel();
    await waitFor(() =>
      expect(screen.getByText("2026-05-30.mp3")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getAllByRole("button", { name: "Vis i mappe" })[0]);
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
  });
});
