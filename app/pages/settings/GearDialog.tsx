/**
 * Tannhjulet — INNSTILLINGER som et sentrert kort (520 px) med slør, over siden
 * man kom fra («Én flate»). Fire rader, hver med et ikon og ett ord:
 *
 *   Språk · Oppdatering · Anonym statistikk · Om
 *
 * Alt annet som en gang bodde her flyttet dit det hører til: lyd, mappe og
 * kvalitet i lydarket på TA OPP, tidsplan og varsler i ukestripa.
 *
 * ## Ruten, ikke en egen tilstand
 *
 * `route.page === "settings"` ER «tannhjulet er åpent». Skallet viser siden
 * under (`underlay`) bak sløret, `inert`, og lukking (`closeGear`) går tilbake
 * dit. Gamle lenker (`?goto=settings:<anker>`) som ikke har flyttet til et ark
 * lander her, og raden ankeret peker på pulserer.
 *
 * ## Arkkontrakten
 *
 * Samme som lydarket og eksportarket (`app/ui/sheet-behavior.ts`): fokus inn
 * ved åpning og tilbake til tannhjulet ved lukking, Esc og trykk på sløret
 * lukker, Tab fanges. En åpen bekreftelsesdialog (beta-valget, «Slett mine
 * data») eier Escape selv.
 */

import { useRef } from "preact/hooks";

import { t } from "../../i18n";
import { closeGear } from "../../router/router";
import { activeDialog } from "../../ui/dialog";
import { Icon } from "../../ui/Icon/Icon";
import { useSheetBehavior } from "../../ui/sheet-behavior";
import { AboutRow } from "./gear/AboutRow";
import styles from "./gear/gear.module.css";
import { LanguageRow } from "./gear/LanguageRow";
import { TelemetryRow } from "./gear/TelemetryRow";
import { UpdateRow } from "./gear/UpdateRow";

export function GearDialog() {
  const card = useRef<HTMLElement | null>(null);
  const closeBtn = useRef<HTMLButtonElement | null>(null);

  useSheetBehavior({
    open: true,
    onClose: closeGear,
    sheet: card,
    initialFocus: closeBtn,
    escapeBlocked: () => activeDialog.peek() !== null,
    returnFocus: () =>
      document.querySelector<HTMLElement>('[data-testid="nav-settings"]'),
  });

  return (
    <div class={styles.layer} data-testid="gear-layer">
      {/* Ingen knapp: sløret ligger under kortet, og en knapp med sentrum
          skjult av kortet er et treffmål ingen kan treffe. Tastaturet har Esc
          og krysset. */}
      <div
        class={styles.scrim}
        data-testid="gear-scrim"
        aria-hidden="true"
        onClick={closeGear}
      />
      <section
        ref={card}
        role="dialog"
        aria-modal="true"
        aria-labelledby="gear-title"
        data-testid="gear-dialog"
        class={styles.card}
      >
        <header class={styles.head}>
          <span class={styles.title}>
            <Icon name="gear" size={20} strokeWidth={1.8} />
            <h2 id="gear-title" class={styles.titleText}>
              {t("app.gear.title")}
            </h2>
          </span>
          <button
            ref={closeBtn}
            type="button"
            class={styles.close}
            data-testid="gear-close"
            aria-label={t("app.gear.close")}
            title={t("app.gear.close")}
            onClick={closeGear}
          >
            <Icon name="x" size={20} strokeWidth={2} />
          </button>
        </header>
        <div class={styles.rows}>
          <LanguageRow />
          <UpdateRow />
          <TelemetryRow />
          <AboutRow />
        </div>
      </section>
    </div>
  );
}
