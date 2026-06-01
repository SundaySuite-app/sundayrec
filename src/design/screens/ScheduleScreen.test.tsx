import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { ScheduleScreen } from "./ScheduleScreen";
import i18n from "@/i18n";

/**
 * Mock the Tauri IPC bridge. ScheduleScreen reads `settings_get`,
 * `scheduler_status` and `wake_capabilities` on mount, and writes via
 * `settings_save` + `scheduler_reschedule`. `settings_save` echoes the passed
 * settings so the cache (and the rendered weekly list) updates after an edit.
 *
 * Slot/Special field names mirror the ts-rs bindings (ScheduleSlot:
 * days/start/stop/max; SpecialRecording: id/date/name/start/stop/deviceId).
 */
const BASE_SETTINGS = {
  slots: [{ days: [6], start: "11:00", stop: "12:00", max: null }],
  specialRecordings: [],
  onboardingDone: true,
};

const invokeMock = vi.fn(
  async (cmd: string, args?: unknown): Promise<unknown> => {
    if (cmd === "settings_get") return BASE_SETTINGS;
    if (cmd === "settings_save") {
      // Echo the settings the caller asked us to persist.
      return (args as { settings: unknown }).settings;
    }
    if (cmd === "scheduler_status") return { next: null, upcoming: [] };
    if (cmd === "scheduler_reschedule") return { next: null, upcoming: [] };
    if (cmd === "wake_capabilities")
      return {
        platform: "mac-arm",
        canWakeFromSleep: true,
        canWakeFromOff: false,
        needsAdmin: true,
        knownIssues: [],
        recommendations: [],
      };
    return null;
  },
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

function renderSchedule() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ScheduleScreen />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invokeMock.mockClear();
  i18n.changeLanguage("no");
});

describe("ScheduleScreen", () => {
  it("renders the page title + weekly card and shows the live weekly slot", async () => {
    renderSchedule();

    // Static chrome is present immediately.
    expect(screen.getByText("Tidsplan")).toBeInTheDocument();
    expect(screen.getByText("Ukentlig tidsplan")).toBeInTheDocument();

    // Once settings_get resolves, the live Sunday slot row is rendered with
    // its time range. (The bare "Søn" weekday header is present even without
    // live data, so we assert on the slot's time range instead.)
    await waitFor(() =>
      expect(screen.getByText("11:00 – 12:00")).toBeInTheDocument(),
    );
  });

  it("reflects wake capabilities in the wake-card badge (Aktiv when wakeable)", async () => {
    renderSchedule();

    // canWakeFromSleep:true → "Aktiv" badge.
    await waitFor(() => expect(screen.getByText("Aktiv")).toBeInTheDocument());
    expect(screen.queryByText("Inaktiv")).not.toBeInTheDocument();
  });

  it("'Legg til tidspunkt' adds a slot and persists via settings_save", async () => {
    renderSchedule();

    // Wait for live data so the add control is enabled (haveLive).
    await waitFor(() => expect(screen.getByText("Søn")).toBeInTheDocument());

    const addBtn = screen.getByText("Legg til tidspunkt").closest("button")!;
    await waitFor(() => expect(addBtn).not.toBeDisabled());
    fireEvent.click(addBtn);

    // A settings_save invoke should eventually fire with the appended slot.
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "settings_save",
        expect.objectContaining({
          settings: expect.objectContaining({
            slots: expect.arrayContaining([
              expect.objectContaining({ start: "11:00", stop: "12:00" }),
            ]),
          }),
        }),
      ),
    );

    // The inline editor row opens on the freshly appended slot.
    await waitFor(() => expect(screen.getByText("Ferdig")).toBeInTheDocument());
  });

  it("supports month navigation without crashing", async () => {
    renderSchedule();

    // The nav chevrons are only enabled once live settings load (haveLive),
    // which is also when the label switches to the real month/year.
    const nextBtn = screen.getByTitle("Neste måned");
    await waitFor(() => expect(nextBtn).not.toBeDisabled());

    const todayBtn = screen.getByText("I dag");
    expect(todayBtn).toBeInTheDocument();

    // The month label is the only "… <year>" string on the page.
    const labelBefore = screen.getByText(/\d{4}$/).textContent;

    // Next chevron advances the displayed month.
    fireEvent.click(nextBtn);
    await waitFor(() => {
      const labelAfter = screen.getByText(/\d{4}$/).textContent;
      expect(labelAfter).toBeTruthy();
      expect(labelAfter).not.toBe(labelBefore);
    });

    // "I dag" returns to the current month without throwing.
    fireEvent.click(todayBtn);
    await waitFor(() =>
      expect(screen.getByText(/\d{4}$/).textContent).toBe(labelBefore),
    );
  });
});
