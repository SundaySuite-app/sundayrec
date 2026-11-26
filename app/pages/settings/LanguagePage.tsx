/**
 * 4 — Hvilket språk?
 *
 * Ett felt, og det sier sant om hva det gjør.
 *
 * ## Språket er en innstilling, ikke en omstart
 *
 * `setLocale` bytter katalogen og flipper signalet, og alt som kaller `t()`
 * rendres på nytt i samme frame. Ingen «trer i kraft etter omstart», og ingen
 * «trykk Lagre først» — det gamle skallet sa begge deler til forskjellige
 * tider.
 *
 * Fra F4 er det bare norsk og engelsk. Står det et språk i basen som ikke finnes lenger (en
 * profil fra før F4), viser boksen språket som
 * GJELDER — det `resolveStartupLocale` valgte etter systemregelen — og ikke den lagrede
 * strengen. Velger du et språk, skrives det over.
 */

import { ALL_LOCALES, locale, setLocale, t, type Locale } from "../../i18n";
import { useSetting } from "../../settings/use-setting";
import { Card } from "../../ui/Card/Card";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { languageOptions } from "./language-core";
import { SubPage } from "./SubPage";

export function LanguagePage() {
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
  // Boksen viser språket som gjelder. Står det noe annet i basen (et fjernet språk), er det
  // `locale` som er sannheten: det valgte `resolveStartupLocale` ved oppstart.
  // Under en skriving står det valgte (utkastet) til den har landet.
  const draft = String(language.draft ?? "");
  const selected = (ALL_LOCALES as readonly string[]).includes(draft)
    ? draft
    : locale.value;

  return (
    <SubPage lede={t("app.setup.language.lede")} testId="setup-language">
      <Card testId="language-card">
        <SettingRow
          label={t("app.setup.language.label")}
          description={t("app.setup.language.desc")}
          receipt={language.receipt}
          error={language.error}
          testId="language-row"
        >
          {(ids) => (
            <Select
              value={selected}
              options={languageOptions()}
              onChange={(next) => language.set(next)}
              disabled={language.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="language-control-input"
            />
          )}
        </SettingRow>
      </Card>
    </SubPage>
  );
}
