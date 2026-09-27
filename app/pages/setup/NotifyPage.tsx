/**
 * 5 — Hvem får beskjed hvis noe går galt? (canvasens artboard 5.3)
 *
 * ## Svaret er: den som står ved maskinen
 *
 * SundayRec sier fra om feil med et systemvarsel på selve opptaksmaskinen, og
 * ingen innstilling kan slå det av — den som står ved PC-en er den eneste som
 * fortsatt kan redde gudstjenesten. E-postvarslene (egen SMTP-server eller
 * SundaySuite-reléet) er fjernet: oppsettet var for tungvint for en frivillig,
 * og appen sender ikke lenger noe ut av maskinen for å melde en feil.
 *
 * ## Én bryter for OS-varsler, ikke to
 *
 * Bakenden har `notifyStart` og `notifyStop`, og dagens app har en bryter for
 * hver. Ingen frivillig har et forhold til den forskjellen: enten sier maskinen
 * fra om opptaket, eller så gjør den ikke det. Så: ÉN bryter som skriver begge.
 * Den gjelder bare «startet»/«avsluttet» — feilvarslene står alltid på.
 */

import { t, tf } from "../../i18n";
import { usePatch } from "../../settings/use-patch";
import { useSetting } from "../../settings/use-setting";
import { settings } from "../../state/settings";
import { Card } from "../../ui/Card/Card";
import { Gate } from "../../ui/Gate/Gate";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { Toggle } from "../../ui/Toggle/Toggle";
import { autoRecordOn } from "./schedule-core";
import { SubPage } from "./SubPage";

/** Minuttene «påminnelse før opptak» tilbyr. 0 = av. */
const REMINDER_CHOICES = [0, 5, 10, 15, 30, 60];

export function NotifyPage() {
  const s = settings.value;

  // ── OS-varsler: én bryter, to nøkler ──────────────────────────────────────
  // `usePatch` og ikke en håndlagd lagring: sekvensen (anvend → skriv →
  // kvittering | rull tilbake) er den samme som `useSetting` kjører, og
  // kvitteringen teller ned i stedet for å bli stående som «Lagret ✓» til
  // siden forlates.
  const osOn = s.notifyStart || s.notifyStop;
  const osNotify = usePatch();

  const reminder = useSetting("reminderMinutes", { kind: "select" });
  // Flagget OG en tid — en påminnelse før et opptak som ikke er armert er en
  // beskjed om noe som ikke skal skje.
  const autoOn = autoRecordOn(s);

  return (
    <SubPage lede={t("app.setup.notify.lede")} testId="setup-notify">
      <Card testId="notify-card">
        <SettingRow
          label={t("app.setup.notify.os")}
          description={t("app.setup.notify.osDesc")}
          receipt={osNotify.receipt}
          testId="notify-os"
        >
          {(ids) => (
            <Toggle
              checked={osOn}
              onChange={(next) =>
                void osNotify.write({ notifyStart: next, notifyStop: next })
              }
              disabled={osNotify.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="notify-os-control-input"
            />
          )}
        </SettingRow>
      </Card>

      <Card testId="notify-reminder-card">
        <Gate
          status={autoOn ? "ok" : "unconfigured"}
          testId="notify-reminder-gate"
          chipText={t("app.setup.notify.remindChip")}
          explanation={t("app.setup.notify.remindGate")}
        >
          <SettingRow
            label={t("app.setup.notify.remind")}
            description={t("app.setup.notify.remindDesc")}
            receipt={reminder.receipt}
            testId="notify-reminder"
          >
            {(ids) => (
              <Select
                value={String(reminder.draft ?? 0)}
                options={REMINDER_CHOICES.map((n) => ({
                  value: String(n),
                  label:
                    n === 0
                      ? t("app.setup.notify.remindOff")
                      : tf("app.setup.notify.minutes", { n }),
                }))}
                onChange={(next) => reminder.set(Number(next))}
                disabled={reminder.busy}
                labelId={ids.labelId}
                describedBy={ids.describedBy}
                testId="notify-reminder-control-input"
              />
            )}
          </SettingRow>
        </Gate>
      </Card>
    </SubPage>
  );
}
