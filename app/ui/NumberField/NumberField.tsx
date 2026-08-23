/**
 * NumberField — et tall med en REGEL, ikke et felt som gjetter.
 *
 * Regelen er en `NumberRule` fra `@lib/ui/bind-setting-core`, og sjekken er
 * `validateNumber` derfra — ikke en kopi. Der ligger også avgjørelsen som er
 * verdt å ikke ta om igjen: noen tall KLEMMES inn i området (en bitrate
 * utenfor er trygg å rette), mens andre AVVISES (`slett etter N dager` må
 * aldri stille bli et tall som sletter opptak på en plan ingen ba om).
 *
 * `coerceValue('number', …)` gir `null` og ikke `NaN` for et tomt felt, så
 * «brukeren tømte feltet» og «brukeren skrev 0» forblir to forskjellige ting.
 * Det er distinksjonen det gamle `parseInt(x) || 0`-idiomet kastet.
 *
 * Feltet SIER ikke hva som er galt. Meldingen er katalogens, den bestemmes av
 * kallstedet (`BoundNumberField` gir den til `useSetting.validate`), og den
 * VISES av SettingRow. Ett sted per rad.
 */

import {
  coerceValue,
  validateNumber,
  type NumberCheck,
  type NumberRule,
} from "@lib/ui/bind-setting-core";

import { TextField } from "../TextField/TextField";

export interface NumberFieldProps {
  /** Rå tekst, slik feltet står nå. Et tømt felt er `""`. */
  value: string;
  onInput: (next: string) => void;
  onCommit?: () => void;
  rule?: NumberRule;
  /** Overstyr sjekken: kalleren vet om verdien alt er avvist lenger nede. */
  invalid?: boolean;
  disabled?: boolean;
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

/**
 * Sjekk én rå feltverdi mot regelen.
 *
 * Eksportert fordi `BoundNumberField`, feltet selv og en side med eget skjema
 * skal stille NØYAKTIG samme spørsmål. To steder som validerer «omtrent likt»
 * er hvordan et felt godtar noe basen avviser.
 */
export function checkNumberInput(
  raw: string,
  rule: NumberRule = {},
): NumberCheck {
  return validateNumber(coerceValue("number", { value: raw }), rule);
}

export function NumberField({
  value,
  onInput,
  onCommit,
  rule = {},
  invalid,
  disabled = false,
  labelId,
  describedBy,
  testId,
}: NumberFieldProps) {
  const bad = invalid ?? !checkNumberInput(value, rule).ok;

  return (
    <TextField
      value={value}
      onInput={onInput}
      onCommit={onCommit}
      type="number"
      inputMode="numeric"
      disabled={disabled}
      invalid={bad}
      labelId={labelId}
      describedBy={describedBy}
      testId={testId}
    />
  );
}
