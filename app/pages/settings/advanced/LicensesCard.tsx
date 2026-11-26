/**
 * «Lisenser» — tredjepartsmerknadene som følger med installasjonsfilene
 * (OMBYGGING A1b).
 *
 * MIT og Apache-2.0 krever at merknadene følger med det vi distribuerer, og
 * ffmpeg er GPL: den som har en installasjonsfil har krav på å vite det og på å
 * få vite hvor kildene finnes. Fila (`THIRD-PARTY-NOTICES.html`) lages av
 * `npm run licenses:notices`, ligger i bunten som ressurs, og leses av Rust
 * (`licenses_notices`) — ingen sti krysser grensen.
 *
 * Teksten vises i en SANDKASSE-ramme (`sandbox=""`, ingen skript, ingen
 * same-origin): innholdet er generert av oss, men det er hundrevis av kilobyte
 * tekst fra andres crates, og en ramme er den ene flaten som ikke gjør om på den.
 * Vises i appen og ikke i nettleseren, fordi en frivillig på en kirkemaskin
 * uten nettilgang skal kunne lese den like godt.
 *
 * ⚠️ En fil som mangler er en FEIL med egen setning, aldri en tom side: en tom
 * lisensside leses som «det finnes ingen».
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { t } from "../../../i18n";
import { Button } from "../../../ui/Button/Button";
import { Card } from "../../../ui/Card/Card";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { toast } from "../../../ui/toast";
import styles from "./LicensesCard.module.css";

export function LicensesCard() {
  const [html, setHtml] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const viewer = useRef<HTMLDivElement>(null);

  // Fokus inn i leseren, ellers når ikke Escape den (tastaturet står fortsatt
  // på «Vis lisenser»-knappen bak sløret).
  useEffect(() => {
    if (html !== null) viewer.current?.focus();
  }, [html]);

  async function open(): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      const text = await window.api.licensesNotices();
      if (text === null) {
        toast("error", t("app.setup.advanced.licensesFailed"));
        return;
      }
      setHtml(text);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card testId="advanced-licenses" anchor="licenses">
      <SettingRow
        label={t("app.setup.advanced.licenses")}
        description={t("app.setup.advanced.licensesDesc")}
        testId="adv-licenses"
      >
        <Button
          variant="ghost"
          testId="adv-licenses-open"
          onClick={() => void open()}
        >
          {t("app.setup.advanced.licensesOpen")}
        </Button>
      </SettingRow>
      {html !== null ? (
        <div
          ref={viewer}
          tabIndex={-1}
          class={styles.scrim}
          role="dialog"
          aria-modal="true"
          aria-label={t("app.setup.advanced.licensesFrameTitle")}
          data-testid="licenses-viewer"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              setHtml(null);
            }
          }}
        >
          <div class={styles.panel}>
            <div class={styles.bar}>
              <h2 class={styles.title}>
                {t("app.setup.advanced.licensesFrameTitle")}
              </h2>
              <Button
                variant="ghost"
                testId="licenses-close"
                onClick={() => setHtml(null)}
              >
                {t("app.setup.advanced.licensesClose")}
              </Button>
            </div>
            <iframe
              class={styles.frame}
              sandbox=""
              srcDoc={html}
              title={t("app.setup.advanced.licensesFrameTitle")}
              data-testid="licenses-frame"
            />
          </div>
        </div>
      ) : null}
    </Card>
  );
}
