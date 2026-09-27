/**
 * «Legg ut» — hvor menigheten legger ut prekenen, og teksten som følger med.
 *
 * SundayRec laster ikke opp noe. Kortet bestemmer bare to ting kvitteringen
 * etter en eksport skal gjøre (`PublishPanel` i `ExportPage.tsx`): hvilken
 * side knappen åpner, og hvilken fast beskrivelse «Innhold» starter fra. Begge
 * settes én gang, av den som setter opp maskinen — derfor Avansert og ikke en
 * egen side.
 *
 * ## Lenken sjekkes to steder, og det er meningen
 *
 * Her, mens den skrives (`customUrlProblem`), så Oppsett kan si «den blir ikke
 * godtatt» med en gang. Og i bakenden, hver gang den skal åpnes
 * (`sundayrec_core::publish::custom_upload_url`), fordi det er DER en adresse
 * blir gitt til operativsystemet. Skjermens sjekk er høflighet; bakendens er
 * vakten.
 */

import { t, tDyn } from "../../../i18n";
import { settings } from "../../../state/settings";
import { useSetting } from "../../../settings/use-setting";
import { BoundRadioCards, BoundTextField } from "../../../ui/Bound/Bound";
import { Card } from "../../../ui/Card/Card";
import type { RadioOption } from "../../../ui/RadioCards/RadioCards";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { TextArea } from "../../../ui/TextArea/TextArea";
import {
  channelName,
  customUrlProblem,
  PUBLISH_TARGETS,
} from "../../../editor/publish-core";

export function PublishCard() {
  const target = settings.value.publishTarget ?? "soundcloud";

  const options: RadioOption[] = PUBLISH_TARGETS.map((id) => ({
    value: id,
    // Produktnavnene er de samme på alle språk; «Egen side» og «Ingen» er ord.
    title: channelName(id) ?? tDyn("app.setup.advanced.publishChannelName", id),
    description: tDyn("app.setup.advanced.publishChannelDesc", id),
  }));

  return (
    <Card
      title={t("app.setup.advanced.publishTitle")}
      description={t("app.setup.advanced.publishDesc")}
      anchor="publish"
      testId="advanced-publish"
    >
      <BoundRadioCards
        setting="publishTarget"
        label={t("app.setup.advanced.publishWhere")}
        options={options}
        columns={2}
        testId="adv-publish-target"
      />
      {target === "custom" ? (
        <BoundTextField
          setting="publishCustomUrl"
          label={t("app.setup.advanced.publishUrl")}
          description={t("app.setup.advanced.publishUrlDesc")}
          placeholder={t("app.setup.advanced.publishUrlDesc")}
          validate={(value) => {
            // Tom er lov: da åpner knappen ingenting, og kvitteringen sier hvorfor.
            const problem = customUrlProblem(String(value ?? ""));
            if (problem === "notHttps")
              return t("app.setup.advanced.publishUrlNotHttps");
            if (problem === "invalid")
              return t("app.setup.advanced.publishUrlInvalid");
            return null;
          }}
          testId="adv-publish-url"
        />
      ) : null}
      {target !== "none" ? <TemplateRow /> : null}
    </Card>
  );
}

/**
 * Den faste beskrivelsen. Over flere linjer, så `BoundTextField` (ett
 * `<input>`) ville spist linjeskiftene — derfor `useSetting` direkte, med den
 * samme rad-formen og den samme kvitteringen som de andre.
 */
function TemplateRow() {
  const bound = useSetting("publishDescriptionTemplate", { kind: "text" });
  return (
    <SettingRow
      label={t("app.setup.advanced.publishTemplate")}
      description={t("app.setup.advanced.publishTemplateDesc")}
      receipt={bound.receipt}
      error={bound.error}
      testId="adv-publish-template"
    >
      {(ids) => (
        <TextArea
          value={String(bound.draft ?? "")}
          onInput={(next) => bound.set(next)}
          onCommit={() => void bound.commit()}
          placeholder={t("app.setup.advanced.publishTemplatePlaceholder")}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId="adv-publish-template-control-input"
        />
      )}
    </SettingRow>
  );
}
