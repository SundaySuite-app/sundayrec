/**
 * De to radene som handler om filer på maskinen: loggen og innstillingsprofilen.
 *
 * Begge er knapper og ingen innstilling, så de går ikke gjennom `useSetting` —
 * det er ikke noe å lagre, bare noe å gjøre. Kvitteringen er en toast, som er
 * husets svar når handlingen ikke tilhører en verdi i en rad.
 */

import { t, tf } from "../../../i18n";
import { hydrateSettings } from "../../../state/settings";
import { Button } from "../../../ui/Button/Button";
import { confirmDialog } from "../../../ui/dialog";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { toast } from "../../../ui/toast";

/** Legacy ber om 200 kB; serveren klamrer uansett til 512 kB. */
const LOG_TAIL_BYTES = 200 * 1024;

/**
 * Loggen — «Vis» åpner mappen, «Kopier» legger halen på utklippstavlen.
 *
 * En vellykket «Vis» sier ingenting: Finder/Utforsker åpner seg foran deg, og
 * en toast oppå det ville vært å fortelle noen om noe de ser på. En FEILET
 * åpning sier fra, fordi da skjedde det ingenting synlig.
 *
 * ⚠️ En tom logg er et gyldig svar fra bakenden, ikke en feil. Legacy skiller
 * de to, og det gjør vi også: «Loggen er tom ennå» er noe helt annet enn «kunne
 * ikke kopiere», og den som feilsøker trenger å vite hvilken av dem det var.
 */
export function LogRow() {
  async function reveal(): Promise<void> {
    const ok = await window.api.logsReveal();
    if (!ok) toast("error", t("app.setup.advanced.logShowFailed"));
  }

  async function copy(): Promise<void> {
    try {
      const text = await window.api.logsTail(LOG_TAIL_BYTES);
      if (!text) {
        toast("info", t("app.setup.advanced.logEmpty"));
        return;
      }
      await navigator.clipboard.writeText(text);
      toast("success", t("app.setup.advanced.logCopied"));
    } catch {
      toast("error", t("app.setup.advanced.logCopyFailed"));
    }
  }

  return (
    <SettingRow
      label={t("app.setup.advanced.log")}
      description={t("app.setup.advanced.logDesc")}
      testId="adv-log"
    >
      <Button
        variant="ghost"
        testId="adv-log-show"
        onClick={() => void reveal()}
      >
        {t("app.setup.advanced.show")}
      </Button>
      <Button variant="ghost" testId="adv-log-copy" onClick={() => void copy()}>
        {t("app.setup.advanced.copy")}
      </Button>
    </SettingRow>
  );
}

/**
 * Innstillingsprofilen — hele oppsettet som én JSON-fil.
 *
 * De native dialogene ER stiautoriseringen; bakenden sjekker på nytt med sine
 * egne les/skriv-policyer. En avbrutt dialog svarer `null` og skal ikke si noe.
 *
 * Importen spør først, fordi den erstatter alt. Etterpå leses innstillingene
 * inn på nytt gjennom den vanlige veien (`hydrateSettings`) i stedet for å
 * stole på returverdien: signalene er det ene stedet skjermen leser fra, og en
 * import som bare oppdaterte returverdien ville latt hver åpne skjerm stå og
 * vise det gamle.
 */
export function ProfileRow() {
  async function exportProfile(): Promise<void> {
    const path = await window.api.pickSavePath({
      defaultPath: "sundayrec-innstillinger.json",
      name: "JSON",
      extensions: ["json"],
    });
    if (!path) return;
    try {
      await window.api.settingsExportToFile(path);
      toast("success", t("app.setup.advanced.exported"));
    } catch (err) {
      toast(
        "error",
        tf("app.setup.advanced.exportFailed", { err: errText(err) }),
      );
    }
  }

  async function importProfile(): Promise<void> {
    const path = await window.api.pickSettingsFile();
    if (!path) return;
    const ok = await confirmDialog({
      title: t("app.setup.advanced.importTitle"),
      message: t("app.setup.advanced.importBody"),
      confirmLabel: t("app.setup.advanced.importConfirm"),
      cancelLabel: t("app.setup.cancel"),
      danger: true,
    });
    if (!ok) return;
    try {
      await window.api.settingsImportFromFile(path);
      await hydrateSettings();
      toast("success", t("app.setup.advanced.imported"));
    } catch (err) {
      toast(
        "error",
        tf("app.setup.advanced.importFailed", { err: errText(err) }),
      );
    }
  }

  return (
    <SettingRow
      label={t("app.setup.advanced.profile")}
      description={t("app.setup.advanced.profileDesc")}
      testId="adv-profile"
    >
      <Button
        variant="ghost"
        testId="adv-profile-export"
        onClick={() => void exportProfile()}
      >
        {t("app.setup.advanced.export")}
      </Button>
      <Button
        variant="ghost"
        testId="adv-profile-import"
        onClick={() => void importProfile()}
      >
        {t("app.setup.advanced.import")}
      </Button>
    </SettingRow>
  );
}

/** Feilteksten en bruker faktisk kan gi videre. «[object Object]» er ikke en. */
function errText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
