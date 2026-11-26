/**
 * Første gang — tre steg, ett spørsmål om gangen.
 *
 *   1. Lydkort og kanaler — `SoundPage` (enhet, kanalpar, live måler)
 *   2. Lagringsmappe      — `FolderPage` (mappevelgeren i Rust)
 *   3. Første faste opptak — `FirstRunAuto` + `WeekView` (variant `page`)
 *
 * Ingen egen veiviser-kode: stegene er de samme komponentene som INNSTILLINGER
 * og PLAN bruker. Språk følger systemet og velges under Innstillinger; kvalitet
 * og varsler har gode standardverdier og ligger der også.
 *
 * ## Porten på steg 1
 *
 * «Neste» er sperret til appen HØRER lyd. Det er den ene tingen som ikke kan
 * repareres etterpå: en gudstjeneste tatt opp fra feil inngang er borte. Og
 * porten har en nødutgang — «Fortsett uten lyd» — fordi en port uten utgang er
 * en app som ikke kan brukes på en maskin der mikseren ikke er slått på ennå.
 *
 * ⚠️ Sperret betyr `aria-disabled` + en GRUNN, ikke en grå knapp. Se `Button`.
 *
 * ## Steg 3
 *
 * Å legge til den første tiden slår på «Ta opp automatisk» og «Start automatisk
 * med maskinen» — et fast opptak som ikke skjer etter en omstart er ikke det
 * noen mente. «Senere» hopper over steget; tidene kan settes under PLAN.
 *
 * ## Chippen
 *
 * `firstRunIndex` er et modulsignal, så «Fortsett oppsettet» (se
 * `FirstRunResumeChip`) bare åpner ruten igjen og står på steget man forlot.
 */

import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";

import { t, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { audioDevices, loadAudioDevices } from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { isRecording } from "../../state/recording";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { ActionBar } from "../../ui/ActionBar/ActionBar";
import { Button } from "../../ui/Button/Button";
import { toast } from "../../ui/toast";
import { WeekView } from "../plan/WeekView";
import { FirstRunAuto } from "./FirstRunAuto";
import { FolderPage } from "../settings/FolderPage";
import { SoundPage } from "../settings/SoundPage";
import { useVuWord } from "../settings/use-vu-word";
import {
  dots,
  FIRST_RUN_STEP_COUNT,
  isGatedStep,
  soundGateOpen,
  stepAt,
  type FirstRunStepId,
} from "./firstrun-core";
import styles from "./firstrun.module.css";

/**
 * Hvor i sekvensen vi er.
 *
 * Et modulnivå-signal, fordi tre ting leser det: rammen under, `PageShell`s
 * overskrift (utenfor denne komponenten) og chippen.
 */
const firstRunIndex = signal(0);

/** Overskriften steget skal ha. `undefined` når sekvensen ikke er i gang. */
export function firstRunHeading(active: boolean): string | undefined {
  if (!active) return undefined;
  return stepTitle(stepAt(firstRunIndex.value).id);
}

function stepTitle(id: FirstRunStepId): string {
  switch (id) {
    case "sound":
      return t("app.first.title.sound");
    case "folder":
      return t("app.first.title.folder");
    case "plan":
      return t("app.first.title.plan");
  }
}

/** «Fortsett oppsettet»-chippens klikk: tilbake til steget man forlot. */
export function resumeFirstRun(): void {
  navigate("settings", { firstRun: true });
}

export function FirstRun() {
  const s = settings.value;
  const index = firstRunIndex.value;
  const { step, id } = stepAt(index);
  const last = step === FIRST_RUN_STEP_COUNT;
  const onIndex = (next: number): void => {
    firstRunIndex.value = stepAt(next).step - 1;
  };
  const [skippedSound, setSkippedSound] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const slotCount = (s.slots ?? []).length;

  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
  }, []);

  // Porten lytter bare på steg 1, og bare når en enhet FINNES å lytte på.
  const chosen = (s.deviceId ?? "").trim();
  const found = !!chosen && !!audioDevices.value?.some((d) => d.id === chosen);
  const vuWord = useVuWord(
    s.deviceName,
    isGatedStep(index) && found && !isRecording.value,
  );
  const gateOpen = !isGatedStep(index) || soundGateOpen(vuWord, skippedSound);

  // Første tid inn i uka → automatisk opptak og automatisk start slås på.
  const prevSlots = useRef(slotCount);
  useEffect(() => {
    const before = prevSlots.current;
    prevSlots.current = slotCount;
    if (id !== "plan" || before > 0 || slotCount === 0) return;
    patchSettings({ autoRecordEnabled: true, launchAtLogin: true });
    void saveSettingsDebounced(120);
  }, [slotCount, id]);

  async function finish(): Promise<void> {
    if (finishing) return;
    setFinishing(true);
    try {
      patchSettings({ onboardingDone: true });
      const ok = await saveSettingsDebounced(120);
      if (!ok) {
        // Rull tilbake OG bli stående: en «ferdig» som ikke ble lagret betyr at
        // sekvensen kommer tilbake ved neste oppstart, og da er det bedre å si
        // fra nå enn å la den dukke opp igjen uten forklaring.
        patchSettings({ onboardingDone: false });
        toast("error", t("general.saveFailed"));
        return;
      }
      firstRunIndex.value = 0;
      navigate("record");
    } finally {
      setFinishing(false);
    }
  }

  return (
    <div data-testid="first-run" class={styles.wrap}>
      <div class={styles.head}>
        <span data-testid="first-run-step" class={styles.stepLabel}>
          {tf("app.first.step", { n: step, total: FIRST_RUN_STEP_COUNT })}
        </span>
        <ol
          aria-label={t("app.first.progress")}
          data-testid="first-run-dots"
          class={styles.dots}
        >
          {dots(index).map((state, i) => (
            <li key={i} data-state={state} class={styles.dot} />
          ))}
        </ol>
      </div>

      {id === "sound" ? <SoundPage /> : null}
      {id === "folder" ? <FolderPage /> : null}
      {id === "plan" ? (
        <div class={styles.plan} data-testid="first-run-plan">
          <p class={styles.lede}>{t("app.first.planLede")}</p>
          <FirstRunAuto />
          <WeekView variant="page" />
        </div>
      ) : null}

      <ActionBar
        hint={
          isGatedStep(index) ? (
            <p data-testid="first-run-gate" class={styles.gate}>
              {t("app.first.gate")}
            </p>
          ) : null
        }
      >
        {step > 1 ? (
          <Button
            variant="ghost"
            testId="first-run-back"
            onClick={() => onIndex(index - 1)}
          >
            {t("app.first.back")}
          </Button>
        ) : null}

        {isGatedStep(index) ? (
          <Button
            variant="ghost"
            testId="first-run-skip-sound"
            onClick={() => {
              setSkippedSound(true);
              onIndex(index + 1);
            }}
          >
            {t("app.first.skipSound")}
          </Button>
        ) : null}

        {last ? (
          slotCount === 0 ? (
            <Button
              variant="primary"
              size="lg"
              busy={finishing}
              testId="first-run-later"
              onClick={() => void finish()}
            >
              {t("app.first.later")}
            </Button>
          ) : (
            <Button
              variant="primary"
              size="lg"
              busy={finishing}
              testId="first-run-open"
              onClick={() => void finish()}
            >
              {t("app.first.open")}
            </Button>
          )
        ) : (
          <Button
            variant="primary"
            size="lg"
            disabled={!gateOpen}
            disabledReason={t("app.first.gateReason")}
            testId="first-run-next"
            onClick={() => onIndex(index + 1)}
          >
            {t("app.first.next")}
          </Button>
        )}
      </ActionBar>
    </div>
  );
}
