/**
 * 4 — Hvilken kirke?
 *
 * Navnet og språket. To felter, og begge sier sant om hva de gjør.
 *
 * ## Den døde påstanden som ikke ble med
 *
 * Dagens kirkeprofil-kort sier at navnet «brukes i filnavn, varslings-e-poster
 * og podcast-RSS». To tredeler av den setningen er usann: `churchName` brukes
 * IKKE i filnavn (`opts.rs` sender `church_name: None`), og podkast-RSS ble
 * fjernet i #139. Her står bare det som faktisk skjer.
 *
 * ## Språket er en innstilling, ikke en omstart
 *
 * `setLocale` bytter katalogen og flipper signalet, og alt som kaller `t()`
 * rendres på nytt i samme frame. Ingen «trer i kraft etter omstart», og ingen
 * «trykk Lagre først» — det gamle skallet sa begge deler til forskjellige
 * tider.
 *
 * ⚠️ Bare norsk og engelsk står i lista. De fem andre katalogene er PAUSET
 * gjennom redesignet (`ACTIVE_LOCALES`), ikke fjernet — å tilby et språk der
 * halvparten av skjermen er tom er verre enn å ikke tilby det ennå. Verdien
 * som står lagret røres ikke: en profil satt til tysk beholder «de» i basen og
 * får språket tilbake i fase B.
 *
 * ## Valgboksen lyver ikke lenger om det pausede valget (F1-R2 / R9)
 *
 * Før la `<Select>` bare fram de to aktive kodene som `<option>`. En profil
 * migrert med `language: "de"` satte da kontrollens `value` til noe INGEN
 * option hadde — og en `<select>` uten treff blant sine egne options viser
 * stille den FØRSTE optionen, uansett hva som faktisk står lagret. Se
 * `church-core.ts`s `languageOptions`: den legger til en tredje, DEAKTIVERT
 * rad med det ekte navnet når det lagrede språket er pauset, og linja under
 * boksen (`isPausedLanguage`) sier hvorfor den ikke kan velges på nytt.
 */

import { locale, setLocale, t, tDyn, tf, type Locale } from "../../i18n";
import { useSetting } from "../../settings/use-setting";
import { Card } from "../../ui/Card/Card";
import { BoundTextField } from "../../ui/Bound/Bound";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { isPausedLanguage, languageOptions } from "./church-core";
import { SubPage } from "./SubPage";
import styles from "./setup.module.css";

export function ChurchPage() {
  // Språket er en vanlig innstilling med vanlig kvittering — men BYTTET skjer
  // i `after`, altså først når skrivningen faktisk landet. Å flippe katalogen
  // før det ville gitt en app på engelsk og en base på norsk hvis lagringen
  // feilet, og `useSetting` ville rullet verdien tilbake under en skjerm som
  // allerede hadde byttet språk.
  const language = useSetting("language", {
    kind: "select",
    after: (value) => {
      const next = String(value ?? "no") as Locale;
      if (next !== locale.peek()) void setLocale(next);
    },
  });
  // Den samme strengen går til BÅDE `<Select value>` og `languageOptions`:
  // det er det som garanterer at boksens valgte verdi alltid finnes blant
  // options'ene den får (se filhodet — det manglende treffet var hele feilen).
  const selected = String(language.draft ?? locale.value);
  const paused = isPausedLanguage(selected);

  return (
    <SubPage lede={t("app.setup.church.lede")} testId="setup-church">
      <Card testId="church-card">
        <BoundTextField
          setting="churchName"
          label={t("app.setup.church.name")}
          description={t("app.setup.church.nameDesc")}
          placeholder={t("app.setup.church.placeholder")}
          testId="church-name"
        />
        <SettingRow
          label={t("app.setup.church.languageLabel")}
          description={t("app.setup.church.languageDesc")}
          receipt={language.receipt}
          error={language.error}
          testId="church-language"
        >
          {(ids) => (
            <Select
              value={selected}
              options={languageOptions(selected)}
              onChange={(next) => language.set(next)}
              disabled={language.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="church-language-control-input"
            />
          )}
        </SettingRow>
        {paused ? (
          <p data-testid="church-language-paused" class={styles.hint}>
            {tf("app.setup.church.languagePaused", {
              language: tDyn("app.language", locale.value),
            })}
          </p>
        ) : null}
      </Card>
    </SubPage>
  );
}
