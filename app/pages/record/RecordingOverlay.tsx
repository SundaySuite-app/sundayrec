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
 * `--z-recording` er UNDER `--z-modal`: en dialog (en feil midt i opptaket)
 * skal ligge OVER flaten den handler om. Og fordi `DialogHost` bare setter
 * `inert` på `#app`, setter overlegget det på seg selv mens en dialog står —
 * ellers ville «Hold» fortsatt vært klikkbar bak dialogen.
 *
 * ## F1-UX1: «Du kan lukke vinduet» er sant igjen
 *
 * Canvasens `ov.hint` lover at opptaket fortsetter i bakgrunnen når vinduet
 * lukkes. Det stemte ikke da denne kommentaren først ble skrevet — på den
 * tiden fantes ingen `on_window_event`-håndterer, og siste vindu som lukket
 * seg fyrte `RunEvent::ExitRequested` rett i `RecorderEngine::stop()`. P3
 * («Frivilligen først») rettet nettopp det: `lib.rs:251` kobler
 * `.on_window_event(window::on_event)` på builderen, og
 * `src-tauri/src/window.rs:80-116` fanger `CloseRequested` under en levende
 * ELLER fullførende økt og kaller `window.hide()` + `api.prevent_close()` i
 * stedet for å la lukkingen gå gjennom — capture og sidecars står urørt, og
 * appen blir i menylinja/Dock. Veien tilbake: «Åpne SundayRec» i menylinja,
 * Dock-ikonet, eller en ny oppstart (single-instance løfter fram vinduet).
 * Utenfor en økt er ingenting endret — lukk betyr avslutt, som før.
 *
 * Setningen står derfor her igjen (`app.overlay.closeHint`), i JSX-en under.
 *
 * ## «Én flate» (E6): bare opptaket
 *
 * Alt annet er borte. Øverst bare den blinkende røde prikken og «Tar opp»
 * (overlegget dekker toppfeltet, så det står her og ikke bak). Uten kamera: en
 * enorm klokke og en bred L/R-måler. Med kamera: bildet i en ramme med rød
 * kant, klokka som brikke oppe til venstre og måleren loddrett i kanten.
 * Nederst «Hold» (`HoldStop`, ett sekund; erstatter bekreftelsesdialogen) og
 * stopplinja: «Stopper 12:30» med «+30» og «∞», eller «Til du stopper».
 *
 * Advarslene (stillhet, gjenkobling, bilder hoppet over, kamera borte) er
 * små ikon + ord-brikker øverst. Ordet er det øyet ser; hele setningen står i
 * tittelen og for skjermleser, så ingenting av det gamle bannerets innhold er
 * borte — bare plassen det tok.
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
 * påstanden som ikke kan være usann. Det er bakendens egen kameraøkt — den
 * samme som opptaket tar opp fra, og som bildet på Opptak allerede kom fra, så
 * det står uavbrutt gjennom Start (`ui/CameraPreview/CameraPreview.tsx`).
 * Begge står: navnet svarer på
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

import { useEffect, useRef, useState } from "preact/hooks";

import { intlParts } from "@lib/status/next-recording-core";

import { locale, t, tf } from "../../i18n";
import { lockApp } from "../../ui/app-inert";
import { activeDialog } from "../../ui/dialog";
import { CameraPreview } from "../../ui/CameraPreview/CameraPreview";
import { Icon, type IconName } from "../../ui/Icon/Icon";
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
  sessionCameraLost,
  sessionVideoProblem,
  sessionVideoDropped,
  silenceDetail,
} from "../../state/recording";
import { settings } from "../../state/settings";
import { cancelAutostop, extendAutostop } from "./autostop";
import { HoldStop } from "./HoldStop";
import { formatClock, spanOfMinutes } from "./record-core";
import { spanText } from "./span-text";
import {
  beginStop,
  completeStop,
  confirmAndStop,
  type StopTicket,
} from "./stop";
import { StereoMeter } from "./StereoMeter";
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
 * fordi det gir ÉN stabil montering per økt: canvas-elementet inne i
 * `CameraPreview` og måleren byttes aldri ut mens et opptak går.
 */
function Overlay() {
  const s = settings.value;
  const done = finalizing.value;
  const startedAt = sessionStartedAtMs.value;
  const [now, setNow] = useState(() => Date.now());
  const root = useRef<HTMLDivElement | null>(null);
  // Øktas identitet slik den var da holdet BEGYNTE (`HoldStop.onBegin`) — ikke
  // da det var ferdig: sjekken i `completeStop` er bare noe verdt mot en
  // billett fra starten av sekundet.
  const ticket = useRef<StopTicket | null>(null);

  // Alt utenfor overlegget er `inert` så lenge opptaket går: Tab skal gå
  // mellom Hold, +30 og ∞ og ingenting bak (skallet, et ark som sto åpent da
  // en planlagt start kom). Telleren (`app-inert.ts`) hindrer at en dialog som
  // lukkes åpner skallet igjen. Tab fanges også her, så den ikke forlater
  // vinduet.
  useEffect(() => {
    const unlock = lockApp();
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== "Tab" || !root.current) return;
      if (activeDialog.peek() !== null) return; // dialogen eier tasten
      const items = [
        ...root.current.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
        ),
      ].filter((el) => el.offsetParent !== null);
      if (items.length === 0) return;
      const first = items[0]!;
      const last = items[items.length - 1]!;
      const active = document.activeElement;
      const inside = active !== null && root.current.contains(active);
      if (e.shiftKey && (!inside || active === first)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (!inside || active === last)) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      unlock();
    };
  }, []);

  // Klokken. Ett hakk i sekundet, og bare mens den faktisk teller: når motoren
  // skriver ferdig er økta over, og en klokke som fortsetter å telle ville
  // påstått noe annet (samme regel som legacy `enterFinalizing`).
  useEffect(() => {
    if (done) return;
    setNow(Date.now());
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [done]);

  /**
   * F2-T1: rehydrer auto-stopp-fristen når denne monteringen ikke fikk den
   * med seg.
   *
   * Overlegget mountes bare når `isRecording` blir sann (vakten i
   * `RecordingOverlay` over), og det skjer på to måter: ett `markSessionStarted()`
   * rett etter `res.ok` fra en manuell start (før motorens FØRSTE
   * `recording://state`-overgang har rukket å komme), og ett event uten last
   * (`recording-overlay-start`, kartlagt fra `recording://started` — se
   * `state/recording.ts`), som planleggeren eller en gjenoppretting fyrer uten
   * en frist i nyttelasten. Begge veiene lar `scheduledStopMs` stå i det den
   * var (typisk `null`, etter forrige økts `endSessionLocally()`) helt til
   * NESTE tilstandsovergang — som først kommer når auto-stoppen selv slår
   * til, potensielt en time unna.
   *
   * Så: mangler tallet ved monteringen, spør motoren direkte. Er det allerede
   * satt (fra en tilstandsovergang som kom FØRST, eller fra en tidligere
   * runde av denne samme effekten), er dette en no-op — ingen ekstra IPC-tur
   * for den vanlige veien.
   */
  useEffect(() => {
    if (scheduledStopMs.value !== null) return;
    let cancelled = false;
    void window.api.recordingScheduledStopMs().then((ms) => {
      if (cancelled) return;
      // En ekte `recording://state`-overgang kan ha landet MENS dette kallet
      // var i flukt — den er den autoritative kilden, og den vinner alltid
      // over en rehydrering som startet før den kom.
      if (scheduledStopMs.peek() === null) scheduledStopMs.value = ms;
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const elapsed = startedAt === null ? 0 : Math.max(0, now - startedAt);
  const room = spanOfMinutes(currentRoomMinutes());
  const stopAt = scheduledStopMs.value;
  const device = (s.deviceName ?? "").trim();
  const withCamera = s.videoEnabled === true;
  const cameraName = withCamera ? (s.videoDeviceName ?? "").trim() : "";
  const clock = formatClock(elapsed);

  return (
    <div
      ref={root}
      data-testid="recording-overlay"
      data-finalizing={done ? "true" : undefined}
      data-camera={withCamera ? "true" : undefined}
      // Se toppen av fila: DialogHost slår bare av `#app`.
      inert={activeDialog.value !== null}
      class={styles.overlay}
    >
      <header class={styles.top}>
        <span data-testid="overlay-chip" class={styles.rec}>
          <span aria-hidden="true" class={styles.recDot} />
          {t("app.status.rec")}
        </span>
      </header>

      <Warnings camera={withCamera} device={device} />

      <main class={styles.stage}>
        {withCamera ? (
          <div data-testid="overlay-panel" class={styles.panel}>
            <div class={styles.picture}>
              <CameraPreview testId="overlay-camera-preview" knownCamera fill />
            </div>
            <span class={styles.clockChip}>
              <span aria-hidden="true" class={styles.recDot} />
              <span
                data-testid="overlay-timer"
                class={done ? styles.timerDone : undefined}
              >
                {clock}
              </span>
            </span>
            {done ? null : (
              <span class={styles.panelMeter}>
                <StereoMeter
                  variant="vertical"
                  live="recording"
                  testId="overlay-vu"
                />
              </span>
            )}
          </div>
        ) : (
          <>
            <div
              data-testid="overlay-timer"
              class={`${styles.clock} ${done ? styles.timerDone : ""}`}
            >
              {clock}
            </div>
            <div class={styles.meter}>
              {done ? null : (
                <StereoMeter live="recording" testId="overlay-vu" />
              )}
            </div>
          </>
        )}

        <div data-testid="overlay-facts" class={styles.facts}>
          {device ? <span data-testid="overlay-device">{device}</span> : null}
          {room.kind !== "none" ? (
            <>
              {device ? <span aria-hidden="true">{DOT}</span> : null}
              <span data-testid="overlay-room">
                {tf("app.overlay.room", { span: spanText(room) })}
              </span>
            </>
          ) : null}
          {cameraName ? (
            <>
              <span aria-hidden="true">{DOT}</span>
              <span data-testid="overlay-camera">
                {tf("app.overlay.camera", { name: cameraName })}
              </span>
            </>
          ) : null}
        </div>
      </main>

      <footer class={styles.bottom}>
        <HoldStop
          disabled={done}
          onBegin={() => {
            // Billetten lages når holdet BEGYNNER; `null` (motoren skriver
            // allerede ferdig) avviser holdet i stedet for å la ringen fylle
            // seg mot et stopp som ikke kommer.
            ticket.current = beginStop();
            return ticket.current !== null;
          }}
          onStop={() => {
            const held = ticket.current;
            ticket.current = null;
            void completeStop(held);
          }}
          onAssistiveClick={() => void confirmAndStop()}
        />
        <span data-testid="overlay-hold-label" class={styles.holdLabel}>
          {done ? t("recording.finalizing") : t("app.rec.hold")}
        </span>

        {done ? (
          <p data-testid="overlay-finalizing-hint" class={styles.hint}>
            {t("app.overlay.finalizingHint")}
          </p>
        ) : (
          <StopLine stopAt={stopAt} now={now} />
        )}

        {/*
          F1-UX1/W1: sant i BÅDE «Recording» og «Stopping» (`window.rs:80-116`
          skjuler vinduet i begge, se filhodet) — derfor uavhengig av `done`.
        */}
        <p data-testid="overlay-close-hint" class={styles.hint}>
          {t("app.overlay.closeHint")}
        </p>
      </footer>
    </div>
  );
}

// ── Stopplinja ──────────────────────────────────────────────────────────────

/**
 * «Stopper 12:30» med «+30» og «∞», eller «Til du stopper».
 *
 * Fristen er MOTORENS (`scheduledStopMs`, via `recording://state`), aldri et
 * lokalt anslag: «+30» ber motoren flytte den og tiden her flytter seg først
 * når motoren sier det (se `./autostop.ts`). Knappene står bare når det
 * FINNES en frist — `manualMaxMinutes` er 0 som standard, og to knapper for noe
 * som ikke skal skje er to knapper å lure på midt i en gudstjeneste. Et
 * manuelt opptak med tak har en frist som alle andre og får samme linje.
 */
function StopLine({ stopAt, now }: { stopAt: number | null; now: number }) {
  const pending = stopAt !== null && stopAt > now;
  const time = pending ? intlParts(locale.value)(stopAt).time : "";
  return (
    <div data-testid="overlay-stop-line" class={styles.stopLine}>
      <Icon name="timer" size={18} />
      {pending ? (
        <>
          <span
            data-testid="overlay-autostop"
            data-stop-ms={String(stopAt)}
            class={styles.stopText}
          >
            {tf("app.rec.stopsAt", { time })}
          </span>
          <span data-testid="overlay-autostop-actions" class={styles.actions}>
            <button
              type="button"
              data-testid="overlay-autostop-extend"
              aria-label={t("app.rec.extendAria")}
              title={t("app.rec.extendTitle")}
              class={styles.extend}
              onClick={() => void extendAutostop()}
            >
              +30
            </button>
            <button
              type="button"
              data-testid="overlay-autostop-cancel"
              aria-label={t("recording.cancelAutostop")}
              title={t("app.rec.noStopTitle")}
              class={styles.noStop}
              onClick={() => void cancelAutostop()}
            >
              ∞
            </button>
          </span>
        </>
      ) : stopAt !== null ? (
        <span data-testid="overlay-autostop-now" class={styles.stopText}>
          {t("app.rec.stoppingNow")}
        </span>
      ) : (
        <span data-testid="overlay-until-stop" class={styles.stopText}>
          {t("app.rec.untilStop")}
        </span>
      )}
    </div>
  );
}

// ── Advarsler: ikon + ord ───────────────────────────────────────────────────

interface WarnChipProps {
  icon: IconName;
  /** Ordet øyet ser. */
  word: string;
  /** Hele setningen: tittel og skjermleser. */
  full: string;
  testId: string;
  onDismiss?: () => void;
}

function WarnChip({ icon, word, full, testId, onDismiss }: WarnChipProps) {
  return (
    <span role="status" data-testid={testId} title={full} class={styles.chip}>
      <Icon name={icon} size={16} />
      <span aria-hidden="true">{word}</span>
      <span class={styles.sr}>{full}</span>
      {onDismiss ? (
        <button
          type="button"
          aria-label={t("app.common.dismiss")}
          class={styles.chipClose}
          onClick={onDismiss}
        >
          <Icon name="x" size={12} strokeWidth={2.5} />
        </button>
      ) : null}
    </span>
  );
}

/**
 * Stillhet, gjenkobling og kameraets tilstand — som små brikker øverst, så de
 * ikke flytter klokka når de kommer og går.
 *
 *   • stillhet / gjenkobling: motorens egne varsler, avvises med kryss (stripa
 *     hadde ingen vei ned uten et `recording://reconnected` som ikke alltid
 *     kommer — se `dismissReconnecting`),
 *   • bilder hoppet over (C3c): lyden er urørt, ingenting å gjøre midt i en
 *     gudstjeneste; selve antallet vises ikke,
 *   • kameraet er borte (C5b) / bildet stoppet eller åpnet aldri (M3R): selve
 *     opptaket går videre — hun skal bare ikke tro at bildet tas opp.
 */
function Warnings({ camera, device }: { camera: boolean; device: string }) {
  const problem = sessionVideoProblem.value;
  return (
    <div class={styles.chips}>
      {reconnecting.value ? (
        <WarnChip
          icon="refresh"
          testId="overlay-reconnect"
          word={t("app.rec.chip.reconnect")}
          full={
            device
              ? tf("app.overlay.reconnect", { name: device })
              : t("app.overlay.reconnectAnon")
          }
          onDismiss={dismissReconnecting}
        />
      ) : null}
      {silenceActive.value ? (
        <WarnChip
          icon="mute"
          testId="overlay-silence"
          word={t("app.rec.chip.silence")}
          full={[t("recording.silenceWarn"), silenceDetail.value]
            .filter(Boolean)
            .join(" ")}
          onDismiss={clearSilence}
        />
      ) : null}
      {camera && sessionVideoDropped.value > 0 ? (
        <WarnChip
          icon="camera"
          testId="overlay-video-skipped"
          word={t("app.rec.chip.videoSkipped")}
          full={t("app.overlay.videoSkipped")}
        />
      ) : null}
      {camera && sessionCameraLost.value ? (
        <WarnChip
          icon="camera"
          testId="overlay-camera-lost"
          word={t("app.rec.chip.cameraLost")}
          full={t("app.overlay.cameraLost")}
        />
      ) : null}
      {camera && problem ? (
        <WarnChip
          icon="camera"
          testId="overlay-video-problem"
          word={t("app.rec.chip.noVideo")}
          full={
            problem === "camera_not_opened"
              ? t("app.overlay.cameraNotOpened")
              : t("app.overlay.videoStopped")
          }
        />
      ) : null}
    </div>
  );
}
