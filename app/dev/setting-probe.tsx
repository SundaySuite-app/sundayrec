/**
 * TODO(S1b): slett denne fila når `SettingRow` finnes.
 *
 * En midlertidig testflate for `useSetting`, montert BARE med `?probe=setting`.
 *
 * ## Hvorfor den er verdt å ha i S1a
 *
 * `use-setting-core.ts` er tabelltestet over hele sekvensen, men kjernen kjenner
 * bare de funksjonene den fikk injisert. Det den IKKE kan si noe om er om de
 * ekte koblingene stemmer: at `patchSettings` treffer riktig nøkkel, at
 * `saveSettingsDebounced` faktisk svarer `false` når `settings_save` avviser,
 * og at revert-på-feil derfor ender med den LAGREDE verdien på skjermen.
 *
 * Det er en skjøt mellom tre lag som hver for seg er testet — nøyaktig formen
 * dekning ikke fanger (`reference-seam-bugs`). `e2e/app/settings-revert.spec.ts`
 * kjører den gjennom fikstursømmen, i en ekte nettleser, med et
 * `settings_save` som kaster.
 *
 * Når S1b har en ekte innstillingsrad flyttes spec-en dit og denne forsvinner.
 */

import { t } from "../i18n";
import { useSetting } from "../settings/use-setting";
import { toasts } from "../ui/toast";

export function SettingProbe() {
  const auto = useSetting("autoUpdate");
  const queue = toasts.value;
  const last = queue.length > 0 ? queue[queue.length - 1] : null;

  return (
    <section data-testid="setting-probe">
      {/* Den LAGREDE verdien — det er den som må rulle tilbake. */}
      <output data-testid="probe-value">{String(auto.value)}</output>
      <output data-testid="probe-draft">{String(auto.draft)}</output>
      <output data-testid="probe-receipt">{auto.receipt}</output>
      <output data-testid="probe-toast">{last ? last.msg : ""}</output>
      <button
        type="button"
        data-testid="probe-toggle"
        disabled={auto.busy}
        onClick={() => auto.set(!auto.value)}
      >
        {t("general.autoUpdate")}
      </button>
    </section>
  );
}
