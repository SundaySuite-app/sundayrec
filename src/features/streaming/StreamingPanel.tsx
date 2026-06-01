import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { StreamStatus } from "@/lib/bindings/StreamStatus";
import type { StreamResolution } from "@/lib/bindings/StreamResolution";
import type { StreamDestinationView } from "@/lib/bindings/StreamDestinationView";
import type { OverlayConfig } from "@/lib/bindings/OverlayConfig";
import type { OverlaySource } from "@/lib/bindings/OverlaySource";
import { STREAM_STATUS_KEY } from "./queryKey";

/** The resolutions the backend renders, in display order. */
const RESOLUTIONS: readonly { value: StreamResolution; label: string }[] = [
  { value: "p480", label: "480p" },
  { value: "p720", label: "720p" },
  { value: "p1080", label: "1080p" },
] as const;

const FRAMERATES = [25, 30] as const;

/** A destination row in the renderer. The key never lives here — it's typed
 *  into a transient input and pushed to the keychain via `stream_set_key`. */
type DestRow = StreamDestinationView & { keyInput: string };

/** True when an IPC rejection is the default-build "streaming feature off"
 *  error, so the panel shows a calm hint rather than a red error. The seam
 *  returns `feature_disabled: …` in the message of a `validation` AppError. */
function isFeatureDisabled(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? String(err);
  return msg.includes("feature_disabled");
}

let nextDestId = 1;

/**
 * R3 live-streaming panel. Manages per-destination RTMP keys in the OS keychain
 * (`stream_set_key`/`stream_delete_key`), an optional lower-third text overlay,
 * and start/stop of the RTMP push (`stream_start`/`stream_stop`) with a live
 * status poll (`stream_status`). The stream spawn is behind the default-off
 * `streaming` cargo feature, so in the default build Start returns
 * `feature_disabled` and the panel shows a "not built into this build" hint —
 * the key vault still works so the user can save keys ahead of a streaming build.
 *
 * Pure IPC + render; exercised in tests with `invoke` mocked.
 */
export function StreamingPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [disabled, setDisabled] = useState(false);

  const [dests, setDests] = useState<DestRow[]>([]);
  const [newName, setNewName] = useState("");
  const [newUrl, setNewUrl] = useState("");

  // Optional lower-third overlay. It can be a text title/subtitle or an image
  // path; `overlayEnabled` is the explicit on/off toggle (R4) so a configured
  // overlay can be parked without losing the text/path the user typed.
  const [overlayEnabled, setOverlayEnabled] = useState(false);
  const [overlayKind, setOverlayKind] = useState<"text" | "image">("text");
  const [overlayTitle, setOverlayTitle] = useState("");
  const [overlaySubtitle, setOverlaySubtitle] = useState("");
  const [overlayImage, setOverlayImage] = useState("");

  const [resolution, setResolution] = useState<StreamResolution>("p720");
  const [framerate, setFramerate] = useState<number>(30);

  const status = useQuery<StreamStatus>({
    queryKey: STREAM_STATUS_KEY,
    queryFn: () => invoke<StreamStatus>("stream_status"),
    // Poll while a stream is active so the bitrate/fps stay fresh.
    refetchInterval: (q) => (q.state.data?.active ? 2000 : false),
  });

  const invalidate = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: STREAM_STATUS_KEY });
  }, [queryClient]);

  const setKeyMutation = useMutation({
    mutationFn: ({ destId, key }: { destId: string; key: string }) =>
      invoke<void>("stream_set_key", { destId, key }),
    onSuccess: (_d, { destId }) => {
      setDests((rows) =>
        rows.map((r) =>
          r.id === destId ? { ...r, hasKey: true, keyInput: "" } : r,
        ),
      );
    },
  });

  const deleteKeyMutation = useMutation({
    mutationFn: (destId: string) =>
      invoke<void>("stream_delete_key", { destId }),
    onSuccess: (_d, destId) => {
      setDests((rows) =>
        rows.map((r) => (r.id === destId ? { ...r, hasKey: false } : r)),
      );
    },
  });

  const startMutation = useMutation({
    mutationFn: () => {
      // Build the lower-third only when the toggle is on AND the chosen source
      // actually has content (a title for text, a path for image). Otherwise we
      // push no overlays so the encode stays clean.
      const source: OverlaySource | null =
        overlayKind === "image"
          ? overlayImage.trim()
            ? { kind: "image", path: overlayImage.trim() }
            : null
          : overlayTitle.trim()
            ? {
                kind: "text",
                title: overlayTitle.trim(),
                subtitle: overlaySubtitle.trim() || null,
              }
            : null;
      const overlays: OverlayConfig[] =
        overlayEnabled && source
          ? [
              {
                id: "lower-third",
                name: "Lower third",
                enabled: true,
                source,
                position: "bl",
                customX: null,
                customY: null,
                scale: 0.3,
                opacity: 1,
              },
            ]
          : [];
      return invoke<StreamStatus>("stream_start", {
        destinations: dests.map(({ keyInput: _k, ...d }) => d),
        resolution,
        framerate,
        videoBitrateKbps: null,
        audioBitrateKbps: null,
        alsoRecordPath: null,
        overlays,
        videoToken: "0",
        macAudioToken: null,
        winAudioName: null,
        snapshotPath: "",
      });
    },
    onSuccess: invalidate,
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });

  const stopMutation = useMutation({
    mutationFn: () => invoke<boolean>("stream_stop"),
    onSuccess: invalidate,
    onError: (e) => setDisabled(isFeatureDisabled(e)),
  });

  const addDestination = useCallback(() => {
    const name = newName.trim();
    const rtmpUrl = newUrl.trim();
    if (!name || !rtmpUrl) return;
    setDests((rows) => [
      ...rows,
      {
        id: `dest-${nextDestId++}`,
        name,
        rtmpUrl,
        enabled: true,
        hasKey: false,
        keyInput: "",
      },
    ]);
    setNewName("");
    setNewUrl("");
  }, [newName, newUrl]);

  const removeDestination = useCallback(
    (id: string) => {
      // Best-effort vault cleanup, then drop the row.
      deleteKeyMutation.mutate(id);
      setDests((rows) => rows.filter((r) => r.id !== id));
    },
    [deleteKeyMutation],
  );

  const st = status.data;
  const active = st?.active ?? false;

  const inputClass =
    "rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text placeholder:text-text3";

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("streaming.title", "Direktesending")}
    >
      {disabled && (
        <p className="rounded-lg border border-accent/60 bg-accent p-3 text-sm text-bg">
          {t(
            "streaming.featureDisabled",
            "Direktesending er ikke bygd inn i denne versjonen. Nøkler kan likevel lagres.",
          )}
        </p>
      )}

      {/* ── Status ──────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between gap-3 rounded-xl border border-border bg-surface p-4">
        <span
          className={`rounded-lg border px-1.5 py-0.5 text-xs ${
            active
              ? "border-emerald-700 text-emerald-300"
              : "border-border text-text3"
          }`}
        >
          {active
            ? t("streaming.live", "Sender direkte")
            : t("streaming.idle", "Av")}
        </span>
        {active && (
          <span className="text-xs text-text2">
            {t("streaming.stats", "{{kbps}} kbps · {{fps}} fps", {
              kbps: st?.bitrateKbps ?? 0,
              fps: st?.fps ?? 0,
            })}
          </span>
        )}
        {active ? (
          <button
            type="button"
            className="rounded-lg border border-red-800 px-2 py-1 text-xs text-red-300 hover:bg-red-950"
            onClick={() => stopMutation.mutate()}
          >
            {t("streaming.stop", "Stopp")}
          </button>
        ) : (
          <button
            type="button"
            disabled={startMutation.isPending}
            className="rounded-lg bg-accent px-3 py-2 text-xs font-medium text-bg hover:bg-accent/90 disabled:opacity-50"
            onClick={() => startMutation.mutate()}
          >
            {t("streaming.start", "Start")}
          </button>
        )}
      </div>

      {/* ── Quality ─────────────────────────────────────────────────── */}
      <div className="flex items-center gap-3">
        <label className="flex items-center gap-2 text-sm text-text2">
          {t("streaming.resolution", "Oppløsning")}
          <select
            className="rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text"
            value={resolution}
            onChange={(e) => setResolution(e.target.value as StreamResolution)}
          >
            {RESOLUTIONS.map((r) => (
              <option key={r.value} value={r.value}>
                {r.label}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-2 text-sm text-text2">
          {t("streaming.framerate", "Bildefrekvens")}
          <select
            className="rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text"
            value={framerate}
            onChange={(e) => setFramerate(Number(e.target.value))}
          >
            {FRAMERATES.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </label>
      </div>

      {/* ── Lower third ─────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-4">
        <label className="flex items-center gap-2 text-sm font-medium text-text">
          <input
            type="checkbox"
            checked={overlayEnabled}
            onChange={(e) => setOverlayEnabled(e.target.checked)}
            aria-label={t("streaming.overlayToggle", "Vis tekstplakat")}
          />
          {t("streaming.lowerThird", "Tekstplakat (nedre tredjedel)")}
        </label>
        <label className="flex items-center gap-2 text-sm text-text2">
          {t("streaming.overlayKind", "Type")}
          <select
            className="rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text"
            value={overlayKind}
            onChange={(e) =>
              setOverlayKind(e.target.value as "text" | "image")
            }
            aria-label={t("streaming.overlayKind", "Type")}
          >
            <option value="text">{t("streaming.overlayText", "Tekst")}</option>
            <option value="image">
              {t("streaming.overlayImage", "Bilde")}
            </option>
          </select>
        </label>
        {overlayKind === "text" ? (
          <>
            <input
              className={inputClass}
              placeholder={t("streaming.lowerThirdTitle", "Tittel")}
              value={overlayTitle}
              onChange={(e) => setOverlayTitle(e.target.value)}
              aria-label={t("streaming.lowerThirdTitle", "Tittel")}
            />
            <input
              className={inputClass}
              placeholder={t("streaming.lowerThirdSubtitle", "Undertittel")}
              value={overlaySubtitle}
              onChange={(e) => setOverlaySubtitle(e.target.value)}
              aria-label={t("streaming.lowerThirdSubtitle", "Undertittel")}
            />
          </>
        ) : (
          <input
            className={inputClass}
            placeholder={t("streaming.lowerThirdImage", "Sti til bilde (PNG)")}
            value={overlayImage}
            onChange={(e) => setOverlayImage(e.target.value)}
            aria-label={t("streaming.lowerThirdImage", "Sti til bilde (PNG)")}
          />
        )}
      </div>

      {/* ── Destinations ────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium text-text">
          {t("streaming.destinations", "Destinasjoner")}
        </h2>
        {dests.length === 0 ? (
          <p className="text-text3">
            {t("streaming.noDestinations", "Ingen destinasjoner ennå")}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {dests.map((d) => (
              <li
                key={d.id}
                className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-4"
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <p className="truncate font-medium text-text" title={d.name}>
                      {d.name}
                    </p>
                    <p className="truncate text-xs text-text2" title={d.rtmpUrl}>
                      {d.rtmpUrl}
                    </p>
                  </div>
                  <button
                    type="button"
                    className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                    onClick={() => removeDestination(d.id)}
                  >
                    {t("streaming.removeDest", "Fjern")}
                  </button>
                </div>
                {d.hasKey ? (
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-emerald-300">
                      {t("streaming.keySaved", "•••• (lagret)")}
                    </span>
                    <button
                      type="button"
                      className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3"
                      onClick={() => deleteKeyMutation.mutate(d.id)}
                    >
                      {t("streaming.deleteKey", "Slett nøkkel")}
                    </button>
                  </div>
                ) : (
                  <div className="flex items-center gap-2">
                    <input
                      type="password"
                      className={`min-w-0 flex-1 ${inputClass}`}
                      placeholder={t("streaming.streamKey", "Strømnøkkel")}
                      value={d.keyInput}
                      aria-label={t("streaming.streamKeyFor", "Strømnøkkel for {{name}}", {
                        name: d.name,
                      })}
                      onChange={(e) =>
                        setDests((rows) =>
                          rows.map((r) =>
                            r.id === d.id
                              ? { ...r, keyInput: e.target.value }
                              : r,
                          ),
                        )
                      }
                    />
                    <button
                      type="button"
                      disabled={!d.keyInput.trim() || setKeyMutation.isPending}
                      className="rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
                      onClick={() =>
                        setKeyMutation.mutate({ destId: d.id, key: d.keyInput })
                      }
                    >
                      {t("streaming.saveKey", "Lagre nøkkel")}
                    </button>
                  </div>
                )}
                {setKeyMutation.isError &&
                  setKeyMutation.variables?.destId === d.id && (
                    <p className="text-xs text-red-400">
                      {t("streaming.keyRejected", "Ugyldig nøkkel")}
                    </p>
                  )}
              </li>
            ))}
          </ul>
        )}

        {/* Add a destination */}
        <div className="flex flex-col gap-2 rounded-xl border border-dashed border-border2 p-4">
          <input
            className={inputClass}
            placeholder={t("streaming.destName", "Navn (f.eks. YouTube)")}
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            aria-label={t("streaming.destName", "Navn (f.eks. YouTube)")}
          />
          <input
            className={inputClass}
            placeholder="rtmp://…"
            value={newUrl}
            onChange={(e) => setNewUrl(e.target.value)}
            aria-label={t("streaming.destUrl", "RTMP-URL")}
          />
          <button
            type="button"
            disabled={!newName.trim() || !newUrl.trim()}
            className="self-start rounded-lg border border-border bg-surface2 px-2 py-1 text-xs text-text2 hover:bg-surface3 disabled:opacity-50"
            onClick={addDestination}
          >
            {t("streaming.addDest", "Legg til destinasjon")}
          </button>
        </div>
      </div>
    </section>
  );
}
