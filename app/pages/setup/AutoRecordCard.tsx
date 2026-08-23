/**
 * Tillegg — «Ta opp automatisk».
 *
 * ÉN ukentlig tid: dag, klokkeslett, varighet. Kalender, spesialopptak og
 * vekke-diagnostikk er Avansert (P1b) — de hører til den som allerede har
 * svart ja på dette spørsmålet.
 *
 * ## ⚠️ Av/på sletter tiden, og det er en eiersak
 *
 * `Settings` har ingen `enabled`-flagg, verken på en slot eller på planen —
 * bakenden kjenner bare `slots: ScheduleSlot[]`, og en tom liste ER «av». Så
 * det er det bryteren skriver. Konsekvensen: slår du av, er tidspunktet borte
 * fra basen.
 *
 * Skjermen demper det den kan uten å lyve om det:
 *
 *   • den siste planen huskes i ØKTEN (`remembered` under), så av-og-på-igjen
 *     før du lukker appen gir deg tiden tilbake,
 *   • har profilen FLERE tidspunkter, spør bryteren først og sier hvor mange
 *     som forsvinner — de er ikke synlige på denne skjermen, og en skjerm skal
 *     ikke slette data den ikke viser uten å nevne det.
 *
 * Å gjøre det ordentlig krever en ny nøkkel i Rust. Den legges ikke til fordi
 * en bryter gjerne vil oppføre seg penere — det er eierens valg.
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

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";

import type { ScheduleSlot } from "@lib/../bindings/ScheduleSlot";

import { t, tDyn, tf } from "../../i18n";
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
import { confirmDialog } from "../../ui/dialog";
import { Receipt } from "../../ui/Receipt/Receipt";
import { Select } from "../../ui/Select/Select";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Toggle } from "../../ui/Toggle/Toggle";
import { toast } from "../../ui/toast";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import {
  DEFAULT_PLAN,
  DURATION_CHOICES,
  planFromSlots,
  slotsFromPlan,
  WEEKDAYS,
  type WeeklyPlan,
} from "./schedule-core";
import styles from "./setup.module.css";

/**
 * Planen som sto her sist den ble slått av — for DENNE økten.
 *
 * Et modulnivå-signal og ikke en innstilling: det er ikke noe vi lover å huske
 * over en omstart, og en nøkkel i Rust for å kunne love det er en beslutning
 * eieren tar (se toppen av fila). Å huske det i økten koster ingenting og gjør
 * det vanligste feiltrykket ufarlig.
 */
const remembered = signal<WeeklyPlan | null>(null);

export function AutoRecordCard() {
  const s = settings.value;
  const slots = s.slots ?? [];
  const plan = planFromSlots(slots);
  const on = plan !== null;
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void syncLaunchAtLoginFromOs();
  }, []);

  async function write(nextSlots: ScheduleSlot[]): Promise<boolean> {
    patchSettings({ slots: nextSlots });
    const ok = await saveSettingsDebounced(120);
    if (!ok) {
      patchSettings({ slots });
      toast("error", t("general.saveFailed"));
    }
    return ok;
  }

  async function toggle(next: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setReceipt("saving");
    try {
      if (next) {
        const restored = remembered.peek() ?? DEFAULT_PLAN;
        setReceipt(
          (await write(slotsFromPlan(restored, slots))) ? "saved" : "failed",
        );
        return;
      }
      // Av. Flere tidspunkter enn det ene nivå 1 viser ⇒ spør først, med
      // antallet i spørsmålet.
      if (slots.length > 1) {
        const ok = await confirmDialog({
          title: t("app.setup.auto.offTitle"),
          message: tf("app.setup.auto.offBody", { n: slots.length }),
          confirmLabel: t("app.setup.auto.offConfirm"),
          cancelLabel: t("app.setup.cancel"),
          danger: true,
        });
        if (!ok) {
          setReceipt("idle");
          return;
        }
      }
      if (plan) remembered.value = plan;
      setReceipt((await write([])) ? "saved" : "failed");
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
            {plan
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

      {plan ? (
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
