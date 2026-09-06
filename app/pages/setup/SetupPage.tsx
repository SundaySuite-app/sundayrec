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
 *   • **Avansert** — opptaksmotor, forhåndsbuffer, oppdateringer, logg, SMTP,
 *     flere tider. Alt som har en trygg standard.
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

import { t } from "../../i18n";
import { navigate, route } from "../../router/router";
import { loadAudioDevices } from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { refreshEmailFacts } from "../../state/email";
import { AdvancedPage } from "./AdvancedPage";
import { ChurchPage } from "./ChurchPage";
import { FirstRunResumeChip } from "./FirstRunResumeChip";
import { isControlId } from "../record/control-core";
import styles from "./setup.module.css";

/** De fem spørsmålene, som `route.tab`-verdier. Fortsatt navnerommet
 *  første-gangs-sekvensen og `decisions-core` bruker. */
export type SetupTab = "sound" | "folder" | "quality" | "church" | "notify";

/** Den gamle System-fanen. Rendrer nøyaktig det samme som ingen fane gjør —
 *  flaten er én — men id-en består, fordi dyplenken gjør det. */
export const ADVANCED_TAB = "advanced";

export function isSetupTab(tab: string | undefined): tab is SetupTab {
  return (
    tab === "sound" ||
    tab === "folder" ||
    tab === "quality" ||
    tab === "church" ||
    tab === "notify"
  );
}

export function SetupPage() {
  const tab = route.value.tab;

  // Fakta flaten og kortene den lenker til trenger. Leses når INNSTILLINGER
  // åpnes, ikke ved oppstart: enhetslisten er den ferskvaren den er, og en
  // liste hentet ved boot ville vært gammel når noen faktisk ser på den.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    void refreshEmailFacts();
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
      <ChurchPage />
      {/*
        Seksjonsnavnet, ikke en fane: «Avansert» er halve denne skjermen, og
        uten navnet ville lista under sett ut som en fortsettelse av
        kirkeprofilen. `?goto=settings:general` lander her, på begge.
      */}
      <div data-testid="setup-advanced-label" class={styles.sectionLabel}>
        {t("app.setup.advanced.title")}
      </div>
      <AdvancedPage />
    </div>
  );
}
