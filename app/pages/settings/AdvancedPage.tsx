/**
 * Resten av Innstillinger — oppdatering, personvern og om/logg.
 *
 * Opptaksvalgene (forhåndsbuffer, stillhet, maks lengde, deling, rydding) bor i
 * lydarket på TA OPP siden E4, og tidsplanen i ukestripa. Det som står igjen er
 * det som ikke hører hjemme noe annet sted (E8 rydder resten). Seksjonene er
 * barn av `SettingsPage`-kolonnen (`display: contents`), så de har samme
 * avstand som resten.
 *
 * ## Hva som IKKE er her, og hvorfor
 *
 * - **«Spør om redigering etter opptak», «Beskytt opptak», «Start med
 *   maskinen»** — tre av de fire uten bakendleser (ATLAS §2.6). «Start med
 *   maskinen» bor på PLAN, inne i «Ta opp automatisk», der den betyr noe.
 *
 *   ⚠️ **«Oppdater automatisk»** har en bakendleser likevel: renderen eier den
 *   timesvise sjekken, og PRIVACY.md lover at den kan slås av: «Slår du den
 *   av, tar appen ikke kontakt med serveren — verken ved oppstart eller den
 *   vanlige sjekken hver time.» En bryter uten timer og en timer uten bryter er
 *   begge et brutt løfte, så de hører sammen. Se `app/state/auto-update.ts`.
 * - **`prerollEnabled`** — uten bakendleser. Sekundene ER bryteren (se
 *   `app/state/preroll.ts`).
 * - **`silenceThreshold` (dBFS)** — −50 dBFS er ikke et tall noen frivillig kan
 *   ha en mening om, og feil verdi stopper opptaket midt i gudstjenesten.
 * - **Tidsplan, vekking og feilsøkingsrader** — tidsplanen er PLAN. Diagnosen
 *   og «Legg ut»-radene gikk ut i F2.
 *
 * ## Hvorfor rader og ikke et trekkspill
 *
 * `SettingRow` har allerede skillelinjen og de fire faste plassene (etikett,
 * kvittering, forklaring, kontroll), og `Card` er beholderen.
 */

import { t } from "../../i18n";
import { BoundToggle } from "../../ui/Bound/Bound";
import { Card } from "../../ui/Card/Card";
import { AsioAttribution } from "./advanced/AsioAttribution";
import { LicensesCard } from "./advanced/LicensesCard";
import { LogRow } from "./advanced/MaintenanceRows";
import { TelemetryRow } from "./advanced/TelemetryRow";
import { UpdateRow } from "./advanced/UpdateRow";
import { SettingsSection } from "./SettingsSection";
import styles from "./settings.module.css";

export function AdvancedPage() {
  return (
    // `display: contents` (se `.advanced`): seksjonene er barn av
    // SettingsPage sin kolonne, så de har samme avstand som seksjonene over.
    <div data-testid="setup-advanced" class={styles.advanced}>
      <SettingsSection
        id="sec-updates"
        sectionKey="updates"
        intro={t("app.setup.sections.updatesDesc")}
      >
        <Card testId="advanced-updates">
          {/*
            Bryteren FØR raden den styrer: «Oppdater automatisk» av betyr at
            ingenting under skjer av seg selv, og rekkefølgen skal lese slik.
            Den manuelle knappen i raden under er med vilje ugatet — PRIVACY.md
            har den som sitt ene unntak, fordi et trykk der er eieren som spør.
          */}
          <BoundToggle
            setting="autoUpdate"
            label={t("app.setup.advanced.autoUpdate")}
            description={t("app.setup.advanced.autoUpdateDesc")}
            testId="adv-auto-update"
          />
          <UpdateRow />
        </Card>
      </SettingsSection>

      <SettingsSection
        id="sec-privacy"
        sectionKey="privacy"
        intro={t("app.setup.sections.privacyDesc")}
      >
        <Card testId="advanced-privacy">
          <TelemetryRow />
        </Card>
      </SettingsSection>

      <SettingsSection
        id="sec-about"
        sectionKey="about"
        intro={t("app.setup.sections.aboutDesc")}
      >
        <Card testId="advanced-help">
          <LogRow />
        </Card>
        <AsioAttribution />
        <LicensesCard />
      </SettingsSection>
    </div>
  );
}
