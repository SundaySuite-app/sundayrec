import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { ScheduleStatus } from "@/lib/bindings/ScheduleStatus";
import type { Settings } from "@/lib/bindings/Settings";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";
import type { ReviewQueueEntry } from "@/lib/bindings/ReviewQueueEntry";
import { SETTINGS_QUERY_KEY } from "@/features/settings/queryKey";
import { REVIEW_QUEUE_KEY } from "@/features/review/queryKey";
import { VuMeter } from "@/features/vu/VuMeter";
import type { ViewName } from "@/lib/routing";

const SCHEDULE_STATUS_KEY = ["scheduler_status"] as const;
const RECORDINGS_LIST_KEY = ["recordings", "list"] as const;

/**
 * Format the milliseconds until `target` as a compact countdown
 * (`2d 03:14:09` / `03:14:09` / `14:09`). Mirrors the Electron
 * `fmtCountdown` shape in `src/renderer/helpers.ts` closely enough for
 * the home hero. Returns `""` once the target is in the past.
 */
export function fmtCountdown(ms: number): string {
  if (ms <= 0) return "";
  const total = Math.floor(ms / 1000);
  const d = Math.floor(total / 86400);
  const h = Math.floor((total % 86400) / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  if (d > 0) return `${d}d ${pad(h)}:${pad(m)}:${pad(s)}`;
  if (h > 0) return `${pad(h)}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

/** Render an ISO-like local datetime (`YYYY-MM-DDTHH:MM:SS`) for the hero. */
export function fmtNext(s: string): string {
  const d = new Date(s);
  if (Number.isNaN(d.getTime())) return s;
  return d.toLocaleString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** A bytes count as a friendly size (`1.2 GB` / `340 MB` / `12 KB`). */
export function fmtBytes(bytes: number | null): string {
  if (bytes == null || bytes <= 0) return "—";
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  return `${Math.round(bytes / 1e3)} KB`;
}

/** A duration in ms as `1t 30m` / `45m` / `30s` (Electron `fmtDurationSec`). */
export function fmtDuration(ms: number | null): string {
  if (ms == null || ms <= 0) return "—";
  const sec = Math.floor(ms / 1000);
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  if (h > 0 && m > 0) return `${h}t ${m}m`;
  if (h > 0) return `${h}t`;
  if (m > 0) return `${m}m`;
  return `${s}s`;
}

/** Render a unix-ms timestamp as a short local date. */
function fmtStarted(ms: number): string {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Tick a re-render every second so the live countdown stays current. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);
  return now;
}

/**
 * The home screen — the real shell's landing view (replaces the Phase-0
 * proof-of-life). Mirrors the Electron `home.ts` page:
 *   - a hero with the next-recording countdown (or a "set up a schedule"
 *     nudge when no slot exists),
 *   - the live microphone VU,
 *   - a review-queue card surfacing episodes awaiting human review,
 *   - the most-recent recording history (with a "see all" jump).
 *
 * Navigation is delegated to `onNavigate` (the shell's `showView`) so the
 * cards can route to the dedicated panels. Pure IPC + render; the countdown,
 * empty-states and navigation clicks are exercised in tests with `invoke`
 * mocked.
 */
export function HomePage({
  onNavigate,
}: {
  onNavigate?: (view: ViewName) => void;
}) {
  const { t } = useTranslation();

  const { data: status } = useQuery<ScheduleStatus>({
    queryKey: SCHEDULE_STATUS_KEY,
    queryFn: () => invoke<ScheduleStatus>("scheduler_status"),
  });

  const { data: settings } = useQuery<Settings>({
    queryKey: SETTINGS_QUERY_KEY,
    queryFn: () => invoke<Settings>("settings_get"),
  });

  const { data: recordings } = useQuery<RecordingRow[]>({
    queryKey: RECORDINGS_LIST_KEY,
    queryFn: () => invoke<RecordingRow[]>("recordings_list"),
  });

  const { data: queue } = useQuery<ReviewQueueEntry[]>({
    queryKey: REVIEW_QUEUE_KEY,
    queryFn: () => invoke<ReviewQueueEntry[]>("review_queue_list"),
  });

  const next = status?.next ?? null;
  const now = useNow(next !== null);
  const countdown = next ? fmtCountdown(new Date(next).getTime() - now) : "";

  const slotCount = settings?.slots?.length ?? 0;
  const specialCount = settings?.specialRecordings?.length ?? 0;
  const hasSchedule = slotCount > 0 || specialCount > 0;

  const recent = (recordings ?? []).slice(0, 5);
  const pending = (queue ?? []).filter((q) => q.prep.status !== "published");

  return (
    <div className="flex w-full max-w-2xl flex-col gap-6">
      {/* ── Hero ─────────────────────────────────────────────────────── */}
      <section
        className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-6"
        aria-label={t("nav.home", "Hjem")}
      >
        <p className="text-xs uppercase tracking-wide text-text3">
          {t("home.nextRecording", "NESTE OPPTAK")}
        </p>
        {next ? (
          <>
            <h1 className="text-2xl font-semibold text-text">{fmtNext(next)}</h1>
            {countdown && (
              <p className="text-lg tabular-nums text-accent">
                {countdown}{" "}
                <span className="text-sm text-text2">
                  {t("home.untilStart", "til oppstart")}
                </span>
              </p>
            )}
          </>
        ) : (
          <h1 className="text-2xl font-semibold text-text">
            {hasSchedule
              ? t("home.readyTitle", "Alt er klart")
              : t(
                  "home.readyNoSchedule",
                  "Klar — sett opp en tidsplan for å starte automatisk",
                )}
          </h1>
        )}
        {!hasSchedule && (
          <button
            type="button"
            className="mt-1 self-start rounded-lg border border-border bg-surface2 px-3 py-1 text-sm text-text2 hover:bg-surface3"
            onClick={() => onNavigate?.("schedule")}
          >
            {t("nav.schedule", "Tidsplan")} →
          </button>
        )}
      </section>

      {/* ── Live VU ──────────────────────────────────────────────────── */}
      <VuMeter />

      {/* ── Review queue card ────────────────────────────────────────── */}
      {pending.length > 0 && (
        <section
          className="flex items-center justify-between gap-3 rounded-xl border border-accent/40 bg-accent p-4"
          aria-label={t("review.title", "Gjennomgang")}
        >
          <div>
            <p className="font-medium text-text">
              {t("home.reviewQueueCount", "{{n}} episoder klare", {
                n: pending.length,
              })}
            </p>
            <p className="text-sm text-text2">
              {t(
                "home.reviewQueueHint",
                "Venter på gjennomgang før publisering",
              )}
            </p>
          </div>
          <button
            type="button"
            className="shrink-0 rounded-lg border border-accent px-3 py-1 text-sm text-accent hover:bg-accent"
            onClick={() => onNavigate?.("review")}
          >
            {t("home.reviewOpen", "Åpne →")}
          </button>
        </section>
      )}

      {/* ── Recent history ───────────────────────────────────────────── */}
      <section
        className="flex flex-col gap-3 rounded-xl border border-border bg-surface p-4"
        aria-label={t("home.recentRecordings", "Siste opptak")}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium text-text">
            {t("home.recentRecordings", "Siste opptak")}
          </h2>
          <button
            type="button"
            className="text-sm text-text2 hover:text-text"
            onClick={() => onNavigate?.("history")}
          >
            {t("home.viewAll", "Se alle →")}
          </button>
        </div>
        {recent.length === 0 ? (
          <p className="text-text3">
            {t("history.empty", "Ingen opptak ennå")}
          </p>
        ) : (
          <ul className="flex flex-col divide-y divide-border">
            {recent.map((r) => (
              <li
                key={r.id}
                className="flex items-center justify-between gap-3 py-2 text-sm"
              >
                <span className="min-w-0 truncate text-text" title={r.file_path}>
                  {r.file_path.split(/[\\/]/).pop() || r.file_path}
                </span>
                <span className="shrink-0 tabular-nums text-text2">
                  {fmtStarted(r.started_at)} · {fmtDuration(r.duration_ms)} ·{" "}
                  {fmtBytes(r.byte_size)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
