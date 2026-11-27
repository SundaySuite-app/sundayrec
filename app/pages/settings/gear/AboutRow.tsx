/**
 * Om — versjonen, loggen, lisensene og (på Windows) ASIO-krediteringen.
 *
 * Loggen er en knapp og ingen innstilling, så den går ikke gjennom
 * `useSetting` — det er ikke noe å lagre, bare noe å gjøre. Kvitteringen er en
 * toast, som er husets svar når handlingen ikke tilhører en verdi i en rad.
 */

import { useState } from "preact/hooks";

import { t } from "../../../i18n";
import { appVersion } from "../../../state/app-info";
import { Icon } from "../../../ui/Icon/Icon";
import { revealResult } from "../../../ui/reveal";
import { toast } from "../../../ui/toast";
import { AsioAttribution } from "./AsioAttribution";
import { GearRow } from "./GearRow";
import styles from "./gear.module.css";
import { LicensesViewer, loadLicenses } from "./LicensesViewer";

/** Legacy ber om 200 kB; serveren klamrer uansett til 512 kB. */
const LOG_TAIL_BYTES = 200 * 1024;

/** Produktnavnet er en konstant, ikke tekst i treet: det oversettes ikke. */
const PRODUCT = "SundayRec";

export function AboutRow() {
  const [html, setHtml] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  /**
   * «Logg» — åpner mappen. En vellykket åpning sier ingenting: Finder/
   * Utforsker åpner seg foran deg, og en toast oppå det ville vært å fortelle
   * noen om noe de ser på. En FEILET åpning sier fra, fordi da skjedde det
   * ingenting synlig.
   */
  async function reveal(): Promise<void> {
    await revealResult(
      await window.api.logsReveal(),
      t("app.setup.advanced.logShowFailed"),
    );
  }

  /**
   * «Kopier logg» — halen på utklippstavlen.
   *
   * ⚠️ En tom logg er et gyldig svar fra bakenden, ikke en feil. «Loggen er
   * tom ennå» er noe helt annet enn «kunne ikke kopiere», og den som feilsøker
   * trenger å vite hvilken av dem det var.
   */
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

  async function openLicenses(): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      setHtml(await loadLicenses());
    } finally {
      setBusy(false);
    }
  }

  return (
    <GearRow
      testId="gear-row-about"
      anchors={["about", "licenses", "log"]}
      icon="info"
      title={t("app.gear.about")}
      sub={
        appVersion.value ? (
          <span data-testid="gear-version" class={styles.version}>
            {PRODUCT} {appVersion.value}
          </span>
        ) : undefined
      }
      controls={
        <>
          <span class={styles.aboutLine}>
            <button
              type="button"
              class={styles.pill}
              data-testid="adv-log-show"
              title={t("app.gear.logShow")}
              onClick={() => void reveal()}
            >
              {t("app.gear.log")}
            </button>
            <button
              type="button"
              class={styles.iconBtn}
              data-testid="adv-log-copy"
              aria-label={t("app.gear.logCopy")}
              title={t("app.gear.logCopy")}
              onClick={() => void copy()}
            >
              <Icon name="copy" size={20} />
            </button>
          </span>
          <button
            type="button"
            class={styles.pill}
            data-testid="adv-licenses-open"
            aria-busy={busy ? "true" : undefined}
            onClick={() => void openLicenses()}
          >
            {t("app.gear.licenses")}
          </button>
        </>
      }
      extra={
        <>
          <AsioAttribution />
          {html !== null ? (
            <LicensesViewer html={html} onClose={() => setHtml(null)} />
          ) : null}
        </>
      }
    />
  );
}
