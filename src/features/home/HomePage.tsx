import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { audioDir, join } from "@tauri-apps/api/path";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { ScheduleStatus } from "@/lib/bindings/ScheduleStatus";
import type { Settings } from "@/lib/bindings/Settings";
import type { RecordingRow } from "@/lib/bindings/RecordingRow";
import type { ReviewQueueEntry } from "@/lib/bindings/ReviewQueueEntry";
import type { RecordingProgress } from "@/lib/bindings/RecordingProgress";
import type { RecordingEvent } from "@/lib/bindings/RecordingEvent";
import type { RecordingOpts } from "@/lib/bindings/RecordingOpts";
import type { RecorderStatePayload } from "@/lib/bindings/RecorderStatePayload";
import type { VuLevels } from "@/lib/bindings/VuLevels";
import type { PreviewFrame } from "@/lib/bindings/PreviewFrame";
import type { DiskSpace } from "@/lib/bindings/DiskSpace";
import { SETTINGS_QUERY_KEY } from "@/features/settings/queryKey";
import { REVIEW_QUEUE_KEY } from "@/features/review/queryKey";
import type { ViewName } from "@/lib/routing";

const SCHEDULE_STATUS_KEY = ["scheduler_status"] as const;
const RECORDINGS_LIST_KEY = ["recordings", "list"] as const;

// ── Helpers ────────────────────────────────────────────────────────────────

/**
 * Format the milliseconds until `target` as a compact countdown
 * (`2d 03:14:09` / `03:14:09` / `14:09`). Returns `""` once past.
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

/** Render an ISO-like local datetime for the hero. */
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

/** A duration in ms as `1t 30m` / `45m` / `30s`. */
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

/** Disk free space: `x.xGB ledig` / `xMB ledig` / `—`. */
function fmtDiskFree(bytes: number | null): string {
  if (bytes == null) return "—";
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB ledig`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB ledig`;
  return "—";
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

/** Build a timestamp string like `2026-06-01_09-30-15`. */
function buildTimestamp(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}` +
    `_${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`
  );
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

// ── VU bar rendering ───────────────────────────────────────────────────────

const DB_FLOOR = -60;
const DB_CLIP = 0;

function dbToPercent(db: number): number {
  return Math.max(0, Math.min(100, ((db - DB_FLOOR) / (DB_CLIP - DB_FLOOR)) * 100));
}

function vuBarColor(db: number): string {
  if (db >= -6) return "bg-red-500";
  if (db >= -18) return "bg-yellow-400";
  return "bg-accent";
}

interface VuBarsProps {
  levels: number[];
  label?: string;
}

function VuBars({ levels, label }: VuBarsProps) {
  return (
    <div className="flex flex-col gap-1">
      {label && <p className="text-xs text-text3 uppercase tracking-wide">{label}</p>}
      <div className="flex gap-1 items-end h-8">
        {levels.length === 0 ? (
          <div className="flex gap-1 items-end h-8">
            <div className="w-4 h-full bg-surface3 rounded-sm opacity-40" />
            <div className="w-4 h-full bg-surface3 rounded-sm opacity-40" />
          </div>
        ) : (
          levels.map((db, i) => {
            const pct = dbToPercent(db);
            const color = vuBarColor(db);
            return (
              <div key={i} className="relative w-4 bg-surface3 rounded-sm overflow-hidden h-full">
                <div
                  className={`absolute bottom-0 left-0 right-0 ${color} transition-all duration-75`}
                  style={{ height: `${pct}%` }}
                />
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

// ── HomePage ───────────────────────────────────────────────────────────────

type RecState = "idle" | "preparing" | "recording" | "reconnecting" | "stopping" | "stopped" | "failed";

export function HomePage({
  onNavigate,
}: {
  onNavigate?: (view: ViewName) => void;
}) {
  const { t } = useTranslation();

  // ── Queries ──────────────────────────────────────────────────────────────
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

  // ── Countdown ────────────────────────────────────────────────────────────
  const next = status?.next ?? null;
  const now = useNow(next !== null);
  const countdown = next ? fmtCountdown(new Date(next).getTime() - now) : "";

  const slotCount = settings?.slots?.length ?? 0;
  const specialCount = settings?.specialRecordings?.length ?? 0;
  const hasSchedule = slotCount > 0 || specialCount > 0;

  // ── Recording state ───────────────────────────────────────────────────────
  const [recState, setRecState] = useState<RecState>("idle");
  const [recStartMs, setRecStartMs] = useState<number | null>(null);
  const [bytesWritten, setBytesWritten] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const recNow = useNow(recState === "recording");

  // ── VU ────────────────────────────────────────────────────────────────────
  const [vuLevels, setVuLevels] = useState<number[]>([]);

  // ── Preview ───────────────────────────────────────────────────────────────
  const [previewSrc, setPreviewSrc] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // ── Disk ──────────────────────────────────────────────────────────────────
  const [diskFree, setDiskFree] = useState<number | null>(null);

  // ── Side-effects: mount/unmount ───────────────────────────────────────────
  useEffect(() => {
    // Start VU
    invoke("start_vu", { deviceName: settings?.deviceName ?? null }).catch(() => {});

    // Start preview if videoEnabled
    if (settings?.videoEnabled) {
      invoke("start_preview", {
        device: settings.videoDeviceName ?? null,
        fps: 15,
      }).catch(() => {});
    }

    // Fetch disk space
    invoke<DiskSpace>("get_disk_space")
      .then((d) => setDiskFree(d.freeBytes))
      .catch(() => {});

    return () => {
      invoke("stop_vu").catch(() => {});
      if (settings?.videoEnabled) {
        invoke("stop_preview").catch(() => {});
      }
    };
    // Only run on mount (settings may not be loaded yet — re-run when they arrive)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings?.videoEnabled, settings?.deviceName, settings?.videoDeviceName]);

  // ── Listen for VU levels ──────────────────────────────────────────────────
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<VuLevels>("vu://levels", (ev) => {
      setVuLevels(ev.payload.peak_dbfs);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  // ── Listen for preview frames ─────────────────────────────────────────────
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<PreviewFrame>("preview://frame", (ev) => {
      setPreviewSrc(`data:image/jpeg;base64,${ev.payload.data}`);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  // ── Listen for recording events ───────────────────────────────────────────
  useEffect(() => {
    const unlisteners: Array<() => void> = [];

    listen<void>("recording://started", () => {
      setRecState("recording");
      setRecStartMs(Date.now());
      setBytesWritten(0);
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<RecordingProgress>("recording://progress", (ev) => {
      setBytesWritten(ev.payload.bytes_written);
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<RecorderStatePayload>("recording://state", (ev) => {
      setRecState(ev.payload.state as RecState);
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<RecordingEvent>("recording://error", (ev) => {
      setErrorMsg(ev.payload.message);
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<RecordingEvent>("recording://silence", () => {
      // silence event — could show a badge; ignore for now
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    return () => unlisteners.forEach((fn) => fn());
  }, []);

  // ── Start / stop recording ────────────────────────────────────────────────
  const handleStart = useCallback(async () => {
    if (!settings) return;
    setErrorMsg(null);
    setRecState("preparing");

    try {
      const folder =
        settings.saveFolder ??
        (await join(await audioDir(), "SundayRec"));

      const timestamp = buildTimestamp();
      const ext = settings.videoEnabled
        ? "mp4"
        : (settings.format as string);
      const output_path = await join(folder, `${timestamp}.${ext}`);

      const opts: RecordingOpts = {
        audio_device_name: settings.deviceName ?? "",
        video_device_name: settings.videoEnabled
          ? (settings.videoDeviceName ?? null)
          : null,
        output_path,
        stop_on_silence: settings.stopOnSilence,
        silence_threshold_db: settings.silenceThreshold,
        silence_timeout_minutes: settings.silenceTimeoutMinutes,
        framerate: 30,
        stereo: settings.channels === "stereo",
        split_minutes: settings.splitMinutes,
        manual_max_minutes: settings.manualMaxMinutes,
      };

      await invoke("start_recording", { opts });
    } catch (err) {
      setRecState("idle");
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  }, [settings]);

  const handleStop = useCallback(async () => {
    setRecState("stopping");
    try {
      await invoke("stop_recording");
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // ── Derived ───────────────────────────────────────────────────────────────
  const isRecording = recState === "recording" || recState === "reconnecting";
  const isBusy = recState === "preparing" || recState === "stopping";
  const elapsedMs = isRecording && recStartMs ? recNow - recStartMs : 0;

  const recent = (recordings ?? []).slice(0, 5);
  const pending = (queue ?? []).filter((q) => q.prep.status !== "published");

  // ── Render ────────────────────────────────────────────────────────────────
  return (
    <div className="flex w-full flex-col gap-4 h-full overflow-auto">

      {/* ── Schedule status strip ──────────────────────────────────────── */}
      <section className="flex items-center justify-between gap-3 rounded-xl border border-border bg-surface px-4 py-3">
        <div className="flex flex-col">
          <p className="text-xs uppercase tracking-wide text-text3">
            {t("home.nextRecording", "NESTE OPPTAK")}
          </p>
          {next ? (
            <p className="text-sm font-medium text-text">
              {fmtNext(next)}
              {countdown && (
                <>
                  <span className="ml-2 tabular-nums text-accent">
                    {countdown}
                  </span>
                  <span className="ml-1 text-xs text-text2">
                    {t("home.untilStart", "til oppstart")}
                  </span>
                </>
              )}
            </p>
          ) : (
            <p className="text-sm font-medium text-text">
              {hasSchedule
                ? t("home.readyTitle", "Alt er klart")
                : t("home.readyNoSchedule", "Ingen tidsplan — klar for manuelt opptak")}
            </p>
          )}
        </div>
        {!hasSchedule && (
          <button
            type="button"
            className="shrink-0 rounded-lg border border-border bg-surface2 px-3 py-1 text-sm text-text2 hover:bg-surface3"
            onClick={() => onNavigate?.("schedule")}
          >
            {t("nav.schedule", "Tidsplan")} →
          </button>
        )}
      </section>

      {/* ── Main two-column layout ─────────────────────────────────────── */}
      <div className="flex gap-4 flex-1">

        {/* ── Left column: preview + record button ──────────────────────── */}
        <div className="flex flex-1 flex-col gap-3">

          {/* Camera preview or VU visualizer */}
          <div className="relative aspect-video w-full rounded-xl border border-border bg-surface overflow-hidden flex items-center justify-center">
            {settings?.videoEnabled && previewSrc ? (
              <img
                src={previewSrc}
                alt={t("home.cameraPreview", "Kameraforhåndsvisning")}
                className="w-full h-full object-contain"
              />
            ) : settings?.videoEnabled ? (
              <div className="flex flex-col items-center gap-2 text-text3">
                <svg className="w-12 h-12 opacity-40" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
                    d="M15 10l4.553-2.277A1 1 0 0121 8.723v6.554a1 1 0 01-1.447.894L15 14M3 8a2 2 0 012-2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V8z" />
                </svg>
                <p className="text-sm">{t("home.cameraConnecting", "Kobler til kamera...")}</p>
              </div>
            ) : (
              /* Audio-only: large VU visualization */
              <div className="flex flex-col items-center justify-center gap-4 w-full h-full px-6">
                <div className="flex gap-2 items-end" style={{ height: 80 }}>
                  {vuLevels.length === 0 ? (
                    <>
                      {Array.from({ length: 20 }).map((_, i) => (
                        <div key={i} className="w-3 bg-surface3 rounded-sm opacity-30" style={{ height: "100%" }} />
                      ))}
                    </>
                  ) : (
                    vuLevels.flatMap((db, ch) =>
                      Array.from({ length: 10 }).map((_, i) => {
                        const pct = dbToPercent(db);
                        const barPct = (i + 1) * 10;
                        const active = barPct <= pct;
                        const color = active ? vuBarColor(db) : "bg-surface3 opacity-30";
                        return (
                          <div
                            key={`${ch}-${i}`}
                            className={`w-3 rounded-sm ${color}`}
                            style={{ height: `${barPct}%` }}
                          />
                        );
                      })
                    )
                  )}
                </div>
                <p className="text-xs text-text3 uppercase tracking-wide">
                  {t("home.audioOnly", "Lydopptak")} · {settings?.deviceName ?? t("home.defaultDevice", "Standard enhet")}
                </p>
              </div>
            )}

            {/* Recording indicator overlay */}
            {isRecording && (
              <div className="absolute top-3 left-3 flex items-center gap-1.5 rounded-full bg-red-600/90 px-3 py-1">
                <span className="inline-block w-2 h-2 rounded-full bg-white animate-pulse" />
                <span className="text-xs font-semibold text-white">
                  {t("home.recording", "TAR OPP")}
                </span>
              </div>
            )}
          </div>

          {/* ── Record / Stop button ─────────────────────────────────────── */}
          {isRecording ? (
            <button
              type="button"
              onClick={handleStop}
              disabled={isBusy}
              className="bg-accent text-bg rounded-xl py-4 px-8 text-xl font-bold w-full disabled:opacity-60"
            >
              {t("home.stopRecording", "Stopp opptak")}
            </button>
          ) : (
            <button
              type="button"
              onClick={handleStart}
              disabled={isBusy || !settings}
              className="bg-red-600 hover:bg-red-500 text-white rounded-xl py-4 px-8 text-xl font-bold w-full disabled:opacity-60 transition-colors"
            >
              {recState === "preparing"
                ? t("home.preparing", "Forbereder...")
                : recState === "stopping"
                ? t("home.stopping", "Stopper...")
                : t("home.startRecording", "Start opptak")}
            </button>
          )}

          {/* ── Recording status line ─────────────────────────────────────── */}
          {isRecording && (
            <div className="flex items-center gap-3 rounded-lg bg-surface px-4 py-2 text-sm">
              <span className="font-medium text-red-400">● {t("home.recordingStatus", "Tar opp")}</span>
              <span className="tabular-nums text-text2">{fmtDuration(elapsedMs)}</span>
              <span className="tabular-nums text-text3">{fmtBytes(bytesWritten)}</span>
              {recState === "reconnecting" && (
                <span className="text-yellow-400">{t("home.reconnecting", "Kobler til igjen...")}</span>
              )}
            </div>
          )}

          {/* ── Error message ─────────────────────────────────────────────── */}
          {errorMsg && (
            <div className="rounded-lg border border-red-500/40 bg-red-500/10 px-4 py-2 text-sm text-red-400">
              {errorMsg}
            </div>
          )}

          {/* ── Channel VU meters (below preview) ─────────────────────────── */}
          <div className="flex gap-4 items-start rounded-xl border border-border bg-surface px-4 py-3">
            {vuLevels.length === 0 ? (
              <>
                <VuBars levels={[-60]} label={t("home.channelL", "L")} />
                <VuBars levels={[-60]} label={t("home.channelR", "R")} />
              </>
            ) : vuLevels.length === 1 ? (
              <VuBars levels={[vuLevels[0]]} label={t("home.channelMono", "Mono")} />
            ) : (
              <>
                <VuBars levels={[vuLevels[0]]} label={t("home.channelL", "L")} />
                <VuBars levels={[vuLevels[1]]} label={t("home.channelR", "R")} />
              </>
            )}
            <div className="ml-auto text-right">
              <p className="text-xs text-text3 uppercase tracking-wide">{t("home.vuPeak", "Peak")}</p>
              <p className="tabular-nums text-sm text-text2">
                {vuLevels.length > 0
                  ? `${Math.max(...vuLevels).toFixed(1)} dB`
                  : "— dB"}
              </p>
            </div>
          </div>
        </div>

        {/* ── Right column: device status ──────────────────────────────── */}
        <div className="flex flex-col gap-3" style={{ width: 220 }}>

          {/* Device status panel */}
          <section className="flex flex-col gap-3 rounded-xl border border-border bg-surface p-4">
            <p className="text-xs uppercase tracking-wide text-text3">
              {t("home.devices", "Enheter")}
            </p>

            {/* Microphone */}
            <div className="flex flex-col gap-0.5">
              <p className="text-xs text-text3">{t("home.microphone", "Mikrofon")}</p>
              <p className="text-sm text-text truncate" title={settings?.deviceName ?? undefined}>
                {settings?.deviceName ?? t("home.defaultDevice", "Standard")}
              </p>
            </div>

            {/* Camera (only if videoEnabled) */}
            {settings?.videoEnabled && (
              <div className="flex flex-col gap-0.5">
                <p className="text-xs text-text3">{t("home.camera", "Kamera")}</p>
                <p className="text-sm text-text truncate" title={settings.videoDeviceName ?? undefined}>
                  {settings.videoDeviceName ?? t("home.defaultDevice", "Standard")}
                </p>
              </div>
            )}

            {/* Disk space */}
            <div className="flex flex-col gap-0.5">
              <p className="text-xs text-text3">{t("home.diskSpace", "Diskplass")}</p>
              <p className={`text-sm font-medium ${diskFree != null && diskFree < 500e6 ? "text-red-400" : "text-text"}`}>
                {fmtDiskFree(diskFree)}
              </p>
            </div>

            {/* Format */}
            <div className="flex flex-col gap-0.5">
              <p className="text-xs text-text3">{t("home.format", "Format")}</p>
              <p className="text-sm text-text uppercase">
                {settings?.videoEnabled ? "MP4" : (settings?.format?.toUpperCase() ?? "MP3")}
              </p>
            </div>

            {/* Recorder state */}
            <div className="flex flex-col gap-0.5">
              <p className="text-xs text-text3">{t("home.status", "Status")}</p>
              <p className={`text-sm font-medium ${
                isRecording ? "text-red-400" :
                recState === "preparing" || recState === "stopping" ? "text-yellow-400" :
                recState === "failed" ? "text-red-600" :
                "text-green-400"
              }`}>
                {recState === "idle" ? t("home.stateIdle", "Klar") :
                 recState === "preparing" ? t("home.statePreparing", "Forbereder") :
                 recState === "recording" ? t("home.stateRecording", "Tar opp") :
                 recState === "reconnecting" ? t("home.stateReconnecting", "Kobler til") :
                 recState === "stopping" ? t("home.stateStopping", "Stopper") :
                 recState === "stopped" ? t("home.stateStopped", "Stoppet") :
                 t("home.stateFailed", "Feil")}
              </p>
            </div>
          </section>

          {/* Review queue nudge */}
          {pending.length > 0 && (
            <section
              className="flex flex-col gap-2 rounded-xl border border-accent/40 bg-accent/10 p-3"
              aria-label={t("review.title", "Gjennomgang")}
            >
              <p className="text-sm font-medium text-text">
                {t("home.reviewQueueCount", "{{n}} episoder klare", {
                  n: pending.length,
                })}
              </p>
              <p className="text-xs text-text2">
                {t("home.reviewQueueHint", "Venter på gjennomgang")}
              </p>
              <button
                type="button"
                className="self-start rounded-lg border border-accent/60 px-2 py-1 text-xs text-accent hover:bg-accent/20"
                onClick={() => onNavigate?.("review")}
              >
                {t("home.reviewOpen", "Åpne →")}
              </button>
            </section>
          )}

          {/* Settings shortcut */}
          <button
            type="button"
            className="rounded-xl border border-border bg-surface px-4 py-3 text-sm text-text2 hover:bg-surface2 text-left"
            onClick={() => onNavigate?.("settings")}
          >
            {t("nav.settings", "Innstillinger")} →
          </button>
        </div>
      </div>

      {/* ── Recent recordings ────────────────────────────────────────────── */}
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
                  {r.file_path.split(/[\\/]/).pop() ?? r.file_path}
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

      {/* Hidden canvas for potential future frame rendering */}
      <canvas ref={canvasRef} className="hidden" />
    </div>
  );
}
