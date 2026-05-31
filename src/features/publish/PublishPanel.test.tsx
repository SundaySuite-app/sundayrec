import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { PublishPanel } from "./PublishPanel";
import type { PublishStatus } from "@/lib/bindings/PublishStatus";
import type { FeedPreview } from "@/lib/bindings/FeedPreview";
import i18n from "@/i18n";

// --- Tauri bridge mock ------------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
const invoke = h.invoke;

const PREVIEW: FeedPreview = {
  xml: '<?xml version="1.0"?><rss><channel><title>St Mary&apos;s</title></channel></rss>',
  episodeCount: 3,
  localPath: null,
  feedUrl: null,
};

/** Route invoke() by command name. `featureBuilt` toggles the `publish` feature;
 *  `generate` lets a test override the generate behaviour (e.g. reject). */
function routeInvoke(opts?: {
  featureBuilt?: boolean;
  episodeCount?: number;
  generate?: () => Promise<unknown>;
}) {
  const status: PublishStatus = {
    featureBuilt: opts?.featureBuilt ?? false,
    episodeCount: opts?.episodeCount ?? 3,
  };
  invoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "publish_feed_status":
        return Promise.resolve(status);
      case "publish_feed_preview":
        return Promise.resolve(PREVIEW);
      case "publish_generate_feed":
        return opts?.generate
          ? opts.generate()
          : Promise.resolve({ ...PREVIEW, localPath: "/save/podcast.xml" });
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
      <PublishPanel />
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

describe("PublishPanel", () => {
  it("shows the candidate episode count from status", async () => {
    renderPanel();
    expect(await screen.findByText("3 opptak i feeden")).toBeInTheDocument();
  });

  it("shows the feature-disabled hint in the default build", async () => {
    routeInvoke({ featureBuilt: false });
    renderPanel();
    expect(
      await screen.findByText(/ikke bygd inn i denne versjonen/),
    ).toBeInTheDocument();
  });

  it("hides the disabled hint when publishing is built", async () => {
    routeInvoke({ featureBuilt: true });
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    await waitFor(() =>
      expect(
        screen.queryByText(/ikke bygd inn i denne versjonen/),
      ).not.toBeInTheDocument(),
    );
  });

  it("previews the feed XML over IPC", async () => {
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    fireEvent.click(screen.getByText("Forhåndsvis feed"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("publish_feed_preview"),
    );
    expect(await screen.findByText(/St Mary/)).toBeInTheDocument();
  });

  it("generates the feed and shows the written path", async () => {
    routeInvoke({ featureBuilt: true });
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    fireEvent.click(screen.getByText("Generer feed nå"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("publish_generate_feed"),
    );
    expect(
      await screen.findByText("Skrevet til: /save/podcast.xml"),
    ).toBeInTheDocument();
  });

  it("shows a calm hint when generate is feature-disabled", async () => {
    routeInvoke({
      featureBuilt: false,
      generate: () =>
        Promise.reject(
          new Error(
            "feature_disabled: podcast publishing requires a build with `--features publish`",
          ),
        ),
    });
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    fireEvent.click(screen.getByText("Generer feed nå"));
    // The amber hint stays calm; no red error is shown.
    await waitFor(() =>
      expect(
        screen.queryByText("Klarte ikke generere feeden."),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.getByText(/ikke bygd inn i denne versjonen/),
    ).toBeInTheDocument();
  });

  it("shows a no-folder hint when generate is unconfigured", async () => {
    routeInvoke({
      featureBuilt: true,
      generate: () =>
        Promise.reject(new Error("no_config: save folder not set")),
    });
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    fireEvent.click(screen.getByText("Generer feed nå"));
    expect(
      await screen.findByText(
        "Velg en lagringsmappe i innstillingene først.",
      ),
    ).toBeInTheDocument();
  });
});
