/**
 * Lisensleseren — tredjepartslisensene, i en nesten full-skjerm flate over
 * appen med egen lukkeknapp.
 *
 * Ordlyden er lisensteksten, ordrett (`licenses_notices` leverer ferdig HTML,
 * vist i en sandboxet iframe). Åpningen (`loadLicenses`) er adskilt fra
 * visningen så «Lisenser»-knappen i `AboutRow` eier trykket og feiltoasten.
 *
 * Escape lukker BARE leseren: hendelsen stoppes her, så den ikke også når
 * tannhjulets egen Esc-lytter på `document`.
 */

import { useEffect, useRef } from "preact/hooks";

import { t } from "../../../i18n";
import { Button } from "../../../ui/Button/Button";
import { toast } from "../../../ui/toast";
import styles from "./LicensesCard.module.css";

/** Hent lisensteksten. `null` + en toast når bakenden ikke fikk gitt den. */
export async function loadLicenses(): Promise<string | null> {
  const text = await window.api.licensesNotices();
  if (text === null) {
    toast("error", t("app.setup.advanced.licensesFailed"));
    return null;
  }
  return text;
}

export function LicensesViewer({
  html,
  onClose,
}: {
  html: string;
  onClose: () => void;
}) {
  const viewer = useRef<HTMLDivElement>(null);

  // Fokus inn i leseren, ellers når ikke Escape den (tastaturet står fortsatt
  // på «Lisenser»-knappen bak sløret).
  useEffect(() => {
    viewer.current?.focus();
  }, []);

  return (
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
          onClose();
        }
      }}
    >
      <div class={styles.panel}>
        <div class={styles.bar}>
          <h2 class={styles.title}>
            {t("app.setup.advanced.licensesFrameTitle")}
          </h2>
          <Button variant="ghost" testId="licenses-close" onClick={onClose}>
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
  );
}
