import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { CloudConnectionStatus } from "@/lib/bindings/CloudConnectionStatus";
import type { CloudService } from "@/lib/bindings/CloudService";
import type { QueueEntryView } from "@/lib/bindings/QueueEntryView";
import type { UploadStatus } from "@/lib/bindings/UploadStatus";
import { CLOUD_CONNECTION_KEY, CLOUD_QUEUE_KEY } from "./queryKey";

/** Human label for each Google service. */
const SERVICE_LABEL: Record<CloudService, string> = {
  "google-drive": "Google Drive",
  youtube: "YouTube",
  gmail: "Gmail",
};

/** Tailwind classes for each upload-status badge. */
const STATUS_BADGE: Record<UploadStatus, string> = {
  pending: "border-border text-text2",
  uploading: "border-sky-700 text-sky-300",
  failed: "border-red-800 text-red-300",
  "reauth-required": "border-accent/60 text-accent",
};

/**
 * Optional byte fields the queue may carry. `QueueEntryView` does not type
 * these today (the durable queue has no size/progress columns yet), so we read
 * them defensively from the runtime payload rather than adding a binding. When
 * absent the size + progress UI simply does not render. FRONTEND-ONLY.
 */
interface QueueByteFields {
  byteSize?: number | null;
  uploadedBytes?: number | null;
  totalBytes?: number | null;
}

/** A queue entry possibly augmented with byte/progress fields. */
type QueueEntryWithBytes = QueueEntryView & QueueByteFields;

/**
 * Human-readable byte size: >=1e9 → "x.x GB", >=1e6 → "x MB", else "x KB".
 * Sub-kilobyte values round up to 1 KB so a non-empty file never reads "0 KB".
 */
export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  return `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}

/** Best-known size for an entry: explicit total, else byteSize, else null. */
function entrySize(e: QueueEntryWithBytes): number | null {
  if (typeof e.totalBytes === "number" && e.totalBytes > 0) return e.totalBytes;
  if (typeof e.byteSize === "number" && e.byteSize > 0) return e.byteSize;
  return null;
}

/**
 * Fase 6 cloud-backup panel. Shows which Google services are connected
 * (`cloud_connection_status`) with a disconnect action, and the durable upload
 * queue (`cloud_queue_status`) with a summary header (total items, pending
 * size, failed count), per-entry size + progress, and retry/remove plus a
 * "clear failed" sweep. The OAuth connect flow and the upload worker (network
 * I/O) are a separate, deferred step — see `docs/PHASE6.md`.
 *
 * Pure IPC + render; exercised in tests with `invoke` mocked.
 */
export function CloudBackupPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const conn = useQuery<CloudConnectionStatus[]>({
    queryKey: CLOUD_CONNECTION_KEY,
    queryFn: () => invoke<CloudConnectionStatus[]>("cloud_connection_status"),
  });

  const queue = useQuery<QueueEntryWithBytes[]>({
    queryKey: CLOUD_QUEUE_KEY,
    queryFn: () => invoke<QueueEntryWithBytes[]>("cloud_queue_status"),
  });

  const invalidate = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: CLOUD_CONNECTION_KEY });
    void queryClient.invalidateQueries({ queryKey: CLOUD_QUEUE_KEY });
  }, [queryClient]);

  const connectMutation = useMutation({
    // Opens the system browser for the OAuth loopback flow (resolves once the
    // refresh token is stored). NETWORK/HARDWARE-UNVERIFIED.
    mutationFn: (service: CloudService) =>
      invoke<void>("cloud_connect", { service }),
    onSuccess: invalidate,
  });
  const disconnectMutation = useMutation({
    mutationFn: (service: CloudService) =>
      invoke<void>("cloud_disconnect", { service }),
    onSuccess: invalidate,
  });
  const retryMutation = useMutation({
    mutationFn: (id: string) => invoke<void>("cloud_retry_upload", { id }),
    onSuccess: invalidate,
  });
  const removeMutation = useMutation({
    mutationFn: (id: string) => invoke<void>("cloud_remove_upload", { id }),
    onSuccess: invalidate,
  });
  const clearFailedMutation = useMutation({
    mutationFn: () => invoke<number>("cloud_clear_failed"),
    onSuccess: invalidate,
  });

  const onDisconnect = useCallback(
    (service: CloudService) => {
      if (
        window.confirm(
          t(
            "cloud.confirmDisconnect",
            "Koble fra denne tjenesten og fjerne dens køede opplastinger?",
          ),
        )
      ) {
        disconnectMutation.mutate(service);
      }
    },
    [disconnectMutation, t],
  );

  const statusLabel = useCallback(
    (status: UploadStatus): string => {
      switch (status) {
        case "pending":
          return t("cloud.statusPending", "Venter");
        case "uploading":
          return t("cloud.statusUploading", "Laster opp");
        case "failed":
          return t("cloud.statusFailed", "Feilet");
        case "reauth-required":
          return t("cloud.statusReauth", "Krever ny innlogging");
      }
    },
    [t],
  );

  const statuses = conn.data ?? [];
  const entries = queue.data ?? [];
  const hasFailed = entries.some((e) => e.status === "failed");

  // ── Summary ──────────────────────────────────────────────────────────
  const totalItems = entries.length;
  const failedCount = entries.filter((e) => e.status === "failed").length;
  const pendingBytes = entries
    .filter((e) => e.status === "pending" || e.status === "uploading")
    .reduce((sum, e) => sum + (entrySize(e) ?? 0), 0);

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("cloud.title", "Sky-backup")}
    >
      {/* ── Connections ─────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-6">
        <h2 className="text-sm font-medium text-text">
          {t("cloud.connectionsTitle", "Tilkoblinger")}
        </h2>
        {conn.isError ? (
          <p className="text-red-400">
            {t("cloud.connError", "Kunne ikke lese tilkoblingsstatus")}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {statuses.map((s) => (
              <li
                key={s.service}
                className="flex items-center justify-between gap-3 rounded-lg border border-border bg-surface2 p-3"
              >
                <div className="flex items-center gap-2">
                  <span className="font-medium text-text">
                    {SERVICE_LABEL[s.service]}
                  </span>
                  <span
                    className={`rounded border px-1.5 py-0.5 text-xs ${
                      s.connected
                        ? "border-emerald-700 text-emerald-300"
                        : "border-border text-text3"
                    }`}
                  >
                    {s.connected
                      ? t("cloud.connected", "Tilkoblet")
                      : t("cloud.disconnected", "Ikke tilkoblet")}
                  </span>
                </div>
                {s.connected ? (
                  <button
                    type="button"
                    className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                    onClick={() => onDisconnect(s.service)}
                  >
                    {t("cloud.disconnect", "Koble fra")}
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={connectMutation.isPending}
                    className="rounded-lg bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent/90 disabled:opacity-50"
                    onClick={() => connectMutation.mutate(s.service)}
                  >
                    {t("cloud.connect", "Koble til")}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* ── Upload queue ────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-6">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium text-text">
            {t("cloud.queueTitle", "Opplastingskø")}
          </h2>
          {hasFailed && (
            <button
              type="button"
              className="rounded-lg border border-red-800 px-2 py-1 text-xs text-red-300 hover:bg-red-950"
              onClick={() => clearFailedMutation.mutate()}
            >
              {t("cloud.clearFailed", "Fjern feilede")}
            </button>
          )}
        </div>

        {/* Summary header: total items, pending size, failed count. */}
        {entries.length > 0 && (
          <div
            className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-text2"
            aria-label={t("cloud.summary", "Køoversikt")}
          >
            <span>
              {t("cloud.summaryItems", "{{n}} i kø", { n: totalItems })}
            </span>
            {pendingBytes > 0 && (
              <>
                <span aria-hidden>·</span>
                <span>
                  {t("cloud.summaryPending", "{{size}} venter", {
                    size: formatBytes(pendingBytes),
                  })}
                </span>
              </>
            )}
            {failedCount > 0 && (
              <>
                <span aria-hidden>·</span>
                <span className="text-red-400">
                  {t("cloud.summaryFailed", "{{n}} feilet", { n: failedCount })}
                </span>
              </>
            )}
          </div>
        )}

        {entries.length === 0 ? (
          <p className="text-text3">
            {t("cloud.queueEmpty", "Ingen køede opplastinger")}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {entries.map((e) => {
              const isFailed = e.status === "failed";
              const size = entrySize(e);
              const total =
                typeof e.totalBytes === "number" && e.totalBytes > 0
                  ? e.totalBytes
                  : null;
              const uploaded =
                typeof e.uploadedBytes === "number" && e.uploadedBytes >= 0
                  ? e.uploadedBytes
                  : null;
              const pct =
                total !== null && uploaded !== null
                  ? Math.max(0, Math.min(100, Math.round((uploaded / total) * 100)))
                  : null;
              return (
                <li
                  key={e.id}
                  className={`flex flex-col gap-1 rounded-lg border bg-surface2 p-3 text-left ${
                    isFailed ? "border-red-800" : "border-border"
                  }`}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <p
                        className="truncate font-medium text-text"
                        title={e.filename}
                      >
                        {e.filename}
                      </p>
                      <p className="text-xs text-text2">
                        {SERVICE_LABEL[e.service]} ·{" "}
                        {t("cloud.attempts", "{{n}} forsøk", { n: e.attempts })}
                        {size !== null && (
                          <>
                            {" · "}
                            <span className="text-text3">
                              {formatBytes(size)}
                            </span>
                          </>
                        )}
                      </p>
                    </div>
                    <span
                      className={`shrink-0 rounded border px-1.5 py-0.5 text-xs ${STATUS_BADGE[e.status]}`}
                    >
                      {statusLabel(e.status)}
                    </span>
                  </div>

                  {/* Progress bar when uploaded/total are both known. */}
                  {pct !== null && (
                    <div
                      className="mt-0.5 h-1.5 w-full overflow-hidden rounded-full bg-surface3"
                      role="progressbar"
                      aria-valuenow={pct}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-label={t("cloud.uploadProgress", "Opplastingsframgang")}
                    >
                      <div
                        className="h-full rounded-full bg-accent transition-[width]"
                        style={{ width: `${pct}%` }}
                      />
                    </div>
                  )}

                  {e.lastError && (
                    <p
                      className="truncate text-xs text-red-400"
                      title={e.lastError}
                    >
                      {e.lastError}
                    </p>
                  )}
                  <div className="flex gap-2 self-end">
                    {(e.status === "failed" ||
                      e.status === "reauth-required") && (
                      <button
                        type="button"
                        className="rounded-lg border border-red-800 bg-surface2 px-2 py-1 text-xs text-red-300 hover:bg-red-950"
                        onClick={() => retryMutation.mutate(e.id)}
                      >
                        {t("cloud.retry", "Prøv igjen")}
                      </button>
                    )}
                    <button
                      type="button"
                      className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                      onClick={() => removeMutation.mutate(e.id)}
                    >
                      {t("cloud.remove", "Fjern")}
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}
