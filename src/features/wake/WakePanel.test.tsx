import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { WakePanel } from "./WakePanel";
import type { WakeCapabilities } from "@/lib/bindings/WakeCapabilities";
import type { SleepConfig } from "@/lib/bindings/SleepConfig";
import type { WakeStatus } from "@/lib/bindings/WakeStatus";

const h = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));

const invoke = h.invoke;

const CAPS: WakeCapabilities = {
  platform: "mac-arm",
  canWakeFromSleep: true,
  canWakeFromOff: false,
  needsAdmin: true,
  knownIssues: ["Apple Silicon kan ikke starte fra avslått."],
  recommendations: ["La maskinen stå i dvale."],
};

const SLEEP_OK: SleepConfig = {
  autopoweroff: false,
  autopoweroffDelay: 0,
  standby: false,
  standbyDelay: 0,
  hibernateMode: 3,
  wakeTimersEnabled: null,
  error: null,
};

const SLEEP_BAD: SleepConfig = { ...SLEEP_OK, standby: true };

const VERIFY: WakeStatus = {
  expectedWakes: ["2026-06-07T10:50:00"],
  observedWakes: [
    { scheduledAt: "2026-06-07T10:50:00", ownerLabel: "SundayRec" },
  ],
  hasMismatch: false,
  onBattery: false,
  standbyEnabled: false,
};

function mockBridge(sleep: SleepConfig = SLEEP_OK) {
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "wake_capabilities":
        return Promise.resolve(CAPS);
      case "wake_get_sleep_config":
        return Promise.resolve(sleep);
      case "wake_fix_sleep":
        return Promise.resolve({ ok: true, message: null });
      case "wake_reschedule":
        return Promise.resolve({
          ok: true,
          count: 2,
          nextWake: "2026-06-07T10:50:00",
          reason: null,
          message: null,
        });
      case "wake_verify":
        return Promise.resolve(VERIFY);
      default:
        return Promise.reject(new Error(`unexpected command: ${cmd}`));
    }
  });
}

function renderPanel() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <WakePanel />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invoke.mockReset();
});

describe("WakePanel", () => {
  it("renders capabilities for the host platform", async () => {
    mockBridge();
    renderPanel();
    await waitFor(() => {
      expect(screen.getByTestId("wake-platform").textContent).toContain(
        "mac-arm",
      );
    });
    expect(screen.getByText(/Apple Silicon/)).toBeTruthy();
  });

  it("hides the sleep warning when config is healthy", async () => {
    mockBridge(SLEEP_OK);
    renderPanel();
    await waitFor(() =>
      expect(screen.getByTestId("wake-platform")).toBeTruthy(),
    );
    expect(screen.queryByTestId("wake-sleep-warning")).toBeNull();
  });

  it("shows the warning + fixes sleep when standby is on", async () => {
    mockBridge(SLEEP_BAD);
    renderPanel();
    await waitFor(() =>
      expect(screen.getByTestId("wake-sleep-warning")).toBeTruthy(),
    );
    fireEvent.click(screen.getByText("Fiks automatisk"));
    await waitFor(() =>
      expect(invoke.mock.calls.some((c) => c[0] === "wake_fix_sleep")).toBe(
        true,
      ),
    );
  });

  it("reschedules and verifies wakes on demand", async () => {
    mockBridge(SLEEP_OK);
    renderPanel();
    await waitFor(() => expect(screen.getByTestId("wake-panel")).toBeTruthy());

    fireEvent.click(screen.getByText("Planlegg vekking nå"));
    await waitFor(() =>
      expect(screen.getByTestId("wake-schedule-result").textContent).toContain(
        "2",
      ),
    );

    fireEvent.click(screen.getByText("Verifiser"));
    await waitFor(() => {
      const res = screen.getByTestId("wake-verify-result");
      expect(res).toBeTruthy();
      expect(res.textContent).toContain("SundayRec");
    });
  });
});
