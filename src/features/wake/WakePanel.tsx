import { invoke } from "@tauri-apps/api/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { WakeCapabilities } from "@/lib/bindings/WakeCapabilities";
import type { SleepConfig } from "@/lib/bindings/SleepConfig";
import type { WakeStatus } from "@/lib/bindings/WakeStatus";
import type { WakeResult } from "@/lib/bindings/WakeResult";
import type { WakeFixResult } from "@/lib/bindings/WakeFixResult";

const CAPS_KEY = ["wake_capabilities"] as const;
const SLEEP_KEY = ["wake_sleep_config"] as const;

/** True if the OS sleep config has a setting that will sabotage wake. */
function sleepNeedsFix(c: SleepConfig): boolean {
  // mac: deep-sleep standby (or autopoweroff) powers down the SoC → wake fails.
  if (c.standby === true || c.autopoweroff === true) return true;
  // windows: wake timers explicitly disabled.
  if (c.wakeTimersEnabled === false) return true;
  return false;
}

function fmt(s: string): string {
  const d = new Date(s);
  if (Number.isNaN(d.getTime())) return s;
  return d.toLocaleString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * Wake-from-sleep panel (Fase 5.2). Surfaces this machine's wake capabilities,
 * flags sleep settings that would stop a scheduled recording from waking it
 * (with a one-click "fix"), and lets the user (re)register the OS wake timers
 * and verify them against the current schedule.
 */
export function WakePanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const { data: caps } = useQuery<WakeCapabilities>({
    queryKey: CAPS_KEY,
    queryFn: () => invoke<WakeCapabilities>("wake_capabilities"),
  });

  const { data: sleep } = useQuery<SleepConfig>({
    queryKey: SLEEP_KEY,
    queryFn: () => invoke<SleepConfig>("wake_get_sleep_config"),
  });

  const fixMutation = useMutation({
    mutationFn: () => invoke<WakeFixResult>("wake_fix_sleep"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: SLEEP_KEY }),
  });

  const rescheduleMutation = useMutation({
    mutationFn: () => invoke<WakeResult>("wake_reschedule"),
  });

  const verifyMutation = useMutation({
    mutationFn: () => invoke<WakeStatus>("wake_verify"),
  });

  return (
    <div className="flex flex-col gap-4 text-left" data-testid="wake-panel">
      {/* Capabilities */}
      {caps && (
        <section className="sr-card pad-lg">
          <h3 className="text-sm font-medium text-text">
            {t("wake.capabilities", "Maskinens muligheter")}
          </h3>
          <p className="mt-1 text-xs text-text2" data-testid="wake-platform">
            {caps.platform} · {t("wake.fromSleep", "Fra dvale")}:{" "}
            {caps.canWakeFromSleep ? "✅" : "❌"} ·{" "}
            {t("wake.fromOff", "Fra avslått")}:{" "}
            {caps.canWakeFromOff ? "✅" : "❌"}
          </p>
          {caps.knownIssues.length > 0 && (
            <ul className="mt-2 list-disc pl-5 text-xs text-accent">
              {caps.knownIssues.map((k) => (
                <li key={k}>{k}</li>
              ))}
            </ul>
          )}
          {caps.recommendations.length > 0 && (
            <ul className="mt-2 list-disc pl-5 text-xs text-text2">
              {caps.recommendations.map((r) => (
                <li key={r}>{r}</li>
              ))}
            </ul>
          )}
        </section>
      )}

      {/* Sleep config warning + fix */}
      {sleep && sleepNeedsFix(sleep) && (
        <section
          className="rounded-xl p-4"
          style={{
            background: "var(--sr-gold-tint)",
            border: "1px solid var(--sr-gold-line)",
          }}
          data-testid="wake-sleep-warning"
        >
          <p className="text-sm" style={{ color: "var(--sr-gold-bright)" }}>
            {t(
              "wake.sleepWarning",
              "Strøminnstillingene kan hindre maskinen i å våkne for planlagte opptak.",
            )}
          </p>
          <button
            type="button"
            className="sr-btn gold sm mt-2"
            disabled={fixMutation.isPending}
            onClick={() => fixMutation.mutate()}
          >
            {t("wake.fixAuto", "Fiks automatisk")}
          </button>
          {fixMutation.data && (
            <p
              className="mt-1 text-xs text-text2"
              data-testid="wake-fix-result"
            >
              {fixMutation.data.ok
                ? t("wake.fixOk", "Fikset.")
                : (fixMutation.data.message ??
                  t("wake.fixFailed", "Mislyktes."))}
            </p>
          )}
        </section>
      )}

      {/* Actions */}
      <section className="flex flex-wrap gap-2">
        <button
          type="button"
          className="sr-btn ghost sm"
          disabled={rescheduleMutation.isPending}
          onClick={() => rescheduleMutation.mutate()}
        >
          {t("wake.scheduleNow", "Planlegg vekking nå")}
        </button>
        <button
          type="button"
          className="sr-btn ghost sm"
          disabled={verifyMutation.isPending}
          onClick={() => verifyMutation.mutate()}
        >
          {t("wake.verify", "Verifiser")}
        </button>
      </section>

      {rescheduleMutation.data && (
        <p className="text-xs text-text2" data-testid="wake-schedule-result">
          {rescheduleMutation.data.ok
            ? t("wake.scheduled", "Planlagt {count} vekkinger.").replace(
                "{count}",
                String(rescheduleMutation.data.count ?? 0),
              )
            : `${t("wake.scheduleFailed", "Kunne ikke planlegge")}: ${
                rescheduleMutation.data.reason ?? "?"
              }`}
        </p>
      )}

      {/* Verification result */}
      {verifyMutation.data && (
        <section
          className="sr-card pad text-xs"
          data-testid="wake-verify-result"
        >
          {verifyMutation.data.hasMismatch ? (
            <p className="text-accent">
              {t(
                "wake.mismatch",
                "Noen forventede vekkinger mangler i operativsystemet.",
              )}
            </p>
          ) : (
            <p style={{ color: "var(--sr-green)" }}>
              {t(
                "wake.allScheduled",
                "Alle forventede vekkinger er registrert.",
              )}
            </p>
          )}
          <p className="mt-2 text-text2">
            {t("wake.expected", "Forventet")}:{" "}
            {verifyMutation.data.expectedWakes.length} ·{" "}
            {t("wake.observed", "Observert")}:{" "}
            {verifyMutation.data.observedWakes.length}
          </p>
          {verifyMutation.data.onBattery === true && (
            <p className="mt-1 text-accent">
              {t(
                "wake.onBattery",
                "På batteri — wake er mindre pålitelig uten strøm.",
              )}
            </p>
          )}
          {verifyMutation.data.observedWakes.length > 0 && (
            <ul className="mt-2 list-disc pl-5 text-text2">
              {verifyMutation.data.observedWakes.slice(0, 5).map((o) => (
                <li key={`${o.scheduledAt}-${o.ownerLabel}`}>
                  {fmt(o.scheduledAt)} — {o.ownerLabel}
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
    </div>
  );
}
