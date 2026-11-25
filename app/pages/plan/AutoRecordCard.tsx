/**
 * «Ta opp automatisk» — øverst på PLAN, over ukevisningen.
 *
 * Kortet har to ting: bryteren, og «Start automatisk med maskinen». Tidene
 * selv står i ukevisningen (`WeekView`) rett under; skrivingen går gjennom
 * `plan-core`/`schedule-core`/`slots-core`, som ikke kjenner presentasjonen.
 *
 * ## Bryteren er i topplinja, valgene i kroppen
 *
 * Samme form som kamera-kortet: en `ControlCard` med bryteren til venstre, det
 * som gjelder som kompaktverdi («Søndag 11:00 · 90 min», eller «3 faste tider
 * hver uke»), og en kropp med radene som hører til. Kroppen står alltid åpen:
 * «start med maskinen» er meningsfull selv før den første tiden er satt.
 *
 * ## Av/på sletter INGENTING
 *
 * P1a skrev dette ned som en eiersak: `Settings` hadde ikke noe sted å huske
 * «armert», så en tom `slots`-liste var den eneste måten å stave «av» på — og
 * bryteren måtte slette tidspunktet for å slå seg av. En bryter som kaster data
 * den ikke viser.
 *
 * Eieren svarte, og P1b la til nøkkelen: `autoRecordEnabled`, med ÉN leser i
 * bakenden (`Settings::active_slots`). «Av» skriver nå bare flagget; tidene
 * blir stående i basen og kommer tilbake nøyaktig som de var — også etter en
 * omstart, som en økt-hukommelse aldri kunne love. Ukevisningen viser dem
 * dempet, med en setning om at ingenting startes.
 *
 * `default = true` i Rust, så en profil skrevet før feltet fantes fortsetter å
 * ta opp. Det motsatte ville stille avvæpnet hver menighet som allerede hadde
 * en søndagstid.
 *
 * ## «Start automatisk med maskinen» bor HER
 *
 * `launchAtLogin` er meningsløs alene: den handler ikke om å starte et program,
 * den handler om at det planlagte opptaket faktisk skjer etter en omstart. Så
 * den er en rad inne i dette kortet, ikke en systeminnstilling et annet sted.
 *
 * OS-et er fasiten på om innloggingselementet finnes: `get_launch_at_login`
 * leses ved oppstart av kortet, og hvis den er uenig med det lagrede vinner
 * OS-et (samme regel som `syncAutostartFromOs` i legacy). En avkrysning som
 * lover at opptaket overlever en omstart, på en maskin der elementet ble
 * fjernet for hånd, er nøyaktig den løgnen som oppdages en søndag.
 *
 * ## `scheduler_reschedule`
 *
 * Skjer i `window.api.saveSettings` etter hver skrivning, som for alt annet.
 */

import { useEffect } from "preact/hooks";

import { t, tDyn, tf, tn } from "../../i18n";
import { usePatch } from "../../settings/use-patch";
import { settings, syncLaunchAtLoginFromOs } from "../../state/settings";
import { BoundToggle } from "../../ui/Bound/Bound";
import { ControlCard } from "../../ui/ControlCard/ControlCard";
import { Receipt } from "../../ui/Receipt/Receipt";
import { Toggle } from "../../ui/Toggle/Toggle";
import { autoValue, type AutoValue } from "../setup/card-values-core";
import { autoRecordOn, DEFAULT_PLAN, slotsFromPlan } from "./schedule-core";

export interface AutoRecordCardProps {
  highlight?: boolean;
}

/** Kortet står alltid åpent på PLAN: det er hele poenget med siden. */
export function AutoRecordCard({ highlight }: AutoRecordCardProps) {
  const s = settings.value;
  const slots = s.slots ?? [];
  const on = autoRecordOn(s);
  // Den delte lagringsmodellen: bryteren skriver TO nøkler (flagget og — når
  // profilen ikke har en tid — den første slotten), så den kan ikke gå gjennom
  // `useSetting`. `usePatch` gir den samme sekvensen, den samme
  // tilbakerullingen og en kvittering som teller ned.
  const save = usePatch();

  useEffect(() => {
    void syncLaunchAtLoginFromOs();
  }, []);

  /**
   * PÅ: arm flagget, og gi profilen en tid hvis den ikke har noen. En profil
   * som HAR tider beholder dem nøyaktig som de sto.
   * AV: bare flagget. Ingen dialog, fordi ingenting forsvinner.
   */
  function toggle(next: boolean): void {
    void save.write(
      next
        ? {
            autoRecordEnabled: true,
            slots: slots.length ? slots : slotsFromPlan(DEFAULT_PLAN, slots),
          }
        : { autoRecordEnabled: false },
      // Bryteren står på `autoRecordOn(s)` — flagget OG en tid — så et klikk
      // som bare fyller inn den manglende tiden er en ekte endring selv om
      // flagget allerede sto.
      { changed: true },
    );
  }

  return (
    <ControlCard
      id="auto"
      testId="setup-auto"
      title={t("app.setup.auto.title")}
      value={autoText(autoValue(s), slots.length)}
      expanded
      highlight={highlight}
      lead={
        <Toggle
          checked={on}
          onChange={toggle}
          disabled={save.busy}
          labelId="setup-auto-title"
          testId="setup-auto-toggle"
        />
      }
      trail={<Receipt state={save.receipt} testId="setup-auto-receipt" />}
    >
      <BoundToggle
        setting="launchAtLogin"
        label={t("app.setup.auto.launch")}
        description={t("app.setup.auto.launchDesc")}
        testId="auto-launch"
      />
    </ControlCard>
  );
}

/** Kompaktverdien som SETNING — kjernen svarer med tallene, ikke med teksten. */
function autoText(value: AutoValue, count: number): string {
  if (value.key === "off") return t("app.setup.auto.desc");
  if (count > 1) return tn("app.plan.summaryMany", count);
  return tf("app.setup.auto.summary", {
    day: tDyn("app.setup.days", String(value.day)),
    start: value.start,
    n: value.minutes,
  });
}
