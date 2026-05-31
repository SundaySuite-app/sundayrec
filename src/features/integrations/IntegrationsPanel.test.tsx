import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { IntegrationsPanel } from "./IntegrationsPanel";
import i18n from "@/i18n";

// --- Tauri bridge mock ------------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
const invoke = h.invoke;

/** Route invoke() by command name. `bridgeBuilt` toggles the native feature;
 *  `settingsJson` is the stored `integrations` blob (or null). */
function routeInvoke(opts?: {
  bridgeBuilt?: boolean;
  settingsJson?: string | null;
}) {
  const bridgeBuilt = opts?.bridgeBuilt ?? false;
  const settingsJson = opts?.settingsJson ?? null;
  invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "setting_get":
        return Promise.resolve(settingsJson);
      case "live_bridge_status":
        return Promise.resolve(bridgeBuilt);
      case "setting_set":
        return Promise.resolve(undefined);
      case "live_bridge_channel": {
        const churchId = String(args?.churchId ?? "");
        const serviceId = String(args?.serviceId ?? "");
        if (!churchId || !serviceId) {
          return Promise.reject(new Error("validation"));
        }
        return Promise.resolve(`church:${churchId}:service:${serviceId}`);
      }
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
      <IntegrationsPanel />
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

describe("IntegrationsPanel", () => {
  it("lists the Sunday-suite peer apps", async () => {
    renderPanel();
    expect(await screen.findByText("SundayPlan")).toBeInTheDocument();
    expect(screen.getByText("SundaySong")).toBeInTheDocument();
    expect(screen.getByText("SundayStage")).toBeInTheDocument();
    expect(screen.getByText("SundayEdit")).toBeInTheDocument();
    expect(screen.getByText("SundayStudio")).toBeInTheDocument();
  });

  it("shows the feature-disabled hint when the native bridge is off", async () => {
    routeInvoke({ bridgeBuilt: false });
    renderPanel();
    expect(
      await screen.findByText(/ikke bygd inn i denne versjonen/),
    ).toBeInTheDocument();
  });

  it("hides the disabled hint when the native bridge is built", async () => {
    routeInvoke({ bridgeBuilt: true });
    renderPanel();
    await screen.findByText("SundayPlan");
    await waitFor(() =>
      expect(
        screen.queryByText(/ikke bygd inn i denne versjonen/),
      ).not.toBeInTheDocument(),
    );
  });

  it("hydrates the connection fields from saved settings", async () => {
    routeInvoke({
      settingsJson: JSON.stringify({ churchId: "stmary", serviceId: "svc7" }),
    });
    renderPanel();
    expect(await screen.findByDisplayValue("stmary")).toBeInTheDocument();
    expect(screen.getByDisplayValue("svc7")).toBeInTheDocument();
  });

  it("persists the connection over IPC", async () => {
    renderPanel();
    const churchInput = await screen.findByLabelText("Menighets-ID");
    fireEvent.change(churchInput, { target: { value: "stmary" } });
    fireEvent.click(screen.getByText("Lagre tilkobling"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("setting_set", {
        key: "integrations",
        value: JSON.stringify({ churchId: "stmary" }),
      }),
    );
  });

  it("resolves the Realtime channel name for the configured ids", async () => {
    renderPanel();
    const churchInput = await screen.findByLabelText("Menighets-ID");
    const serviceInput = screen.getByLabelText("Gudstjeneste-ID (live)");
    fireEvent.change(churchInput, { target: { value: "stmary" } });
    fireEvent.change(serviceInput, { target: { value: "svc7" } });
    fireEvent.click(screen.getByText("Vis kanalnavn"));
    expect(
      await screen.findByText("church:stmary:service:svc7"),
    ).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("live_bridge_channel", {
      churchId: "stmary",
      serviceId: "svc7",
    });
  });
});
