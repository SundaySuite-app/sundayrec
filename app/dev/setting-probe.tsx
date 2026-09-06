/**
 * TODO(P): slett denne fila når OPPSETT har ekte rader.
 *
 * En midlertidig testflate, montert BARE med `?probe=setting`.
 *
 * ## Hvorfor den fortsatt er verdt å ha etter S1b
 *
 * `use-setting-core.ts` er tabelltestet over hele sekvensen, men kjernen
 * kjenner bare de funksjonene den fikk injisert. Det den IKKE kan si noe om er
 * om de EKTE koblingene stemmer: at `patchSettings` treffer riktig nøkkel, at
 * `saveSettingsDebounced` faktisk svarer `false` når `settings_save` avviser,
 * at revert-på-feil derfor ender med den LAGREDE verdien på skjermen — og nå
 * også at feiltoasten havner i `ToastHost`, og at `confirmIf` faktisk åpner
 * `DialogHost`.
 *
 * Det er en skjøt mellom fem lag som hver for seg er testet — nøyaktig formen
 * dekning ikke fanger (`reference-seam-bugs`).
 *
 * ## Forskjellen fra S1a-utgaven
 *
 * Kontrollene er ikke lenger håndlagde `<button>`-er: raden er en ekte
 * `SettingRow` og bryteren en ekte `Toggle`. Testid-ene er de samme, så
 * `e2e/settings-revert.spec.ts` gjelder uendret — men nå driver den
 * biblioteket i stedet for en stedfortreder for det.
 */

import { t, tf } from "../i18n";
import { useSetting } from "../settings/use-setting";
import { SettingRow } from "../ui/SettingRow/SettingRow";
import { Toggle } from "../ui/Toggle/Toggle";
import { toasts } from "../ui/toast";

export function SettingProbe() {
  const auto = useSetting("autoUpdate");
  // Den andre raden spør ALLTID. Den ekte vakten (`recordingImminentGuard`)
  // spør bare når et opptak er nært, og en e2e-test kan ikke få en maskin til
  // å være fire minutter før en gudstjeneste — men veien gjennom
  // `confirmIf → confirmDialog → DialogHost` er den samme.
  const guarded = useSetting("notifyStart", {
    confirmIf: () => ({
      title: tf("guard.title", { what: t("general.notifyStart") }),
      message: t("guard.duringRecording"),
      confirmLabel: t("guard.confirm"),
      cancelLabel: t("guard.cancel"),
    }),
  });

  const queue = toasts.value;
  const last = queue.length > 0 ? queue[queue.length - 1] : null;

  return (
    <section data-testid="setting-probe">
      {/* Den LAGREDE verdien — det er den som må rulle tilbake. */}
      <output data-testid="probe-value">{String(auto.value)}</output>
      <output data-testid="probe-draft">{String(auto.draft)}</output>
      <output data-testid="probe-receipt">{auto.receipt}</output>
      <output data-testid="probe-toast">{last ? last.msg : ""}</output>
      <output data-testid="probe-guarded-value">{String(guarded.value)}</output>

      <SettingRow label={t("general.autoUpdate")} receipt={auto.receipt}>
        <Toggle
          checked={auto.draft === true}
          disabled={auto.busy}
          onChange={(next) => auto.set(next)}
          testId="probe-toggle"
        />
      </SettingRow>

      <SettingRow label={t("general.notifyStart")} receipt={guarded.receipt}>
        <Toggle
          checked={guarded.draft === true}
          disabled={guarded.busy}
          onChange={(next) => guarded.set(next)}
          testId="probe-guarded"
        />
      </SettingRow>
    </section>
  );
}
