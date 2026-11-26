/**
 * Resten av Innstillinger — opptaksvalg, oppdatering, personvern og om/logg.
 *
 * Dette var «Avansert» (artboard 5.4). F5 flyttet tidsplanen til PLAN og
 * lyd/kamera/mappe/varsler/språk opp som egne seksjoner i `SettingsPage`; det
 * som står igjen her er det som ikke hører hjemme noe annet sted. Seksjonene er
 * barn av `SettingsPage`-kolonnen (`display: contents`), så de har samme
 * avstand som resten.
 *
 * ## Hva som IKKE er her, og hvorfor
 *
 * - **«Spør om redigering etter opptak», «Beskytt opptak», «Start med
 *   maskinen»** — tre av de fire uten bakendleser (ATLAS §2.6). «Start med
 *   maskinen» bor på PLAN, inne i «Ta opp automatisk», der den betyr noe.
 *
 *   ⚠️ **«Oppdater automatisk»** har en bakendleser likevel: renderen eier den
 *   timesvise sjekken, og PRIVACY.md lover at den kan slås av: «Slår du den
 *   av, tar appen ikke kontakt med serveren — verken ved oppstart eller den
 *   vanlige sjekken hver time.» En bryter uten timer og en timer uten bryter er
 *   begge et brutt løfte, så de hører sammen. Se `app/state/auto-update.ts`.
 * - **`prerollEnabled`** — uten bakendleser. Sekundene ER bryteren (se
 *   `app/state/preroll.ts`).
 * - **`silenceThreshold` (dBFS)** — −50 dBFS er ikke et tall noen frivillig kan
 *   ha en mening om, og feil verdi stopper opptaket midt i gudstjenesten.
 * - **Tidsplan, vekking og feilsøkingsrader** — tidsplanen er PLAN. Diagnosen
 *   og «Legg ut»-radene gikk ut i F2.
 *
 * ## Hvorfor rader og ikke et trekkspill
 *
 * `SettingRow` har allerede skillelinjen og de fire faste plassene (etikett,
 * kvittering, forklaring, kontroll), og `Card` er beholderen.
 */

import { t, tDyn } from "../../i18n";
import { usePatch } from "../../settings/use-patch";
import { settings, type Settings } from "../../state/settings";
import {
  BoundNumberField,
  BoundSelect,
  BoundToggle,
} from "../../ui/Bound/Bound";
import { Card } from "../../ui/Card/Card";
import { SettingRow, type RowIds } from "../../ui/SettingRow/SettingRow";
import { Toggle } from "../../ui/Toggle/Toggle";
import { AsioAttribution } from "./advanced/AsioAttribution";
import { LicensesCard } from "./advanced/LicensesCard";
import { LogRow } from "./advanced/MaintenanceRows";
import { TelemetryRow } from "./advanced/TelemetryRow";
import { UpdateRow } from "./advanced/UpdateRow";
import { SettingsSection } from "./SettingsSection";
import styles from "./settings.module.css";

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

export function AdvancedPage() {
  return (
    // `display: contents` (se `.advanced`): seksjonene er barn av
    // SettingsPage sin kolonne, så de har samme avstand som seksjonene over.
    <div data-testid="setup-advanced" class={styles.advanced}>
      <SettingsSection
        id="sec-recording"
        sectionKey="recording"
        intro={t("app.setup.sections.recordingDesc")}
      >
        {/*
          Ankeret heter fortsatt «engine»: Lyd-sida lenker hit for «Avansert
          lyd», og lenka skal lande på opptaksinnstillingene.
        */}
        <Card testId="advanced-recording" anchor="engine">
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
          {/*
            Opptakslistens bunnlinje lenker hit («Slettes automatisk etter 90
            dager · Endre»), og `SettingsPage` ruller til `route.anchor`.
            Ankeret er en bar `id` og ikke et `Card`-anker, fordi radene deler
            kort med resten av opptaksvalgene.
          */}
          <div id="autodelete">
            <AutoDeleteRows />
          </div>
        </Card>
      </SettingsSection>

      <SettingsSection
        id="sec-updates"
        sectionKey="updates"
        intro={t("app.setup.sections.updatesDesc")}
      >
        <Card testId="advanced-updates">
          {/*
            Bryteren FØR raden den styrer: «Oppdater automatisk» av betyr at
            ingenting under skjer av seg selv, og rekkefølgen skal lese slik.
            Den manuelle knappen i raden under er med vilje ugatet — PRIVACY.md
            har den som sitt ene unntak, fordi et trykk der er eieren som spør.
          */}
          <BoundToggle
            setting="autoUpdate"
            label={t("app.setup.advanced.autoUpdate")}
            description={t("app.setup.advanced.autoUpdateDesc")}
            testId="adv-auto-update"
          />
          <UpdateRow />
        </Card>
      </SettingsSection>

      <SettingsSection
        id="sec-privacy"
        sectionKey="privacy"
        intro={t("app.setup.sections.privacyDesc")}
      >
        <Card testId="advanced-privacy">
          <TelemetryRow />
        </Card>
      </SettingsSection>

      <SettingsSection
        id="sec-about"
        sectionKey="about"
        intro={t("app.setup.sections.aboutDesc")}
      >
        <Card testId="advanced-help">
          <LogRow />
        </Card>
        <AsioAttribution />
        <LicensesCard />
      </SettingsSection>
    </div>
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
  patchFor,
  testId,
}: {
  label: string;
  description: string;
  checked: boolean;
  /** Nøkkelen og tallet bryteren skriver — «på» velger en standard, «av» er 0. */
  patchFor: (on: boolean) => Partial<Settings>;
  testId: string;
}) {
  const row = usePatch();

  return (
    <SettingRow
      label={label}
      description={description}
      receipt={row.receipt}
      testId={testId}
    >
      {(ids: RowIds) => (
        <Toggle
          checked={checked}
          onChange={(next) => void row.write(patchFor(next))}
          disabled={row.busy}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId={`${testId}-control-input`}
        />
      )}
    </SettingRow>
  );
}

function SplitRows() {
  const minutes = settings.value.splitMinutes ?? 0;
  return (
    <>
      <NumericToggleRow
        label={t("app.setup.advanced.split")}
        description={t("app.setup.advanced.splitDesc")}
        checked={minutes > 0}
        patchFor={(on) => ({
          splitMinutes: on ? SPLIT_DEFAULT_MINUTES : 0,
        })}
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
        patchFor={(on) => ({
          autoDeleteDays: on ? AUTO_DELETE_DEFAULT_DAYS : 0,
        })}
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
