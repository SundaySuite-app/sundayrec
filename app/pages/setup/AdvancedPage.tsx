/**
 * Avansert — canvasens artboard 5.4.
 *
 * ÉN liste. Ingen faner, ingen seksjoner med egne overskrifter, ingen kort som
 * grupperer to rader fordi de tilfeldigvis het noe likt. Grunnen står i
 * canvasens egen beslutning: «Alt som har en leser i Rust og som noen faktisk
 * trenger, som én liste med ett ord per rad.»
 *
 * ## Hva som IKKE er her, og hvorfor
 *
 * - **«Spør om redigering etter opptak», «Beskytt opptak», «Oppdater
 *   automatisk», «Start med maskinen»** — de fire uten bakendleser (ATLAS
 *   §2.6). «Start med maskinen» bor på nivå 1 inne i «Ta opp automatisk», der
 *   den betyr noe; de tre andre er ute.
 * - **`prerollEnabled`** — også uten bakendleser. Sekundene ER bryteren nå (se
 *   `app/state/preroll.ts`), så to kontroller for én ting er blitt én.
 * - **`silenceThreshold` (dBFS)** — Rust leser den, men −50 dBFS er ikke et
 *   tall noen frivillig kan ha en mening om, og feil verdi stopper opptaket
 *   midt i gudstjenesten. Standarden står; den er ikke synlig.
 * - **«Mikser og lydbehandling»** — canvasen tegner raden med en «Åpne»-knapp.
 *   Mikseren bygges i P4, så knappen ville ikke åpnet noe. En død knapp lærer
 *   en frivillig at knappene her ikke er til å stole på, så raden kommer
 *   sammen med mikseren.
 * - **Månedskalenderen** — spesialopptak er en LISTE her, ikke et rutenett. Se
 *   `advanced/ScheduleCard.tsx` for hva som ble utelatt.
 *
 * ## Hvorfor rader og ikke et trekkspill
 *
 * `SettingRow` har allerede skillelinjen og de fire faste plassene (etikett,
 * kvittering, forklaring, kontroll), og `Card` er beholderen. En ny
 * `Accordion`-komponent ville vært en komponent for å skjule det canvasen
 * viser åpent.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tDyn } from "../../i18n";
import { route } from "../../router/router";
import { confirmIfRecordingImminent } from "../../settings/guards";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import {
  BoundNumberField,
  BoundSelect,
  BoundToggle,
} from "../../ui/Bound/Bound";
import { Card } from "../../ui/Card/Card";
import { Select } from "../../ui/Select/Select";
import { SettingRow, type RowIds } from "../../ui/SettingRow/SettingRow";
import { Toggle } from "../../ui/Toggle/Toggle";
import { toast } from "../../ui/toast";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import { LogRow, ProfileRow } from "./advanced/MaintenanceRows";
import { currentOs } from "./advanced/platform-core";
import { ScheduleCard } from "./advanced/ScheduleCard";
import { SmtpCard } from "./advanced/SmtpCard";
import { TelemetryRow } from "./advanced/TelemetryRow";
import { UpdateRow } from "./advanced/UpdateRow";
import { SubPage } from "./SubPage";

/** Sekundene forhåndsbufferen tilbyr. 0 = av — den ENE bryteren. */
const PREROLL_CHOICES = [0, 15, 30];
/** Minuttene «stopp ved lang stillhet» venter. Legacys fire. */
const SILENCE_CHOICES = [2, 5, 10, 15];
/** Maks lengde, i minutter. 0 = ingen grense. Legacys seks. */
const MAX_LEN_CHOICES = [0, 60, 120, 180, 240, 360];
/** Hvor ofte en lang opptaksfil deles. Legacys fem. */
const SPLIT_CHOICES = [30, 45, 60, 90, 120];
/** Standarden «Del opp» slår på med — legacys `60 min (1 t)`. */
const SPLIT_DEFAULT_MINUTES = 60;
/** Standarden «Slett gamle opptak» slår på med. Legacys felt viser 90. */
const AUTO_DELETE_DEFAULT_DAYS = 90;
/** Under dette spør vi først — samme grense som legacy. */
const AUTO_DELETE_GUARD_DAYS = 30;

/** De tre opptaksmotorene, som ett valg. */
type Engine = "native" | "ffmpeg" | "dshow";

export function AdvancedPage() {
  // Nivå 1 og de gamle dyplenkene peker på et sted INNE i denne lista
  // (`?goto=settings:general` → toppen, «Flere tider …» → `schedule`). Uten
  // dette lander man bare på toppen og må lete etter det man klikket seg fram
  // til.
  const anchor = route.value.anchor;
  useEffect(() => {
    if (!anchor) return;
    document.getElementById(anchor)?.scrollIntoView({ block: "start" });
  }, [anchor]);

  return (
    <SubPage lede={t("app.setup.advanced.lede")} testId="setup-advanced">
      <Card testId="advanced-recording" anchor="engine">
        <EngineRow />
        <BoundSelect
          setting="preRollSeconds"
          label={t("app.setup.advanced.preroll")}
          description={t("app.setup.advanced.prerollDesc")}
          options={PREROLL_CHOICES.map((n) => ({
            value: String(n),
            label: tDyn("app.setup.advanced.prerollChoice", String(n)),
          }))}
          testId="adv-preroll"
        />
        <SilenceRows />
        <BoundSelect
          setting="manualMaxMinutes"
          label={t("app.setup.advanced.maxLen")}
          description={t("app.setup.advanced.maxLenDesc")}
          options={MAX_LEN_CHOICES.map((n) => ({
            value: String(n),
            label: tDyn("app.setup.advanced.maxLenChoice", String(n)),
          }))}
          testId="adv-maxlen"
        />
        <SplitRows />
        <AutoDeleteRows />
      </Card>

      <Card testId="advanced-system">
        <TelemetryRow />
        <UpdateRow />
        <LogRow />
        <ProfileRow />
      </Card>

      <SmtpCard />
      <ScheduleCard />
    </SubPage>
  );
}

/**
 * Opptaksmotor — ett valg, to nøkler.
 *
 * `classicFfmpegAudio` og `classicDirectshow` er to brytere i basen, men ETT
 * spørsmål: hvilken motor tar opp? To brytere kan dessuten stå PÅ samtidig,
 * som er en tilstand ingen har en mening om. Derfor én `Select` og én eksplisitt
 * skrivning av begge — samme mønster som OS-varselet på spørsmål 5 og
 * enhetsvalget på spørsmål 1, og av samme grunn: `useSetting` eier én nøkkel og
 * har én kvittering.
 *
 * DirectShow-valget finnes bare på Windows, slik legacy også gjør det — men
 * avgjørelsen kommer fra `detectOs`, ikke fra en delstreng i UA-linja. En gate
 * som gjetter feil vei viser en bryter som skriver en verdi ingenting leser.
 */
function EngineRow() {
  const s = settings.value;
  const os = currentOs();
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  const current: Engine = s.classicDirectshow
    ? "dshow"
    : s.classicFfmpegAudio
      ? "ffmpeg"
      : "native";

  const options: Engine[] =
    os === "win" ? ["native", "ffmpeg", "dshow"] : ["native", "ffmpeg"];

  async function choose(next: Engine): Promise<void> {
    if (busy || next === current) return;
    // Å bytte motor fire minutter før gudstjenesten er endringen som stille
    // koster opptaket. Samme vakt som legacy, samme terskel som alt annet.
    const ok = await confirmIfRecordingImminent(
      t("app.setup.advanced.engineConfirm"),
    );
    if (!ok) return;

    setBusy(true);
    setReceipt("saving");
    const before = {
      classicFfmpegAudio: s.classicFfmpegAudio,
      classicDirectshow: s.classicDirectshow,
    };
    try {
      patchSettings({
        classicFfmpegAudio: next === "ffmpeg",
        classicDirectshow: next === "dshow",
      });
      const saved = await saveSettingsDebounced(120);
      setReceipt(saved ? "saved" : "failed");
      if (!saved) {
        patchSettings(before);
        toast("error", t("general.saveFailed"));
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingRow
      label={t("app.setup.advanced.engine")}
      description={t("app.setup.advanced.engineDesc")}
      receipt={receipt}
      testId="adv-engine"
    >
      {(ids) => (
        <Select
          value={current}
          options={options.map((id) => ({
            value: id,
            label: tDyn("app.setup.advanced.engineChoice", id),
          }))}
          onChange={(next) => void choose(next as Engine)}
          disabled={busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId="adv-engine-control-input"
        />
      )}
    </SettingRow>
  );
}

/**
 * Stopp ved lang stillhet — bryteren, og varigheten bak den.
 *
 * Varigheten vises bare når bryteren står på. En select som står der uansett er
 * et valg uten virkning, og det er den formen for kontroll som lærer folk at
 * ingenting her henger sammen.
 */
function SilenceRows() {
  const on = settings.value.stopOnSilence === true;
  return (
    <>
      <BoundToggle
        setting="stopOnSilence"
        label={t("app.setup.advanced.silence")}
        description={t("app.setup.advanced.silenceDesc")}
        testId="adv-silence"
      />
      {on ? (
        <BoundSelect
          setting="silenceTimeoutMinutes"
          label={t("app.setup.advanced.silenceAfter")}
          options={SILENCE_CHOICES.map((n) => ({
            value: String(n),
            label: tDyn("app.setup.advanced.silenceChoice", String(n)),
          }))}
          testId="adv-silence-after"
        />
      ) : null}
    </>
  );
}

/**
 * Én nøkkel der 0 betyr av, vist som en bryter + en verdi.
 *
 * `splitMinutes` og `autoDeleteDays` har samme form, og formen har en felle:
 * bryteren skriver et TALL, ikke en boolean. Å skru på må derfor velge en
 * standard, og standarden er synlig i raden under med én gang — ikke en
 * usynlig hukommelse fra forrige gang, som er umulig å forklare når den bommer.
 */
function NumericToggleRow({
  label,
  description,
  checked,
  onChange,
  testId,
}: {
  label: string;
  description: string;
  checked: boolean;
  onChange: (on: boolean) => Promise<boolean>;
  testId: string;
}) {
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  async function flip(next: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setReceipt("saving");
    try {
      setReceipt((await onChange(next)) ? "saved" : "failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingRow
      label={label}
      description={description}
      receipt={receipt}
      testId={testId}
    >
      {(ids: RowIds) => (
        <Toggle
          checked={checked}
          onChange={(next) => void flip(next)}
          disabled={busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={`${testId}-control-input`}
        />
      )}
    </SettingRow>
  );
}

/** Skriv ett tallfelt, og rull tilbake hvis basen sa nei. */
async function writeNumber(
  key: "splitMinutes" | "autoDeleteDays",
  next: number,
  before: number,
): Promise<boolean> {
  patchSettings({ [key]: next });
  const ok = await saveSettingsDebounced(120);
  if (!ok) {
    patchSettings({ [key]: before });
    toast("error", t("general.saveFailed"));
  }
  return ok;
}

function SplitRows() {
  const minutes = settings.value.splitMinutes ?? 0;
  return (
    <>
      <NumericToggleRow
        label={t("app.setup.advanced.split")}
        description={t("app.setup.advanced.splitDesc")}
        checked={minutes > 0}
        onChange={(on) =>
          writeNumber("splitMinutes", on ? SPLIT_DEFAULT_MINUTES : 0, minutes)
        }
        testId="adv-split"
      />
      {minutes > 0 ? (
        <BoundSelect
          setting="splitMinutes"
          label={t("app.setup.advanced.splitEvery")}
          options={SPLIT_CHOICES.map((n) => ({
            value: String(n),
            label: tDyn("app.setup.advanced.splitChoice", String(n)),
          }))}
          testId="adv-split-every"
        />
      ) : null}
    </>
  );
}

function AutoDeleteRows() {
  const days = settings.value.autoDeleteDays ?? 0;
  return (
    <>
      <NumericToggleRow
        label={t("app.setup.advanced.autoDelete")}
        description={t("app.setup.advanced.autoDeleteDesc")}
        checked={days > 0}
        onChange={(on) =>
          writeNumber("autoDeleteDays", on ? AUTO_DELETE_DEFAULT_DAYS : 0, days)
        }
        testId="adv-autodelete"
      />
      {days > 0 ? (
        <BoundNumberField
          setting="autoDeleteDays"
          label={t("app.setup.advanced.autoDeleteAfter")}
          rule={{ min: 1, max: 3650, integer: true }}
          message={() => t("app.setup.advanced.autoDeleteRange")}
          // Legacys grense, og legacys grunn: «7» skrevet i et felt er sju
          // gudstjenester i papirkurven før noen har rukket å laste dem ned.
          confirmIf={(value) =>
            Number(value) > 0 && Number(value) < AUTO_DELETE_GUARD_DAYS
              ? {
                  title: t("app.setup.advanced.autoDeleteConfirm"),
                  message: t("app.setup.advanced.autoDeleteConfirmBody"),
                  confirmLabel: t("app.setup.advanced.autoDeleteConfirmYes"),
                  cancelLabel: t("app.setup.cancel"),
                }
              : null
          }
          testId="adv-autodelete-days"
        />
      ) : null}
    </>
  );
}
