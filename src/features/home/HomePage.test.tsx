import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import {
  HomePage,
  fmtBytes,
  fmtCountdown,
  fmtDuration,
  fmtNext,
} from "./HomePage";
import type { ScheduleStatus } from "@/lib/bindings/ScheduleStatus";
import type { Settings } from "@/lib/bindings/Settings";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";
import type { ReviewQueueEntry } from "@/lib/bindings/ReviewQueueEntry";
import type { EpisodePrep } from "@/lib/bindings/EpisodePrep";
import i18n from "@/i18n";

// --- Tauri bridge mocks -----------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
const invoke = h.invoke;

const SETTINGS: Settings = {
  slots: [{ days: [6], start: "11:00", stop: "12:30" }],
  specialRecordings: [],
} as unknown as Settings;

const RECORDINGS: RecordingRow[] = [
  {
    id: "r1",
    file_path: "/Users/x/SundayRec/2026-05-31.mp3",
    device_name: "Mixer",
    started_at: 1_716_000_000_000,
    duration_ms: 5_400_000,
    byte_size: 130_000_000,
    created_at: 1_716_000_000_000,
    note: null,
  },
];

function prep(status: EpisodePrep["status"]): ReviewQueueEntry {
  return {
    id: `q-${status}`,
    prep: { id: `p-${status}`, status } as unknown as EpisodePrep,
    addedAt: 0,
    reminded: 0,
    ageInDays: 0,
  };
}

function routeInvoke(opts?: {
  status?: ScheduleStatus;
  settings?: Settings;
  recordings?: RecordingRow[];
  queue?: ReviewQueueEntry[];
}) {
  const status = opts?.status ?? { next: null, upcoming: [] };
  const settings = opts?.settings ?? SETTINGS;
  const recordings = opts?.recordings ?? RECORDINGS;
  const queue = opts?.queue ?? [];
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "scheduler_status":
        return Promise.resolve(status);
      case "settings_get":
        return Promise.resolve(settings);
      case "recordings_list":
        return Promise.resolve(recordings);
      case "review_queue_list":
        return Promise.resolve(queue);
      case "list_input_devices":
        return Promise.resolve({ host: "CoreAudio", inputs: [] });
      default:
        return Promise.resolve(undefined);
    }
  });
}

function renderHome(onNavigate?: (v: string) => void) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <HomePage onNavigate={onNavigate as never} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invoke.mockReset();
  routeInvoke();
  i18n.changeLanguage("no");
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.setSystemTime(new Date("2026-05-31T10:00:00"));
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("home formatting helpers", () => {
  it("formats a countdown with days/hours/minutes", () => {
    expect(fmtCountdown(0)).toBe("");
    expect(fmtCountdown(-5)).toBe("");
    expect(fmtCountdown(65_000)).toBe("01:05");
    expect(fmtCountdown(3_725_000)).toBe("01:02:05");
    expect(fmtCountdown(90_000_000)).toBe("1d 01:00:00");
  });

  it("formats bytes and durations with em-dash fallbacks", () => {
    expect(fmtBytes(null)).toBe("—");
    expect(fmtBytes(130_000_000)).toBe("130 MB");
    expect(fmtBytes(1_300_000_000)).toBe("1.3 GB");
    expect(fmtDuration(null)).toBe("—");
    expect(fmtDuration(5_400_000)).toBe("1t 30m");
    expect(fmtDuration(45_000)).toBe("45s");
  });

  it("passes through an unparseable next string", () => {
    expect(fmtNext("not-a-date")).toBe("not-a-date");
  });
});

describe("HomePage", () => {
  it("shows the next-recording countdown when scheduled", async () => {
    routeInvoke({ status: { next: "2026-05-31T11:00:00", upcoming: [] } });
    renderHome();
    // 1h to start at 11:00 from 10:00.
    await waitFor(() =>
      expect(screen.getByText(/til oppstart/)).toBeInTheDocument(),
    );
    expect(screen.getByText("01:00:00")).toBeInTheDocument();
  });

  it("nudges toward the schedule when no slot is configured", async () => {
    routeInvoke({
      status: { next: null, upcoming: [] },
      settings: { slots: [], specialRecordings: [] } as unknown as Settings,
    });
    renderHome();
    expect(await screen.findByText(/sett opp en tidsplan/)).toBeInTheDocument();
  });

  it("shows 'all ready' when a schedule exists but no next start", async () => {
    routeInvoke({ status: { next: null, upcoming: [] } });
    renderHome();
    expect(await screen.findByText("Alt er klart")).toBeInTheDocument();
  });

  it("renders recent recordings with filename + size", async () => {
    renderHome();
    expect(await screen.findByText("2026-05-31.mp3")).toBeInTheDocument();
    expect(screen.getByText(/130 MB/)).toBeInTheDocument();
  });

  it("shows the history empty state when there are no recordings", async () => {
    routeInvoke({ recordings: [] });
    renderHome();
    expect(await screen.findByText("Ingen opptak ennå")).toBeInTheDocument();
  });

  it("surfaces the review-queue card for pending episodes", async () => {
    routeInvoke({ queue: [prep("ready"), prep("needs-attention")] });
    renderHome();
    expect(await screen.findByText("2 episoder klare")).toBeInTheDocument();
  });

  it("excludes already-published episodes from the review count", async () => {
    routeInvoke({ queue: [prep("ready"), prep("published")] });
    renderHome();
    expect(await screen.findByText("1 episoder klare")).toBeInTheDocument();
  });

  it("hides the review card when nothing is pending", async () => {
    routeInvoke({ queue: [prep("published")] });
    renderHome();
    await screen.findByText("2026-05-31.mp3");
    expect(screen.queryByText(/episoder klare/)).not.toBeInTheDocument();
  });

  it("navigates to history via 'see all'", async () => {
    const onNavigate = vi.fn();
    renderHome(onNavigate);
    await screen.findByText("2026-05-31.mp3");
    fireEvent.click(screen.getByText("Se alle →"));
    expect(onNavigate).toHaveBeenCalledWith("history");
  });

  it("navigates to schedule from the no-schedule nudge", async () => {
    const onNavigate = vi.fn();
    routeInvoke({
      status: { next: null, upcoming: [] },
      settings: { slots: [], specialRecordings: [] } as unknown as Settings,
    });
    renderHome(onNavigate);
    await screen.findByText(/sett opp en tidsplan/);
    fireEvent.click(screen.getByText("Tidsplan →"));
    expect(onNavigate).toHaveBeenCalledWith("schedule");
  });

  it("navigates to review from the queue card", async () => {
    const onNavigate = vi.fn();
    routeInvoke({ queue: [prep("ready")] });
    renderHome(onNavigate);
    await screen.findByText("1 episoder klare");
    fireEvent.click(screen.getByText("Åpne →"));
    expect(onNavigate).toHaveBeenCalledWith("review");
  });
});
