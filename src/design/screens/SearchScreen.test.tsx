import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { SearchScreen } from "./SearchScreen";
import i18n from "@/i18n";
import type { TranscriptSidecar } from "@/features/search/searchIndex";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";

// One recording with a transcript sidecar whose single segment contains the
// word we search for ("nåden …"). Shapes match the RecordingRow + ts-rs
// TranscriptData / TranscriptSegment bindings the index consumes.
const RECORDINGS: RecordingRow[] = [
  {
    id: "1",
    file_path: "/x/pinse.mp4",
    device_name: null,
    started_at: 1716000000,
    duration_ms: 600000,
    byte_size: null,
    created_at: 1716000000,
    note: null,
  },
];

const SIDECARS: TranscriptSidecar[] = [
  {
    basePath: "/x/pinse",
    transcript: {
      version: 1,
      model: "m",
      language: "no",
      duration: 600,
      createdAt: 1716000000,
      translated: null,
      segments: [
        { start: 134, end: 138, text: "og nåden bærer oss gjennom alt" },
      ],
    },
  },
];

// Mock the Tauri IPC bridge — there is no backend in the jsdom test runner.
const invokeMock = vi.fn(
  async (cmd: string, _args?: unknown): Promise<unknown> => {
    if (cmd === "recordings_list") return RECORDINGS;
    if (cmd === "transcripts_list") return SIDECARS;
    if (cmd === "open_in_sundayedit") return null;
    return null;
  },
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

function renderSearch() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SearchScreen />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  invokeMock.mockClear();
  i18n.changeLanguage("no");
});

describe("SearchScreen", () => {
  it("renders the page title, the search input and the reindex button", () => {
    renderSearch();
    expect(screen.getByText("Søk i prekener")).toBeInTheDocument();
    // The search box exposes the title as its aria-label.
    expect(
      screen.getByRole("searchbox", { name: "Søk i prekener" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Oppdater indeks/ }),
    ).toBeInTheDocument();
  });

  it("finds the matching transcript text and counts the hit", async () => {
    renderSearch();

    // Wait for the transcripts_list query feeding the index to resolve.
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("transcripts_list", undefined),
    );

    const input = screen.getByRole("searchbox", { name: "Søk i prekener" });
    fireEvent.change(input, { target: { value: "nåde" } });

    // The hit card surfaces the matched segment text (with a gold <mark> around
    // the matched term, so assert on stable surrounding substrings).
    await waitFor(() =>
      expect(screen.getByText(/bærer oss gjennom alt/)).toBeInTheDocument(),
    );

    // The count line reflects at least one hit in one recording.
    await waitFor(() =>
      expect(screen.getByText(/1 treff i 1 opptak/)).toBeInTheDocument(),
    );
  });

  it("re-fetches the transcript index when Oppdater indeks is clicked", async () => {
    renderSearch();

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("transcripts_list", undefined),
    );

    const callsBefore = invokeMock.mock.calls.filter(
      (c) => c[0] === "transcripts_list",
    ).length;

    fireEvent.click(screen.getByRole("button", { name: /Oppdater indeks/ }));

    // Invalidating the query triggers a fresh transcripts_list invoke.
    await waitFor(() => {
      const callsAfter = invokeMock.mock.calls.filter(
        (c) => c[0] === "transcripts_list",
      ).length;
      expect(callsAfter).toBeGreaterThan(callsBefore);
    });
  });
});
