import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

/** The `{ ok, error?, ... }` shape every integration hand-off command returns
 *  (mirrors the Electron handlers — structured, never throws). */
type OpResult = {
  ok: boolean;
  error?: string | null;
  hint?: string | null;
  submitted?: number | null;
  transcriptPath?: string | null;
};

const SONG_APIKEY_KEY = ["integrations", "song-apikey"] as const;

/**
 * Sunday-suite hand-off actions for a single recording (the per-recording half
 * of the integrations area; the shared connection + the live cue-bridge live in
 * `IntegrationsPanel`). Three flows, all over the typed P2b commands:
 *
 *   • SundaySong API key — stored encrypted in the keychain
 *     (`integrations_song_set_apikey`), presence read via
 *     `integrations_song_has_apikey`.
 *   • Submit usage for a recording with a `.service.json` sidecar
 *     (`integrations_song_submit_usage`).
 *   • Send a recording to Verbatim/SundayEdit via the `verbatim://` deep link
 *     (`integrations_verbatim_send`).
 *
 * The HTTP submission + the deep-link launch are NETWORK-UNVERIFIED on the Rust
 * side; here the handlers are fully testable with `invoke` mocked — we assert
 * the IPC calls + the rendered result/hint. Pixel paint is GUI-UNVERIFIED.
 */
export function SuiteHandoffPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const hasApiKey = useQuery<boolean>({
    queryKey: SONG_APIKEY_KEY,
    queryFn: () => invoke<boolean>("integrations_song_has_apikey"),
  });

  const [apiKey, setApiKey] = useState("");
  const [recordingPath, setRecordingPath] = useState("");
  const [submitResult, setSubmitResult] = useState<OpResult | null>(null);
  const [sendResult, setSendResult] = useState<OpResult | null>(null);

  const saveKeyMutation = useMutation({
    mutationFn: (plaintext: string) =>
      invoke<void>("integrations_song_set_apikey", { plaintext }),
    onSuccess: () => {
      setApiKey("");
      void queryClient.invalidateQueries({ queryKey: SONG_APIKEY_KEY });
    },
  });

  const submitUsageMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<OpResult>("integrations_song_submit_usage", {
        recordingPath: path,
      }),
    onSuccess: setSubmitResult,
  });

  const verbatimSendMutation = useMutation({
    mutationFn: (path: string) =>
      invoke<OpResult>("integrations_verbatim_send", { videoPath: path }),
    onSuccess: setSendResult,
  });

  const onSaveKey = useCallback(() => {
    if (apiKey.trim()) saveKeyMutation.mutate(apiKey.trim());
  }, [apiKey, saveKeyMutation]);

  const onSubmitUsage = useCallback(() => {
    setSubmitResult(null);
    if (recordingPath.trim())
      submitUsageMutation.mutate(recordingPath.trim());
  }, [recordingPath, submitUsageMutation]);

  const onVerbatimSend = useCallback(() => {
    setSendResult(null);
    if (recordingPath.trim())
      verbatimSendMutation.mutate(recordingPath.trim());
  }, [recordingPath, verbatimSendMutation]);

  const connected = hasApiKey.data ?? false;

  return (
    <section
      className="flex w-full max-w-md flex-col gap-4"
      aria-label={t("handoff.title", "Suite-overlevering")}
    >
      {/* ── SundaySong API key ──────────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">
          {t("handoff.songKeyTitle", "SundaySong API-nøkkel")}
        </h2>
        <span
          className={`self-start rounded border px-1.5 py-0.5 text-xs ${
            connected
              ? "border-emerald-700 text-emerald-300"
              : "border-zinc-600 text-zinc-400"
          }`}
        >
          {connected
            ? t("handoff.keyStored", "Nøkkel lagret")
            : t("handoff.keyMissing", "Ingen nøkkel")}
        </span>
        <input
          type="password"
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("handoff.songKey", "API-nøkkel")}
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          aria-label={t("handoff.songKey", "API-nøkkel")}
        />
        <button
          type="button"
          disabled={!apiKey.trim() || saveKeyMutation.isPending}
          className="self-start rounded border border-zinc-700 px-2 py-1 text-xs hover:bg-zinc-800 disabled:opacity-50"
          onClick={onSaveKey}
        >
          {t("handoff.saveKey", "Lagre nøkkel")}
        </button>
      </div>

      {/* ── Per-recording hand-off ──────────────────────────────────── */}
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">
          {t("handoff.recordingTitle", "Opptak")}
        </h2>
        <input
          className="rounded border border-zinc-700 bg-transparent px-2 py-1 text-sm"
          placeholder={t("handoff.recordingPath", "Sti til opptak")}
          value={recordingPath}
          onChange={(e) => setRecordingPath(e.target.value)}
          aria-label={t("handoff.recordingPath", "Sti til opptak")}
        />
        <div className="flex gap-2">
          <button
            type="button"
            disabled={!recordingPath.trim() || submitUsageMutation.isPending}
            className="rounded border border-zinc-700 px-2 py-1 text-xs hover:bg-zinc-800 disabled:opacity-50"
            onClick={onSubmitUsage}
          >
            {t("handoff.submitUsage", "Send bruk til SundaySong")}
          </button>
          <button
            type="button"
            disabled={!recordingPath.trim() || verbatimSendMutation.isPending}
            className="rounded border border-zinc-700 px-2 py-1 text-xs hover:bg-zinc-800 disabled:opacity-50"
            onClick={onVerbatimSend}
          >
            {t("handoff.verbatimSend", "Åpne i SundayEdit")}
          </button>
        </div>
        {submitResult && (
          <p
            className={`text-xs ${submitResult.ok ? "text-emerald-300" : "text-amber-300"}`}
            role="status"
          >
            {submitResult.ok
              ? t("handoff.usageOk", "Bruk sendt ({{n}}).", {
                  n: submitResult.submitted ?? 0,
                })
              : (submitResult.hint ??
                submitResult.error ??
                t("handoff.usageFailed", "Kunne ikke sende bruk."))}
          </p>
        )}
        {sendResult && (
          <p
            className={`text-xs ${sendResult.ok ? "text-emerald-300" : "text-amber-300"}`}
            role="status"
          >
            {sendResult.ok
              ? t("handoff.sendOk", "Åpnet i SundayEdit.")
              : t(
                  "handoff.sendFailed",
                  "SundayEdit er ikke installert (verbatim://-skjema mangler).",
                )}
          </p>
        )}
      </div>
    </section>
  );
}
