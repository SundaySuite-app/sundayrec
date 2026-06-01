import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { PublishStatus } from "@/lib/bindings/PublishStatus";
import type { FeedPreview } from "@/lib/bindings/FeedPreview";
import type { Settings } from "@/lib/bindings/Settings";
import { SETTINGS_QUERY_KEY } from "@/features/settings/queryKey";
import { PUBLISH_STATUS_KEY } from "./queryKey";

/** True when an IPC rejection is the default-build "publish feature off" error,
 *  so the panel shows a calm hint rather than a red error. The seam returns
 *  `feature_disabled: …` in the message of a `validation` AppError. */
function isFeatureDisabled(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? String(err);
  return msg.includes("feature_disabled");
}

/** True when the rejection is the "no save folder" guard — a different hint. */
function isNoConfig(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? String(err);
  return msg.includes("no_config");
}

/** How long the "Kopiert ✓" confirmation stays visible. */
const COPY_FLASH_MS = 1500;

/**
 * PU-3 podcast-publish / RSS panel. Generates + previews the podcast RSS feed,
 * and (toward the Electron "Publisering" screen) edits the podcast metadata,
 * picks episode artwork, and copies the public feed URL.
 *
 * `publish_feed_status` (works in every build) reports whether this build can
 * write/upload the feed (the default-off `publish` cargo feature) plus the
 * candidate episode count. "Preview feed" renders the feed XML in memory from
 * the recording history + the channel metadata from settings
 * (`publish_feed_preview`, pure shaping — available everywhere). "Generate feed"
 * (`publish_generate_feed`) is the impure write behind the `publish` feature;
 * in the default build it returns `feature_disabled` and the panel shows a calm
 * "not built into this build" hint rather than a dead button.
 *
 * Metadata: the podcast TITLE maps to the existing `churchName` setting and the
 * AUTHOR to `responsiblePerson` (persisted via the shared `settings_save`
 * debounce when settings are loaded). DESCRIPTION and the episode IMAGE have no
 * dedicated Settings field yet (adding one would drift the ts-rs bindings), so
 * they live in local state and are NOT-YET-PERSISTED.
 *
 * Pure IPC + render; exercised in tests with `invoke` + the dialog plugin mocked.
 */
export function PublishPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const status = useQuery<PublishStatus>({
    queryKey: PUBLISH_STATUS_KEY,
    queryFn: () => invoke<PublishStatus>("publish_feed_status"),
  });

  // Read existing settings so we can pre-fill + persist title/author.
  const settings = useQuery<Settings>({
    queryKey: SETTINGS_QUERY_KEY,
    queryFn: () => invoke<Settings>("settings_get"),
  });

  const [preview, setPreview] = useState<FeedPreview | null>(null);
  const [disabled, setDisabled] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Local NOT-YET-PERSISTED metadata (no Settings field exists for these and we
  // must not add one — that would drift the generated ts-rs bindings).
  const [description, setDescription] = useState("");
  const [imagePath, setImagePath] = useState<string | null>(null);

  // Save settings (mirrors SettingsPage: keep the cache canonical).
  const saveSettings = useMutation({
    mutationFn: (next: Settings) =>
      invoke<Settings>("settings_save", { settings: next }),
    onSuccess: (saved) => {
      queryClient.setQueryData(SETTINGS_QUERY_KEY, saved);
    },
  });

  const previewMutation = useMutation({
    mutationFn: () => invoke<FeedPreview>("publish_feed_preview"),
    onSuccess: (p) => setPreview(p),
  });

  const generateMutation = useMutation({
    mutationFn: () => invoke<FeedPreview>("publish_generate_feed"),
    onSuccess: (p) => {
      setDisabled(false);
      setPreview(p);
    },
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });

  const onPreview = useCallback(() => {
    setDisabled(false);
    previewMutation.mutate();
  }, [previewMutation]);

  const onGenerate = useCallback(() => {
    setDisabled(false);
    generateMutation.mutate();
  }, [generateMutation]);

  // Persist a metadata field that maps onto a real Settings column. No-op until
  // settings have loaded (we never patch a half-built bag).
  const patchSetting = useCallback(
    (partial: Partial<Settings>) => {
      const current = settings.data;
      if (!current) return;
      saveSettings.mutate({ ...current, ...partial });
    },
    [settings.data, saveSettings],
  );

  // Native artwork picker. The chosen path is local-only (NOT-YET-PERSISTED).
  const pickImage = useCallback(async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Bilde", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (typeof picked === "string") setImagePath(picked);
  }, []);

  // Copy the public feed URL (or local path fallback) to the clipboard.
  // `@tauri-apps/plugin-clipboard-manager` is not a dependency, so use the web
  // clipboard API (available in the Tauri webview).
  const feedTarget = preview?.feedUrl ?? preview?.localPath ?? null;
  const copyFeedUrl = useCallback(async () => {
    if (!feedTarget) return;
    try {
      await navigator.clipboard.writeText(feedTarget);
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), COPY_FLASH_MS);
    } catch {
      // Clipboard can reject (permissions / no webview); fail quietly.
    }
  }, [feedTarget]);

  useEffect(() => {
    return () => {
      if (copyTimer.current) clearTimeout(copyTimer.current);
    };
  }, []);

  const featureBuilt = status.data?.featureBuilt ?? false;
  const episodeCount = status.data?.episodeCount ?? 0;

  const title = settings.data?.churchName ?? "";
  const author = settings.data?.responsiblePerson ?? "";
  const metaReady = !!settings.data;

  return (
    <section
      className="sr-card pad flex w-full max-w-md flex-col gap-4"
      aria-label={t("publish.title", "Publisering")}
    >
      <p className="text-xs text-text2">
        {t(
          "publish.podcastIntro",
          "Genererer en RSS-feed automatisk etter hvert opptak. Send feed-URL-en én gang til Spotify for Podcasters og Apple Podcasts Connect — nye gudstjenester dukker opp av seg selv.",
        )}
      </p>

      {(disabled || (status.data && !featureBuilt)) && (
        <p
          className="rounded-lg p-3 text-sm"
          style={{
            background: "var(--sr-gold-tint-2)",
            color: "var(--sr-gold-bright)",
            border: "1px solid var(--sr-gold-line)",
          }}
        >
          {t(
            "publish.featureDisabled",
            "Publisering til disk/sky er ikke bygd inn i denne versjonen. Du kan likevel forhåndsvise feeden.",
          )}
        </p>
      )}

      {/* ── Status ──────────────────────────────────────────────────── */}
      <p className="text-sm text-text2">
        {t("publish.candidateCount", "{{n}} opptak i feeden", {
          n: episodeCount,
        })}
      </p>

      {/* ── Podcast metadata ────────────────────────────────────────── */}
      <fieldset className="flex flex-col gap-3 rounded-xl border border-border bg-surface p-4">
        <legend className="px-1 text-xs font-medium text-text2">
          {t("publish.metadata", "Podkast-info")}
        </legend>

        <label className="flex flex-col gap-1 text-xs text-text2">
          {t("publish.podcastTitle", "Tittel")}
          <input
            type="text"
            value={title}
            disabled={!metaReady}
            placeholder={t("publish.podcastTitlePh", "Navn på menigheten")}
            className="sr-input disabled:opacity-50"
            onChange={(e) => patchSetting({ churchName: e.target.value })}
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-text2">
          {t("publish.podcastAuthor", "Forfatter")}
          <input
            type="text"
            value={author}
            disabled={!metaReady}
            placeholder={t("publish.podcastAuthorPh", "Ansvarlig person")}
            className="sr-input disabled:opacity-50"
            onChange={(e) =>
              patchSetting({ responsiblePerson: e.target.value })
            }
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-text2">
          {t("publish.podcastDescription", "Beskrivelse")}
          <textarea
            value={description}
            rows={2}
            placeholder={t(
              "publish.podcastDescriptionPh",
              "Kort beskrivelse av podkasten",
            )}
            className="sr-input resize-none"
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>

        {/* Episode artwork — local only (NOT-YET-PERSISTED). */}
        <div className="flex flex-col gap-1 text-xs text-text2">
          {t("publish.episodeImage", "Episodebilde")}
          <div className="flex items-center gap-2">
            {imagePath && (
              <img
                src={`file://${imagePath}`}
                alt={t("publish.episodeImageAlt", "Episodebilde")}
                className="h-12 w-12 rounded-lg border border-border object-cover"
              />
            )}
            <button
              type="button"
              className="sr-btn ghost sm"
              onClick={() => void pickImage()}
            >
              {imagePath
                ? t("publish.changeImage", "Bytt bilde")
                : t("publish.pickImage", "Velg bilde")}
            </button>
          </div>
          {imagePath && <p className="break-all text-text3">{imagePath}</p>}
        </div>
      </fieldset>

      {/* ── Actions ─────────────────────────────────────────────────── */}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={previewMutation.isPending}
          className="sr-btn ghost sm disabled:opacity-50"
          onClick={onPreview}
        >
          {t("publish.previewFeed", "Forhåndsvis feed")}
        </button>
        <button
          type="button"
          disabled={generateMutation.isPending}
          className="sr-btn gold sm disabled:opacity-50"
          onClick={onGenerate}
        >
          {t("publish.regenerateFeed", "Generer feed nå")}
        </button>
      </div>

      {generateMutation.isError && !disabled && (
        <p
          className="text-xs"
          style={{ color: "var(--sr-red-bright)" }}
          role="alert"
        >
          {isNoConfig(generateMutation.error)
            ? t(
                "publish.noFolder",
                "Velg en lagringsmappe i innstillingene først.",
              )
            : t("publish.generateFailed", "Klarte ikke generere feeden.")}
        </p>
      )}

      {preview && (
        <div className="sr-card pad flex flex-col gap-2">
          <p className="text-xs text-text2">
            {preview.localPath
              ? t("publish.writtenTo", "Skrevet til: {{path}}", {
                  path: preview.localPath,
                })
              : t("publish.previewTitle", "Forhåndsvisning ({{n}} episoder)", {
                  n: preview.episodeCount,
                })}
          </p>
          {preview.feedUrl && (
            <p
              className="break-all text-xs"
              style={{ color: "var(--sr-green)" }}
            >
              {t("publish.feedUrlLabel", "FEED-URL")}: {preview.feedUrl}
            </p>
          )}
          {feedTarget && (
            <button
              type="button"
              className="sr-btn ghost sm self-start"
              onClick={() => void copyFeedUrl()}
            >
              {copied
                ? t("publish.copied", "Kopiert ✓")
                : t("publish.copyFeedUrl", "Kopier RSS-URL")}
            </button>
          )}
          <pre
            className="max-h-64 overflow-auto rounded-lg border border-border bg-surface2 p-3 text-left text-[11px] leading-tight text-text"
            aria-label={t("publish.feedXml", "Feed-XML")}
          >
            {preview.xml}
          </pre>
        </div>
      )}
    </section>
  );
}
