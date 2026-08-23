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

import { locale, t, tDyn, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { audioDevices } from "../../state/devices";
import { currentRoomMinutes, diskFreeBytes } from "../../state/disk";
import { emailTransport } from "../../state/email";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { DecisionCard } from "../../ui/DecisionCard/DecisionCard";
import { AutoRecordCard } from "./AutoRecordCard";
import { CameraCard } from "./CameraCard";
import {
  decisionsFor,
  needsSetUp,
  type Answer,
  type Decision,
  type DecisionId,
  type Detail,
} from "./decisions-core";
import styles from "./setup.module.css";
import { useVuWord } from "./use-vu-word";

/**
 * Spørsmålet hvert kort stiller.
 *
 * Fem literaler og ikke `tDyn('app.setup.q', n)`: nøklene heter `q1`…`q5` og
 * bor side om side med `lede`, `notSetUp` og resten, så et dynamisk oppslag
 * ville pekt på et subtre som er mye større enn de fem det gjelder — og da kan
 * `check-i18n-keys.mjs` ikke si noe om hvorvidt akkurat de fem finnes.
 */
function questionText(id: DecisionId): string {
  switch (id) {
    case "sound":
      return t("app.setup.q1");
    case "folder":
      return t("app.setup.q2");
    case "quality":
      return t("app.setup.q3");
    case "church":
      return t("app.setup.q4");
    case "notify":
      return t("app.setup.q5");
  }
}

/** Svaret som står nå, som setning. */
function answerText(answer: Answer): string {
  switch (answer.key) {
    case "notSetUp":
      return t("app.setup.notSetUp");
    case "device":
      return answer.pair
        ? tf("app.setup.sound.deviceWithPair", {
            name: answer.name,
            l: answer.pair.l,
            r: answer.pair.r,
          })
        : answer.name;
    case "deviceMissing":
      return tf("app.setup.sound.gone", { name: answer.name });
    case "path":
      return answer.path;
    case "quality":
      return tDyn("app.setup.quality", answer.format);
    case "qualityCustom":
      return tf("app.setup.qualityCustom", {
        format: answer.format,
        bitrate: answer.bitrate,
      });
    case "church":
      return answer.name;
    case "nobody":
      return t("app.setup.nobodyYet");
    case "email":
      return answer.address;
  }
}

/** Linja under svaret. `null` når det ikke er noe mer å si. */
function detailText(detail: Detail | null): string | null {
  if (!detail) return null;
  switch (detail.key) {
    case "heard":
      return tDyn("app.vu", detail.word);
    case "deviceGone":
      return t("app.setup.sound.goneDesc");
    case "noDevice":
      return t("app.setup.sound.noneDesc");
    case "space": {
      // Hele GB og hele timer. «412,37 GB» og «299,8 t» er presist om et tall
      // ingen tar en beslutning på tredje desimal av.
      const gb = Math.round(detail.freeBytes / 1e9);
      if (detail.roomMinutes === null)
        return tf("app.setup.folder.free", { gb });
      return tf("app.setup.folder.space", {
        gb,
        hours: Math.floor(detail.roomMinutes / 60),
      });
    }
    case "noFolder":
      return t("app.setup.folder.noneDesc");
    case "qualityDesc":
      return tDyn("app.setup.qDesc", detail.format);
    case "qualityCustomDesc":
      return t("app.setup.qualityCustomDesc");
    case "language":
      return tf("app.setup.church.language", {
        language: tDyn("app.language", detail.language),
      });
    case "nobodyDesc":
      return t("app.setup.notify.nobodyDesc");
    case "emailDesc":
      return t("app.setup.notify.emailDesc");
  }
}

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
