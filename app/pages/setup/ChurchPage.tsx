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
 */

import {
  ACTIVE_LOCALES,
  locale,
  setLocale,
  t,
  tDyn,
  type Locale,
} from "../../i18n";
import { useSetting } from "../../settings/use-setting";
import { Card } from "../../ui/Card/Card";
import { BoundTextField } from "../../ui/Bound/Bound";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { SubPage } from "./SubPage";

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
              value={String(language.draft ?? locale.value)}
              options={ACTIVE_LOCALES.map((code) => ({
                value: code,
                label: tDyn("app.language", code),
              }))}
              onChange={(next) => language.set(next)}
              disabled={language.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="church-language-control-input"
            />
          )}
        </SettingRow>
      </Card>
    </SubPage>
  );
}
