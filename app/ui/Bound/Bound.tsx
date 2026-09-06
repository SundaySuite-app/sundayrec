/**
 * Bound* — `useSetting` + `SettingRow` + kontrollen, i ett.
 *
 * ## Hvorfor de finnes
 *
 * En innstilling er alltid de samme fem tingene: hooken, raden, kontrollen,
 * kvitteringen og feillinjen. Skrevet for hånd hver gang er det fem steder å
 * glemme å koble kvitteringen — og en rad uten kvittering ser ut som en rad
 * der ingenting ble lagret. Så: én komponent per kontrolltype, og siden sier
 * bare hvilken nøkkel og hva raden heter.
 *
 * ```tsx
 * <BoundToggle setting="notifyStart" label={t("…")} description={t("…")} />
 * ```
 *
 * ## Hva de IKKE gjør
 *
 * De finner ikke på tekst. Etikett, forklaring og valgalternativer kommer inn
 * som ferdig oversatte strenger fra kallstedet, fordi katalognøkkelen hører
 * til siden — et bibliotek som slo opp `settings.${key}.label` ville vært
 * usynlig for `check-i18n-keys.mjs`, og en manglende nøkkel ville blitt en tom
 * etikett ingen oppdager.
 *
 * De bestemmer heller ikke NÅR noe lagres. Det gjør `planCommit` i
 * `@lib/ui/bind-setting-core`, gjennom `useSetting().events`: en bryter
 * committer på valget, et tekstfelt med etterslep. Derfor kaller
 * `BoundTextField` `set()` på hvert tastetrykk (hooken debouncer) og
 * `commit()` på blur/Enter — begge, fordi et felt som bare lyttet på `change`
 * aldri ville lagret en verdi som ble skrevet og forlatt.
 */

import {
  useSetting,
  type ScalarSettingKey,
  type UseSettingOpts,
} from "../../settings/use-setting";
import type { Settings } from "../../state/settings";
import { NumberField, checkNumberInput } from "../NumberField/NumberField";
import { RadioCards, type RadioOption } from "../RadioCards/RadioCards";
import { Select, type SelectOption } from "../Select/Select";
import { SettingRow } from "../SettingRow/SettingRow";
import { TextField } from "../TextField/TextField";
import { Toggle } from "../Toggle/Toggle";
import type { NumberRule, NumberIssue } from "@lib/ui/bind-setting-core";

/** Det hver Bound*-komponent deler. */
interface BoundBase<K extends ScalarSettingKey> {
  setting: K;
  label: string;
  description?: string;
  /** Slår av kontrollen uten å påstå hvorfor — bruk `Gate` for grunnen. */
  disabled?: boolean;
  /** Spør først. `recordingImminentGuard(...)` er den vanlige. */
  confirmIf?: UseSettingOpts<Settings[K]>["confirmIf"];
  testId?: string;
}

export function BoundToggle<K extends ScalarSettingKey>({
  setting,
  label,
  description,
  disabled = false,
  confirmIf,
  testId,
}: BoundBase<K>) {
  const bound = useSetting(setting, { kind: "toggle", confirmIf });
  return (
    <SettingRow
      label={label}
      description={description}
      receipt={bound.receipt}
      error={bound.error}
      disabled={disabled}
      testId={testId}
    >
      {(ids) => (
        <Toggle
          checked={bound.draft === true}
          onChange={(next) => bound.set(next)}
          disabled={disabled || bound.busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={testId ? `${testId}-control-input` : undefined}
        />
      )}
    </SettingRow>
  );
}

export function BoundSelect<K extends ScalarSettingKey>({
  setting,
  label,
  description,
  options,
  disabled = false,
  confirmIf,
  testId,
}: BoundBase<K> & { options: readonly SelectOption[] }) {
  const bound = useSetting(setting, { kind: "select", confirmIf });
  return (
    <SettingRow
      label={label}
      description={description}
      receipt={bound.receipt}
      error={bound.error}
      disabled={disabled}
      testId={testId}
    >
      {(ids) => (
        <Select
          value={String(bound.draft ?? "")}
          options={options}
          onChange={(next) => bound.set(next)}
          disabled={disabled || bound.busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={testId ? `${testId}-control-input` : undefined}
        />
      )}
    </SettingRow>
  );
}

export function BoundRadioCards<K extends ScalarSettingKey>({
  setting,
  label,
  description,
  options,
  columns,
  disabled = false,
  confirmIf,
  testId,
}: BoundBase<K> & { options: readonly RadioOption[]; columns?: 1 | 2 }) {
  const bound = useSetting(setting, { kind: "radio", confirmIf });
  return (
    <SettingRow
      label={label}
      description={description}
      receipt={bound.receipt}
      error={bound.error}
      disabled={disabled}
      testId={testId}
    >
      {(ids) => (
        <RadioCards
          value={String(bound.draft ?? "")}
          options={options}
          columns={columns}
          onChange={(next) => bound.set(next)}
          disabled={disabled || bound.busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={testId ? `${testId}-control-input` : undefined}
        />
      )}
    </SettingRow>
  );
}

export function BoundTextField<K extends ScalarSettingKey>({
  setting,
  label,
  description,
  placeholder,
  type,
  validate,
  disabled = false,
  confirmIf,
  testId,
}: BoundBase<K> & {
  placeholder?: string;
  type?: "text" | "email";
  /** Returner en melding for å avvise verdien. */
  validate?: (value: Settings[K]) => string | null;
}) {
  const bound = useSetting(setting, { kind: "text", validate, confirmIf });
  return (
    <SettingRow
      label={label}
      description={description}
      receipt={bound.receipt}
      error={bound.error}
      disabled={disabled}
      testId={testId}
    >
      {(ids) => (
        <TextField
          value={String(bound.draft ?? "")}
          type={type}
          placeholder={placeholder}
          invalid={bound.error !== null}
          onInput={(next) => bound.set(next)}
          onCommit={() => void bound.commit()}
          disabled={disabled}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={testId ? `${testId}-control-input` : undefined}
        />
      )}
    </SettingRow>
  );
}

export function BoundNumberField<K extends ScalarSettingKey>({
  setting,
  label,
  description,
  rule,
  message,
  disabled = false,
  confirmIf,
  testId,
}: BoundBase<K> & {
  rule?: NumberRule;
  /** Teksten for en avvist verdi. Katalogens, ikke bibliotekets. */
  message: (issue: NumberIssue, bound?: number) => string;
}) {
  const bound = useSetting(setting, {
    kind: "number",
    // ÉN validering: feltet og hooken stiller samme spørsmål gjennom
    // `checkNumberInput`, som er `validateNumber` fra bind-setting-core.
    // To steder som validerer «omtrent likt» er hvordan et felt godtar noe
    // basen avviser.
    validate: (value) => {
      const check = checkNumberInput(String(value ?? ""), rule);
      return check.ok ? null : message(check.issue ?? "nan", check.bound);
    },
    confirmIf,
  });
  return (
    <SettingRow
      label={label}
      description={description}
      receipt={bound.receipt}
      error={bound.error}
      disabled={disabled}
      testId={testId}
    >
      {(ids) => (
        <NumberField
          value={String(bound.draft ?? "")}
          rule={rule}
          invalid={bound.error !== null}
          onInput={(next) => bound.set(next)}
          onCommit={() => void bound.commit()}
          disabled={disabled}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={testId ? `${testId}-control-input` : undefined}
        />
      )}
    </SettingRow>
  );
}
