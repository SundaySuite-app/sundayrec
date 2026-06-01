import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { PublishStatus } from "@/lib/bindings/PublishStatus";
import type { FeedPreview } from "@/lib/bindings/FeedPreview";
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

/**
 * PU-3 podcast-publish / RSS panel. Generates + previews the podcast RSS feed.
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
 * Pure IPC + render; exercised in tests with `invoke` mocked.
 */
export function PublishPanel() {
  const { t } = useTranslation();

  const status = useQuery<PublishStatus>({
    queryKey: PUBLISH_STATUS_KEY,
    queryFn: () => invoke<PublishStatus>("publish_feed_status"),
  });

  const [preview, setPreview] = useState<FeedPreview | null>(null);
  const [disabled, setDisabled] = useState(false);

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

  const featureBuilt = status.data?.featureBuilt ?? false;
  const episodeCount = status.data?.episodeCount ?? 0;

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("publish.title", "Publisering")}
    >
      <p className="text-xs text-text2">
        {t(
          "publish.podcastIntro",
          "Genererer en RSS-feed automatisk etter hvert opptak. Send feed-URL-en én gang til Spotify for Podcasters og Apple Podcasts Connect — nye gudstjenester dukker opp av seg selv.",
        )}
      </p>

      {(disabled || (status.data && !featureBuilt)) && (
        <p className="rounded-lg border border-accent/60 bg-accent p-3 text-sm text-bg">
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

      {/* ── Actions ─────────────────────────────────────────────────── */}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={previewMutation.isPending}
          className="rounded-lg border border-border bg-surface2 px-3 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
          onClick={onPreview}
        >
          {t("publish.previewFeed", "Forhåndsvis feed")}
        </button>
        <button
          type="button"
          disabled={generateMutation.isPending}
          className="rounded-lg bg-accent px-3 py-2 text-xs font-medium text-bg hover:bg-accent/90 disabled:opacity-50"
          onClick={onGenerate}
        >
          {t("publish.regenerateFeed", "Generer feed nå")}
        </button>
      </div>

      {generateMutation.isError && !disabled && (
        <p className="text-xs text-red-400" role="alert">
          {isNoConfig(generateMutation.error)
            ? t("publish.noFolder", "Velg en lagringsmappe i innstillingene først.")
            : t("publish.generateFailed", "Klarte ikke generere feeden.")}
        </p>
      )}

      {preview && (
        <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-4">
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
            <p className="break-all text-xs text-emerald-300">
              {t("publish.feedUrlLabel", "FEED-URL")}: {preview.feedUrl}
            </p>
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
