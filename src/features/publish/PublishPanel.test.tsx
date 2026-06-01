import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { PublishPanel } from "./PublishPanel";
import type { PublishStatus } from "@/lib/bindings/PublishStatus";
import type { FeedPreview } from "@/lib/bindings/FeedPreview";
import type { Settings } from "@/lib/bindings/Settings";
import i18n from "@/i18n";

// --- Tauri bridge mocks -----------------------------------------------------

const h = vi.hoisted(() => ({ invoke: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => h.invoke(...args),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => h.open(...args),
}));
const invoke = h.invoke;
const openDialog = h.open;

const PREVIEW: FeedPreview = {
  xml: '<?xml version="1.0"?><rss><channel><title>St Mary&apos;s</title></channel></rss>',
  episodeCount: 3,
  localPath: null,
  feedUrl: null,
};

/** A minimal Settings the mocked `settings_get` returns. Only the metadata
 *  fields the panel touches matter; the rest are cast through. */
const SETTINGS = {
  language: "no",
  churchName: "Domkirken",
  responsiblePerson: "Kari Nordmann",
} as unknown as Settings;

/** Route invoke() by command name. `featureBuilt` toggles the `publish` feature;
 *  `generate` lets a test override the generate behaviour (e.g. reject);
 *  `preview` overrides the preview payload (e.g. to carry a feedUrl). */
function routeInvoke(opts?: {
  featureBuilt?: boolean;
  episodeCount?: number;
  generate?: () => Promise<unknown>;
  preview?: FeedPreview;
}) {
  const status: PublishStatus = {
    featureBuilt: opts?.featureBuilt ?? false,
    episodeCount: opts?.episodeCount ?? 3,
  };
  invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "publish_feed_status":
        return Promise.resolve(status);
      case "publish_feed_preview":
        return Promise.resolve(opts?.preview ?? PREVIEW);
      case "publish_generate_feed":
        return opts?.generate
          ? opts.generate()
          : Promise.resolve({ ...PREVIEW, localPath: "/save/podcast.xml" });
      case "settings_get":
        return Promise.resolve(SETTINGS);
      case "settings_save":
        return Promise.resolve((args?.settings as Settings) ?? SETTINGS);
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
  openDialog.mockReset();
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

  // --- New metadata / image-picker / copy coverage --------------------------

  it("pre-fills title + author from existing settings", async () => {
    renderPanel();
    await waitFor(() =>
      expect((screen.getByLabelText("Tittel") as HTMLInputElement).value).toBe(
        "Domkirken",
      ),
    );
    expect((screen.getByLabelText("Forfatter") as HTMLInputElement).value).toBe(
      "Kari Nordmann",
    );
  });

  it("persists an edited title into churchName via settings_save", async () => {
    renderPanel();
    await waitFor(() =>
      expect((screen.getByLabelText("Tittel") as HTMLInputElement).value).toBe(
        "Domkirken",
      ),
    );
    fireEvent.change(screen.getByLabelText("Tittel"), {
      target: { value: "Storkirken" },
    });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("settings_save", {
        settings: expect.objectContaining({ churchName: "Storkirken" }),
      }),
    );
  });

  it("opens the native dialog and shows the picked artwork path", async () => {
    openDialog.mockResolvedValue("/disk/cover.png");
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    fireEvent.click(screen.getByRole("button", { name: "Velg bilde" }));
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(await screen.findByText("/disk/cover.png")).toBeInTheDocument();
    // Button flips to the "change" label once an image is chosen.
    expect(
      screen.getByRole("button", { name: "Bytt bilde" }),
    ).toBeInTheDocument();
  });

  it("copies the feed URL to the clipboard and flashes a confirmation", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    routeInvoke({
      preview: { ...PREVIEW, feedUrl: "https://example.com/feed.xml" },
    });
    renderPanel();
    await screen.findByText("3 opptak i feeden");
    // Preview first so the feed URL + copy button render.
    fireEvent.click(screen.getByText("Forhåndsvis feed"));
    const copyBtn = await screen.findByRole("button", {
      name: "Kopier RSS-URL",
    });
    fireEvent.click(copyBtn);
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("https://example.com/feed.xml"),
    );
    expect(
      await screen.findByRole("button", { name: "Kopiert ✓" }),
    ).toBeInTheDocument();
  });
});
