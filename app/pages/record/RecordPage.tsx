/**
 * TA OPP — «Én flate»: en stor knapp, kilden som en brikke, og kameraet når
 * det er på. Resten (lydvalg, mappe, kvalitet, opptaksvalg) bor i lydarket
 * (`SourceSheet`), og planen i ukestripa (`WeekStrip`).
 *
 * ## Hvilen
 *
 *   Uten kamera: kilde-brikka øverst, den store knappen med pustende glorie,
 *   «Ta opp», og den vannrette L/R-måleren. Kamerabryteren sitter ved siden av
 *   knappen.
 *   Med kamera: bildet tar flaten (`CameraStage`), den lille opptaksknappen
 *   ligger over bildet og måleren står loddrett i kanten.
 *
 * Opptaksoverlegget (`RecordingOverlay`) er E6 — denne fila eier bare hvilen.
 *
 * ## Regelen hele flaten hviler på
 *
 * Start er sperret til en lydkilde er valgt EKSPLISITT — også når valget er
 * maskinens egen mikrofon. Regelen bor i `record-core.ts` med en tabell rundt
 * seg, og knappen bærer grunnen sin (`aria-disabled` + `title`) i stedet for
 * bare å være grå.
 *
 * ## Måleren, og hvem som eier mikrofonen
 *
 * `StereoMeter` holder den delte bakenden-strømmen (`acquireVuFeed`) mens
 * siden står åpen. Den slippes to steder, og begge er viktige:
 *
 *   - `off` når ingen kilde er valgt: å måle «systemets standardinngang» der
 *     ville vært å åpne nøyaktig den mikrofonen vi finnes for å slutte å ta
 *     opp fra uten å spørre.
 *   - målerne fjernes helt før `start_recording` og mens et opptak går.
 *     ⚠️ `@lib/audio/vu-feed` har ingen egen opptaksvakt, og `app/`
 *     gjenskaper ikke `window.__isRecording` med vilje. Her er det derfor
 *     MONTERINGEN som er vakten: ingen måler i treet, ingen `start_vu`.
 *
 * ## Brikkene «Redigert» / «Eksportert» er ikke med
 *
 * `recordings_list` bærer ingen slik status (se `state/recordings.ts`). Et
 * merke som gjettes er verre enn ingen merke.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tDyn } from "../../i18n";
import {
  consumePendingAction,
  navigate,
  panel,
  pendingAction,
} from "../../router/router";
import { FirstRunResumeChip } from "../firstrun/FirstRunResumeChip";
import {
  audioDevices,
  loadAudioDevices,
  loadVideoDevices,
  videoDevices,
} from "../../state/devices";
import { refreshDiskSpace } from "../../state/disk";
import { prerollActive } from "../../state/preroll";
import { runSilentPreflightOnce } from "../../state/preflight";
import { isRecording, markSessionStarted } from "../../state/recording";
import { settings } from "../../state/settings";
import { Chip } from "../../ui/Chip/Chip";
import { toast } from "../../ui/toast";
import { BigButton } from "./BigButton";
import { CameraControls } from "./CameraControls";
import { CameraStage } from "./CameraStage";
import { nativeErrorSuffixFromText, sourceState } from "./record-core";
import { RecordBanners } from "./RecordBanners";
import { Consent, Done } from "./RecordDone";
import styles from "./RecordPage.module.css";
import { pickForSource } from "./source-pick";
import { SourceControl } from "./SourceChip";
import { SourceSheet } from "./SourceSheet";
import { confirmAndStop } from "./stop";
import { StereoMeter } from "./StereoMeter";
import { WeekStrip } from "./WeekStrip";

export function RecordPage() {
  const sheetOpen = panel.value === "source" || panel.value === "source-more";
  const s = settings.value;
  const devices = audioDevices.value;
  const source = sourceState(s, devices);
  const live = isRecording.value;
  const [starting, setStarting] = useState(false);

  // Enhetslisten og ledig plass leses når SIDEN åpnes, ikke ved oppstart:
  // fakta hentet ved boot er gamle når noen faktisk står foran mikseren. Begge
  // er inndata kilden og bannerne sier noe SANT med.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    // Én gang per oppstart, ikke per besøk — se `state/preflight.ts`.
    void runSilentPreflightOnce();
  }, []);

  // Kameralista leses bare når tillegget er på: enumereringen spør OS-et om
  // videoenheter, og det er ingen grunn til å gjøre det for menigheter som bare
  // tar opp lyd. Forhåndsvisningen leser lista for å si hvilket kamera den
  // ber bakenden om — og for å skille «ingen kameraer» fra «lesingen feilet».
  const cameraOn = s.videoEnabled === true;
  useEffect(() => {
    if (cameraOn && videoDevices.peek() === null) void loadVideoDevices();
  }, [cameraOn]);

  /**
   * Start.
   *
   * Gjennom shimmen: `startRecordingNow` → `start_recording`, ett kall. Bare
   * de tre feltene shimmen faktisk videresender settes; ALT annet (mappe,
   * filnavn, format, kanaler, kamera-navn) planlegger Rust selv fra den
   * lagrede profilen, som er den ene sannheten siden R4. Sida sender aldri en
   * sti — hvor opptaket havner, avgjøres i Rust (sikkerhetsfunn E1; før gikk
   * planen hit og tilbake via `plan_recording_opts`, med samme resultat).
   *
   * Ingen `#modal-manual`: kilde, kamera og filnavn er Oppsett-beslutninger
   * (eiervalg, canvas sett 2). Derfor heller ikke `customName` — filnavnet
   * følger mønsteret profilen allerede har.
   */
  async function handleStart(): Promise<void> {
    if (starting || live || !source.canStart) return;
    setStarting(true);
    try {
      // Kameraet trenger ingen forberedelse: bildet kommer fra bakendens
      // egen økt (`ui/CameraPreview/CameraPreview.tsx`), og opptaket tar opp
      // fra den samme — ingen overlevering, ingen ventetid, og bildet står
      // gjennom hele Start. Pinnet i `e2e/record.spec.ts` («bildet blinker ikke»).
      // Gi Preact rammen den trenger til å faktisk avmontere måleren, så
      // VU-strømmen er sluppet FØR opptaksmotoren ber om enheten. Den harde
      // garantien er Rust-sidens eget `vu.stop()` inne i `start_recording`;
      // dette er bare den ryddige veien dit, akkurat som legacy
      // `releaseRendererAudioCaptures()` er det.
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      const res = await window.api.startRecordingNow({
        maxMinutes: s.manualMaxMinutes || undefined,
        videoEnabled: s.videoEnabled === true,
      });
      if (res?.ok) {
        markSessionStarted();
        return;
      }
      // Den LOKALISERTE grunnen, aldri en rå kode eller «[object Object]» —
      // den historiske feilmodusen på nøyaktig denne stien.
      toast(
        "error",
        tDyn("recording", nativeErrorSuffixFromText(res?.error ?? null)),
      );
    } catch (err) {
      console.error("[record] start feilet:", err);
      toast(
        "error",
        tDyn(
          "recording",
          nativeErrorSuffixFromText(
            err instanceof Error ? err.message : String(err),
          ),
        ),
      );
    } finally {
      setStarting(false);
    }
  }

  // ── Menylinjen ────────────────────────────────────────────────────────────
  //
  // Et SIGNAL, ikke et syntetisk klikk på en knapp som må finnes på en side
  // som må være vist. Bare handlingene som hører hjemme HER plukkes opp; de
  // andre blir stående til flaten sin.
  const armed = pendingAction.value;
  useEffect(() => {
    if (armed === null) return;
    if (armed === "start-recording") {
      consumePendingAction();
      // Er kilden ikke valgt, gjør knappen ingenting — og kortet over den
      // sier hvorfor. Å starte på en kilde ingen har valgt fra menylinjen
      // ville vært den samme løgnen, bare et annet sted.
      if (source.canStart && !live) void handleStart();
      return;
    }
    if (armed === "stop-recording") {
      consumePendingAction();
      if (live) void confirmAndStop();
    }
  }, [armed, source.canStart, live]);

  const showMeter = !(live || starting);
  const sourceBlocked = source.kind === "no-source";
  const cameraStage = cameraOn;

  return (
    <div class={styles.page}>
      <div class={styles.notices}>
        <RecordBanners />
        {/* R6: synlig helt til «Åpne SundayRec» setter onboardingDone — se
            `FirstRunResumeChip.tsx`. */}
        <FirstRunResumeChip />
        <Consent />
        <Done />
      </div>

      <div class={styles.stage}>
        <div class={styles.listening}>
          <Listening />
        </div>

        {cameraStage ? (
          <>
            <div class={styles.top}>
              <SourceControl source={source} meter={showMeter} />
            </div>
            <CameraStage
              source={source}
              onStart={() => void handleStart()}
              busy={starting}
              meter={showMeter}
            />
          </>
        ) : (
          <div class={styles.center}>
            <SourceControl source={source} meter={showMeter} />

            <div class={styles.big}>
              <BigButton
                onStart={() => void handleStart()}
                busy={starting}
                canStart={source.canStart}
                reason={t("app.record.whyBlocked")}
              />
              <div class={styles.cam}>
                <CameraControls place="rest" />
              </div>
            </div>

            <div class={styles.bigLabel}>{t("app.record.bigLabel")}</div>
            {sourceBlocked ? (
              <p data-testid="record-why-blocked" class={styles.why}>
                {t("app.record.whyBlocked")}
              </p>
            ) : source.kind === "source-missing" ? (
              <p data-testid="record-can-start" class={styles.why}>
                {t("app.record.canStart")}
              </p>
            ) : (
              <p data-testid="record-start-hint" class={styles.why}>
                {t("app.record.startHint")}
              </p>
            )}

            {/*
              Måleren er MONTERINGEN som er vakten: ingen måler i treet, ingen
              `start_vu`. Se toppen av fila.
            */}
            {showMeter ? (
              <div class={styles.meter}>
                <StereoMeter
                  testId="record-vu"
                  deviceName={sourceBlocked ? null : source.name}
                  // Ingen kilde er valgt ⇒ ingen enhet åpnes.
                  off={sourceBlocked}
                  pick={() => pickForSource(settings.peek(), source)}
                />
              </div>
            ) : null}
          </div>
        )}

        <WeekStrip />

        <SourceSheet open={sheetOpen} onClose={() => navigate("record")} />
      </div>
    </div>
  );
}

// ── «Lytter» ────────────────────────────────────────────────────────────────

/**
 * Brikka som sier at forhåndsbufferen faktisk kjører.
 *
 * `prerollActive` er BAKENDENS svar, ikke innstillingen: `preroll_start` kan
 * svare `false` (ingen enhetstreff, eller motorens egen kopi sier av), og en
 * brikke som påsto noe annet ville vært den samme løgnen atlaset fant i §2.6.
 *
 * Og aldri en bryter her — eiervalget er «pre-roll på og usynlig». Sekundene
 * settes under Avansert; her er det bare et faktum.
 */
function Listening() {
  const active = prerollActive.value;
  if (!active) return null;
  return (
    <Chip tone="gold" dot="listen" testId="record-listening">
      {t("app.record.listening")}
    </Chip>
  );
}
