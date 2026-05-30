import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { FfmpegHealth } from "./FfmpegHealth";
import type { FfmpegHealth as FfmpegHealthInfo } from "@/lib/bindings/FfmpegHealth";

const h = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));

function renderHealth() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <FfmpegHealth />
    </QueryClientProvider>,
  );
}

describe("FfmpegHealth", () => {
  it("shows the version banner when ffmpeg is available", async () => {
    const info: FfmpegHealthInfo = {
      available: true,
      version: "ffmpeg version 6.0",
      path: "/app/ffmpeg",
    };
    h.invoke.mockResolvedValueOnce(info);

    renderHealth();
    await waitFor(() =>
      expect(
        screen.getByText(/ffmpeg version 6\.0/, { exact: false }),
      ).toBeInTheDocument(),
    );
    expect(h.invoke).toHaveBeenCalledWith("ffmpeg_health");
  });

  it("shows not-found when ffmpeg is unavailable", async () => {
    const info: FfmpegHealthInfo = {
      available: false,
      version: null,
      path: "ffmpeg",
    };
    h.invoke.mockResolvedValueOnce(info);

    renderHealth();
    await waitFor(() =>
      expect(screen.getByText(/ikke funnet/)).toBeInTheDocument(),
    );
  });

  it("shows a failure message when the command rejects", async () => {
    h.invoke.mockRejectedValueOnce(new Error("bridge down"));

    renderHealth();
    await waitFor(() =>
      expect(screen.getByText(/helsesjekk feilet/)).toBeInTheDocument(),
    );
  });
});
