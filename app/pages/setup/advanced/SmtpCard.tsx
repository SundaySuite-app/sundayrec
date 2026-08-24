/**
 * E-postserveren (SMTP) — porten spørsmål 5 står bak.
 *
 * ## Hvorfor den bor HER og ikke på spørsmål 5
 *
 * `feature-gate-core` skriver ned regelen: en gate som slår av sine egne
 * oppsettsfelter kan aldri konfigureres. Spørsmål 5 har en `Gate` foran
 * e-postbryteren som sier «Krever en e-postserver (SMTP). Sett opp under
 * Avansert» — og det er trygt nettopp fordi feltene er på en ANNEN skjerm.
 * Denne. Når de tre feltene og passordet er på plass, åpner gaten seg der.
 *
 * ## Eksplisitt Lagre
 *
 * `useDraftForm`, ikke auto-anvend. En halvskrevet vert («smtp.gmai») ville
 * blitt lagret på hvert tastetrykk, og bakenden ville prøvd å sende gjennom den
 * ved neste feil. Og en feilet lagring ruller IKKE tilbake: det er noe brukeren
 * har skrevet.
 *
 * ## Passordet er ikke en innstilling
 *
 * Det er ikke et `Settings`-felt i det hele tatt, så det kan aldri ri med en
 * lagring inn i basen. `email_set_smtp_password` legger det i OS-nøkkelringen,
 * `email_has_smtp_password` svarer bare JA/NEI, og selve hemmeligheten krysser
 * aldri inn i webviewet igjen. Derfor: et felt som alltid er tomt, og en linje
 * som sier om det ligger et passord der fra før.
 *
 * ## Porten (`emailSmtpPort`)
 *
 * Ikke synlig. Legacy har den som `type="hidden"` med fast 587, og en frivillig
 * som skal gjette et portnummer har allerede tapt. Verdien i basen står urørt.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tf } from "../../../i18n";
import { useDraftForm } from "../../../settings/use-draft-form";
import { emailFacts, refreshEmailFacts } from "../../../state/email";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../../state/settings";
import { Button } from "../../../ui/Button/Button";
import { Card } from "../../../ui/Card/Card";
import { Chip } from "../../../ui/Chip/Chip";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { TextField } from "../../../ui/TextField/TextField";
import { toast } from "../../../ui/toast";
import { useReceipt } from "../../../settings/use-receipt";
import styles from "../setup.module.css";

/** Samme regex som spørsmål 5 og legacy bruker på det samme feltet. */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

interface SmtpDraft {
  host: string;
  user: string;
  from: string;
}

export function SmtpCard() {
  const s = settings.value;
  const facts = emailFacts.value;
  const { receipt, show: showReceipt, reset: resetReceipt } = useReceipt();
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refreshEmailFacts();
  }, []);

  const form = useDraftForm<SmtpDraft>(
    () => ({
      host: s.emailSmtp ?? "",
      user: s.emailSmtpUser ?? "",
      from: s.emailSmtpFrom ?? "",
    }),
    async (value) => {
      patchSettings({
        emailSmtp: value.host.trim(),
        emailSmtpUser: value.user.trim(),
        emailSmtpFrom: value.from.trim(),
      });
      const ok = await saveSettingsDebounced(120);
      if (!ok) toast("error", t("general.saveFailed"));
      // Porten på spørsmål 5 åpner seg av dette, så fakta må leses på nytt —
      // ellers står den gaten og sier «mangler e-postserver» på en maskin som
      // nettopp fikk en.
      await refreshEmailFacts();
      return ok;
    },
  );

  const from = form.draft.from.trim();
  const fromInvalid = from.length > 0 && !EMAIL_RE.test(from);

  async function save(): Promise<void> {
    if (fromInvalid) {
      setError(t("app.setup.advanced.smtpInvalidFrom"));
      return;
    }
    setError(null);
    showReceipt("saving");
    showReceipt((await form.save()) ? "saved" : "failed");
  }

  return (
    <Card
      title={t("app.setup.advanced.smtpTitle")}
      description={t("app.setup.advanced.smtpDesc")}
      anchor="smtp"
      testId="advanced-smtp"
    >
      <SettingRow
        label={t("app.setup.advanced.smtpHost")}
        description={t("app.setup.advanced.smtpHostDesc")}
        receipt={receipt}
        testId="adv-smtp-host"
      >
        {(ids) => (
          <TextField
            value={form.draft.host}
            onInput={(next) => {
              resetReceipt();
              form.set({ host: next });
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-smtp-host-control-input"
          />
        )}
      </SettingRow>

      <SettingRow
        label={t("app.setup.advanced.smtpUser")}
        testId="adv-smtp-user"
      >
        {(ids) => (
          <TextField
            value={form.draft.user}
            onInput={(next) => {
              resetReceipt();
              form.set({ user: next });
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-smtp-user-control-input"
          />
        )}
      </SettingRow>

      <SettingRow
        label={t("app.setup.advanced.smtpFrom")}
        description={t("app.setup.advanced.smtpFromDesc")}
        error={error}
        testId="adv-smtp-from"
      >
        {(ids) => (
          <TextField
            value={form.draft.from}
            type="email"
            invalid={fromInvalid}
            onInput={(next) => {
              setError(null);
              resetReceipt();
              form.set({ from: next });
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-smtp-from-control-input"
          />
        )}
      </SettingRow>

      <div class={styles.footer}>
        <Button
          variant="primary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.advanced.smtpNothingToSave")}
          testId="adv-smtp-save"
          onClick={() => void save()}
        >
          {t("app.setup.save")}
        </Button>
        <Button
          variant="secondary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.advanced.smtpNothingToSave")}
          testId="adv-smtp-cancel"
          onClick={() => {
            setError(null);
            resetReceipt();
            form.cancel();
          }}
        >
          {t("app.setup.cancel")}
        </Button>
      </div>

      <PasswordRow stored={facts?.smtpPasswordAvailable === true} />
    </Card>
  );
}

/**
 * Passordet — det ene feltet som ikke leses tilbake.
 *
 * Feltet står alltid tomt. Å fylle det med prikker som representerer en verdi
 * vi ikke har ville vært en løgn om hva som ligger i nøkkelringen, og en som
 * bare avsløres den dagen «Lagre» skriver prikkene tilbake.
 */
function PasswordRow({ stored }: { stored: boolean }) {
  const [value, setValue] = useState("");
  const { receipt, show: showReceipt, reset: resetReceipt } = useReceipt();
  const [busy, setBusy] = useState(false);

  async function write(password: string | undefined): Promise<void> {
    if (busy) return;
    setBusy(true);
    showReceipt("saving");
    try {
      await window.api.emailSetSmtpPassword(password);
      setValue("");
      showReceipt("saved");
      toast(
        "success",
        password
          ? t("app.setup.advanced.smtpPasswordSaved")
          : t("app.setup.advanced.smtpPasswordCleared"),
      );
      await refreshEmailFacts();
    } catch (err) {
      // IKKE svelget: en feilet nøkkelring-skrivning som ser ut som en
      // vellykket er hvordan en menighet tror varslene er satt opp.
      showReceipt("failed");
      toast(
        "error",
        tf("app.setup.advanced.smtpPasswordFailed", {
          err: err instanceof Error ? err.message : String(err),
        }),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingRow
      label={t("app.setup.advanced.smtpPassword")}
      description={t("app.setup.advanced.smtpPasswordDesc")}
      receipt={receipt}
      testId="adv-smtp-password"
    >
      {(ids) => (
        <>
          <Chip
            tone={stored ? "good" : "neutral"}
            testId="adv-smtp-password-state"
          >
            {stored
              ? t("app.setup.advanced.smtpPasswordStored")
              : t("app.setup.advanced.smtpPasswordNone")}
          </Chip>
          <TextField
            value={value}
            type="password"
            disabled={busy}
            onInput={(next) => {
              resetReceipt();
              setValue(next);
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-smtp-password-control-input"
          />
          <Button
            variant="secondary"
            disabled={value.trim().length === 0 || busy}
            disabledReason={t("app.setup.advanced.smtpNothingToSave")}
            testId="adv-smtp-password-save"
            onClick={() => void write(value)}
          >
            {t("app.setup.advanced.smtpPasswordSave")}
          </Button>
          <Button
            variant="ghost"
            disabled={!stored || busy}
            disabledReason={t("app.setup.advanced.smtpPasswordNone")}
            testId="adv-smtp-password-clear"
            onClick={() => void write(undefined)}
          >
            {t("app.setup.advanced.smtpPasswordClear")}
          </Button>
        </>
      )}
    </SettingRow>
  );
}
