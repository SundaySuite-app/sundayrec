import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { BRIDGE_STATUS_KEY, INTEGRATIONS_SETTINGS_KEY } from "./queryKey";

/** The connection settings persisted under the `integrations` settings key
 *  (a JSON blob, mirroring the Electron `integrations.connection` object). */
type IntegrationSettings = {
  churchId?: string;
  serviceId?: string;
  songApiUrl?: string;
  planApiUrl?: string;
};

/** The peer apps in the Sunday-suite we can hand work off to / link with. */
const PEERS: { id: string; label: string; note: string }[] = [
  { id: "plan", label: "SundayPlan", note: "integrations.peerPlan" },
  { id: "song", label: "SundaySong", note: "integrations.peerSong" },
  { id: "stage", label: "SundayStage", note: "integrations.peerStage" },
  { id: "edit", label: "SundayEdit", note: "integrations.peerEdit" },
  { id: "studio", label: "SundayStudio", note: "integrations.peerStudio" },
];

/** Read the integration settings JSON blob (raw string) → object. */
async function readSettings(): Promise<IntegrationSettings> {
  const raw = await invoke<string | null>("setting_get", {
    key: "integrations",
  });
  if (!raw) return {};
  try {
    return JSON.parse(raw) as IntegrationSettings;
  } catch {
    return {};
  }
}

/**
 * Sunday-suite integrations panel. Two halves:
 *
 *  1. The peer apps (Plan/Song/Stage/Edit/Studio) — the desktop↔desktop handoff
 *     happens per-recording from the history (`open_in_sundayedit` /
 *     `open_in_sundaystudio` deep links); here we surface the suite + the shared
 *     "connection" fields (churchId / serviceId / Song + Plan API URLs) the
 *     handoffs and the live bridge read. Saved to the `integrations` settings
 *     key (`setting_set`).
 *
 *  2. The live cue-bridge listener — resolves the SundayStage Realtime channel
 *     for the configured churchId+serviceId (`live_bridge_channel`, pure, works
 *     in every build) and reports whether the native WebSocket subscribe is
 *     compiled in (`live_bridge_status`). The native bridge is behind the
 *     default-off `bridge` cargo feature, so the default build reports
 *     `false` and the panel shows a calm "not built into this build" hint
 *     rather than a dead button.
 *
 * Pure IPC + render; exercised in tests with `invoke` mocked.
 */
export function IntegrationsPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const settings = useQuery<IntegrationSettings>({
    queryKey: INTEGRATIONS_SETTINGS_KEY,
    queryFn: readSettings,
  });

  // Whether the native Realtime subscribe is compiled in (default-off `bridge`).
  const bridgeBuilt = useQuery<boolean>({
    queryKey: BRIDGE_STATUS_KEY,
    queryFn: () => invoke<boolean>("live_bridge_status"),
  });

  const [churchId, setChurchId] = useState("");
  const [serviceId, setServiceId] = useState("");
  const [songApiUrl, setSongApiUrl] = useState("");
  const [planApiUrl, setPlanApiUrl] = useState("");
  // The resolved Realtime channel name (or an error hint) after a "test" click.
  const [channel, setChannel] = useState<string | null>(null);
  const [channelError, setChannelError] = useState(false);

  // Hydrate the form ONCE the persisted settings first load — never clobber what
  // the user is currently typing (a slow load must not wipe a fast typer, and
  // later refetches must not reset the fields). We only seed fields the user has
  // not touched yet, then latch.
  const hydrated = useRef(false);
  useEffect(() => {
    if (hydrated.current || !settings.data) return;
    hydrated.current = true;
    const s = settings.data;
    if (s.churchId) setChurchId((v) => v || s.churchId!);
    if (s.serviceId) setServiceId((v) => v || s.serviceId!);
    if (s.songApiUrl) setSongApiUrl((v) => v || s.songApiUrl!);
    if (s.planApiUrl) setPlanApiUrl((v) => v || s.planApiUrl!);
  }, [settings.data]);

  const saveMutation = useMutation({
    mutationFn: (next: IntegrationSettings) =>
      invoke<void>("setting_set", {
        key: "integrations",
        value: JSON.stringify(next),
      }),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: INTEGRATIONS_SETTINGS_KEY }),
  });

  const onSave = useCallback(() => {
    saveMutation.mutate({
      churchId: churchId.trim() || undefined,
      serviceId: serviceId.trim() || undefined,
      songApiUrl: songApiUrl.trim() || undefined,
      planApiUrl: planApiUrl.trim() || undefined,
    });
  }, [saveMutation, churchId, serviceId, songApiUrl, planApiUrl]);

  // Resolve the Realtime channel name for the configured ids (pure core; this
  // never subscribes — it just validates that the ids form a valid topic).
  const onTestChannel = useCallback(async () => {
    setChannel(null);
    setChannelError(false);
    try {
      const name = await invoke<string>("live_bridge_channel", {
        churchId: churchId.trim(),
        serviceId: serviceId.trim(),
      });
      setChannel(name);
    } catch {
      setChannelError(true);
    }
  }, [churchId, serviceId]);

  const nativeBridge = bridgeBuilt.data ?? false;

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("integrations.title", "Integrasjoner")}
    >
      {/* ── Peer apps ───────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">
          {t("integrations.peersTitle", "Sunday-appene")}
        </h2>
        <ul className="flex flex-col gap-2">
          {PEERS.map((p) => (
            <li
              key={p.id}
              className="flex items-center justify-between gap-3 rounded-lg border border-zinc-700 p-3"
            >
              <span className="font-medium">{p.label}</span>
              <span className="text-xs opacity-70">{t(p.note, p.label)}</span>
            </li>
          ))}
        </ul>
        <p className="text-xs opacity-60">
          {t(
            "integrations.handoffHint",
            "Send et ferdig opptak til SundayEdit eller SundayStudio fra Historikk.",
          )}
        </p>
      </div>

      {/* ── Connection ──────────────────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">
          {t("integrations.connectionTitle", "Tilkobling")}
        </h2>
        <input
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("integrations.churchId", "Menighets-ID")}
          value={churchId}
          onChange={(e) => setChurchId(e.target.value)}
          aria-label={t("integrations.churchId", "Menighets-ID")}
        />
        <input
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("integrations.serviceId", "Gudstjeneste-ID (live)")}
          value={serviceId}
          onChange={(e) => setServiceId(e.target.value)}
          aria-label={t("integrations.serviceId", "Gudstjeneste-ID (live)")}
        />
        <input
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("integrations.songApiUrl", "SundaySong API-URL")}
          value={songApiUrl}
          onChange={(e) => setSongApiUrl(e.target.value)}
          aria-label={t("integrations.songApiUrl", "SundaySong API-URL")}
        />
        <input
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("integrations.planApiUrl", "SundayPlan API-URL")}
          value={planApiUrl}
          onChange={(e) => setPlanApiUrl(e.target.value)}
          aria-label={t("integrations.planApiUrl", "SundayPlan API-URL")}
        />
        <button
          type="button"
          disabled={saveMutation.isPending}
          className="self-start rounded border border-zinc-700 px-2 py-1 text-xs hover:bg-zinc-800 disabled:opacity-50"
          onClick={onSave}
        >
          {t("integrations.save", "Lagre tilkobling")}
        </button>
        {saveMutation.isSuccess && (
          <p className="text-xs text-emerald-300" role="status">
            {t("integrations.saved", "Tilkobling lagret.")}
          </p>
        )}
      </div>

      {/* ── Live cue-bridge ─────────────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">
          {t("integrations.bridgeTitle", "Live cue-bro (SundayStage)")}
        </h2>
        {!nativeBridge && (
          <p className="rounded-lg border border-amber-700 bg-amber-950/40 p-3 text-sm text-amber-200">
            {t(
              "integrations.bridgeDisabled",
              "Den innebygde live-broen er ikke bygd inn i denne versjonen. Du kan likevel lagre tilkoblingen og teste kanalnavnet.",
            )}
          </p>
        )}
        <button
          type="button"
          disabled={!churchId.trim() || !serviceId.trim()}
          className="self-start rounded border border-zinc-700 px-2 py-1 text-xs hover:bg-zinc-800 disabled:opacity-50"
          onClick={() => void onTestChannel()}
        >
          {t("integrations.testChannel", "Vis kanalnavn")}
        </button>
        {channel && (
          <p className="break-all text-xs text-emerald-300" role="status">
            {channel}
          </p>
        )}
        {channelError && (
          <p className="text-xs text-red-400" role="alert">
            {t(
              "integrations.channelError",
              "Fyll inn både menighets-ID og gudstjeneste-ID.",
            )}
          </p>
        )}
      </div>
    </section>
  );
}
