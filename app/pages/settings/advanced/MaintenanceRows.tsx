/**
 * Raden som handler om en fil på maskinen: loggen.
 *
 * Den er en knapp og ingen innstilling, så den går ikke gjennom `useSetting` —
 * det er ikke noe å lagre, bare noe å gjøre. Kvitteringen er en toast, som er
 * husets svar når handlingen ikke tilhører en verdi i en rad.
 */

import { t } from "../../../i18n";
import { Button } from "../../../ui/Button/Button";
import { revealResult } from "../../../ui/reveal";
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
