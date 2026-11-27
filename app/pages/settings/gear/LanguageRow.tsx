/**
 * Språk — «Norsk | English».
 *
 * Språket er en vanlig innstilling med vanlig kvittering, men BYTTET skjer i
 * `after`, altså først når skrivningen faktisk landet. Å flippe katalogen før
 * det ville gitt en app på engelsk og en base på norsk hvis lagringen feilet,
 * og `useSetting` ville rullet verdien tilbake under en skjerm som allerede
 * hadde byttet språk.
 */

import {
  ALL_LOCALES,
  locale,
  setLocale,
  t,
  tDyn,
  type Locale,
} from "../../../i18n";
import { useSetting } from "../../../settings/use-setting";
import { Receipt } from "../../../ui/Receipt/Receipt";
import { Segmented } from "../../../ui/Segmented/Segmented";
import { GearRow } from "./GearRow";

export function LanguageRow() {
  const language = useSetting("language", {
    kind: "select",
    after: (value) => {
      const next = String(value ?? "no") as Locale;
      if (next !== locale.peek()) void setLocale(next);
    },
  });
  // Boksen viser språket som gjelder. Står det noe annet i basen (et fjernet
  // språk), er `locale` sannheten: det valgte `resolveStartupLocale` ved
  // oppstart. Under en skriving står det valgte (utkastet) til den har landet.
  const draft = String(language.draft ?? "");
  const selected = (ALL_LOCALES as readonly string[]).includes(draft)
    ? draft
    : locale.value;

  return (
    <GearRow
      testId="gear-row-language"
      anchors={["language"]}
      icon="globe"
      title={t("app.gear.language")}
      sub={<Receipt state={language.receipt} testId="gear-language-receipt" />}
      controls={
        <Segmented
          testId="gear-language"
          label={t("app.gear.language")}
          value={selected}
          disabled={language.busy}
          options={ALL_LOCALES.map((code) => ({
            value: code,
            label: tDyn("app.gear.lang", code),
            testId: `gear-language-${code}`,
          }))}
          onChange={(next) => language.set(next)}
        />
      }
    />
  );
}
