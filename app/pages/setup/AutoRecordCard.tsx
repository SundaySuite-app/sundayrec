/**
 * Tillegg — «Ta opp automatisk».
 *
 * ÉN ukentlig tid: dag, klokkeslett, varighet. Flere tider, spesialopptak og
 * vekkingen er Avansert — de hører til den som allerede har svart ja på dette
 * spørsmålet. Lenken nederst på nivå 1 går dit.
 *
 * ## Av/på sletter INGENTING lenger
 *
 * P1a skrev dette ned som en eiersak: `Settings` hadde ikke noe sted å huske
 * «armert», så en tom `slots`-liste var den eneste måten å stave «av» på — og
 * bryteren måtte slette tidspunktet for å slå seg av. En bryter som kaster data
 * den ikke viser.
 *
 * Eieren svarte, og P1b la til nøkkelen: `autoRecordEnabled`, med ÉN leser i
 * bakenden (`Settings::active_slots`). «Av» skriver nå bare flagget; tiden blir
 * stående i basen og kommer tilbake nøyaktig som den var — også etter en
 * omstart, som en økt-hukommelse aldri kunne love.
 *
 * `default = true` i Rust, så en profil skrevet før feltet fantes fortsetter å
 * ta opp. Det motsatte ville stille avvæpnet hver menighet som allerede hadde
 * en søndagstid.
 *
 * ## «Start automatisk med maskinen» bor HER
 *
 * `launchAtLogin` er meningsløs alene: den handler ikke om å starte et program,
 * den handler om at det planlagte opptaket faktisk skjer etter en omstart. Så
 * den er en rad inne i dette kortet, ikke en systeminnstilling et annet sted.
 *
 * OS-et er fasiten på om innloggingselementet finnes: `get_launch_at_login`
 * leses ved oppstart av kortet, og hvis den er uenig med det lagrede vinner
 * OS-et (samme regel som `syncAutostartFromOs` i legacy). En avkrysning som
 * lover at opptaket overlever en omstart, på en maskin der elementet ble
 * fjernet for hånd, er nøyaktig den løgnen som oppdages en søndag.
 *
 * ## `scheduler_reschedule`
 *
 * Skjer i `window.api.saveSettings` etter hver skrivning, som for alt annet.
 */

import { useEffect, useState } from "preact/hooks";

import type { ScheduleSlot } from "@lib/../bindings/ScheduleSlot";

import { t, tDyn, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { useDraftForm } from "../../settings/use-draft-form";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
  syncLaunchAtLoginFromOs,
} from "../../state/settings";
import { BoundToggle } from "../../ui/Bound/Bound";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Receipt } from "../../ui/Receipt/Receipt";
import { Select } from "../../ui/Select/Select";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Toggle } from "../../ui/Toggle/Toggle";
import { toast } from "../../ui/toast";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import {
  autoRecordOn,
  DEFAULT_PLAN,
  DURATION_CHOICES,
  planFromSlots,
  slotsFromPlan,
  WEEKDAYS,
  type WeeklyPlan,
} from "./schedule-core";
import styles from "./setup.module.css";

export function AutoRecordCard() {
  const s = settings.value;
  const slots = s.slots ?? [];
  const plan = planFromSlots(slots);
  const on = autoRecordOn(s);
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void syncLaunchAtLoginFromOs();
  }, []);

  /** Skriv, og rull tilbake HELE endringen hvis basen sa nei. */
  async function write(patch: {
    autoRecordEnabled?: boolean;
    slots?: ScheduleSlot[];
  }): Promise<boolean> {
    const before = { autoRecordEnabled: s.autoRecordEnabled, slots };
    patchSettings(patch);
    const ok = await saveSettingsDebounced(120);
    if (!ok) {
      patchSettings(before);
      toast("error", t("general.saveFailed"));
    }
    return ok;
  }

  async function toggle(next: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setReceipt("saving");
    try {
      // PÅ: arm flagget, og gi profilen en tid hvis den ikke har noen. En
      // profil som HAR tider beholder dem nøyaktig som de sto.
      // AV: bare flagget. Ingen dialog, fordi ingenting forsvinner.
      const ok = next
        ? await write({
            autoRecordEnabled: true,
            slots: slots.length ? slots : slotsFromPlan(DEFAULT_PLAN, slots),
          })
        : await write({ autoRecordEnabled: false });
      setReceipt(ok ? "saved" : "failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card testId="setup-auto" anchor="auto">
      <div class={styles.addonHead}>
        <Toggle
          checked={on}
          onChange={(next) => void toggle(next)}
          disabled={busy}
          labelId="setup-auto-label"
          testId="setup-auto-toggle"
        />
        <div class={styles.grow}>
          <div id="setup-auto-label" class={styles.addonTitle}>
            {t("app.setup.auto.title")}
          </div>
          <div data-testid="setup-auto-summary" class={styles.addonSummary}>
            {on && plan
              ? tf("app.setup.auto.summary", {
                  day: tDyn("app.setup.days", String(plan.day)),
                  start: plan.start,
                  n: plan.minutes,
                })
              : t("app.setup.auto.desc")}
          </div>
        </div>
        <Receipt state={receipt} testId="setup-auto-receipt" />
      </div>

      {on && plan ? (
        <div class={styles.addonBody}>
          <PlanEditor plan={plan} slots={slots} />
          {slots.length > 1 ? (
            <p data-testid="setup-auto-more" class={styles.hint}>
              {tf("app.setup.auto.more", { n: slots.length })}
            </p>
          ) : null}
          <BoundToggle
            setting="launchAtLogin"
            label={t("app.setup.auto.launch")}
            description={t("app.setup.auto.launchDesc")}
            testId="auto-launch"
          />
          <div class={styles.footer}>
            <Button
              variant="ghost"
              testId="setup-auto-advanced"
              onClick={() =>
                navigate("setup", { tab: "advanced", anchor: "schedule" })
              }
            >
              {t("app.setup.advanced.schedTitle")}
            </Button>
          </div>
        </div>
      ) : null}
    </Card>
  );
}

/**
 * Dag, klokkeslett og varighet — med eksplisitt Lagre.
 *
 * Auto-lagring er feil her, og det er den ene grunnen `useDraftForm` finnes:
 * «søndag 10:0» er et halvskrevet klokkeslett bakenden ville armet en OS-vekking
 * på. Det er også slik legacy-slot-redigereren gjør det.
 */
function PlanEditor({
  plan,
  slots,
}: {
  plan: WeeklyPlan;
  slots: readonly ScheduleSlot[];
}) {
  const [receipt, setReceipt] = useState<ReceiptState>("idle");

  const form = useDraftForm<WeeklyPlan>(
    () => plan,
    async (value) => {
      patchSettings({ slots: slotsFromPlan(value, slots) });
      const ok = await saveSettingsDebounced(120);
      if (!ok) toast("error", t("general.saveFailed"));
      return ok;
    },
  );

  return (
    <>
      <SettingRow
        label={t("app.setup.auto.day")}
        receipt={receipt}
        testId="auto-day"
      >
        {(ids) => (
          <Select
            value={String(form.draft.day)}
            options={WEEKDAYS.map((day) => ({
              value: String(day),
              label: tDyn("app.setup.days", String(day)),
            }))}
            onChange={(next) =>
              form.set({ day: Number(next) as WeeklyPlan["day"] })
            }
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="auto-day-control-input"
          />
        )}
      </SettingRow>

      <SettingRow label={t("app.setup.auto.start")} testId="auto-start">
        {(ids) => (
          // Et ekte `<input type="time">`: OS-ets egen tidsvelger, tastaturet
          // og 12/24-timers oppsett følger brukerens system, og et ugyldig
          // klokkeslett kan ikke skrives inn i det hele tatt.
          <input
            type="time"
            value={form.draft.start}
            aria-labelledby={ids.labelId}
            aria-describedby={ids.describedBy}
            data-testid="auto-start-control-input"
            class={styles.time}
            onInput={(event) =>
              form.set({ start: (event.target as HTMLInputElement).value })
            }
          />
        )}
      </SettingRow>

      <SettingRow
        label={t("app.setup.auto.duration")}
        description={t("app.setup.auto.durationDesc")}
        testId="auto-duration"
      >
        {(ids) => (
          <Select
            value={String(form.draft.minutes)}
            options={DURATION_CHOICES.map((n) => ({
              value: String(n),
              label: tf("app.setup.auto.minutes", { n }),
            }))}
            onChange={(next) => form.set({ minutes: Number(next) })}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="auto-duration-control-input"
          />
        )}
      </SettingRow>

      <div class={styles.footer}>
        <Button
          variant="primary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.auto.nothingToSave")}
          testId="auto-save"
          onClick={() => {
            setReceipt("saving");
            void form.save().then((ok) => setReceipt(ok ? "saved" : "failed"));
          }}
        >
          {t("app.setup.save")}
        </Button>
        <Button
          variant="secondary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.auto.nothingToSave")}
          testId="auto-cancel"
          onClick={() => {
            setReceipt("idle");
            form.cancel();
          }}
        >
          {t("app.setup.cancel")}
        </Button>
      </div>
    </>
  );
}
