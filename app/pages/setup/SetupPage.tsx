/**
 * OPPSETT — fem spørsmål, og de fem skjermene de åpner.
 *
 * ## Hvorfor sidene er faner og ikke egne ruter
 *
 * `route.tab` er allerede navnerommet ruteren oversetter alt det gamle inn i
 * (`?goto=settings:audio` → `setup/sound`), og hver av de fem har en gammel
 * dyplenke som peker på seg. En egen rute-akse ville betydd to tabeller å
 * holde i takt for én informasjonsarkitektur.
 *
 * ## Overskriften bytter, destinasjonen gjør det ikke
 *
 * Skinnen står på OPPSETT hele veien, men `<h1>` blir spørsmålet: siden
 * HANDLER om «Hvilken lyd?», og en overskrift som sa «Oppsett» på alle seks
 * skjermene ville vært det ene ordet som aldri hjelper. `PageShell` tar derfor
 * imot en overskrift, og `setupHeading` er det ene stedet som vet hvilken.
 *
 * ## «Avansert» finnes ikke her ennå
 *
 * Canvasen har en «Avansert»-lenke nederst på nivå 1. Den er IKKE med: skjermen
 * den skulle åpne bygges i P1b, og en lenke til en tom side (eller til en tekst
 * som sier «kommer senere») lærer en frivillig at lenkene i denne appen ikke er
 * til å stole på. Den legges til sammen med siden den åpner.
 *
 * Av samme grunn lander `?goto=settings:general` — som fortsatt er den gamle
 * System-fanen — på nivå 1 og ikke på en tom Avansert-side. `data-tab` står
 * likevel på `<main>`, så dyplenken er ikke tapt: P1b rendrer den.
 */

import { useEffect } from "preact/hooks";

import { t } from "../../i18n";
import { route } from "../../router/router";
import { loadAudioDevices } from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { refreshEmailFacts } from "../../state/email";
import { ChurchPage } from "./ChurchPage";
import { FolderPage } from "./FolderPage";
import { Level1 } from "./Level1";
import { NotifyPage } from "./NotifyPage";
import { QualityPage } from "./QualityPage";
import { SoundPage } from "./SoundPage";

/** De fem undersidene. Navnene er `route.tab`-verdier. */
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

/**
 * Overskriften OPPSETT skal ha for denne fanen.
 *
 * Nøklene slås opp med en literal hver — ikke `tDyn('app.setup', key)` — fordi
 * `q1`…`q5` bor side om side med `lede`, `notSetUp` og resten i katalogen, og
 * et dynamisk oppslag der ville pekt på et subtre som er mye større enn de fem
 * det gjelder. Fem literaler er fem ting gaten kan sjekke hver for seg.
 */
export function setupHeading(tab: string | undefined): string | undefined {
  if (!isSetupTab(tab)) return undefined;
  switch (tab) {
    case "sound":
      return t("app.setup.q1");
    case "folder":
      return t("app.setup.q2");
    case "quality":
      return t("app.setup.q3");
    case "church":
      return t("app.setup.q4");
    case "notify":
      return t("app.setup.q5");
  }
}

export function SetupPage() {
  const tab = route.value.tab;

  // Fakta nivå 1 trenger for å kunne si sant om spørsmål 1, 2 og 5. Leses når
  // OPPSETT åpnes, ikke ved oppstart: enhetslisten er den ferskvaren den er,
  // og en liste hentet ved boot ville vært gammel når noen faktisk ser på den.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    void refreshEmailFacts();
  }, []);

  switch (tab) {
    case "sound":
      return <SoundPage />;
    case "folder":
      return <FolderPage />;
    case "quality":
      return <QualityPage />;
    case "church":
      return <ChurchPage />;
    case "notify":
      return <NotifyPage />;
    default:
      return <Level1 />;
  }
}
