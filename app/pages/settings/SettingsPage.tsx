/**
 * INNSTILLINGER — det man setter én gang, samlet på én flate (F5).
 *
 * ## Hva som bor her
 *
 * Fram til F5 lå svarene på de fem spørsmålene som kort i kontrollrommet på TA
 * OPP, og «resten» på en egen Avansert-flate. Eieren beskar TA OPP til den
 * store knappen — alt man setter én gang bor nå her, i denne rekkefølgen:
 *
 *   Lyd og kanaler · Kamera · Lagringsmappe (og format) · Varsler · Språk ·
 *   Opptaksvalg · Oppdatering · Personvern · Om og logg
 *
 * Hver seksjon har overskrift og lede; innholdslista øverst hopper mellom dem.
 * Skjermene inni er de SAMME som første-gangs-sekvensen bruker (`SoundPage`,
 * `FolderPage`, `NotifyPage` …) — ikke kopier.
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
import { loadAudioDevices } from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { refreshNotificationPermission } from "../../state/notification-permission";
import { CameraCard } from "../setup/CameraCard";
import { FirstRunResumeChip } from "../setup/FirstRunResumeChip";
import { FolderPage } from "../setup/FolderPage";
import { LanguagePage } from "../setup/LanguagePage";
import { NotifyPage } from "../setup/NotifyPage";
import { QualityPage } from "../setup/QualityPage";
import styles from "../setup/setup.module.css";
import { SoundPage } from "../setup/SoundPage";
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

  // Fakta seksjonene trenger. Leses når INNSTILLINGER åpnes, ikke ved oppstart:
  // enhetslisten er den ferskvaren den er.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
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

      <SettingsSection id="sec-sound" sectionKey="sound">
        <SoundPage />
      </SettingsSection>

      <SettingsSection
        id="sec-camera"
        sectionKey="camera"
        intro={t("app.setup.sections.cameraDesc")}
      >
        <CameraCard
          highlight={route.value.highlight === true && anchor === "camera"}
        />
      </SettingsSection>

      <SettingsSection id="sec-folder" sectionKey="folder">
        <FolderPage />
        <h3 class={styles.sectionLabel}>{t("app.setup.q3")}</h3>
        <QualityPage />
      </SettingsSection>

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
