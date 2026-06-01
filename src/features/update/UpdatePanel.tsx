import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { UpdateStatus } from "@/lib/bindings/UpdateStatus";
import { UPDATE_STATUS_KEY } from "./queryKey";

/** The `{ code, message }` shape an `AppError` serialises to over IPC. */
type IpcError = { code?: string; message?: string };

/** True when a command failed because the `updater` cargo feature is off — the
 *  default build returns `feature_disabled` so we show a calm hint, not a red
 *  error (mirrors the CloudBackupPanel / streaming "not built in" idiom). */
function isFeatureDisabled(err: unknown): boolean {
  const message = (err as IpcError)?.message ?? String(err ?? "");
  return message.includes("feature_disabled");
}

/**
 * R7 auto-update panel. Polls the live `update_status` and offers the
 * Electron-parity flow: "Se etter oppdateringer" → (if newer) "Last ned" →
 * (when ready) "Start på nytt og installer". The check/download/install path is
 * behind the default-off `updater` cargo feature; in the default build the
 * commands return `feature_disabled`, so the panel shows a "not built into this
 * build" hint instead of a dead button (the status still reads as `idle`).
 *
 * The release-notes block renders the installed version (`getVersion()`) next
 * to the available version straight from the `update_status` payload — no new
 * IPC is added. Download progress is driven by the `downloading.percent` field
 * the status query already carries (the recorder-style event feed feeds the
 * same query), so no extra `listen` is needed.
 *
 * Pure IPC + render; exercised in tests with `invoke` mocked. The actual feed
 * fetch / signature verify / install are NETWORK/GUI-UNVERIFIED (need a signed
 * release — see docs/NEEDS-RICHARD.md).
 */
export function UpdatePanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const status = useQuery<UpdateStatus>({
    queryKey: UPDATE_STATUS_KEY,
    queryFn: () => invoke<UpdateStatus>("update_status"),
  });

  // The installed version, for the "current vs available" release-notes block.
  const installed = useQuery<string>({
    queryKey: ["update", "installedVersion"],
    queryFn: () => getVersion(),
    staleTime: Infinity,
  });

  const setStatus = useCallback(
    (next: UpdateStatus) => queryClient.setQueryData(UPDATE_STATUS_KEY, next),
    [queryClient],
  );

  const checkMutation = useMutation({
    mutationFn: () => invoke<UpdateStatus>("update_check"),
    onSuccess: setStatus,
  });
  const downloadMutation = useMutation({
    mutationFn: () => invoke<UpdateStatus>("update_download_install"),
    onSuccess: setStatus,
  });
  const relaunchMutation = useMutation({
    mutationFn: () => invoke<void>("update_relaunch"),
  });

  // The combined "is the feature missing" flag from either action's last error.
  const featureMissing =
    isFeatureDisabled(checkMutation.error) ||
    isFeatureDisabled(downloadMutation.error);

  const phase = status.data?.phase ?? "idle";
  const version =
    status.data && "version" in status.data ? status.data.version : undefined;
  const percent =
    status.data && status.data.phase === "downloading"
      ? status.data.percent
      : 0;
  const errorMessage =
    status.data && status.data.phase === "error"
      ? status.data.message
      : undefined;

  // The human line for the current phase (Electron `update.*` catalog keys).
  const statusLine = useCallback((): string => {
    switch (phase) {
      case "idle":
        return t(
          "update.checkHint",
          "Klikk «Se etter oppdateringer» for å sjekke",
        );
      case "checking":
        return t("update.checking", "Sjekker etter oppdateringer…");
      case "upToDate":
        return t("update.upToDate", "Du er oppdatert");
      case "available":
        return t(
          "update.available",
          "Ny versjon {v} er tilgjengelig — laster ned…",
        ).replace("{v}", version ?? "");
      case "downloading":
        return t("update.downloading", "Laster ned… {pct}%").replace(
          "{pct}",
          String(percent),
        );
      case "readyToInstall":
        return t(
          "update.readyInstall",
          "Versjon {v} er klar — start på nytt for å installere",
        ).replace("{v}", version ?? "");
      case "error":
        return (
          errorMessage ??
          t("update.error", "Kunne ikke sjekke for oppdateringer")
        );
    }
  }, [phase, version, percent, errorMessage, t]);

  const checking = phase === "checking" || checkMutation.isPending;
  const downloading = phase === "downloading" || downloadMutation.isPending;

  // A small per-phase status dot to make the lifecycle legible at a glance.
  const dotClass: Record<UpdateStatus["phase"], string> = {
    idle: "",
    checking: "animate-pulse",
    upToDate: "",
    available: "",
    downloading: "animate-pulse",
    readyToInstall: "",
    error: "",
  };
  const dotColor: Record<UpdateStatus["phase"], string> = {
    idle: "var(--sr-text-3)",
    checking: "var(--sr-gold)",
    upToDate: "var(--sr-green)",
    available: "var(--sr-gold)",
    downloading: "var(--sr-gold)",
    readyToInstall: "var(--sr-green)",
    error: "var(--sr-red)",
  };

  const installedLabel = installed.data ?? "—";
  // Whether to render the "current vs available" comparison row.
  const showComparison =
    phase === "available" ||
    phase === "downloading" ||
    phase === "readyToInstall";

  return (
    <section
      className="sr-card pad flex w-full max-w-md flex-col gap-3"
      aria-label={t("general.updates", "Oppdateringer")}
    >
      <div className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className={`h-2.5 w-2.5 shrink-0 rounded-full ${dotClass[phase]}`}
          style={{ background: dotColor[phase] }}
        />
        <p className="text-sm text-text2">{statusLine()}</p>
      </div>

      {/* Release-notes / version block. When an update is in play we show the
          installed → available comparison; otherwise just the installed line
          with an up-to-date / available summary. */}
      <div className="rounded-lg border border-border bg-surface2 p-3 text-sm sr-num">
        <div className="flex items-baseline justify-between gap-3">
          <span className="text-text3">
            {t("update.installedVersion", "Installert versjon")}
          </span>
          <span className="font-medium text-text tabular-nums">
            {installedLabel}
          </span>
        </div>

        {showComparison ? (
          <div className="mt-2 flex items-baseline justify-between gap-3 border-t border-border pt-2">
            <span className="text-text3">
              {t("update.newVersion", "Ny versjon")}
            </span>
            <span className="font-medium text-accent tabular-nums">
              {version ?? "—"}
            </span>
          </div>
        ) : (
          <p className="mt-2 text-xs text-text3">
            {phase === "upToDate"
              ? t("update.youHaveLatest", "Du har nyeste versjon")
              : t(
                  "update.checkHintShort",
                  "Se etter oppdateringer for å sammenligne med siste utgivelse.",
                )}
          </p>
        )}
      </div>

      {downloading && (
        <div
          className="h-2 w-full overflow-hidden rounded-full bg-surface3"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          aria-label={t("update.downloadProgress", "Nedlastingsfremdrift")}
        >
          <div
            className="h-full bg-accent transition-all"
            style={{ width: `${percent}%` }}
          />
        </div>
      )}

      {featureMissing && (
        <p className="rounded-lg border border-border bg-surface2 p-2 text-xs text-text3">
          {t(
            "update.notBuilt",
            "Automatisk oppdatering er ikke bygget inn i denne versjonen.",
          )}
        </p>
      )}

      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={checking}
          className="sr-btn ghost disabled:opacity-50"
          onClick={() => checkMutation.mutate()}
        >
          {checking
            ? t("update.checking", "Sjekker etter oppdateringer…")
            : t("general.checkNow", "Se etter oppdateringer")}
        </button>

        {phase === "available" && (
          <button
            type="button"
            disabled={downloading}
            className="sr-btn gold disabled:opacity-50"
            onClick={() => downloadMutation.mutate()}
          >
            {t("update.downloadNow", "Last ned og installer")}
          </button>
        )}

        {phase === "readyToInstall" && (
          <button
            type="button"
            className="sr-btn"
            style={{
              color: "var(--sr-green)",
              borderColor: "var(--sr-green)",
            }}
            onClick={() => relaunchMutation.mutate()}
          >
            {t("update.restartInstall", "↺ Start på nytt og installer")}
          </button>
        )}
      </div>
    </section>
  );
}
