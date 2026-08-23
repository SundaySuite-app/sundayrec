/**
 * Nivå 1 — de fem spørsmålene, og de to tilleggene.
 *
 * Canvasens artboard 5.1. 65 kontroller i fem faner er foldet til fem
 * spørsmål en frivillig kan svare på uten å vite hva en samplingsrate er.
 *
 * ## Hva denne fila IKKE gjør
 *
 * Den avgjør ingenting. Om et spørsmål er besvart, hva svaret er og hvorfor
 * det ikke holder, kommer ferdig fra `decisions-core.ts` som DATA — denne fila
 * oversetter det til setninger og maler dem. Grunnen står i kjernen: reglene
 * er de samme påstandene som gjorde at dagens app skriver «Innebygd mikrofon ·
 * Tilkoblet ✓» om en innstilling som ikke er satt, og en regel som bor i en
 * JSX-linje leses aldri to ganger.
 *
 * ## Tilleggene folder seg ut, de har ingen «Endre»
 *
 * Canvasen tegner tilleggskortene med en «Endre»-knapp ved siden av bryteren.
 * Her utvider kortet seg i stedet når bryteren slås PÅ — som canvasens egen
 * innledning til sett 5 sier: «to tillegg som utvider siden når de slås på».
 * En «Endre» ved siden av rader som allerede står åpne ville vært en knapp
 * uten noe å gjøre, og en død knapp lærer en frivillig at knappene her ikke er
 * til å stole på.
 */

import { locale, t } from "../../i18n";
import { navigate } from "../../router/router";
import { audioDevices } from "../../state/devices";
import { currentRoomMinutes, diskFreeBytes } from "../../state/disk";
import { emailTransport } from "../../state/email";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { DecisionCard } from "../../ui/DecisionCard/DecisionCard";
import { AutoRecordCard } from "./AutoRecordCard";
import { CameraCard } from "./CameraCard";
import { answerText, detailText, questionText } from "./decision-text";
import { decisionsFor, needsSetUp, type Decision } from "./decisions-core";
import styles from "./setup.module.css";
import { useVuWord } from "./use-vu-word";

export function Level1() {
  const s = settings.value;
  const devices = audioDevices.value;

  // Måleren lytter bare når det FINNES en valgt enhet som faktisk er der, og
  // aldri mens det tas opp (se use-vu-word.ts).
  const chosen = (s.deviceId ?? "").trim();
  const found = !!chosen && !!devices?.some((d) => d.id === chosen);
  const vuWord = useVuWord(s.deviceName, found && !isRecording.value);

  const decisions = decisionsFor({
    settings: s,
    devices,
    diskFreeBytes: diskFreeBytes.value,
    roomMinutes: currentRoomMinutes(),
    emailTransport: emailTransport(),
    // Språket som FAKTISK rendres, ikke det som står lagret: fem språk er
    // pauset gjennom redesignet, så en profil med «de» leser engelsk — og
    // kortet skal si det som er sant på skjermen.
    locale: locale.value,
    vuWord,
  });

  return (
    <div class={styles.page}>
      <p data-testid="setup-lede" class={styles.lede}>
        {t("app.setup.lede")}
      </p>

      <div class={styles.list}>
        {decisions.map((decision, index) => (
          <Row key={decision.id} decision={decision} number={index + 1} />
        ))}
      </div>

      <div class={styles.sectionLabel}>{t("app.setup.addons")}</div>
      <div class={styles.addons}>
        <CameraCard />
        <AutoRecordCard />
      </div>

      {/*
        Canvasens «Avansert»-lenke nederst. P1a lot den være ute med vilje —
        skjermen fantes ikke, og en lenke til en tom side lærer en frivillig at
        lenkene her ikke er til å stole på. Nå finnes den.
      */}
      <div class={styles.footer}>
        <Button
          variant="ghost"
          testId="setup-advanced-link"
          onClick={() => navigate("setup", { tab: "advanced" })}
        >
          {t("app.setup.advanced.title")}
        </Button>
      </div>
    </div>
  );
}

function Row({ decision, number }: { decision: Decision; number: number }) {
  return (
    <DecisionCard
      number={number}
      status={decision.status}
      question={questionText(decision.id)}
      answer={answerText(decision.answer)}
      detail={detailText(decision.detail)}
      // «Sett opp» når det ikke STÅR et svar, «Endre» når det gjør det. Samme
      // knapp, to sanne etiketter — canvasens sett 5. (`needsSetUp` og ikke
      // `answered`: en mappe som er valgt, men der disken ikke har svart ennå,
      // er noe man endrer, ikke noe man setter opp.)
      actionLabel={
        needsSetUp(decision) ? t("app.setup.setUp") : t("app.setup.change")
      }
      onAction={() => navigate("setup", { tab: decision.id })}
      anchor={decision.id}
      testId={`setup-row-${decision.id}`}
    />
  );
}
