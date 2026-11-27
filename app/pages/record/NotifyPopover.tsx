/**
 * Varsler — popoveren bak hjørneknappen på «Varsel»-flisen i ukestripa.
 *
 * Flisen er bare en bryter (og påminnelsen under den). Spørsmålet «kommer
 * varslene faktisk fram på DENNE maskinen?» hadde sin egen rad på den gamle
 * Varsler-siden: statuslinja fra operativsystemet, «Send testvarsel» og
 * «Åpne systeminnstillinger». Uten dem fantes ingen vei til å PRØVE varslene
 * før en feil midt i en gudstjeneste skulle bruke dem.
 *
 * Tilstanden er `notificationPermission` (delt med stripa). På macOS kan den
 * ikke leses (`"unknown"`): da er testvarselet selve svaret, og
 * systeminnstillingene tilbys alltid der.
 */

import { useState } from "preact/hooks";

import { t } from "../../i18n";
import {
  notificationPermission,
  sendTestNotification,
} from "../../state/notification-permission";
import { Button } from "../../ui/Button/Button";
import { toast } from "../../ui/toast";
import styles from "./WakePopover.module.css";

export function NotifyPopover() {
  const permission = notificationPermission.value;
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<
    "sent" | "sentDenied" | "failed" | null
  >(null);

  async function sendTest(): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      // Første trykk på en Mac som aldri er spurt: macOS-dialogen kommer FØR
      // varselet, og statuslinja leses på nytt med svaret.
      const result = await sendTestNotification(window.api);
      if (!result.sent) {
        setOutcome("failed");
        return;
      }
      // Sendt er ikke det samme som vist: står varsler av, sier teksten det.
      setOutcome(result.permission === "denied" ? "sentDenied" : "sent");
    } catch (err) {
      console.warn("[strip] testvarsel:", err);
      setOutcome("failed");
    } finally {
      setBusy(false);
    }
  }

  async function openSystemSettings(): Promise<void> {
    if (!(await window.api.notificationOpenSettings())) {
      toast("error", t("app.setup.notify.openSettingsFailed"));
    }
  }

  const status =
    permission === "granted"
      ? t("app.setup.notify.permissionGranted")
      : permission === "denied"
        ? t("app.setup.notify.permissionDenied")
        : t("app.setup.notify.permissionUnknown");

  return (
    <div class={styles.pop} data-testid="strip-notify-pop">
      <section class={styles.section}>
        <p
          class={styles.sentence}
          data-testid="strip-notify-status"
          data-permission={permission ?? "unknown"}
        >
          {status}
        </p>
        <div class={styles.row}>
          <Button
            variant="secondary"
            busy={busy}
            testId="strip-notify-test"
            onClick={() => void sendTest()}
          >
            {t("app.setup.notify.sendTest")}
          </Button>
          {permission !== "granted" ? (
            <Button
              variant={permission === "denied" ? "primary" : "ghost"}
              testId="strip-notify-pop-open-settings"
              onClick={() => void openSystemSettings()}
            >
              {t("app.setup.notify.openSettings")}
            </Button>
          ) : null}
        </div>
        {outcome ? (
          <p
            class={styles.sentence}
            role="status"
            data-testid="strip-notify-test-word"
          >
            {outcome === "sent"
              ? t("app.setup.notify.testSent")
              : outcome === "sentDenied"
                ? t("app.setup.notify.testSentDenied")
                : t("app.setup.notify.testFailed")}
          </p>
        ) : null}
      </section>
    </div>
  );
}
