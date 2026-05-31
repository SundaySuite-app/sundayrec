import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { StreamingPanel } from "./StreamingPanel";
import type { StreamStatus } from "@/lib/bindings/StreamStatus";
import i18n from "@/i18n";

// --- Tauri bridge mock ------------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
const invoke = h.invoke;

const IDLE: StreamStatus = {
  active: false,
  startedAt: null,
  bitrateKbps: 0,
  fps: 0,
  dropped: 0,
  lastLine: "",
};

const LIVE: StreamStatus = {
  active: true,
  startedAt: 1_700_000_000_000n,
  bitrateKbps: 4500,
  fps: 30,
  dropped: 0,
  lastLine: "",
};

/** Route invoke() by command name. `status` controls the status poll; other
 *  commands resolve to a benign value unless overridden. */
function routeInvoke(opts?: {
  status?: StreamStatus;
  startError?: unknown;
  setKeyError?: unknown;
}) {
  const status = opts?.status ?? IDLE;
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "stream_status":
        return Promise.resolve(status);
      case "stream_start":
        return opts?.startError
          ? Promise.reject(opts.startError)
          : Promise.resolve(LIVE);
      case "stream_set_key":
        return opts?.setKeyError
          ? Promise.reject(opts.setKeyError)
          : Promise.resolve(undefined);
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
      <StreamingPanel />
    </QueryClientProvider>,
  );
}

/** Add a destination via the form so the per-destination UI is present. */
async function addDestination(name = "YouTube", url = "rtmp://a/live2") {
  fireEvent.change(screen.getByLabelText("Navn (f.eks. YouTube)"), {
    target: { value: name },
  });
  fireEvent.change(screen.getByLabelText("RTMP-URL"), {
    target: { value: url },
  });
  fireEvent.click(screen.getByText("Legg til destinasjon"));
  await screen.findByText(name);
}

beforeEach(() => {
  invoke.mockReset();
  routeInvoke();
  i18n.changeLanguage("no");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("StreamingPanel", () => {
  it("shows idle status with a Start button", async () => {
    renderPanel();
    expect(await screen.findByText("Av")).toBeInTheDocument();
    expect(screen.getByText("Start")).toBeInTheDocument();
  });

  it("shows live status + stats + a Stop button when streaming", async () => {
    routeInvoke({ status: LIVE });
    renderPanel();
    expect(await screen.findByText("Sender direkte")).toBeInTheDocument();
    expect(screen.getByText("4500 kbps · 30 fps")).toBeInTheDocument();
    expect(screen.getByText("Stopp")).toBeInTheDocument();
  });

  it("adds a destination and shows its name + url", async () => {
    renderPanel();
    await screen.findByText("Av");
    await addDestination();
    expect(screen.getByText("rtmp://a/live2")).toBeInTheDocument();
    // A new destination has no key yet → the key input is offered.
    expect(screen.getByText("Lagre nøkkel")).toBeInTheDocument();
  });

  it("saves a stream key over IPC and flips to the saved badge", async () => {
    renderPanel();
    await screen.findByText("Av");
    await addDestination();
    fireEvent.change(screen.getByLabelText("Strømnøkkel for YouTube"), {
      target: { value: "valid-stream-key" },
    });
    fireEvent.click(screen.getByText("Lagre nøkkel"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("stream_set_key", {
        destId: expect.stringMatching(/^dest-/),
        key: "valid-stream-key",
      }),
    );
    // The row now shows the saved badge + a delete action.
    expect(await screen.findByText("•••• (lagret)")).toBeInTheDocument();
    expect(screen.getByText("Slett nøkkel")).toBeInTheDocument();
  });

  it("starts the stream over IPC including a text lower-third overlay", async () => {
    renderPanel();
    await screen.findByText("Av");
    // Toggle the overlay on, then type the title.
    fireEvent.click(screen.getByLabelText("Vis tekstplakat"));
    fireEvent.change(screen.getByLabelText("Tittel"), {
      target: { value: "Pastor Ola" },
    });
    fireEvent.click(screen.getByText("Start"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "stream_start",
        expect.objectContaining({
          resolution: "p720",
          framerate: 30,
          overlays: [
            expect.objectContaining({
              source: { kind: "text", title: "Pastor Ola", subtitle: null },
              position: "bl",
            }),
          ],
        }),
      ),
    );
  });

  it("sends an image lower-third when the overlay kind is image", async () => {
    renderPanel();
    await screen.findByText("Av");
    fireEvent.click(screen.getByLabelText("Vis tekstplakat"));
    fireEvent.change(screen.getByLabelText("Type"), {
      target: { value: "image" },
    });
    fireEvent.change(screen.getByLabelText("Sti til bilde (PNG)"), {
      target: { value: "/tmp/lower.png" },
    });
    fireEvent.click(screen.getByText("Start"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "stream_start",
        expect.objectContaining({
          overlays: [
            expect.objectContaining({
              source: { kind: "image", path: "/tmp/lower.png" },
            }),
          ],
        }),
      ),
    );
  });

  it("omits the overlay when the toggle is off even if a title is typed", async () => {
    renderPanel();
    await screen.findByText("Av");
    // Title typed but the toggle is left OFF.
    fireEvent.change(screen.getByLabelText("Tittel"), {
      target: { value: "Pastor Ola" },
    });
    fireEvent.click(screen.getByText("Start"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "stream_start",
        expect.objectContaining({ overlays: [] }),
      ),
    );
  });

  it("removes a destination and best-effort clears its key over IPC", async () => {
    renderPanel();
    await screen.findByText("Av");
    await addDestination();
    fireEvent.click(screen.getByText("Fjern"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("stream_delete_key", {
        destId: expect.stringMatching(/^dest-/),
      }),
    );
    expect(screen.queryByText("rtmp://a/live2")).not.toBeInTheDocument();
  });

  it("deletes a saved key over IPC and offers the input again", async () => {
    renderPanel();
    await screen.findByText("Av");
    await addDestination();
    fireEvent.change(screen.getByLabelText("Strømnøkkel for YouTube"), {
      target: { value: "valid-stream-key" },
    });
    fireEvent.click(screen.getByText("Lagre nøkkel"));
    await screen.findByText("•••• (lagret)");
    fireEvent.click(screen.getByText("Slett nøkkel"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("stream_delete_key", {
        destId: expect.stringMatching(/^dest-/),
      }),
    );
    // Back to the unsaved state → the key input + Save button return.
    expect(await screen.findByText("Lagre nøkkel")).toBeInTheDocument();
  });

  it("stops the stream over IPC when live", async () => {
    routeInvoke({ status: LIVE });
    renderPanel();
    await screen.findByText("Sender direkte");
    fireEvent.click(screen.getByText("Stopp"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("stream_stop"),
    );
  });

  it("shows the feature-disabled hint when start rejects with feature_disabled", async () => {
    routeInvoke({
      startError: { code: "validation", message: "feature_disabled: streaming.start …" },
    });
    renderPanel();
    await screen.findByText("Av");
    fireEvent.click(screen.getByText("Start"));
    expect(
      await screen.findByText(
        "Direktesending er ikke bygd inn i denne versjonen. Nøkler kan likevel lagres.",
      ),
    ).toBeInTheDocument();
  });

  it("changes resolution + framerate selections", async () => {
    renderPanel();
    await screen.findByText("Av");
    const resSelect = screen.getByLabelText("Oppløsning") as HTMLSelectElement;
    fireEvent.change(resSelect, { target: { value: "p1080" } });
    expect(resSelect.value).toBe("p1080");

    fireEvent.click(screen.getByText("Start"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "stream_start",
        expect.objectContaining({ resolution: "p1080" }),
      ),
    );
  });
});
