/**
 * Opptaksoverlegget — hele vinduet, mens gudstjenesten går (canvas 2.4–2.5).
 *
 * ## Hvorfor det bor i `#overlays`
 *
 * Søsken av `#app`, ved siden av `DialogHost` og `ToastHost`. To grunner, og
 * begge er praktiske: overlegget skal ligge over skinnen (som er inne i
 * `#app`), og det skal ikke rives ned av et rutebytte. Et opptak som går er
 * ikke en side man er på.
 *
 * `--z-recording` er UNDER `--z-modal`: stopp-bekreftelsen er en dialog, og
 * en bekreftelse bak flaten den handler om er en app der spørsmålet ikke går
 * an å svare på. Og fordi `DialogHost` bare setter `inert` på `#app`, setter
 * overlegget det på seg selv mens en dialog står — ellers ville stopp-knappen
 * fortsatt vært klikkbar bak spørsmålet om å stoppe.
 *
 * ## «Du kan lukke vinduet» ble FJERNET
 *
 * Canvasens `ov.hint` lover at opptaket fortsetter i bakgrunnen når vinduet
 * lukkes. Det gjør det ikke. `src-tauri/src/lib.rs` har ingen
 * `on_window_event`-håndterer som hindrer lukking, ingen `prevent_exit` og
 * ingen `ActivationPolicy::Accessory` — så når siste vindu lukkes fyrer
 * `RunEvent::ExitRequested`, og HÅNDTEREN DER STOPPER OPPTAKEREN
 * (`state::<RecorderEngine>().stop()`). Setningen ville altså vært en
 * oppfordring til å avslutte gudstjenestens opptak. Den står ikke her, og
 * nøkkelen er ikke lagt i katalogen.
 *
 * ## Måleren leser opptaket, ikke rommet
 *
 * `recordingLevelsSource` — motorens egen `recording://levels`. Se den fila
 * for hvorfor en andre enhetsåpning er utelukket.
 *
 * ## Kamerabildet er ikke det samme som kamera-BRIKKA
 *
 * Brikka «Kamera Logitech BRIO» er en påstand om INNSTILLINGEN, og den ser
 * nøyaktig lik ut med lokk på linsen (`docs/SMOKE-TEST.md` §4 sto lenge på at
 * et dødt kamera først ble oppdaget når fila ble åpnet). Bildet er den eneste
 * påstanden som ikke kan være usann. Det er en POLL mot motorens preview-JPEG
 * og ikke en strøm — webviewet kan ikke åpne kameraet mens opptakeren eier det
 * (`ui/CameraPreview/PolledCameraPreview.tsx`). Begge står: navnet svarer på
 * «hvilket kamera», bildet på «kommer det noe fra det».
 *
 * ## Ett ordforråd for nivå
 *
 * Canvasen skriver «Alt ser bra ut» / «Lyden er borte!» i akkurat den slissen
 * der 2.1 skriver «Vi hører lyd». Måleren har allerede ordet (`app.vu.*`,
 * `audio/level-words.ts`), og to setninger om det samme ved siden av hverandre
 * er verre enn én: de kan bli uenige, og de blir det den dagen tersklene
 * flyttes ett sted. Så måleren beholder sitt ord her også. Motorens EGET
 * stillhetsvarsel — som er noe annet enn «måleren ser lavt nivå», det er
 * detektoren som varsler før auto-stoppen — får sitt eget banner.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tf } from "../../i18n";
import { activeDialog } from "../../ui/dialog";
import { Banner } from "../../ui/Banner/Banner";
import { Button } from "../../ui/Button/Button";
import { Chip } from "../../ui/Chip/Chip";
import { VuMeter } from "../../ui/VuMeter/VuMeter";
import { PolledCameraPreview } from "../../ui/CameraPreview/PolledCameraPreview";
import { currentRoomMinutes } from "../../state/disk";
import {
  clearSilence,
  dismissReconnecting,
  finalizing,
  isRecording,
  reconnecting,
  scheduledStopMs,
  sessionStartedAtMs,
  silenceActive,
  silenceDetail,
} from "../../state/recording";
import { settings } from "../../state/settings";
import { cancelAutostop, extendAutostop } from "./autostop";
import { formatClock, spanOfMinutes } from "./record-core";
import { recordingLevelsSource } from "./recording-levels";
import { spanText } from "./span-text";
import { confirmAndStop } from "./stop";
import styles from "./overlay.module.css";

/** Skilletegnet mellom fakta på én linje. Et tegn, ikke prosa. */
const DOT = "·";

export function RecordingOverlay() {
  const live = isRecording.value;
  if (!live) return null;
  return <Overlay />;
}

/**
 * Selve flaten, som en egen komponent.
 *
 * Delt fra vakten over fordi hooks ikke kan stå etter en tidlig `return`, og
 * fordi det gir ÉN stabil montering per økt: canvas-elementet inne i `VuMeter`
 * byttes aldri ut mens et opptak går. En canvas som remonteres mister
 * konteksten sin, og tegneløkka maler videre i et element ingen ser — måleren
 * «fryser» over et opptak som er helt i orden.
 */
function Overlay() {
  const s = settings.value;
  const done = finalizing.value;
  const startedAt = sessionStartedAtMs.value;
  const [now, setNow] = useState(() => Date.now());

  // Klokken. Ett hakk i sekundet, og bare mens den faktisk teller: når motoren
  // skriver ferdig er økta over, og en klokke som fortsetter å telle ville
  // påstått noe annet (samme regel som legacy `enterFinalizing`).
  useEffect(() => {
    if (done) return;
    setNow(Date.now());
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [done]);

  const elapsed = startedAt === null ? 0 : Math.max(0, now - startedAt);
  const room = spanOfMinutes(currentRoomMinutes());
  const stopAt = scheduledStopMs.value;
  const device = (s.deviceName ?? "").trim();
  const camera = s.videoEnabled === true ? (s.videoDeviceName ?? "") : "";

  return (
    <div
      data-testid="recording-overlay"
      data-finalizing={done ? "true" : undefined}
      // Se toppen av fila: DialogHost slår bare av `#app`.
      inert={activeDialog.value !== null}
      class={styles.overlay}
    >
      <div class={styles.banners}>
        {reconnecting.value ? (
          <Banner
            tone="warn"
            testId="overlay-reconnect"
            title={
              device
                ? tf("app.overlay.reconnect", { name: device })
                : t("app.overlay.reconnectAnon")
            }
            // Se `dismissReconnecting` i state/recording.ts: stripa hadde ingen
            // vei ned uten et `recording://reconnected` som ikke alltid kommer.
            onDismiss={dismissReconnecting}
          />
        ) : null}
        {silenceActive.value ? (
          <Banner
            tone="warn"
            testId="overlay-silence"
            title={t("recording.silenceWarn")}
            detail={silenceDetail.value ?? undefined}
            onDismiss={clearSilence}
          />
        ) : null}
      </div>

      <Chip tone="rec" dot="rec" testId="overlay-chip">
        {t("app.status.rec")}
      </Chip>

      <div
        data-testid="overlay-timer"
        class={`${styles.timer} ${done ? styles.timerDone : ""}`}
      >
        {formatClock(elapsed)}
      </div>

      <div class={styles.meter}>
        <VuMeter
          testId="overlay-vu"
          source={recordingLevelsSource}
          // Tallene finnes her og ingen andre steder på nivå 1: den som sitter
          // ved maskinen under en gudstjeneste er nettopp den som vil vite
          // hvor mange dB det er igjen til klipping.
          showNumbers
        />
      </div>

      {/* Kamerabildet, når kamera er en del av dette opptaket. Brikka under
          navngir enheten; bare BILDET kan si at det faktisk kommer noe fra
          den. Se PolledCameraPreview.tsx. */}
      {camera ? (
        <div class={styles.camera}>
          <PolledCameraPreview />
        </div>
      ) : null}

      <div data-testid="overlay-facts" class={styles.facts}>
        {device ? <span data-testid="overlay-device">{device}</span> : null}
        {room.kind !== "none" ? (
          <>
            <span aria-hidden="true">{DOT}</span>
            <span data-testid="overlay-room">
              {`${t("app.overlay.room")} ${spanText(room)}`}
            </span>
          </>
        ) : null}
        {camera ? (
          <>
            <span aria-hidden="true">{DOT}</span>
            <span data-testid="overlay-camera">
              {`${t("app.overlay.camera")} ${camera}`}
            </span>
          </>
        ) : null}
      </div>

      <Button
        variant="secondary"
        size="lg"
        testId="overlay-stop"
        disabled={done}
        disabledReason={t("app.overlay.finalizingHint")}
        onClick={() => void confirmAndStop()}
      >
        {done ? t("recording.finalizing") : t("app.overlay.stop")}
      </Button>

      {done ? (
        <p data-testid="overlay-finalizing-hint" class={styles.hint}>
          {t("app.overlay.finalizingHint")}
        </p>
      ) : stopAt !== null && stopAt > now ? (
        <>
          <p data-testid="overlay-autostop" class={styles.hint}>
            {tf("app.overlay.autoStop", { left: formatClock(stopAt - now) })}
          </p>
          {/* ⚠️ Nedtellingen var en KUNNGJØRING og ikke en kontroll: fristen
              kunne vises og ikke flyttes (se `./autostop.ts`). Knappene står
              bare når det FINNES en frist — `manualMaxMinutes` er 0 som
              standard, og to knapper for noe som ikke skal skje er to knapper
              å lure på midt i en gudstjeneste. */}
          <div data-testid="overlay-autostop-actions" class={styles.autostop}>
            <Button
              variant="secondary"
              testId="overlay-autostop-extend"
              onClick={() => void extendAutostop()}
            >
              {t("recording.extend15")}
            </Button>
            <Button
              variant="ghost"
              testId="overlay-autostop-cancel"
              onClick={() => void cancelAutostop()}
            >
              {t("recording.cancelAutostop")}
            </Button>
          </div>
        </>
      ) : null}
    </div>
  );
}
