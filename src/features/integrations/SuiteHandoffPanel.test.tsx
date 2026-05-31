import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { SuiteHandoffPanel } from "./SuiteHandoffPanel";
import i18n from "@/i18n";

// --- Tauri bridge mock ------------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
const invoke = h.invoke;

type Routes = {
  hasKey?: boolean;
  submit?: Record<string, unknown>;
  send?: Record<string, unknown>;
};

function routeInvoke(opts?: Routes) {
  const hasKey = opts?.hasKey ?? false;
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "integrations_song_has_apikey":
        return Promise.resolve(hasKey);
      case "integrations_song_set_apikey":
        return Promise.resolve(undefined);
      case "integrations_song_submit_usage":
        return Promise.resolve(opts?.submit ?? { ok: true, submitted: 2 });
      case "integrations_verbatim_send":
        return Promise.resolve(opts?.send ?? { ok: true });
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
      <SuiteHandoffPanel />
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

describe("SuiteHandoffPanel", () => {
  it("shows the key-missing badge when no API key is stored", async () => {
    routeInvoke({ hasKey: false });
    renderPanel();
    expect(await screen.findByText("Ingen nøkkel")).toBeInTheDocument();
  });

  it("shows the key-stored badge when a key is present", async () => {
    routeInvoke({ hasKey: true });
    renderPanel();
    expect(await screen.findByText("Nøkkel lagret")).toBeInTheDocument();
  });

  it("saves the API key over IPC", async () => {
    renderPanel();
    const input = await screen.findByLabelText("API-nøkkel");
    fireEvent.change(input, { target: { value: "secret-123" } });
    fireEvent.click(screen.getByText("Lagre nøkkel"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("integrations_song_set_apikey", {
        plaintext: "secret-123",
      }),
    );
  });

  it("submits usage for the entered recording path and shows the count", async () => {
    routeInvoke({ submit: { ok: true, submitted: 3 } });
    renderPanel();
    const path = await screen.findByLabelText("Sti til opptak");
    fireEvent.change(path, { target: { value: "/rec/svc.mp3" } });
    fireEvent.click(screen.getByText("Send bruk til SundaySong"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("integrations_song_submit_usage", {
        recordingPath: "/rec/svc.mp3",
      }),
    );
    expect(await screen.findByText(/Bruk sendt \(3\)/)).toBeInTheDocument();
  });

  it("surfaces the hint when usage submission is not ready", async () => {
    routeInvoke({
      submit: { ok: false, error: "no_service_link", hint: "Link first." },
    });
    renderPanel();
    const path = await screen.findByLabelText("Sti til opptak");
    fireEvent.change(path, { target: { value: "/rec/x.mp3" } });
    fireEvent.click(screen.getByText("Send bruk til SundaySong"));
    expect(await screen.findByText("Link first.")).toBeInTheDocument();
  });

  it("reports verbatim-not-installed when the send fails", async () => {
    routeInvoke({ send: { ok: false, error: "verbatim_not_installed" } });
    renderPanel();
    const path = await screen.findByLabelText("Sti til opptak");
    fireEvent.change(path, { target: { value: "/rec/x.mp4" } });
    fireEvent.click(screen.getByText("Åpne i SundayEdit"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("integrations_verbatim_send", {
        videoPath: "/rec/x.mp4",
      }),
    );
    expect(
      await screen.findByText(/SundayEdit er ikke installert/),
    ).toBeInTheDocument();
  });
});
