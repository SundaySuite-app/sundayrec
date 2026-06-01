import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { CloudBackupPanel } from "./CloudBackupPanel";
import type { CloudConnectionStatus } from "@/lib/bindings/CloudConnectionStatus";
import type { QueueEntryView } from "@/lib/bindings/QueueEntryView";
import i18n from "@/i18n";

// --- Tauri bridge mock ------------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
const invoke = h.invoke;

const CONNECTIONS: CloudConnectionStatus[] = [
  { service: "google-drive", connected: true },
  { service: "youtube", connected: false },
  { service: "gmail", connected: false },
];

// The durable queue may carry optional byte/progress fields the binding does
// not type yet; the panel reads them defensively, so we attach them here to
// exercise the size formatting + progress-bar rendering.
type QueueEntryWithBytes = QueueEntryView & {
  byteSize?: number | null;
  uploadedBytes?: number | null;
  totalBytes?: number | null;
};

const QUEUE: QueueEntryWithBytes[] = [
  {
    id: "q1",
    service: "google-drive",
    filename: "2026-05-31.mp4",
    attempts: 2,
    nextAttempt: 1_700_000_000_000,
    lastError: "network down",
    status: "failed",
    byteSize: 2_500_000_000, // 2.5 GB
  },
  {
    id: "q2",
    service: "google-drive",
    filename: "2026-05-24.mp4",
    attempts: 0,
    nextAttempt: 1_700_000_000_000,
    lastError: null,
    status: "pending",
    totalBytes: 8_000_000, // 8 MB
    uploadedBytes: 2_000_000, // 25%
  },
];

/** Route invoke() by command name; mutations resolve to a benign value. */
function routeInvoke(
  connections: CloudConnectionStatus[] = CONNECTIONS,
  queue: QueueEntryWithBytes[] = QUEUE,
) {
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "cloud_connection_status":
        return Promise.resolve(connections);
      case "cloud_queue_status":
        return Promise.resolve(queue);
      default:
        return Promise.resolve(undefined);
    }
  });
}

function renderPanel() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <CloudBackupPanel />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invoke.mockReset();
  routeInvoke();
  i18n.changeLanguage("no");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("CloudBackupPanel", () => {
  it("shows connection status per service", async () => {
    renderPanel();
    expect(await screen.findByText("Google Drive")).toBeInTheDocument();
    expect(screen.getByText("YouTube")).toBeInTheDocument();
    expect(screen.getByText("Gmail")).toBeInTheDocument();
    // The connected service offers a disconnect action.
    expect(screen.getByText("Koble fra")).toBeInTheDocument();
  });

  it("lists the upload queue with status and error", async () => {
    renderPanel();
    expect(await screen.findByText("2026-05-31.mp4")).toBeInTheDocument();
    expect(screen.getByText("network down")).toBeInTheDocument();
    expect(screen.getByText("Feilet")).toBeInTheDocument();
  });

  it("renders a summary header with item count and failed count", async () => {
    renderPanel();
    // Two entries total, one of them failed.
    expect(await screen.findByText("2 i kø")).toBeInTheDocument();
    expect(screen.getByText("1 feilet")).toBeInTheDocument();
  });

  it("summarises pending size from the only pending entry", async () => {
    renderPanel();
    // q2 is the single pending entry: 8 MB total.
    expect(await screen.findByText("8 MB venter")).toBeInTheDocument();
  });

  it("formats per-entry byte size (GB / MB)", async () => {
    renderPanel();
    await screen.findByText("2026-05-31.mp4");
    // q1 byteSize 2.5 GB, q2 totalBytes 8 MB.
    expect(screen.getByText("2.5 GB")).toBeInTheDocument();
    expect(screen.getByText("8 MB")).toBeInTheDocument();
  });

  it("shows a progress bar for an entry with uploaded/total bytes", async () => {
    renderPanel();
    await screen.findByText("2026-05-24.mp4");
    // q2 uploaded 2 MB of 8 MB → 25%.
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "25");
  });

  it("marks a failed entry red with an inline retry button", async () => {
    renderPanel();
    await screen.findByText("2026-05-31.mp4");
    // Failed entry surfaces its error and an inline retry.
    expect(screen.getByText("network down")).toBeInTheDocument();
    expect(screen.getByText("Prøv igjen")).toBeInTheDocument();
  });

  it("retries a failed entry over IPC", async () => {
    renderPanel();
    await screen.findByText("2026-05-31.mp4");
    fireEvent.click(screen.getByText("Prøv igjen"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cloud_retry_upload", { id: "q1" }),
    );
  });

  it("removes an entry over IPC", async () => {
    renderPanel();
    await screen.findByText("2026-05-24.mp4");
    // The pending entry has only a "Fjern" button (no retry).
    const removeButtons = screen.getAllByText("Fjern");
    fireEvent.click(removeButtons[removeButtons.length - 1]!);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cloud_remove_upload", { id: "q2" }),
    );
  });

  it("clears failed entries when any are failed", async () => {
    renderPanel();
    await screen.findByText("2026-05-31.mp4");
    fireEvent.click(screen.getByText("Fjern feilede"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cloud_clear_failed"),
    );
  });

  it("disconnects after confirmation", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderPanel();
    await screen.findByText("Google Drive");
    fireEvent.click(screen.getByText("Koble fra"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cloud_disconnect", {
        service: "google-drive",
      }),
    );
    confirmSpy.mockRestore();
  });

  it("connects a disconnected service over IPC", async () => {
    renderPanel();
    await screen.findByText("YouTube");
    // YouTube + Gmail are disconnected → two "Koble til" buttons.
    const connectButtons = screen.getAllByText("Koble til");
    fireEvent.click(connectButtons[0]!);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cloud_connect", {
        service: "youtube",
      }),
    );
  });

  it("shows the empty state with no queued uploads", async () => {
    routeInvoke(CONNECTIONS, []);
    renderPanel();
    expect(
      await screen.findByText("Ingen køede opplastinger"),
    ).toBeInTheDocument();
  });
});
