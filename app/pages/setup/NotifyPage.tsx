/**
 * 5 — Hvem får beskjed hvis noe går galt? (canvasens artboard 5.3)
 *
 * ## Én bryter for OS-varsler, ikke to
 *
 * Bakenden har `notifyStart` og `notifyStop`, og dagens app har en bryter for
 * hver. Ingen frivillig har et forhold til den forskjellen: enten sier maskinen
 * fra om opptaket, eller så gjør den ikke det. Så: ÉN bryter som skriver begge.
 *
 * ⚠️ Canvasens tekst her sa «Alltid på» ved siden av en bryter som kan slås av.
 * Den motsigelsen er rettet — teksten sier hva bryteren gjør, ikke hva vi
 * skulle ønske den gjorde.
 *
 * ## «Sendes via SundaySuite» er IKKE sant
 *
 * Canvasens SMTP-setning lover et relé som ikke finnes: det er ingen
 * SundaySuite-tjeneste som sender disse e-postene. Sendingen går gjennom
 * menighetens EGEN SMTP-server, og uten en slik server går det ingen e-post
 * uansett hva som står i adressefeltet. Teksten her sier det, og bryteren står
 * bak en `Gate` som sier hvor man setter det opp.
 *
 * Gaten er trygg her (og bare her): SMTP-feltene bor under Avansert, ikke på
 * denne skjermen. `feature-gate-core` advarer mot det motsatte — en gate som
 * slår av sine egne oppsettsfelter kan aldri konfigureres.
 *
 * ## Adressen lagres EKSPLISITT
 *
 * Den ene innstillingen i appen der en halvskrevet verdi er aktivt skadelig:
 * «post@» er en adresse ingenting kommer fram til, og du oppdager det den
 * dagen opptaket faktisk feiler. Så `useDraftForm` med Lagre/Avbryt, og en
 * feilet lagring ruller IKKE tilbake — det er noe brukeren skrev.
 */

import { useEffect, useState } from "preact/hooks";

import { emailBlockReason, type GateStatus } from "@lib/ui/feature-gate-core";

import { locale, t, tf } from "../../i18n";
import { useDraftForm } from "../../settings/use-draft-form";
import { usePatch } from "../../settings/use-patch";
import { useReceipt } from "../../settings/use-receipt";
import { useSetting } from "../../settings/use-setting";
import { emailFacts, refreshEmailFacts } from "../../state/email";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Gate } from "../../ui/Gate/Gate";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { TextField } from "../../ui/TextField/TextField";
import { Toggle } from "../../ui/Toggle/Toggle";
import { toast } from "../../ui/toast";
import { notifyGateStatus } from "./decisions-core";
import { autoRecordOn } from "./schedule-core";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

/** Samme regex som legacy bruker på det samme feltet. */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** Minuttene «påminnelse før opptak» tilbyr. 0 = av. */
const REMINDER_CHOICES = [0, 5, 10, 15, 30, 60];

export function NotifyPage() {
  const s = settings.value;
  const facts = emailFacts.value;
  const gate = notifyGateStatus(facts);

  useEffect(() => {
    void refreshEmailFacts();
  }, []);

  // ── OS-varsler: én bryter, to nøkler ──────────────────────────────────────
  // `usePatch` og ikke en håndlagd lagring: sekvensen (anvend → skriv →
  // kvittering | rull tilbake) er den samme som `useSetting` kjører, og
  // kvitteringen teller ned i stedet for å bli stående som «Lagret ✓» til
  // siden forlates.
  const osOn = s.notifyStart || s.notifyStop;
  const osNotify = usePatch();

  const emailOn = useSetting("emailOnError", {
    kind: "toggle",
    after: () => void refreshEmailFacts(),
  });

  const reminder = useSetting("reminderMinutes", { kind: "select" });
  // Flagget OG en tid — en påminnelse før et opptak som ikke er armert er en
  // beskjed om noe som ikke skal skje.
  const autoOn = autoRecordOn(s);

  return (
    <SubPage lede={t("app.setup.notify.lede")} testId="setup-notify">
      <Card testId="notify-card">
        <SettingRow
          label={t("app.setup.notify.os")}
          description={t("app.setup.notify.osDesc")}
          receipt={osNotify.receipt}
          testId="notify-os"
        >
          {(ids) => (
            <Toggle
              checked={osOn}
              onChange={(next) =>
                void osNotify.write({ notifyStart: next, notifyStop: next })
              }
              disabled={osNotify.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="notify-os-control-input"
            />
          )}
        </SettingRow>

        <Gate
          status={gate}
          testId="notify-email-gate"
          chipText={
            gate === "unavailable"
              ? t("app.setup.notify.gateChipUnavailable")
              : t("app.setup.notify.gateChip")
          }
          explanation={
            gate === "unavailable"
              ? t("app.setup.notify.gateNoFeature")
              : t("app.setup.notify.gateSmtp")
          }
        >
          <SettingRow
            label={t("app.setup.notify.mail")}
            description={t("app.setup.notify.mailDesc")}
            receipt={emailOn.receipt}
            error={emailOn.error}
            testId="notify-email"
          >
            {(ids) => (
              <Toggle
                checked={emailOn.draft === true}
                onChange={(next) => emailOn.set(next)}
                disabled={emailOn.busy}
                labelId={ids.labelId}
                describedBy={ids.describedBy}
                testId="notify-email-control-input"
              />
            )}
          </SettingRow>
        </Gate>
      </Card>

      <AddressCard gate={gate} />

      <Card testId="notify-reminder-card">
        <Gate
          status={autoOn ? "ok" : "unconfigured"}
          testId="notify-reminder-gate"
          chipText={t("app.setup.notify.remindChip")}
          explanation={t("app.setup.notify.remindGate")}
        >
          <SettingRow
            label={t("app.setup.notify.remind")}
            description={t("app.setup.notify.remindDesc")}
            receipt={reminder.receipt}
            testId="notify-reminder"
          >
            {(ids) => (
              <Select
                value={String(reminder.draft ?? 0)}
                options={REMINDER_CHOICES.map((n) => ({
                  value: String(n),
                  label:
                    n === 0
                      ? t("app.setup.notify.remindOff")
                      : tf("app.setup.notify.minutes", { n }),
                }))}
                onChange={(next) => reminder.set(Number(next))}
                disabled={reminder.busy}
                labelId={ids.labelId}
                describedBy={ids.describedBy}
                testId="notify-reminder-control-input"
              />
            )}
          </SettingRow>
        </Gate>
      </Card>
    </SubPage>
  );
}

/**
 * Adressen — det ene skjemaet på denne siden med Lagre/Avbryt.
 *
 * «Avbryt» ANGRER her, i motsetning til de tre lagre-knappene i det gamle
 * skallet: utkastet er en egen kopi som ingenting utenfor skjemaet ser før
 * `save()`.
 */
function AddressCard({ gate }: { gate: GateStatus }) {
  const s = settings.value;
  const facts = emailFacts.value;
  const { receipt, show: showReceipt, reset: resetReceipt } = useReceipt();
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);

  const form = useDraftForm(
    () => ({ address: s.emailAddress ?? "" }),
    async (value) => {
      patchSettings({ emailAddress: value.address.trim() });
      const ok = await saveSettingsDebounced(120);
      if (!ok) toast("error", t("general.saveFailed"));
      return ok;
    },
  );

  const address = form.draft.address.trim();
  const invalid = address.length > 0 && !EMAIL_RE.test(address);
  const blocked = emailBlockReason(
    facts ?? {
      featureBuilt: false,
      smtpConfigured: false,
      smtpPasswordAvailable: false,
    },
    !!s.emailAddress,
  );

  async function save(): Promise<void> {
    if (invalid) {
      setError(t("app.setup.notify.invalid"));
      return;
    }
    setError(null);
    showReceipt("saving");
    showReceipt((await form.save()) ? "saved" : "failed");
  }

  async function sendTest(): Promise<void> {
    const to = (s.emailAddress ?? "").trim();
    if (!to || testing) return;
    setTesting(true);
    try {
      const result = await window.api.testEmail({
        recipient: to,
        language: locale.peek(),
        host: s.emailSmtp,
        port: s.emailSmtpPort,
        user: s.emailSmtpUser,
        // Tomt betyr «utled den» i bakenden, som er det hver eksisterende
        // konfigurasjon gjør.
        from: (s.emailSmtpFrom ?? "").trim() || undefined,
      });
      toast(
        result.ok ? "success" : "error",
        result.ok
          ? tf("app.setup.notify.testSent", { to })
          : tf("app.setup.notify.testFailed", { err: result.error ?? "" }),
      );
    } finally {
      setTesting(false);
    }
  }

  return (
    <Card testId="notify-address-card">
      <SettingRow
        label={t("app.setup.notify.to")}
        description={t("app.setup.notify.toDesc")}
        receipt={receipt}
        error={error}
        testId="notify-address"
      >
        {(ids) => (
          <TextField
            value={form.draft.address}
            type="email"
            placeholder={t("app.setup.notify.toPlaceholder")}
            invalid={invalid}
            onInput={(next) => {
              setError(null);
              resetReceipt();
              form.set({ address: next });
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="notify-address-control-input"
          />
        )}
      </SettingRow>

      <div class={styles.footer}>
        <Button
          variant="primary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.notify.nothingToSave")}
          testId="notify-save"
          onClick={() => void save()}
        >
          {t("app.setup.save")}
        </Button>
        <Button
          variant="secondary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.notify.nothingToSave")}
          testId="notify-cancel"
          onClick={() => {
            setError(null);
            resetReceipt();
            form.cancel();
          }}
        >
          {t("app.setup.cancel")}
        </Button>
        <Button
          variant="ghost"
          busy={testing}
          disabled={gate !== "ok" || blocked !== null}
          disabledReason={
            gate === "unavailable"
              ? t("app.setup.notify.gateNoFeature")
              : blocked === "noRecipient"
                ? t("app.setup.notify.testNoRecipient")
                : t("app.setup.notify.gateSmtp")
          }
          testId="notify-test"
          onClick={() => void sendTest()}
        >
          {t("app.setup.notify.test")}
        </Button>
      </div>
    </Card>
  );
}
