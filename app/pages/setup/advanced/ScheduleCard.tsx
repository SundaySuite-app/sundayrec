/**
 * «Flere tider og spesialopptak» — tidsplanens avanserte halvdel.
 *
 * ## Hva som ble utelatt, og hvorfor
 *
 * Legacys Tidsplan-side er 23 kontroller: en månedskalender med kirkeårets
 * helligdager, en dagsdetalj som folder seg ut når du klikker en dato, et
 * slot-redigeringsskjema med sju dagbrikker, og et vekkings-diagnosekort med
 * strøm, standby, søvnkonfigurasjon, verifisering, feilhistorikk og en
 * 60-sekunders vekketest. Alt det er PORTET SOM LOGIKK der det finnes en ren
 * kjerne (`schedule-core.ts`, `specials-core.ts`), men IKKE som skjerm:
 *
 *   • **Månedskalenderen** — et rutenett er en fin måte å BLA i datoer på, og
 *     en dårlig måte å svare på «når er konserten?». Her er spesialopptak en
 *     liste med en dato-velger, og datovelgeren er nettleserens egen.
 *   • **Helligdagene fra kirkeåret** — de fylte kalenderen med forslag ingen
 *     hadde bedt om. `getChurchHolidays` er urørt i `legacy/shared/`.
 *   • **Vekke-diagnostikken** — seks statuslinjer og en test som tar et minutt.
 *     Det som avgjør om det planlagte opptaket skjer er ÉN ting: kan maskinen
 *     vekkes, og koster det et administratorpassord? Det er én setning her
 *     (`wakeWord`), ikke et kort. Testen, `wake_verify` og feilhistorikken er
 *     ikke med.
 *   • **Flere DAGER per fast tid** — `ScheduleSlot.days` er en liste, og
 *     legacys dagbrikker lar deg velge flere. Her er én rad én dag; en profil
 *     som allerede har flere vises med den første, og listen røres ikke.
 *
 * ## Rekkefølgen er lagringens, ikke visningens
 *
 * Begge listene slettes ved INDEKS, og spesialopptakene VISES sortert. Å bruke
 * visningsindeksen mot den lagrede listen sletter feil rad — det er
 * skjøtefeilens form, og derfor bærer hver rad sin lagrede indeks med seg fra
 * `specials-core.ts`.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tDyn, tf } from "../../../i18n";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../../state/settings";
import { useReceipt } from "../../../settings/use-receipt";
import { refreshNextRecording } from "../../../state/next-recording";
import { BoundToggle } from "../../../ui/Bound/Bound";
import { Button } from "../../../ui/Button/Button";
import { Card } from "../../../ui/Card/Card";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { Select } from "../../../ui/Select/Select";
import { TextField } from "../../../ui/TextField/TextField";
import { toast } from "../../../ui/toast";
import {
  DEFAULT_PLAN,
  DURATION_CHOICES,
  WEEKDAYS,
  type Weekday,
} from "../schedule-core";
import styles from "../setup.module.css";
import {
  checkSpecial,
  isoDate,
  slotDay,
  slotRows,
  specialRows,
  wakeArmWord,
  wakeWord,
  withoutIndex,
  withSlot,
  withSpecial,
  type SpecialDraft,
  type WakeFacts,
} from "./specials-core";

export function ScheduleCard() {
  return (
    <Card
      title={t("app.setup.advanced.schedTitle")}
      description={t("app.setup.advanced.schedDesc")}
      anchor="schedule"
      testId="advanced-schedule"
    >
      <WeeklyTimes />
      <Specials />
      <WakeRow />
    </Card>
  );
}

/** Skriv en delmengde og rull tilbake hvis basen sa nei. */
async function write(
  patch: Parameters<typeof patchSettings>[0],
  before: Parameters<typeof patchSettings>[0],
): Promise<boolean> {
  patchSettings(patch);
  const ok = await saveSettingsDebounced(120);
  if (!ok) {
    patchSettings(before);
    toast("error", t("general.saveFailed"));
  }
  return ok;
}

/**
 * De faste ukentlige tidene, i lagret rekkefølge.
 *
 * Den første er den nivå 1 viser og redigerer, og raden sier det. Å sortere
 * listen ville flyttet «tiden min» rundt uten at noen forsto hvorfor.
 */
function WeeklyTimes() {
  const s = settings.value;
  const rows = slotRows(s.slots);
  const [day, setDay] = useState<Weekday>(DEFAULT_PLAN.day);
  const [start, setStart] = useState(DEFAULT_PLAN.start);
  const [minutes, setMinutes] = useState(DEFAULT_PLAN.minutes);
  const [busy, setBusy] = useState(false);

  async function add(): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      await write(
        {
          // Å legge til en tid er også å arme planen — ellers står en ny rad i
          // en liste bakenden ikke leser.
          autoRecordEnabled: true,
          slots: withSlot(s.slots, day, start, minutes),
        },
        { autoRecordEnabled: s.autoRecordEnabled, slots: s.slots },
      );
    } finally {
      setBusy(false);
    }
  }

  async function remove(index: number): Promise<void> {
    await write({ slots: withoutIndex(s.slots, index) }, { slots: s.slots });
  }

  return (
    <>
      <SettingRow label={t("app.setup.advanced.schedTimes")} testId="adv-slots">
        <span data-testid="adv-slots-count" class={styles.hint}>
          {rows.length === 0
            ? t("app.setup.advanced.schedNone")
            : t("app.setup.advanced.schedFirstIsSetup")}
        </span>
      </SettingRow>

      {rows.map((row) => {
        const d = slotDay(row.value);
        return (
          <SettingRow
            key={`${row.index}-${row.value.start}`}
            label={tf("app.setup.advanced.schedRow", {
              day:
                d === null
                  ? t("app.setup.notSetUp")
                  : tDyn("app.setup.days", String(d)),
              start: row.value.start,
              stop: row.value.stop,
            })}
            testId={`adv-slot-${row.index}`}
          >
            <Button
              variant="ghost"
              testId={`adv-slot-${row.index}-remove`}
              onClick={() => void remove(row.index)}
            >
              {t("app.setup.advanced.schedRemove")}
            </Button>
          </SettingRow>
        );
      })}

      <SettingRow
        label={t("app.setup.advanced.schedAdd")}
        testId="adv-slot-add"
      >
        {(ids) => (
          <>
            <Select
              value={String(day)}
              options={WEEKDAYS.map((d) => ({
                value: String(d),
                label: tDyn("app.setup.days", String(d)),
              }))}
              onChange={(next) => setDay(Number(next) as Weekday)}
              labelId={ids.labelId}
              testId="adv-slot-add-day"
            />
            <input
              type="time"
              value={start}
              aria-label={t("app.setup.auto.start")}
              data-testid="adv-slot-add-start"
              class={styles.time}
              onInput={(event) =>
                setStart((event.target as HTMLInputElement).value)
              }
            />
            <Select
              value={String(minutes)}
              options={DURATION_CHOICES.map((n) => ({
                value: String(n),
                label: tf("app.setup.auto.minutes", { n }),
              }))}
              onChange={(next) => setMinutes(Number(next))}
              testId="adv-slot-add-duration"
            />
            <Button
              variant="secondary"
              busy={busy}
              testId="adv-slot-add-save"
              onClick={() => void add()}
            >
              {t("app.setup.advanced.specialAdd")}
            </Button>
          </>
        )}
      </SettingRow>
    </>
  );
}

/** Standardvarigheten et nytt spesialopptak foreslår. Samme som en gudstjeneste. */
const SPECIAL_DEFAULT_MINUTES = 90;

function Specials() {
  const s = settings.value;
  const today = isoDate(new Date());
  const rows = specialRows(s.specialRecordings, today);
  const [draft, setDraft] = useState<SpecialDraft>({
    name: "",
    date: "",
    start: "19:00",
    minutes: SPECIAL_DEFAULT_MINUTES,
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function add(): Promise<void> {
    const issue = checkSpecial(draft, today);
    if (issue) {
      setError(
        issue === "past"
          ? t("app.setup.advanced.specialPast")
          : t("app.setup.advanced.specialNeedsDate"),
      );
      return;
    }
    setError(null);
    if (busy) return;
    setBusy(true);
    try {
      const ok = await write(
        {
          specialRecordings: withSpecial(
            s.specialRecordings,
            draft,
            t("app.setup.advanced.specialDefaultName"),
          ),
        },
        { specialRecordings: s.specialRecordings },
      );
      if (ok) setDraft({ ...draft, name: "", date: "" });
    } finally {
      setBusy(false);
    }
  }

  async function remove(index: number): Promise<void> {
    await write(
      { specialRecordings: withoutIndex(s.specialRecordings, index) },
      { specialRecordings: s.specialRecordings },
    );
  }

  return (
    <>
      <SettingRow
        label={t("app.setup.advanced.specialsTitle")}
        description={t("app.setup.advanced.specialsDesc")}
        testId="adv-specials"
      >
        <span data-testid="adv-specials-count" class={styles.hint}>
          {rows.length === 0 ? t("app.setup.advanced.specialsNone") : ""}
        </span>
      </SettingRow>

      {rows.map((row) => (
        <SettingRow
          key={`${row.index}-${row.value.date}`}
          label={row.value.name}
          description={tf("app.setup.advanced.specialRow", {
            date: row.value.date,
            start: row.value.start,
            stop: row.value.stop,
          })}
          testId={`adv-special-${row.index}`}
        >
          <Button
            variant="ghost"
            testId={`adv-special-${row.index}-remove`}
            onClick={() => void remove(row.index)}
          >
            {t("app.setup.advanced.specialRemove")}
          </Button>
        </SettingRow>
      ))}

      <SettingRow
        label={t("app.setup.advanced.specialAdd")}
        error={error}
        testId="adv-special-add"
      >
        {(ids) => (
          <>
            <TextField
              value={draft.name}
              placeholder={t("app.setup.advanced.specialDefaultName")}
              onInput={(next) => setDraft({ ...draft, name: next })}
              labelId={ids.labelId}
              testId="adv-special-add-name"
            />
            <input
              type="date"
              value={draft.date}
              aria-label={t("app.setup.advanced.specialDate")}
              data-testid="adv-special-add-date"
              class={styles.time}
              onInput={(event) => {
                setError(null);
                setDraft({
                  ...draft,
                  date: (event.target as HTMLInputElement).value,
                });
              }}
            />
            <input
              type="time"
              value={draft.start}
              aria-label={t("app.setup.advanced.specialStart")}
              data-testid="adv-special-add-start"
              class={styles.time}
              onInput={(event) =>
                setDraft({
                  ...draft,
                  start: (event.target as HTMLInputElement).value,
                })
              }
            />
            <Select
              value={String(draft.minutes)}
              options={DURATION_CHOICES.map((n) => ({
                value: String(n),
                label: tf("app.setup.auto.minutes", { n }),
              }))}
              onChange={(next) => setDraft({ ...draft, minutes: Number(next) })}
              testId="adv-special-add-duration"
            />
            <Button
              variant="secondary"
              busy={busy}
              testId="adv-special-add-save"
              onClick={() => void add()}
            >
              {t("app.setup.advanced.specialAdd")}
            </Button>
          </>
        )}
      </SettingRow>
    </>
  );
}

/**
 * «Vekk maskinen fra dvale» — bryteren, én setning om hva maskinen klarer, og
 * handlingen som gjør bryteren til noe.
 *
 * `wake_capabilities` svarer med seks felter og to lister. Fem av dem er
 * diagnostikk for den som feilsøker; det som avgjør om søndagen blir tatt opp
 * er om maskinen kan vekkes i det hele tatt.
 *
 * ## Hvorfor det MÅ finnes en knapp
 *
 * Bryteren skrev en boolean, og ingenting mer. Planleggeren armer OS-vekkingen
 * i sin egen stille runde — men den runden kan ikke be om et
 * administratorpassord, og på macOS er det nøyaktig det arming koster. Så på
 * en fersk Mac sto bryteren på «på», OS-et hadde ingen vekking, og appen
 * lovte likevel på TA OPP at maskinen ville våkne. `wake_reschedule` er
 * bakendens EGEN vei rundt det (`allow_admin = true`, «user-initiated, so it
 * may prompt»), og uten en knapp fantes den veien ikke i det nye skallet.
 *
 * Svaret vises som en setning og ikke som en toast: «trenger
 * administratorpassord» er noe man skal kunne lese om igjen mens man leter
 * etter passordet, ikke noe som glir bort etter fem sekunder.
 */
function WakeRow() {
  const [facts, setFacts] = useState<WakeFacts | null>(null);
  const [armResult, setArmResult] = useState<{
    ok: boolean;
    reason: string | null;
  } | null>(null);
  const [arming, setArming] = useState(false);
  const { receipt, show: showReceipt } = useReceipt();
  const on = settings.value.wakeFromSleep !== false;

  useEffect(() => {
    void window.api
      .wakeDetectCapabilities()
      .then((caps) =>
        setFacts({
          canWakeFromSleep: caps.canWakeFromSleep === true,
          needsAdmin: caps.needsAdmin === true,
        }),
      )
      // En probe vi ikke fikk kjørt er ikke bevis i noen retning — `null`
      // holder setningen på «vi vet ikke ennå» i stedet for å påstå «kan ikke».
      .catch(() => setFacts(null));
  }, []);

  async function arm(): Promise<void> {
    if (arming) return;
    setArming(true);
    showReceipt("saving");
    try {
      const result = await window.api.wakeReschedule();
      setArmResult(result);
      showReceipt(result.ok ? "saved" : "failed");
      // Helten på TA OPP leser `wake_verify`, ikke bryteren. Etter en armering
      // skal den lese den på nytt med én gang — ellers står den ærlige
      // «vi har ikke fått bekreftet noen vekking» igjen på en maskin som
      // nettopp ble armet, til neste minuttpoll.
      if (result.ok) await refreshNextRecording();
    } catch (err) {
      // En avvist kommando er ikke «det gikk bra». Shimmen svarer med
      // `{ ok:false, reason:"error" }`, men et kast herfra må lande samme sted.
      console.warn("[schedule] wake_reschedule failed:", err);
      setArmResult({ ok: false, reason: "error" });
      showReceipt("failed");
    } finally {
      setArming(false);
    }
  }

  return (
    <>
      <BoundToggle
        setting="wakeFromSleep"
        label={t("app.setup.advanced.wake")}
        description={t("app.setup.advanced.wakeDesc")}
        testId="adv-wake"
      />
      <p data-testid="adv-wake-caps" class={styles.hint}>
        {tDyn("app.setup.advanced.wakeWord", wakeWord(facts))}
      </p>
      <SettingRow
        label={t("app.setup.advanced.wakeArm")}
        description={tDyn(
          "app.setup.advanced.wakeArmWord",
          wakeArmWord(armResult),
        )}
        receipt={receipt}
        // Grået ut når bryteren er av: å registrere en vekking for en
        // innstilling som sier «ikke vekk meg» er en handling uten mening, og
        // bakenden svarer `reason: "disabled"` uansett.
        disabled={!on}
        testId="adv-wake-arm"
      >
        <Button
          variant="secondary"
          busy={arming}
          disabled={!on}
          disabledReason={t("app.setup.advanced.wakeArmWord.disabled")}
          testId="adv-wake-arm-control-input"
          onClick={() => void arm()}
        >
          {t("app.setup.advanced.wakeArm")}
        </Button>
      </SettingRow>
    </>
  );
}
