/**
 * INNSTILLINGER — det tannhjulet åpner, og ingenting mer.
 *
 * ## Hvorfor destinasjonen er blitt ÉN flate (D2)
 *
 * Fram til D2 var dette OPPSETT: nivå 1 med de fem spørsmålene, og fem
 * undersider bak «Endre». Eieren så på det og sa det som var sant — de fem
 * spørsmålene er ting man endrer mens man står og gjør seg klar, og da skal de
 * redigeres DER man gjør seg klar. De bor derfor i kontrollrommet på OPPTAK nå
 * (`app/pages/record/RecordPage.tsx`), og nivå 1 er borte sammen med `Level1`.
 *
 * Igjen står de to tingene som IKKE hører til en søndag:
 *
 *   • **kirkeprofilen** — navnet og språket, spørsmål 4. Det er ikke noe man
 *     tar stilling til fem minutter før gudstjenesten; det settes én gang.
 *   • **Resten** — tidsplan, utlegging, opptaksvalg, oppdateringer, feilsøking
 *     og lisenser, delt i seksjoner med en innholdsliste øverst
 *     (`AdvancedPage.tsx`).
 *
 * Én flate, ikke to faner: `?goto=settings:general` (den gamle System-fanen) og
 * tannhjulet lander på det samme, og det er hele skjermen.
 *
 * ## De fire gamle fanene lever, som en omdirigering
 *
 * `?goto=settings:audio` og de tre andre pekes på `record#<anker>` i
 * `TAB_ALIASES`, så de kommer normalt aldri hit. Men en `navigate("setup", {
 * tab: "sound" })` fra en kallsted vi ikke har funnet — eller fra en eldre
 * tray/dyplenke — ville ellers landet på en flate som ikke har spørsmålet, og
 * det ville sett ut som at lenken virket. Effekten under sender den videre til
 * kortet i stedet. Høylytt vil den ikke være: en dyplenke som lander riktig er
 * ikke en feil, den er en id vi ikke rakk å rydde.
 *
 * ⚠️ `firstRun` kommer ALDRI hit — `Shell` bytter ut hele innholdet med
 * `FirstRun` for den ruten — så omdirigeringen kan ikke rive en frivillig ut av
 * sekvensen.
 */

import { useEffect } from "preact/hooks";

import { t, tDyn } from "../../i18n";
import { navigate, route } from "../../router/router";
import { loadAudioDevices } from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { AdvancedPage } from "./AdvancedPage";
import { ChurchPage } from "./ChurchPage";
import { SETTINGS_SECTIONS } from "./sections";
import { SettingsSection } from "./SettingsSection";
import { FirstRunResumeChip } from "./FirstRunResumeChip";
import { isControlId } from "../record/control-core";
import styles from "./setup.module.css";

/** De fem spørsmålene, som `route.tab`-verdier. Fortsatt navnerommet
 *  første-gangs-sekvensen og `decisions-core` bruker. */
export type SetupTab = "sound" | "folder" | "quality" | "church" | "notify";

export function isSetupTab(tab: string | undefined): tab is SetupTab {
  return (
    tab === "sound" ||
    tab === "folder" ||
    tab === "quality" ||
    tab === "church" ||
    tab === "notify"
  );
}

/** Hopp til en seksjon: rull dit og flytt fokus til overskriften, så en
 *  tastatur- eller skjermleserbruker havner der synet gjør. */
function jumpTo(id: string): void {
  const section = document.getElementById(id);
  if (!section) return;
  section.scrollIntoView({ block: "start" });
  document.getElementById(`${id}-h`)?.focus({ preventScroll: true });
}

export function SetupPage() {
  const tab = route.value.tab;

  // Fakta flaten og kortene den lenker til trenger. Leses når INNSTILLINGER
  // åpnes, ikke ved oppstart: enhetslisten er den ferskvaren den er, og en
  // liste hentet ved boot ville vært gammel når noen faktisk ser på den.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
  }, []);

  // En gammel fane-id som slapp gjennom: send den til kortet som eier
  // spørsmålet. `church` er unntaket — kirkeprofilen bor HER.
  useEffect(() => {
    if (isSetupTab(tab) && tab !== "church" && isControlId(tab)) {
      navigate("record", { anchor: tab });
    }
  }, [tab]);

  return (
    <div class={styles.page}>
      {/* R6: kirkeradens «Sett opp» lander nettopp HER — se `Checklist` i
          `FirstRun.tsx` — så chippen som fører tilbake må stå her også, ikke
          bare på OPPTAK. */}
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
                data-testid={`setup-toc-${section.id}`}
                onClick={() => jumpTo(section.id)}
              >
                {tDyn("app.setup.sections", section.key)}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <SettingsSection id="sec-church" sectionKey="church">
        {/* Setningen under overskriften er kirkeprofilens egen «lede». */}
        <ChurchPage />
      </SettingsSection>
      <AdvancedPage />
    </div>
  );
}
