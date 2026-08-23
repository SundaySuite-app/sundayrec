/**
 * Første gang — canvasens sett 6.
 *
 * Ikke en veiviser. De samme fem skjermene som ligger bak «Endre» på nivå 1,
 * vist ett spørsmål om gangen, med en linjal på toppen og én foot med
 * navigasjonen. Legacys veiviser bygger sine egne enhetslister, sin egen
 * VU-måler og sitt eget slot-skjema — 521 linjer som speiler skjermer som
 * allerede finnes, og som har kommet i utakt med dem: den spør aldri om
 * lagringsmappe, og sier likevel «Alt er klart!» til en app som ikke kan ta opp.
 *
 * ## Porten på steg 1
 *
 * «Neste» er sperret til appen HØRER lyd. Det er den ene tingen som ikke kan
 * repareres etterpå: en gudstjeneste tatt opp fra feil inngang er borte. Og
 * porten har en nødutgang — «Fortsett uten lyd», i grått — fordi en port uten
 * utgang er en app som ikke kan brukes på en maskin der mikseren ikke er slått
 * på ennå.
 *
 * ⚠️ Sperret betyr `aria-disabled` + en GRUNN, ikke en grå knapp. Se `Button`.
 *
 * ## Den siste skjermen påstår ingenting
 *
 * Sjekklisten er `decisions-core.ts` — de samme fem radene, med de samme tre
 * tilstandene, som nivå 1. Det er derfor den kan være gul: «Alt er klart!» over
 * en app uten lagringsmappe er atlasets funn (§3e), og den setningen finnes
 * ikke her. Overskriften sier «Klar til søndag», og raden som ikke er det står
 * gul med en «Sett opp»-knapp som følger med inn i Oppsett.
 */

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";

import { locale, t, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { audioDevices, loadAudioDevices } from "../../state/devices";
import {
  currentRoomMinutes,
  diskFreeBytes,
  refreshDiskSpace,
} from "../../state/disk";
import { emailTransport, refreshEmailFacts } from "../../state/email";
import { isRecording } from "../../state/recording";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { DecisionCard } from "../../ui/DecisionCard/DecisionCard";
import { toast } from "../../ui/toast";
import { ChurchPage } from "./ChurchPage";
import { answerText, detailText, questionText } from "./decision-text";
import { decisionsFor, needsSetUp, type DecisionId } from "./decisions-core";
import {
  dots,
  FIRST_RUN_STEP_COUNT,
  isGatedStep,
  screenAt,
  soundGateOpen,
} from "./firstrun-core";
import { FolderPage } from "./FolderPage";
import { NotifyPage } from "./NotifyPage";
import { QualityPage } from "./QualityPage";
import { SoundPage } from "./SoundPage";
import { inSequence } from "./SubPage";
import styles from "./firstrun.module.css";
import setup from "./setup.module.css";
import { useVuWord } from "./use-vu-word";

/**
 * Hvor i sekvensen vi er.
 *
 * Et modulnivå-signal, fordi TO ting leser det: rammen under, og `PageShell`s
 * overskrift — som ligger utenfor denne komponenten. Å løfte en `useState` opp
 * i `Shell` for én av dem ville betydd at hele skallet rendres på nytt hver
 * gang noen trykker «Neste».
 */
export const firstRunIndex = signal(0);

/** Overskriften den gjeldende posisjonen skal ha. `undefined` når sekvensen
 *  ikke er i gang — da er det destinasjonens eget navn som gjelder. */
export function firstRunHeading(active: boolean): string | undefined {
  if (!active) return undefined;
  const screen = screenAt(firstRunIndex.value);
  return screen.kind === "ready"
    ? t("app.first.readyTitle")
    : questionText(screen.tab);
}

export function FirstRun() {
  const s = settings.value;
  const index = firstRunIndex.value;
  const onIndex = (next: number): void => {
    firstRunIndex.value = Math.max(0, next);
  };
  const [skippedSound, setSkippedSound] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const screen = screenAt(index);

  // Sekvensen eier navigasjonen: undersidene skal ikke ha sin egen «Tilbake».
  useEffect(() => {
    inSequence.value = true;
    return () => {
      inSequence.value = false;
    };
  }, []);

  // De samme fakta nivå 1 leser. Sjekklisten er nivå 1 sine regler, så den
  // trenger nivå 1 sine inndata.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    void refreshEmailFacts();
  }, []);

  // Porten lytter bare på steg 1, og bare når en enhet FINNES å lytte på.
  const chosen = (s.deviceId ?? "").trim();
  const found = !!chosen && !!audioDevices.value?.some((d) => d.id === chosen);
  const vuWord = useVuWord(
    s.deviceName,
    screen.kind === "question" &&
      isGatedStep(index) &&
      found &&
      !isRecording.value,
  );

  const gateOpen = !isGatedStep(index) || soundGateOpen(vuWord, skippedSound);

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
      navigate("record");
    } finally {
      setFinishing(false);
    }
  }

  return (
    <div data-testid="first-run" class={styles.wrap}>
      <div class={styles.head}>
        <span data-testid="first-run-step" class={styles.stepLabel}>
          {screen.kind === "ready"
            ? t("app.first.readyDesc")
            : tf("app.first.step", {
                n: screen.step,
                total: FIRST_RUN_STEP_COUNT,
              })}
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

      {screen.kind === "ready" ? <Checklist /> : <Question tab={screen.tab} />}

      <div class={styles.foot}>
        {index > 0 && screen.kind === "question" ? (
          <Button
            variant="ghost"
            testId="first-run-back"
            onClick={() => onIndex(index - 1)}
          >
            {t("app.first.back")}
          </Button>
        ) : null}

        {screen.kind === "question" && isGatedStep(index) ? (
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

        {screen.kind === "ready" ? (
          <Button
            variant="primary"
            size="lg"
            busy={finishing}
            testId="first-run-open"
            onClick={() => void finish()}
          >
            {t("app.first.open")}
          </Button>
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
      </div>

      {screen.kind === "question" && isGatedStep(index) ? (
        <p data-testid="first-run-gate" class={styles.gate}>
          {t("app.first.gate")}
        </p>
      ) : null}
    </div>
  );
}

/** Den ene av de fem skjermene som hører til dette steget. */
function Question({ tab }: { tab: DecisionId }) {
  switch (tab) {
    case "sound":
      return <SoundPage />;
    case "folder":
      return <FolderPage />;
    case "quality":
      return <QualityPage />;
    case "church":
      return <ChurchPage />;
    case "notify":
      return <NotifyPage />;
  }
}

/**
 * «Klar til søndag» — de fem spørsmålene med svaret som står nå.
 *
 * Identisk regelverk med nivå 1, med vilje: to lister som svarte hver for seg
 * ville før eller siden vært uenige, og den uenigheten ville stått side om side
 * med seg selv på to skjermer en frivillig ser rett etter hverandre.
 */
function Checklist() {
  const s = settings.value;
  const decisions = decisionsFor({
    settings: s,
    devices: audioDevices.value,
    diskFreeBytes: diskFreeBytes.value,
    roomMinutes: currentRoomMinutes(),
    emailTransport: emailTransport(),
    locale: locale.value,
    // Ingen måler på sjekklisten: den er et sammendrag, ikke en test.
    vuWord: null,
  });

  return (
    <div class={setup.list}>
      {decisions.map((decision, index) => (
        <DecisionCard
          key={decision.id}
          number={index + 1}
          status={decision.status}
          question={questionText(decision.id)}
          answer={answerText(decision.answer)}
          detail={
            // Canvasens ene ekstra setning: den gule raden sier hva den KOSTER,
            // ikke bare at den mangler.
            decision.id === "notify" && decision.status === "todo"
              ? t("app.first.notifyTodo")
              : detailText(decision.detail)
          }
          actionLabel={
            needsSetUp(decision) ? t("app.setup.setUp") : t("app.setup.change")
          }
          onAction={() => navigate("setup", { tab: decision.id })}
          anchor={decision.id}
          testId={`first-run-row-${decision.id}`}
        />
      ))}
    </div>
  );
}
