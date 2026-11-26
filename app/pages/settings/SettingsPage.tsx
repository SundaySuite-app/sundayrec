/**
 * INNSTILLINGER — det man setter én gang, samlet på én flate (F5).
 *
 * ## Hva som bor her
 *
 * Alt man setter én gang bor her, i denne rekkefølgen:
 *
 *   Varsler · Språk · Oppdatering · Personvern · Om og logg
 *
 * Lyd og kanaler, kamera, mappe, kvalitet og opptaksvalg flyttet til lydarket
 * på TA OPP (E4): ruten `settings#sound|engine|folder|autodelete` åpner arket.
 *
 * Hver seksjon har overskrift og lede; innholdslista øverst hopper mellom dem.
 * Skjermene inni er de SAMME som første-gangs-sekvensen bruker (`NotifyPage` …)
 * — ikke kopier.
 *
 * ## Ankre
 *
 * `route.anchor` er et element-id uten `sec-`-prefikset (`sound`, `camera`,
 * `folder`, `engine`, `autodelete` …). Siden ruller dit når ruten settes, og
 * fokuserer overskriften når ankeret er en hel seksjon.
 *
 * ⚠️ `firstRun` kommer ALDRI hit — `Shell` bytter ut hele innholdet med
 * `FirstRun` for den ruten.
 */

import { useEffect } from "preact/hooks";

import { t, tDyn } from "../../i18n";
import { route } from "../../router/router";
import { refreshNotificationPermission } from "../../state/notification-permission";
import { FirstRunResumeChip } from "../firstrun/FirstRunResumeChip";
import { LanguagePage } from "./LanguagePage";
import { NotifyPage } from "./NotifyPage";
import styles from "./settings.module.css";
import { AdvancedPage } from "./AdvancedPage";
import { SETTINGS_SECTIONS } from "./sections";
import { SettingsSection } from "./SettingsSection";

/** Hopp til en seksjon: rull dit og flytt fokus til overskriften, så en
 *  tastatur- eller skjermleserbruker havner der synet gjør. */
function jumpTo(id: string): void {
  const section = document.getElementById(id);
  if (!section) return;
  section.scrollIntoView({ block: "start" });
  document.getElementById(`${id}-h`)?.focus({ preventScroll: true });
}

export function SettingsPage() {
  const current = route.value;
  const anchor = current.anchor;

  // Fakta seksjonene trenger. Leses når INNSTILLINGER åpnes, ikke ved oppstart.
  useEffect(() => {
    // Viser OS-et varslene? Seksjonen «Varsler» følger svaret.
    void refreshNotificationPermission();
  }, []);

  // Ankeret: rull til raden eller seksjonen. Effekten henger på HELE ruten og
  // ikke ankerstrengen — `navigate` lager et nytt ruteobjekt for hvert trykk,
  // og to trykk på samme lenke skal rulle to ganger.
  useEffect(() => {
    if (!anchor) return;
    const id = requestAnimationFrame(() => {
      const el =
        document.getElementById(anchor) ??
        document.getElementById(`sec-${anchor}`);
      el?.scrollIntoView({ block: "start" });
    });
    return () => cancelAnimationFrame(id);
  }, [current, anchor]);

  return (
    <div class={styles.page} data-testid="settings-page">
      {/* R6: kirkeradens «Sett opp» lander nettopp HER — se `Checklist` i
          `FirstRun.tsx` — så chippen som fører tilbake må stå her også, ikke
          bare på TA OPP. */}
      <FirstRunResumeChip />
      <nav aria-label={t("app.setup.sections.nav")} data-testid="setup-toc">
        <ul class={styles.toc}>
          <li aria-hidden="true" class={styles.tocLabel}>
            {t("app.setup.sections.nav")}
          </li>
          {SETTINGS_SECTIONS.map((section) => (
            <li key={section.id}>
              <button
                type="button"
                class={styles.tocLink}
                data-testid={`setup-toc-${section.key}`}
                onClick={() => jumpTo(section.id)}
              >
                {tDyn("app.setup.sections", section.key)}
              </button>
            </li>
          ))}
        </ul>
      </nav>

      <SettingsSection id="sec-notify" sectionKey="notify">
        <NotifyPage />
      </SettingsSection>

      <SettingsSection id="sec-language" sectionKey="language">
        {/* Setningen under overskriften er språkvalgets egen «lede». */}
        <LanguagePage />
      </SettingsSection>

      <AdvancedPage />
    </div>
  );
}
