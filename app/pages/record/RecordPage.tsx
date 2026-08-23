/**
 * OPPTAK — jobb nr. 1 av fire, og siden alt annet i appen finnes for.
 *
 * Rekkefølgen på skjermen er rekkefølgen i hodet til en frivillig som kom inn
 * fem minutter før gudstjenesten: HVOR kommer lyden fra, HØRER vi den, og så
 * den store knappen. Kamera og automatisk opptak er tillegg som bare står der
 * hvis noen har slått dem på i Oppsett.
 *
 * ## Den ene adferdsendringen
 *
 * Start er sperret til en lydkilde er valgt EKSPLISITT — også når valget er
 * maskinens egen mikrofon. Regelen bor i `record-core.ts` med en tabell rundt
 * seg, og knappen bærer grunnen sin (`disabledReason`) i stedet for bare å
 * være grå. I dag tar appen opp på laptop-mikrofonen uten å si fra.
 *
 * ## Måleren, og hvem som eier mikrofonen
 *
 * `VuMeter` holder den delte bakenden-strømmen (`acquireVuFeed`) mens siden
 * står åpen. Den slippes to steder, og begge er viktige:
 *
 *   - `off` når ingen kilde er valgt: å måle «systemets standardinngang» der
 *     ville vært å åpne nøyaktig den mikrofonen sett 2 finnes for å slutte å
 *     ta opp fra uten å spørre.
 *   - måleren fjernes helt før `start_recording` og mens et opptak går.
 *     ⚠️ `@lib/audio/vu-feed` avstår fra `start_vu` ved å lese
 *     `window.__isRecording`, og `app/` gjenskaper ikke den globalen med
 *     vilje. Her er det derfor MONTERINGEN som er vakten: ingen måler i treet,
 *     ingen `start_vu`. Overlegget leser motorens egen `recording://levels`.
 *
 * ## «Rediger» finnes ikke ennå
 *
 * Canvasens «Siste opptak»-kort og kvitteringen har begge «Åpne i Rediger».
 * Redigeringsflaten er P4. En knapp til en side som ikke finnes lærer en
 * frivillig at knappene i denne appen ikke er til å stole på, så inntil
 * videre står «Vis i Finder» der i stedet — den gjør noe, i dag.
 *
 * ## Brikkene «Redigert» / «Eksportert» er ikke med
 *
 * `recordings_list` bærer ingen slik status (se `state/recordings.ts`). Et
 * merke som gjettes er verre enn ingen merke.
 */

import { useEffect, useState } from "preact/hooks";

import type { TelemetryConsent } from "@legacy/bindings/TelemetryConsent";
import {
  formatWakeHint,
  intlParts,
  parseLocalIso,
} from "@lib/status/next-recording-core";
import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";

import { openInEditor } from "../../editor/entry";
import { locale, t, tDyn, tf, tn } from "../../i18n";
import {
  consumePendingAction,
  navigate,
  pendingAction,
} from "../../router/router";
import { showTelemetryPreview } from "../setup/advanced/TelemetryRow";
import { banners, dismissBanner } from "../../state/banners";
import { interpolate, WARNING_SUFFIXES } from "../../state/backend-warning";
import {
  audioDevices,
  loadAudioDevices,
  loadVideoDevices,
  videoDevices,
} from "../../state/devices";
import { currentRoomMinutes, refreshDiskSpace } from "../../state/disk";
import { prerollActive } from "../../state/preroll";
import {
  dismissMissed,
  dismissPreflight,
  nextRecording,
} from "../../state/next-recording";
import { runSilentPreflightOnce } from "../../state/preflight";
import {
  dismissFinishedRecording,
  finishedRecording,
  isRecording,
  markSessionStarted,
} from "../../state/recording";
import { lastRecording, loadRecordingCount } from "../../state/recordings";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { LOW_DISK_MINUTES } from "../../state/status-line";
import { Banner } from "../../ui/Banner/Banner";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Chip } from "../../ui/Chip/Chip";
import { ConsentCard } from "../../ui/ConsentCard/ConsentCard";
import { Toggle } from "../../ui/Toggle/Toggle";
import { VuMeter } from "../../ui/VuMeter/VuMeter";
import { alertDialog } from "../../ui/dialog";
import { toast } from "../../ui/toast";
import { useSetting } from "../../settings/use-setting";
import { spanText } from "./span-text";
import { confirmAndStop } from "./stop";
import {
  basename,
  capitalizeFirst,
  defaultDeviceOf,
  formatBytes,
  nativeErrorSuffix,
  nativeErrorSuffixFromText,
  qualityReasonSuffix,
  sourceState,
  spanOfMinutes,
  spanOfSeconds,
} from "./record-core";
import styles from "./record.module.css";

/** Skilletegn mellom fakta på én linje. Et tegn, ikke prosa. */
const DOT = " · ";

export function RecordPage() {
  const s = settings.value;
  const devices = audioDevices.value;
  const source = sourceState(s, devices);
  const live = isRecording.value;
  const [starting, setStarting] = useState(false);

  // Enhetslisten og ledig plass leses når SIDEN åpnes, ikke ved oppstart: en
  // liste hentet ved boot er gammel når noen faktisk står foran mikseren.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    // Én gang per oppstart, ikke per besøk — se `state/preflight.ts`.
    void runSilentPreflightOnce();
  }, []);

  /**
   * Start.
   *
   * Nøyaktig samme to kall som legacy, i samme rekkefølge og gjennom den samme
   * shimmen: `startRecordingNow` → `plan_recording_opts` → `start_recording`.
   * Bare de tre feltene shimmen faktisk videresender settes; ALT annet
   * (mappe, format, kanaler, kamera-navn) plukker `plan_recording_opts` opp
   * fra den lagrede profilen, som er den ene sannheten siden R4.
   *
   * Ingen `#modal-manual`: kilde, kamera og filnavn er Oppsett-beslutninger
   * (eiervalg, canvas sett 2). Derfor heller ikke `customName` — filnavnet
   * følger mønsteret profilen allerede har.
   */
  async function handleStart(): Promise<void> {
    if (starting || live || !source.canStart) return;
    setStarting(true);
    try {
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
      return;
    }
    if (armed === "run-preflight") {
      consumePendingAction();
      navigate("setup", { tab: "sound" });
    }
  }, [armed, source.canStart, live]);

  return (
    <div class={styles.page}>
      <RecordBanners />

      <div class={styles.head}>
        <p data-testid="record-sub" class={styles.sub}>
          {t("app.record.sub")}
        </p>
        <Listening />
      </div>

      <Consent />

      <SourceCard source={source} />

      <Card testId="record-meter">
        <VuMeter
          testId="record-vu"
          deviceName={s.deviceName}
          // Tre grunner til at ingen enhet åpnes herfra: ingen kilde er valgt,
          // motoren eier den allerede, eller den er i ferd med å overta. Se
          // toppen av fila.
          off={source.kind === "no-source" || live || starting}
        />
      </Card>

      <div class={styles.startRow}>
        <div class={styles.start}>
          <Button
            variant="record"
            size="lg"
            block
            busy={starting}
            disabled={!source.canStart}
            disabledReason={t("app.record.whyBlocked")}
            testId="record-start"
            onClick={() => void handleStart()}
          >
            {t("app.record.start")}
          </Button>
        </div>
        <CameraCard />
      </div>

      {source.kind === "no-source" ? (
        <p data-testid="record-why-blocked" class={styles.why}>
          {t("app.record.whyBlocked")}
        </p>
      ) : source.kind === "source-missing" ? (
        <p data-testid="record-can-start" class={styles.why}>
          {t("app.record.canStart")}
        </p>
      ) : null}

      <div class={styles.cards}>
        <NextAutoCard />
        <LastRecordingCard />
      </div>

      <Done />
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

// ── Samtykkekortet ──────────────────────────────────────────────────────────

/**
 * Spurt ÉN gang, på OPPTAK (canvas sett 6 flyttet det ut av sekvensen).
 *
 * `needsPrompt` er bakendens svar, ikke vårt: den er sann når ingen har svart
 * ennå, OG igjen den dagen omfanget utvides — også for den som sa nei sist.
 */
function Consent() {
  const [consent, setConsent] = useState<TelemetryConsent | null>(null);

  useEffect(() => {
    void window.api
      .telemetryConsentGet()
      .then(setConsent)
      // En probe vi ikke fikk kjørt er ikke en grunn til å spørre — et kort som
      // dukker opp fordi IPC-en glapp er et spørsmål brukeren ikke kan svare på.
      .catch(() => setConsent(null));
  }, []);

  if (consent?.needsPrompt !== true) return null;
  return (
    <ConsentCard
      status={consent.status}
      onExplain={() => void showTelemetryPreview()}
      onAnswered={() => setConsent({ ...consent, needsPrompt: false })}
    />
  );
}

// ── Kilde-kortet (2.1 / 2.2 / 2.3) ──────────────────────────────────────────

function SourceCard({ source }: { source: ReturnType<typeof sourceState> }) {
  const [searching, setSearching] = useState(false);
  const [switching, setSwitching] = useState(false);
  const fallback = defaultDeviceOf(audioDevices.value);

  /**
   * Nødutgangen: ta valget på brukerens vegne, med en EKTE enhets-id.
   *
   * To nøkler må lande sammen (`deviceId` + `deviceName`), akkurat som i
   * `SoundPage`s «Bruk denne» — derfor én eksplisitt handling og én lagring i
   * stedet for to `useSetting`. Og aldri `deviceId: null`: da ville
   * nødutgangen gjort valget UTATT igjen, som er tilstanden 2.2 handler om.
   */
  async function useBuiltIn(): Promise<void> {
    if (!fallback || switching) return;
    setSwitching(true);
    try {
      patchSettings({ deviceId: fallback.id, deviceName: fallback.name });
      const ok = await saveSettingsDebounced(120);
      if (!ok) toast("error", t("general.saveFailed"));
    } finally {
      setSwitching(false);
    }
  }

  async function searchAgain(): Promise<void> {
    setSearching(true);
    try {
      await loadAudioDevices();
    } finally {
      setSearching(false);
    }
  }

  if (source.kind === "no-source") {
    return (
      <Card
        tone="warn"
        testId="record-no-source"
        title={t("app.record.noSource")}
        description={t("app.record.noSourceDesc")}
        actions={
          <Button
            variant="primary"
            testId="record-choose-sound"
            onClick={() => navigate("setup", { tab: "sound" })}
          >
            {t("app.record.chooseSound")}
          </Button>
        }
      />
    );
  }

  if (source.kind === "source-missing") {
    return (
      <Card
        tone="warn"
        testId="record-source-missing"
        title={tf("app.setup.sound.gone", { name: source.name })}
        description={t("app.setup.sound.goneDesc")}
        actions={
          <>
            {fallback ? (
              <Button
                variant="secondary"
                busy={switching}
                testId="record-use-builtin"
                onClick={() => void useBuiltIn()}
              >
                {t("app.record.useBuiltIn")}
              </Button>
            ) : null}
            <Button
              variant="primary"
              busy={searching}
              testId="record-retry"
              onClick={() => void searchAgain()}
            >
              {t("app.setup.sound.retry")}
            </Button>
          </>
        }
      />
    );
  }

  return (
    <Card testId="record-source">
      <div class={styles.row}>
        <div class={styles.grow}>
          <div class={styles.label}>{t("app.record.soundFrom")}</div>
          <div data-testid="record-source-value" class={styles.value}>
            {source.pair
              ? tf("app.setup.sound.deviceWithPair", {
                  name: source.name,
                  l: source.pair.l,
                  r: source.pair.r,
                })
              : source.name}
          </div>
        </div>
        <Button
          variant="ghost"
          testId="record-change-source"
          onClick={() => navigate("setup", { tab: "sound" })}
        >
          {t("app.setup.change")}
        </Button>
      </div>
    </Card>
  );
}

// ── Kamera-tillegget ────────────────────────────────────────────────────────

/**
 * Kamera av og på for DENNE gudstjenesten.
 *
 * Kortet står bare der kamera er en del av oppsettet — enten fordi tillegget
 * er på, eller fordi et kamera er valgt. Den andre halvdelen er ikke pynt:
 * uten den ville bryteren gjemt sitt eget kort i det øyeblikket noen slo den
 * av, og veien tilbake gikk gjennom Oppsett.
 *
 * Hvilket kamera, oppløsning og resten er Oppsett-beslutninger; her er det ett
 * spørsmål, og det er «blir kamera med i dag?».
 */
function CameraCard() {
  const s = settings.value;
  const on = s.videoEnabled === true;
  const chosen = (s.videoDeviceName ?? "").trim();
  const enabled = useSetting("videoEnabled", { kind: "toggle" });
  const cameras = videoDevices.value;

  useEffect(() => {
    if (on && videoDevices.peek() === null) void loadVideoDevices();
  }, [on]);

  if (!on && !chosen) return null;

  const none = on && cameras !== null && cameras.length === 0;

  return (
    <div data-testid="record-camera" class={styles.camera}>
      <Toggle
        checked={enabled.draft === true}
        onChange={(next) => enabled.set(next)}
        disabled={enabled.busy}
        labelId="record-camera-label"
        testId="record-camera-toggle"
      />
      <div class={styles.grow}>
        <div id="record-camera-label">{t("app.setup.camera.title")}</div>
        <div data-testid="record-camera-summary" class={styles.muted}>
          {none
            ? t("app.setup.camera.none")
            : chosen || t("app.setup.camera.noneChosen")}
        </div>
      </div>
    </div>
  );
}

// ── «Neste automatiske opptak» ──────────────────────────────────────────────

function NextAutoCard() {
  const state = nextRecording.value;
  const next = state.next;

  if (!next) {
    // Ingen tid er kjent ⇒ ingenting kommer til å skje av seg selv, og DET er
    // det kortet skal si — ikke «neste opptak: —».
    return (
      <Card
        testId="record-auto-question"
        title={t("app.record.autoQuestion")}
        description={t("app.setup.auto.desc")}
        actions={
          <Button
            variant="secondary"
            testId="record-auto-setup"
            onClick={() => navigate("setup", { anchor: "auto" })}
          >
            {t("app.setup.setUp")}
          </Button>
        }
      />
    );
  }

  const parts = intlParts(locale.value)(next.atMs);
  const wake = formatWakeHint(state, {
    t,
    tf,
    tn,
    parts: intlParts(locale.value),
    nowMs: Date.now(),
  });

  return (
    <Card testId="record-next-auto">
      <div class={styles.label}>{t("app.record.nextAuto")}</div>
      <div data-testid="record-next-auto-when" class={styles.value}>
        {`${capitalizeFirst(parts.weekdayLong, locale.value)} ${parts.time}`}
      </div>
      {next.label ? (
        <div data-testid="record-next-auto-label" class={styles.muted}>
          {next.label}
        </div>
      ) : null}
      {wake ? (
        <div data-testid="record-next-auto-wake" class={styles.muted}>
          {wake}
        </div>
      ) : null}
    </Card>
  );
}

// ── «Siste opptak» ──────────────────────────────────────────────────────────

function LastRecordingCard() {
  const last = lastRecording.value;
  // Ikke lest ennå, eller ingen opptak: ingen påstand i noen retning.
  if (!last) return null;

  const when = last.timestamp
    ? capitalizeFirst(
        intlParts(locale.value)(last.timestamp).dateLong,
        locale.value,
      )
    : "";
  // ⚠️ `rowToEntry` gjør en ukjent `duration_ms` til `durationSec: 0`, så 0 er
  // tvetydig: enten et opptak uten lyd, eller en rad som aldri fikk en
  // varighet. «0 min» er da en påstand vi ikke kan stå for — WKWebView-proben
  // fant nøyaktig den setningen på eierens egen maskin.
  const span = spanOfSeconds(last.durationSec || null);
  const line = [when, span.kind === "none" ? "" : spanText(span)]
    .filter(Boolean)
    .join(DOT);

  return (
    <Card testId="record-last">
      <div class={styles.label}>{t("app.record.last")}</div>
      <div data-testid="record-last-when" class={styles.value}>
        {line || last.filename}
      </div>
      <div class={styles.row}>
        <span data-testid="record-last-name" class={styles.muted}>
          {last.filename}
        </span>
        <Button
          variant="ghost"
          testId="record-last-reveal"
          onClick={() => void reveal(last.path ?? null)}
        >
          {t("app.done.show")}
        </Button>
      </div>
    </Card>
  );
}

/** «Vis i Finder». Sier fra når det ikke gikk — en knapp som stille ikke gjør
 *  noe er verre enn ingen knapp. */
async function reveal(path: string | null): Promise<void> {
  if (!path) return;
  const ok = await window.api.revealFile(path);
  if (!ok) toast("error", t("app.done.revealFailed"));
}

// ── Kvitteringen (2.6) ──────────────────────────────────────────────────────

/**
 * «Opptaket er lagret» — et KORT, ikke en toast.
 *
 * Erstatter legacy `#editor-prompt-toast`, som spurte «vil du redigere?» og
 * forsvant av seg selv. Kortet blir stående til noen har sett det: at fila
 * finnes, hvor stor den ble, og hvor den ligger.
 *
 * Varigheten og størrelsen leses fra historikkraden bakenden skriver FØR den
 * fyrer `recording://finished` (`finalize_one` i `recorder/engine.rs`) — ikke
 * fra overleggets egen klokke. Klokken vet hvor lenge det har gått; bare fila
 * vet hva som faktisk ble skrevet, og det er forskjellen datatapsalarmen
 * finnes for.
 *
 * «Åpne i Rediger» er med fra P4a: flaten finnes, og den er det man som oftest
 * vil gjøre med et opptak som nettopp ble ferdig. Den er PRIMÆR og «Vis i
 * Finder» sekundær — Finder er der man går når man skal gjøre noe utenfor
 * appen, og redigering er inne i den.
 *
 * ⚠️ `askOpenEditor` har fortsatt ingen leser i Rust (ATLAS §2.6), så kortet
 * vises uansett hva den innstillingen sier.
 */
function Done() {
  const finished = finishedRecording.value;
  const rows = lastRecording.value;

  // Historikken leses på nytt når et opptak er ferdig — det nye opptaket ER
  // den nyeste raden, og «Siste opptak»-kortet skal ikke vise det forrige.
  useEffect(() => {
    if (finished) void loadRecordingCount();
  }, [finished?.path]);

  if (!finished) return null;

  const row = rows?.path === finished.path ? rows : null;
  // Se `LastRecordingCard`: 0 er «ukjent», ikke «null sekunder».
  const span = spanOfSeconds(row?.durationSec || null);
  const size = formatBytes(row?.fileSizeBytes ?? null, locale.value);
  const folder = (settings.value.saveFolder ?? "").trim();
  const meta = [span.kind === "none" ? "" : spanText(span), size, folder]
    .filter(Boolean)
    .join(DOT);

  return (
    <Card
      tone="good"
      testId="record-done"
      title={t("app.done.title")}
      description={meta || undefined}
    >
      <div data-testid="record-done-file" class={styles.value}>
        {row?.filename ?? basename(finished.path)}
      </div>
      <div class={styles.row}>
        <Button
          variant="primary"
          testId="record-done-edit"
          onClick={() =>
            openInEditor(
              finished.path,
              row?.startedAt ?? row?.timestamp ?? null,
            )
          }
        >
          {t("editor.promptOpen")}
        </Button>
        <Button
          variant="secondary"
          testId="record-done-reveal"
          onClick={() => void reveal(finished.path)}
        >
          {t("app.done.show")}
        </Button>
        <Button
          variant="ghost"
          testId="record-done-ok"
          onClick={dismissFinishedRecording}
        >
          {t("app.done.ok")}
        </Button>
      </div>
    </Card>
  );
}

// ── Bannerne (canvas sett 7) ────────────────────────────────────────────────

/**
 * Fire stripper, én form.
 *
 * Rekkefølgen er hvor mye det haster: noe som ER tapt først, så det som
 * stopper søndagen, så det som stoppet forrige søndag.
 */
function RecordBanners() {
  // BARE opptakssidens egne. Køen er delt, og P3 la til `update`, som ikke
  // hører til noen side og derfor rendres av skallet — over hvilken side som
  // enn står. Et filter og ikke en else-gren: en nøkkel som havnet her uten å
  // være ventet ville blitt malt som et kvalitetsbanner uten at noe sa fra.
  const list = banners.value.filter(
    (entry) =>
      entry.key === "recording-error" || entry.key === "recording-quality",
  );
  // Bakendens egne advarsler (`backend://warning`) — se
  // `state/backend-warning.ts`, som eier både kanalen og dedupliseringen mot
  // stripene under. De rendres HER og ikke i skallet fordi alle fire handler om
  // opptaket: forhåndsbufferen, gjenopprettingen, lydenheten, disken.
  const warnings = banners.value.filter((entry) =>
    entry.key.startsWith("backend-"),
  );
  const state = nextRecording.value;
  const room = currentRoomMinutes();
  const lowDisk = room !== null && room < LOW_DISK_MINUTES;

  return (
    <div class={styles.banners}>
      {list.map((entry) =>
        entry.key === "recording-error" ? (
          <Banner
            key={entry.key}
            tone="bad"
            testId="banner-recording-error"
            title={tf("app.banner.errorTitle", {
              time: clockOf(entry.atMs),
            })}
            detail={tDyn("recording", nativeErrorSuffix(entry.code))}
            onDismiss={() => dismissBanner("recording-error")}
            actions={
              <Button
                variant="secondary"
                testId="banner-recording-error-open"
                onClick={() => navigate("library")}
              >
                {t("app.banner.errorShow")}
              </Button>
            }
          />
        ) : (
          <Banner
            key={entry.key}
            tone="bad"
            testId="banner-recording-quality"
            title={tf("recording.qualityAlarm", {
              m: entry.measuredSec,
              e: entry.expectedSec,
            })}
            detail={qualityReasonText(entry.reasonCodes, entry.reasons)}
            onDismiss={() => dismissBanner("recording-quality")}
            actions={
              <Button
                variant="secondary"
                testId="banner-recording-quality-open"
                onClick={() => navigate("library")}
              >
                {t("recording.qualityAction")}
              </Button>
            }
          />
        ),
      )}

      {warnings.map((entry) =>
        entry.key === "recording-error" ||
        entry.key === "recording-quality" ||
        entry.key === "update" ? null : (
          <Banner
            key={entry.key}
            // Bakendens egen alvorlighetsgrad, ikke vår gjetning. `error` er
            // `role="alert"`: en mikser som ikke er i huset en halvtime før et
            // planlagt opptak SKAL avbryte det skjermleseren holder på med.
            tone={entry.severity === "error" ? "bad" : "warn"}
            testId={`banner-${entry.key}`}
            title={warningText(entry.code, entry.msg, entry.params)}
            onDismiss={() => dismissBanner(entry.key)}
          />
        ),
      )}

      {lowDisk ? (
        <Banner
          tone="warn"
          testId="banner-low-disk"
          title={tf("app.banner.diskTitle", {
            room: spanText(spanOfMinutes(room)),
          })}
          detail={t("app.banner.diskDesc")}
          actions={
            <Button
              variant="secondary"
              testId="banner-low-disk-free"
              onClick={() => navigate("setup", { tab: "folder" })}
            >
              {t("app.banner.diskFree")}
            </Button>
          }
        />
      ) : null}

      {state.missed.length > 0 ? (
        <Banner
          tone="warn"
          testId="banner-missed"
          title={tf("app.banner.missedTitle", {
            when: capitalizeFirst(
              intlParts(locale.value)(parseLocalIso(state.missed[0].at))
                .dateLong,
              locale.value,
            ),
          })}
          detail={
            state.missed.length > 1
              ? tn("missed.banner", state.missed.length)
              : t("app.banner.missedDesc")
          }
          onDismiss={dismissMissed}
          actions={
            <Button
              variant="secondary"
              testId="banner-missed-why"
              onClick={() =>
                void alertDialog({
                  title: t("app.banner.missedWhy"),
                  message: t("app.banner.missedHelp"),
                })
              }
            >
              {t("app.banner.missedWhy")}
            </Button>
          }
        />
      ) : null}

      {state.preflight.length > 0 ? (
        <Banner
          tone="warn"
          testId="banner-preflight"
          title={preflightHeadline(state.preflight)}
          // Funnene som REN TEKST: bakenden formulerer dem allerede for et
          // menneske, og en oppsummering her ville vært et andre sted de kunne
          // begynne å si noe annet enn det som faktisk ble funnet.
          detail={state.preflight.map((f) => f.message).join(DOT)}
          onDismiss={dismissPreflight}
        />
      ) : null}
    </div>
  );
}

/** «{n} feil må rettes før opptaket» / «{n} ting å se på». Katalognøklene er
 *  legacy-skallets egne og finnes i alle sju språk — ingen NY tellende nøkkel. */
function preflightHeadline(findings: readonly PreflightFinding[]): string {
  const errors = findings.filter((f) => f.severity === "error").length;
  return errors > 0
    ? tn("status.preflightErrors", errors)
    : tn("status.preflightWarns", findings.length);
}

/**
 * Årsakslinja under kvalitetsalarmen.
 *
 * ## ⚠️ Den var motorens hardkodede NORSKE prosa
 *
 * `sundayrec_core::selftest` setter sammen setninger som «3.42s manglende/
 * stille lyd — hakking/dropp» med `format!`, og de gikk rett inn i banneret.
 * En engelsk bruker fikk altså norsk teknisk sjargong i det ene varselet som
 * betyr «ikke stol på dette opptaket».
 *
 * Motoren sender nå kodene ved siden av prosaen. Regelen er én linje:
 *
 *   • `reasonCodes === null` — FELTET mangler, altså en eldre bakende. Da er
 *     prosaen alt som finnes, og en sann setning på feil språk slår en tom
 *     linje. (Samme avveining som `backend://warning`s ukjente koder.)
 *   • ellers oversettes hver kode; en kode katalogen ikke kjenner faller
 *     tilbake på motorens prosalinje på SAMME indeks, ikke på en generisk
 *     «ukjent årsak» — den ville byttet informasjon mot språk.
 */
function qualityReasonText(
  codes: readonly string[] | null,
  reasons: readonly string[],
): string | undefined {
  if (codes === null) {
    return reasons.length
      ? tf("recording.qualityReasons", { r: reasons.join(", ") })
      : undefined;
  }
  if (codes.length === 0) return undefined;
  const parts = codes.map((code, i) => {
    const suffix = qualityReasonSuffix(code);
    return suffix ? tDyn("recording", suffix) : (reasons[i] ?? code);
  });
  return tf("recording.qualityReasons", { r: parts.join(", ") });
}

/**
 * Setningen for én bakende-advarsel.
 *
 * Rekkefølgen er legacys, og den er hele designet:
 *
 *   1. `code` → en `notify.*`-nøkkel → setningen på brukerens språk,
 *   2. ellers `msg`, motorens egen (norske) ordlyd,
 *   3. ellers den bare koden.
 *
 * Steg 2 er det som gjør det trygt for bakenden å lære en ny advarsel før
 * denne katalogen gjør det: brukeren får en SANN setning på feil språk i
 * stedet for stillhet. Stillhet er nøyaktig det denne kanalen produserte i
 * månedsvis. `tDyn` kaster i DEV på en ukjent nøkkel, så oppslaget gjøres bare
 * for koder tabellen faktisk kjenner — en ny kode skal falle til steg 2, ikke
 * ta ned siden.
 */
function warningText(
  code: string,
  msg: string | null,
  params: Readonly<Record<string, string>>,
): string {
  const suffix = WARNING_SUFFIXES[code];
  const template = suffix ? tDyn("notify", suffix) : (msg ?? code);
  return interpolate(template, { ...params });
}

/** «11:42» — klokkeslettet i en feilmelding er halve informasjonen. */
function clockOf(atMs: number): string {
  return new Intl.DateTimeFormat(locale.value, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(atMs));
}
