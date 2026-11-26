/**
 * TA OPP — jobb nr. 1 av fire, og siden alt annet i appen finnes for.
 *
 * ## F5: bare det man trenger idet man trykker
 *
 * Fram til F5 var dette et kontrollrom med fem kort (mappe, kvalitet, kamera,
 * automatisk opptak, varsling) og en utfoldbar lydskjerm. Eieren beskar det:
 * en frivillig skal trykke ÉN stor knapp. Det som står igjen:
 *
 *   **Venstre, sticky, LEVENDE.** Hvor kommer lyden fra, hører vi den, og den
 *   store knappen. Linja «neste planlagte opptak» står under, med en lenke til
 *   PLAN. Kilden har en «Endre»-knapp som fører til INNSTILLINGER → Lyd og
 *   kanaler; kanalrutenettet og enhetsvalget bor der.
 *
 *   **Høyre, bildet.** Kameraforhåndsvisningen, når kamera er på. Kamera,
 *   mappe, varsler og den ukentlige tiden settes på INNSTILLINGER og PLAN.
 *
 * Under 1100 px blir det én kolonne, i den samme rekkefølgen.
 *
 * ## Den ene adferdsendringen (fra før F5)
 *
 * Start er sperret til en lydkilde er valgt EKSPLISITT — også når valget er
 * maskinens egen mikrofon. Regelen bor i `record-core.ts` med en tabell rundt
 * seg, og knappen bærer grunnen sin (`disabledReason`) i stedet for bare å
 * være grå.
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
 *     ⚠️ `@lib/audio/vu-feed` har ingen egen opptaksvakt, og `app/`
 *     gjenskaper ikke `window.__isRecording` med vilje. Her er det derfor
 *     MONTERINGEN som er vakten: ingen måler i treet, ingen `start_vu`.
 *     Overlegget leser motorens egen `recording://levels`.
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
import { showTelemetryPreview } from "../settings/advanced/TelemetryRow";
import { FirstRunResumeChip } from "../firstrun/FirstRunResumeChip";
import { preflightText } from "./preflight-text";
import {
  banners,
  dismissBanner,
  type BackendWarningBanner,
  type BannerData,
} from "../../state/banners";
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
import { ActionBar } from "../../ui/ActionBar/ActionBar";
import { Banner } from "../../ui/Banner/Banner";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Chip } from "../../ui/Chip/Chip";
import { ConsentCard } from "../../ui/ConsentCard/ConsentCard";
import { VuMeter } from "../../ui/VuMeter/VuMeter";
import { CameraPreview } from "../../ui/CameraPreview/CameraPreview";
import { alertDialog } from "../../ui/dialog";
import { reveal, revealLabel } from "../../ui/reveal";
import { toast } from "../../ui/toast";
import { spanText } from "./span-text";
import { confirmAndStop } from "./stop";
import {
  basename,
  defaultDeviceOf,
  formatBytes,
  nativeErrorDetail,
  nativeErrorSuffix,
  nativeErrorSuffixFromText,
  qualityReasonSuffix,
  sourceState,
  spanOfMinutes,
  spanOfSeconds,
} from "./record-core";
import { capitalizeFirst } from "@lib/ui/capitalize";
import { DOT } from "@lib/ui/dot";
import styles from "./record.module.css";

export function RecordPage() {
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

  return (
    <div class={styles.page}>
      <RecordBanners />
      {/* R6: synlig helt til «Åpne SundayRec» setter onboardingDone — se
          `FirstRunResumeChip.tsx`. Under banner-raden, over «Lytter»: en
          ekte feilbanner haster mer enn en påminnelse om å fullføre
          oppsettet. */}
      <FirstRunResumeChip />

      <div class={styles.head}>
        <p data-testid="record-sub" class={styles.sub}>
          {t("app.record.sub")}
        </p>
        <Listening />
      </div>

      <Consent />

      <div class={styles.room}>
        {/*
          VENSTRE: det levende. Sticky, så Start står der man så den sist.
        */}
        <div class={styles.live}>
          <SoundControl source={source} />

          {/*
            Måleren er MONTERINGEN som er vakten: ingen måler i treet, ingen
            `start_vu`. Se toppen av fila.
          */}
          {live || starting ? null : (
            <Card testId="record-meter">
              <VuMeter
                testId="record-vu"
                deviceName={s.deviceName}
                // Ingen kilde er valgt ⇒ ingen enhet åpnes.
                off={source.kind === "no-source"}
              />
            </Card>
          )}

          <NextPlannedCard />
        </div>

        {/* HØYRE: bildet, når kamera er på. */}
        <div class={styles.prep}>
          <CameraPreviewBlock />
        </div>
      </div>

      <Done />

      <ActionBar
        hint={
          source.kind === "no-source" ? (
            <p data-testid="record-why-blocked" class={styles.why}>
              {t("app.record.whyBlocked")}
            </p>
          ) : source.kind === "source-missing" ? (
            <p data-testid="record-can-start" class={styles.why}>
              {t("app.record.canStart")}
            </p>
          ) : (
            // F2-T3: samme slisse de to andre hintene bruker — den står tom
            // når kilden er klar, og en tom slisse er nøyaktig der en snarvei
            // kan nevnes uten å legge en ekstra linje til en side som alt har
            // to varianter av den fra før.
            <p data-testid="record-start-hint" class={styles.why}>
              {t("app.record.startHint")}
            </p>
          )
        }
      >
        <Button
          variant="record"
          size="lg"
          busy={starting}
          disabled={!source.canStart}
          disabledReason={t("app.record.whyBlocked")}
          testId="record-start"
          onClick={() => void handleStart()}
        >
          {t("app.record.start")}
        </Button>
      </ActionBar>
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

/**
 * Hvor lyden kommer fra — og veien til INNSTILLINGER → Lyd og kanaler.
 *
 * Tre tilstander (`record-core.ts` avgjør hvilken). Enhetsvalget og
 * kanalrutenettet bor på INNSTILLINGER; her står bare svaret og knappen som
 * fører dit. De to gule tilstandene beholder sine egne kort: en advarsel med
 * to nødutganger er ikke en kompakt rad, og `record-no-source` /
 * `record-source-missing` er kontrakten resten av appen kjenner dem på.
 */
function SoundControl({ source }: { source: ReturnType<typeof sourceState> }) {
  const [searching, setSearching] = useState(false);
  const [switching, setSwitching] = useState(false);
  const fallback = defaultDeviceOf(audioDevices.value);
  const goSound = () => navigate("settings", { anchor: "sound" });

  /**
   * Nødutgangen: ta valget på brukerens vegne, med en EKTE enhets-id.
   * To nøkler må lande sammen (`deviceId` + `deviceName`), og aldri
   * `deviceId: null`: da ville nødutgangen gjort valget UTATT igjen.
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
      <section id="sound" data-testid="control-sound">
        <Card
          tone="warn"
          testId="record-no-source"
          title={t("app.record.noSource")}
          description={t("app.record.noSourceDesc")}
          actions={
            <Button
              variant="primary"
              testId="record-choose-sound"
              onClick={goSound}
            >
              {t("app.record.chooseSound")}
            </Button>
          }
        />
      </section>
    );
  }

  if (source.kind === "source-missing") {
    return (
      <section id="sound" data-testid="control-sound">
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
              <Button
                variant="ghost"
                testId="record-change-source"
                onClick={goSound}
              >
                {t("app.setup.change")}
              </Button>
            </>
          }
        />
      </section>
    );
  }

  return (
    <section id="sound" data-testid="control-sound">
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
            onClick={goSound}
          >
            {t("app.setup.change")}
          </Button>
        </div>
      </Card>
    </section>
  );
}

// ── Kamerabildet ────────────────────────────────────────────────────────────

/**
 * Kamerabildet, ØVERST i høyrekolonnen.
 *
 * ## Hvorfor det står der og ikke i kamera-kortet
 *
 * Bildet er ikke en innstilling — det er et FAKTUM om rommet, og det skal
 * kunne leses uten å folde ut noe. Kamera-kortet er der man velger enhet;
 * bildet er der man ser at valget var riktig, og de to trenger ikke å være
 * åpne samtidig for at bildet skal gjøre jobben sin.
 *
 * Og det står i HØYRE kolonne, ikke ved siden av Start: Start er det ene
 * elementet på siden som ikke skal endre form eller plass fordi et tillegg er
 * på.
 *
 * ## Hvorfor bildet i det hele tatt
 *
 * Å velge kamera forteller deg hvilken enhet appen vil bruke. Det forteller deg
 * ikke om linsen peker på menigheten, om lokket er på, eller om kabelen ble
 * dratt ut i går. Det er det bare et bilde som gjør — og de fem minuttene før
 * gudstjenesten er den ene anledningen noen har til å se det.
 */
function CameraPreviewBlock() {
  if (settings.value.videoEnabled !== true) return null;
  return (
    <div class={styles.preview}>
      <CameraPreview />
    </div>
  );
}

// ── «Neste planlagte opptak» ────────────────────────────────────────────────

/**
 * Linja om neste planlagte opptak — med en lenke til PLAN.
 *
 * Finnes det ett, sier linja når (og vekkehintet, når maskinen må vekkes).
 * Finnes det ingen, sier den det og tilbyr veien til planen: det er her en
 * frivillig oppdager at appen kan starte selv.
 */
function NextPlannedCard() {
  const state = nextRecording.value;
  const next = state.next;
  const toPlan = (
    <Button
      variant="ghost"
      testId="record-next-plan"
      onClick={() => navigate("plan")}
    >
      {t("app.record.openPlan")}
    </Button>
  );

  if (!next) {
    return (
      <Card testId="record-next-none">
        <div class={styles.row}>
          <div class={styles.grow}>
            <div class={styles.label}>{t("app.record.nextAuto")}</div>
            <div class={styles.muted}>{t("app.record.nextNone")}</div>
          </div>
          {toPlan}
        </div>
      </Card>
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
      <div class={styles.row}>
        <div class={styles.grow}>
          <div class={styles.label}>{t("app.record.nextAuto")}</div>
          <div data-testid="record-next-auto-when" class={styles.value}>
            {tf("app.record.nextAutoWhen", {
              day: capitalizeFirst(parts.weekdayLong, locale.value),
              time: parts.time,
            })}
          </div>
          {wake ? (
            <div data-testid="record-next-auto-wake" class={styles.muted}>
              {wake}
            </div>
          ) : null}
        </div>
        {toPlan}
      </div>
    </Card>
  );
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
 *
 * ## F1-R4: kortet henter seg selv fram
 *
 * Kvitteringen rendres NEDERST i `.page` — under kilde, måler, Start, de to
 * opptakskortene og hele høyrekolonnen. Med et utfoldet kontrollrom er den
 * langt under det synlige vinduet, og en frivillig som ikke ruller ned aner
 * ikke at fila ble lagret. Samme historie for en tastaturbruker: ingenting
 * flyttet fokus, så skjermleseren sa aldri at noe nytt kom.
 *
 * Så når `finished` går fra `null` til et opptak, ruller siden kortet til
 * midten og gir det fokus. `tabIndex` settes IMPERATIVT på DOM-noden og ikke som en prop
 * på `Card` (som ikke videresender den): kortet skal være fokuserbart bare i
 * det øyeblikket det dukker opp, ikke stå i tabrekkefølgen for alltid.
 * `preventScroll` på selve `focus()`-kallet fordi rullingen allerede er gjort
 * med `block: "center"` — nettleserens egen fokus-scroll retter seg bare inn
 * mot nærmeste kant, og ville flyttet kortet vekk fra midten igjen.
 */
/**
 * Raden kvitteringens «Rediger» og «Vis i Finder» handler på — den EKTE veien
 * inn i begge, som navngir en rad og aldri en sti (A2, B-familien). Rust skrev
 * raden rett før `recording://finished` og la id-en i hendelsen; at den ikke
 * er der betyr at raden ikke ble skrevet, og da finnes ingenting å handle på.
 * Som reserve (en hendelse uten id) kan historikken ha lest raden inn mens
 * kvitteringen sto: samme fil, samme rad. Aldri en søk på stien etterpå.
 */
function finishedRowId(
  finished: { path: string; recordingId: string | null },
  loaded: { path?: string; id?: string | null } | null | undefined,
): string | null {
  if (finished.recordingId) return finished.recordingId;
  return loaded?.path === finished.path ? (loaded.id ?? null) : null;
}

function Done() {
  const finished = finishedRecording.value;
  const rows = lastRecording.value;

  // Historikken leses på nytt når et opptak er ferdig — det nye opptaket ER
  // den nyeste raden, og «Siste opptak»-kortet skal ikke vise det forrige.
  useEffect(() => {
    if (finished) void loadRecordingCount();
  }, [finished?.path]);

  // F1-R4: rull til og fokuser kvitteringen når den DUKKER OPP — ikke på hver
  // gjengivelse mens den samme `finished` står (historikkraden som kommer inn
  // et øyeblikk senere skal ikke rive fokus bort igjen).
  useEffect(() => {
    if (!finished) return;
    const el = document.querySelector<HTMLElement>(
      '[data-testid="record-done"]',
    );
    if (!el) return;
    el.tabIndex = -1;
    el.scrollIntoView({ block: "center" });
    el.focus({ preventScroll: true });
  }, [finished?.path]);

  if (!finished) return null;

  const row = rows?.path === finished.path ? rows : null;
  const rowId = finishedRowId(finished, rows);
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
        {/*
          Uten rad finnes ingenting å åpne eller vise, og knappene sier det
          (som i biblioteket) i stedet for å lukke seg uten et ord.
        */}
        <Button
          variant="primary"
          disabled={!rowId}
          disabledReason={t("app.done.revealFailed")}
          testId="record-done-edit"
          onClick={() =>
            openInEditor(
              rowId ?? undefined,
              row?.startedAt ?? row?.timestamp ?? null,
              basename(finished.path),
            )
          }
        >
          {t("editor.promptOpen")}
        </Button>
        <Button
          variant="secondary"
          disabled={!rowId}
          disabledReason={t("app.done.revealFailed")}
          testId="record-done-reveal"
          onClick={() => void reveal(rowId)}
        >
          {revealLabel()}
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
  const warnings = banners.value.filter(isBackendWarning);
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
            detail={nativeErrorDetail(
              tDyn("recording", nativeErrorSuffix(entry.code)),
              entry.code,
              entry.message,
            )}
            onDismiss={() => dismissBanner("recording-error")}
            actions={
              <Button
                variant="secondary"
                testId="banner-recording-error-open"
                onClick={() => navigate("recordings")}
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
                onClick={() => navigate("recordings")}
              >
                {t("recording.qualityAction")}
              </Button>
            }
          />
        ),
      )}

      {warnings.map((entry) => (
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
      ))}

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
              onClick={() => navigate("settings", { anchor: "folder" })}
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
          // Funnene i BRUKERENS språk: motoren sender en `PreflightCode`, og
          // `preflightText` slår den opp under `status.preflightCode`. Ingen
          // oppsummering — hvert funn er nøyaktig det funnet motoren gjorde,
          // bare sagt på riktig språk.
          detail={state.preflight.map(preflightText).join(DOT)}
          onDismiss={dismissPreflight}
        />
      ) : null}
    </div>
  );
}

/** «{n} feil må rettes før opptaket» / «{n} ting å se på». Katalognøklene er
 *  legacy-skallets egne og finnes i begge språk — ingen NY tellende nøkkel. */
function preflightHeadline(findings: readonly PreflightFinding[]): string {
  const errors = findings.filter((f) => f.severity === "error").length;
  return errors > 0
    ? tn("status.preflightErrors", errors)
    : tn("status.preflightWarns", findings.length);
}

/**
 * Er dette en bakende-advarsel?
 *
 * En typevakt og ikke et `startsWith` i filteret: `BannerData` er en union, og
 * et filter som ikke SIER at det smalner den lar `entry.severity` være et
 * felt TypeScript ikke vet finnes. Vakten er det ene stedet «hvilke nøkler er
 * bakendens» står skrevet, og `BackendWarningKey` i `state/banners.ts` er
 * listen den holdes mot.
 */
function isBackendWarning(entry: BannerData): entry is BackendWarningBanner {
  return entry.key.startsWith("backend-");
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
