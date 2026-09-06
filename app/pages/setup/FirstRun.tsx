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
 * tilstandene, som kortene i kontrollrommet. Det er derfor den kan være gul:
 * «Alt er klart!» over en app uten lagringsmappe er atlasets funn (§3e), og den
 * setningen finnes ikke her. Overskriften sier «Klar til søndag», og raden som
 * ikke er det står gul med en «Sett opp»-knapp.
 *
 * ## R6 → F2-T4: «Sett opp» forlater ikke sekvensen i det hele tatt
 *
 * R6 gjorde avgangen tilbakevendende: knappen husket hvor den gikk FRA, og en
 * chip på OPPTAK førte tilbake. F2-T4 fjerner avgangen. Raden folder ut den
 * samme skjermen PÅ STEDET — nøyaktig slik kontrollrommet på OPPTAK gjør det
 * (`RecordPage`s `ControlCard`-rader over `embedded`-signalet i `SubPage.tsx`)
 * — så en frivillig som retter mappen midt i sjekklisten blir stående i
 * sjekklisten, med de fire andre svarene synlige rundt seg.
 *
 * Alle FEM radene folder ut; ingen faller tilbake på en navigering. De fem
 * spørsmålene ER de fem skjermene sekvensen nettopp gikk gjennom, så det
 * finnes ingen rad uten en skjerm å vise. («Ta med kamera» og «Ta opp
 * automatisk» er ikke rader her — de er ikke ett av de fem spørsmålene, se
 * `firstrun-core.ts`.)
 *
 * ## Chippen står — det finnes fortsatt veier ut
 *
 * `firstRunReturn` + `FirstRunResumeChip` (rendret på OPPTAK og INNSTILLINGER,
 * se den fila) beholdes, og de er ikke arbeidsledige: bunnlinja står under
 * første gang også, og et utfoldet kort kan ha sin egen lenke ut («Avansert
 * lyd» i `SoundPage` går til Innstillinger). Begge kan skje fra et SPØRSMÅL og
 * ikke bare fra sjekklisten, så signalet speiler posisjonen fortløpende i
 * stedet for å bli skrevet av én knapp — se `useRememberPosition` under.
 *
 * ## VU-regelen, arvet fra kontrollrommet
 *
 * Sjekklisten har fortsatt INGEN egen måler (`vuWord: null` under): den er et
 * sammendrag, ikke en test. Måleren finnes bare inne i det utfoldede lyd-
 * kortet (`sound-vu`), og `acquireVuFeed` er refcountet, så et kort som åpnes
 * mens sekvensens eget steg 1 lytter ville uansett vært ÉN økt på enheten.
 *
 * ⚠️ Og lyd-raden KOLLAPSER når et opptak starter, akkurat som kilde-kortet på
 * OPPTAK: monteringen er vakten som holder appen fra å be om enheten opptaket
 * nettopp tok (`@lib/audio/vu-feed`s `window.__isRecording`-sjekk er inert).
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
  firstRunResumeIndex,
  isGatedStep,
  screenAt,
  soundGateOpen,
  withRow,
} from "./firstrun-core";
import { FolderPage } from "./FolderPage";
import { NotifyPage } from "./NotifyPage";
import { QualityPage } from "./QualityPage";
import { SoundPage } from "./SoundPage";
import { useEmbedded } from "./SubPage";
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

/**
 * R6: hvor sekvensen sist STO. `null` til den har vært åpen denne økten.
 *
 * ⚠️ F2-T4 byttet skriveren. R6 skrev den fra sjekklistens `onAction`, rett før
 * den navigerte bort — den ene veien ut som fantes. Nå folder radene ut på
 * stedet, så den knappen navigerer ikke; det som fortsatt kan forlate
 * sekvensen er bunnlinja (som står under første gang også) og en lenke inne i
 * et utfoldet kort. Ingen av dem går gjennom kode denne fila eier, og begge
 * kan skje fra et SPØRSMÅL — så posisjonen speiles fortløpende
 * (`useRememberPosition`) i stedet for å bli skrevet av én knapp som ikke
 * lenger finnes.
 *
 * `resumeFirstRun` leser den; `finish()` lar den stå, fordi den blir
 * uinteressant i samme kall som setter `onboardingDone`, og chippen som leser
 * den forsvinner med resten av sekvensen (`showFirstRunResumeChip`,
 * `firstrun-core.ts`).
 */
export const firstRunReturn = signal<number | null>(null);

/** Overskriften den gjeldende posisjonen skal ha. `undefined` når sekvensen
 *  ikke er i gang — da er det destinasjonens eget navn som gjelder. */
export function firstRunHeading(active: boolean): string | undefined {
  if (!active) return undefined;
  const screen = screenAt(firstRunIndex.value);
  return screen.kind === "ready"
    ? t("app.first.readyTitle")
    : questionText(screen.tab);
}

/**
 * «Fortsett oppsettet»-chippens klikk: tilbake til stedet man forlot fra.
 *
 * Nøyaktig posisjonen — spørsmål 3 hvis det var der bunnlinja tok en frivillig
 * ut, sjekklisten hvis det var der. Reserven når ingenting er husket er
 * sjekklisten (`firstRunResumeIndex`).
 *
 * Tømmingen står fordi den er ærlig: signalet betyr «sist sett», og i det
 * øyeblikket sekvensen er åpen igjen er det `useRememberPosition` som eier
 * svaret, ikke minnet fra forrige gang.
 */
export function resumeFirstRun(): void {
  firstRunIndex.value = firstRunResumeIndex(firstRunReturn.value);
  firstRunReturn.value = null;
  navigate("setup", { firstRun: true });
}

/**
 * Speil posisjonen inn i `firstRunReturn` mens sekvensen står åpen.
 *
 * Én effekt, ingen betingelser: hooken kjører bare mens `FirstRun` er montert,
 * og `FirstRun` er montert bare mens `route.firstRun` er sann (`Shell.tsx`).
 * Det som forlater sekvensen — bunnlinja, en lenke inne i et utfoldet kort —
 * river komponenten ned, og da står den siste posisjonen igjen i signalet.
 */
function useRememberPosition(index: number): void {
  useEffect(() => {
    firstRunReturn.value = index;
  }, [index]);
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
  useRememberPosition(index);

  // De samme fakta kontrollrommet leser. Sjekklisten er de samme reglene, så
  // den trenger de samme inndataene.
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

      {screen.kind === "ready" ? (
        <Checklist />
      ) : (
        <DecisionScreen id={screen.tab} />
      )}

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

/**
 * Skjermen som eier ett av de fem spørsmålene.
 *
 * ÉN tabell, to kallsteder: sekvensens eget steg, og kroppen i en utfoldet
 * sjekklistrad. To `switch`-er over de samme fem id-ene ville vært to steder å
 * glemme et spørsmål — og den ene som glemte det ville rendret ingenting, uten
 * å feile.
 */
function DecisionScreen({ id }: { id: DecisionId }) {
  switch (id) {
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
 * «Klar til søndag» — de fem spørsmålene med svaret som står nå, og hver av dem
 * med hele skjermen sin ett klikk unna, PÅ STEDET.
 *
 * Identisk regelverk med kontrollrommet, med vilje: to lister som svarte hver
 * for seg ville før eller siden vært uenige, og den uenigheten ville stått side
 * om side med seg selv på to skjermer en frivillig ser rett etter hverandre.
 *
 * ## Utfoldingen, og hvilke regler som er lånt
 *
 *   - **Flere kan stå åpne.** Samme som `useControlCards` på OPPTAK; regelen
 *     er `withRow` i `firstrun-core.ts`.
 *   - **Kortet blir stående til brukeren lukker det.** Ikke «kollapser når
 *     raden blir grønn»: lagringen har en KVITTERING inne i kortet
 *     («Lagret ✓»), og en skjerm som rev seg selv bort i det øyeblikket
 *     kvitteringen kom ville tatt bort det ene beviset på at det virket. Det
 *     er den samme avgjørelsen `SoundPage` er skrevet rundt («INGEN navigering
 *     her lenger (D2)»), og raden over kortet oppdaterer seg likevel med én
 *     gang — den leser `settings`-signalet.
 *   - **Unntaket er lyd, under et opptak.** Se filhodet: monteringen er VU-
 *     vakten, så raden kollapser når `isRecording` blir sann, akkurat som
 *     kilde-kortet i kontrollrommet gjør.
 *
 * `useEmbedded()` står HER og ikke i `FirstRun`: spørsmålsskjermene skal
 * beholde leden sin (den er hele forklaringen når skjermen står alene), og
 * sjekklistas rad har allerede sagt hva kortet er for. Hooken er symmetrisk
 * ved konstruksjon (`SubPage.tsx`), og `Checklist` monteres og avmonteres med
 * den ene skjermen den gjelder for.
 */
function Checklist() {
  const s = settings.value;
  const live = isRecording.value;
  const [open, setOpen] = useState<readonly DecisionId[]>([]);
  useEmbedded();

  // VU-regelen, som en effekt og ikke som en `&&` i JSX: kortet skal FAKTISK
  // ut av treet, ikke bare skjules, og tilstanden må huske at det ble lukket
  // (ellers spretter det opp igjen i det opptaket stopper — med en måler som
  // ber om enheten på nytt uten at noen ba om det).
  useEffect(() => {
    if (!live) return;
    setOpen((prev) => withRow(prev, "sound", false));
  }, [live]);

  const decisions = decisionsFor({
    settings: s,
    devices: audioDevices.value,
    diskFreeBytes: diskFreeBytes.value,
    roomMinutes: currentRoomMinutes(),
    emailTransport: emailTransport(),
    locale: locale.value,
    // Fortsatt ingen måler på selve sjekklisten: den er et sammendrag, ikke en
    // test. Hørselstesten står inne i lyd-kortet, der den alltid har stått.
    vuWord: null,
  });

  return (
    <div class={setup.list}>
      <p data-testid="first-run-fix-here" class={styles.fixHere}>
        {t("app.first.fixHere")}
      </p>
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
          // Samme ord som i kontrollrommet, og med vilje: «Lukk» er den samme
          // tilstanden på begge skjermene, og to nøkler for det ene ordet er
          // hvordan de to begynner å si forskjellige ting.
          collapseLabel={t("app.record.close")}
          expanded={open.includes(decision.id)}
          onExpand={() =>
            setOpen((prev) =>
              withRow(prev, decision.id, !prev.includes(decision.id)),
            )
          }
          anchor={decision.id}
          testId={`first-run-row-${decision.id}`}
        >
          <DecisionScreen id={decision.id} />
        </DecisionCard>
      ))}
    </div>
  );
}
