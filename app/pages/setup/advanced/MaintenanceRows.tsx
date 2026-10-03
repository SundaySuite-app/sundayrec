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
import { revealResult } from "../../../ui/reveal";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { toast } from "../../../ui/toast";
import { oneAtATime, runExport, runImport } from "./profile-core";

/** Ett profilvindu om gangen, for begge knappene (se `oneAtATime`). */
const oneProfileDialog = oneAtATime();

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
  // `reveal(path)` (`app/ui/reveal.ts`) tar ikke logg-raden: `logs_reveal` er
  // en annen kommando enn `revealFile`, MED VILJE uten en sti (se
  // `src-tauri/src/commands/logs.rs`s filhode) — så bare `revealResult`, den
  // delte toast-på-`false`-formen, passer her. Egen feiltekst: `revealFailed`
  // handler om en FIL, denne om en MAPPE.
  async function reveal(): Promise<void> {
    await revealResult(
      await window.api.logsReveal(),
      t("app.setup.advanced.logShowFailed"),
    );
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
 * Lagre-/åpne-vinduet åpnes av RUST, ikke herfra: ingen sti krysser grensen,
 * så et webview kan ikke peke eksporten på en annen fil enn den brukeren
 * valgte (funn A1). Rekkefølgen og hva svarene betyr står i `profile-core.ts`;
 * et avbrutt vindu skal ikke si noe.
 *
 * Importen spør først, fordi den erstatter alt — og FØR vinduet, fordi vinduet
 * og importen nå er ett steg i bakenden.
 */
export function ProfileRow() {
  // Nøklene står som literaler i hvert kall, ikke som parametre til en felles
  // hjelper: i18n-gatene leser `t("…")`-kallene, og en nøkkel i en variabel
  // er en nøkkel de ikke ser.
  async function exportProfile(): Promise<void> {
    const outcome = await runExport(() => window.api.settingsExportProfile());
    if (outcome.kind === "done") {
      toast("success", t("app.setup.advanced.exported"));
    } else if (outcome.kind === "failed") {
      toast(
        "error",
        tf("app.setup.advanced.exportFailed", { err: outcome.err }),
      );
    }
  }

  async function importProfile(): Promise<void> {
    const outcome = await runImport({
      confirm: () =>
        confirmDialog({
          title: t("app.setup.advanced.importTitle"),
          message: t("app.setup.advanced.importBody"),
          confirmLabel: t("app.setup.advanced.importConfirm"),
          cancelLabel: t("app.setup.cancel"),
          danger: true,
        }),
      importProfile: () => window.api.settingsImportProfile(),
      rehydrate: hydrateSettings,
    });
    if (outcome.kind === "done") {
      toast("success", t("app.setup.advanced.imported"));
    } else if (outcome.kind === "failed") {
      // En fil som ikke er en profil — eller er alt for stor — har sin egen
      // setning: «Ingenting ble endret» er det viktigste å få vite da.
      toast(
        "error",
        outcome.refusal === "notProfile"
          ? t("app.setup.advanced.importNotProfile")
          : outcome.refusal === "tooLarge"
            ? t("app.setup.advanced.importTooLarge")
            : tf("app.setup.advanced.importFailed", { err: outcome.err }),
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
        onClick={() => void oneProfileDialog(exportProfile)}
      >
        {t("app.setup.advanced.export")}
      </Button>
      <Button
        variant="ghost"
        testId="adv-profile-import"
        onClick={() => void oneProfileDialog(importProfile)}
      >
        {t("app.setup.advanced.import")}
      </Button>
    </SettingRow>
  );
}
