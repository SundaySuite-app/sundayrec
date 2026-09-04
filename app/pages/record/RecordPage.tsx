/**
 * OPPTAK — kontrollrommet. Jobb nr. 1 av fire, og siden alt annet i appen
 * finnes for.
 *
 * ## D2: alt som betyr noe redigeres HER
 *
 * Fram til D2 sa denne siden hva som var galt og sendte deg til Oppsett for å
 * rette det. Eieren så på det og sa det som var sant: de fem minuttene før
 * gudstjenesten er den ene anledningen noen har til å gjøre appen klar, og en
 * app som da sender deg til en annen skjerm — og tilbake, og til en tredje —
 * er en app som bruker de fem minuttene på navigasjon.
 *
 * Så skjermen er to kolonner:
 *
 *   **Venstre, sticky, LEVENDE.** Hvor kommer lyden fra, hører vi den, og den
 *   store knappen. Den halvdelen skal aldri flytte på seg fordi noe til høyre
 *   ble foldet ut — Start er det ene elementet på siden som må stå der man så
 *   det sist.
 *
 *   **Høyre, KLARGJØRING.** Kamerabildet øverst (når kamera er på), så de fem
 *   kortene: mappe, kvalitet, kamera, automatisk opptak, varsling. Hvert kort
 *   viser svaret som gjelder nå og folder ut HELE skjermen som eier spørsmålet
 *   — den samme skjermen første gang bruker, ikke en kopi av den.
 *
 * Under 1100 px blir det én kolonne, i den samme rekkefølgen.
 *
 * ## Kortene er `SubPage`-skjermer, innbygget
 *
 * `useEmbedded()` sier fra til `SubPage` at rammen (leden) er unødvendig her:
 * kortraden over har allerede sagt hva skjermen er for. Effekten er symmetrisk
 * — forsvinner oppryddingen, mister INNSTILLINGER leden sin etter et besøk på
 * OPPTAK, og det er vakten `e2e/control-room.spec.ts` står for.
 *
 * ## Ankeret folder ut
 *
 * `?goto=settings:audio` → `record#sound`. `route.anchor` er ikke bare et
 * rullemål: kortet med det navnet foldes ut, rulles til, og pulserer når man
 * KOM dit (ikke ved en ren skjermbilde-lenke, som setter `highlight: false`).
 * Lista over gyldige ankre er `control-core.ts`, og `router.test.ts` krysser
 * den mot aliastabellen.
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
 * står åpen. Den slippes tre steder, og alle tre er viktige:
 *
 *   - `off` når ingen kilde er valgt: å måle «systemets standardinngang» der
 *     ville vært å åpne nøyaktig den mikrofonen sett 2 finnes for å slutte å
 *     ta opp fra uten å spørre.
 *   - måleren fjernes helt før `start_recording` og mens et opptak går.
 *     ⚠️ `@lib/audio/vu-feed` avstår fra `start_vu` ved å lese
 *     `window.__isRecording`, og `app/` gjenskaper ikke den globalen med
 *     vilje. Her er det derfor MONTERINGEN som er vakten: ingen måler i treet,
 *     ingen `start_vu`. Overlegget leser motorens egen `recording://levels`.
 *   - **kilde-kortet KOLLAPSER når opptaket starter.** D2 la en ANDRE måler på
 *     skjermen: «Hvilken lyd?» har sin egen (`sound-vu`), og et utfoldet
 *     kilde-kort viser den ved siden av sidens (`record-vu`). Strømmen er
 *     refcountet, så de to er én økt på enheten — men bare så lenge BEGGE
 *     forsvinner ved opptaksstart. En måler som ble stående ville holdt
 *     refcounten over null og bedt om enheten opptaket nettopp tok. Kortet
 *     foldes derfor sammen, som river hele `SoundPage` ut av treet.
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

import type { ComponentChildren } from "preact";
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
  route,
} from "../../router/router";
import { showTelemetryPreview } from "../setup/advanced/TelemetryRow";
import { AutoRecordCard } from "../setup/AutoRecordCard";
import { CameraCard } from "../setup/CameraCard";
import { answerText, detailText } from "../setup/decision-text";
import { decisionsFor } from "../setup/decisions-core";
import { FolderPage } from "../setup/FolderPage";
import { NotifyPage } from "../setup/NotifyPage";
import { QualityPage } from "../setup/QualityPage";
import { SoundPage } from "../setup/SoundPage";
import { useEmbedded } from "../setup/SubPage";
import { autoExpandable, decisionRows, isControlId } from "./control-core";
import type { ControlId } from "./control-core";
import {
  banners,
  dismissBanner,
  type BackendWarningBanner,
  type BannerData,
} from "../../state/banners";
import { interpolate, WARNING_SUFFIXES } from "../../state/backend-warning";
import { audioDevices, loadAudioDevices } from "../../state/devices";
import {
  currentRoomMinutes,
  diskFreeBytes,
  refreshDiskSpace,
} from "../../state/disk";
import { emailTransport, refreshEmailFacts } from "../../state/email";
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
import { ControlCard } from "../../ui/ControlCard/ControlCard";
import { VuMeter } from "../../ui/VuMeter/VuMeter";
import { LiveCameraPreview } from "../../ui/CameraPreview/LiveCameraPreview";
import {
  releaseCameraPreview,
  resumeCameraPreview,
} from "../../ui/CameraPreview/ownership";
import { alertDialog } from "../../ui/dialog";
import { toast } from "../../ui/toast";
import { spanText } from "./span-text";
import { confirmAndStop } from "./stop";
import {
  basename,
  capitalizeFirst,
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
import { DOT } from "@lib/ui/dot";
import styles from "./record.module.css";

export function RecordPage() {
  const s = settings.value;
  const devices = audioDevices.value;
  const source = sourceState(s, devices);
  const live = isRecording.value;
  const [starting, setStarting] = useState(false);
  const { open, toggle, setOpen } = useControlCards(live || starting);

  // Kortene bygger inn de samme skjermene Oppsett hadde. Rammen (leden) er
  // unødvendig når kortraden allerede har sagt hva skjermen er for — og
  // oppryddingen er hele poenget, se toppen av fila.
  useEmbedded();

  // Enhetslisten, ledig plass og e-postveien leses når SIDEN åpnes, ikke ved
  // oppstart: fakta hentet ved boot er gamle når noen faktisk står foran
  // mikseren. Alle tre er inndata kortene sier noe SANT med.
  useEffect(() => {
    void loadAudioDevices();
    void refreshDiskSpace();
    void refreshEmailFacts();
    // Én gang per oppstart, ikke per besøk — se `state/preflight.ts`.
    void runSilentPreflightOnce();
  }, []);

  // Ankeret: fold ut kortet, rull dit, og puls når man KOM hit. Rekkefølgen er
  // ikke likegyldig — utfoldingen må ha skjedd før vi ruller, ellers ruller vi
  // til en rad som er i ferd med å bli dobbelt så høy.
  //
  // ⚠️ Effekten henger på HELE ruten og ikke på ankerstrengen. To trykk på den
  // samme lenken («Frigjør plass», menylinjens «Sjekk oppsettet») gir det samme
  // ankeret, og med strengen som avhengighet ville det andre trykket vært en
  // knapp som stille ikke gjorde noe — for kortet kan være lukket igjen i
  // mellomtiden. `navigate` lager et nytt ruteobjekt hver gang, som er nøyaktig
  // «noen ba om dette på nytt».
  const current = route.value;
  const anchor = current.anchor;
  const highlight = current.highlight === true;
  useEffect(() => {
    if (!isControlId(anchor)) return;
    setOpen((prev) => withCard(prev, anchor, true));
    // Neste frame: kortet er malt, og elementet har den høyden det skal ha.
    const id = requestAnimationFrame(() => {
      document.getElementById(anchor)?.scrollIntoView({ block: "start" });
    });
    return () => cancelAnimationFrame(id);
  }, [current, setOpen]);

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
      // ⚠️ KAMERAET FØRST, og eksplisitt.
      //
      // macOS gir ÉN klient om gangen tilgang til en kameraenhet. Previewen
      // slipper også når `isRecording` blir sann — men det signalet settes
      // ETTER at motoren har svart ja (`markSessionStarted`), altså etter at
      // ffmpeg allerede har prøvd å åpne enheten mens webviewet holdt den.
      // Rekkefølgen er hele forskjellen mellom et videoopptak og en tom fil, og
      // den er pinnet i `e2e/record.spec.ts` («slipper kameraet FØR
      // start_recording»). Se `ui/CameraPreview/ownership.ts`.
      releaseCameraPreview();
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
      // Ble det ikke noe opptak likevel — motoren sa nei, eller kallet feilet —
      // skal kamerabildet tilbake. En svart rute etter et trykk som ikke førte
      // fram er en app som ser ødelagt ut av å ha sagt fra.
      if (!isRecording.peek()) resumeCameraPreview();
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
      // Menylinjens «Sjekk oppsettet» er et spørsmål om LYDEN. Den folder ut
      // kilde-kortet der man står, i stedet for å bytte skjerm — flaten som
      // svarer er allerede her. LOKALT og ikke gjennom ruteren: handlingen
      // kommer fra menylinjen mens siden allerede står åpen, og en navigering
      // til stedet man er ville vært et rutebytte for å gjøre ingenting.
      setOpen((prev) => withCard(prev, "sound", true));
      document.getElementById("sound")?.scrollIntoView({ block: "start" });
    }
  }, [armed, source.canStart, live, setOpen]);

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

      <div class={styles.room}>
        {/*
          VENSTRE: det levende. Sticky, så Start står der man så den sist selv
          om et kort til høyre folder seg ut.
        */}
        <div class={styles.live}>
          <SoundControl
            source={source}
            expanded={open.includes("sound")}
            onExpand={() => toggle("sound")}
            highlight={highlight && anchor === "sound"}
          />

          {/*
            Måleren er MONTERINGEN som er vakten: ingen måler i treet, ingen
            `start_vu`. Se toppen av fila.
          */}
          {live || starting ? null : (
            <Card testId="record-meter">
              <VuMeter
                testId="record-vu"
                deviceName={s.deviceName}
                // Ingen kilde er valgt ⇒ ingen enhet åpnes: å måle «systemets
                // standardinngang» ville vært å åpne nøyaktig den mikrofonen
                // sett 2 finnes for å slutte å ta opp fra uten å spørre.
                off={source.kind === "no-source"}
              />
            </Card>
          )}

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

          {source.kind === "no-source" ? (
            <p data-testid="record-why-blocked" class={styles.why}>
              {t("app.record.whyBlocked")}
            </p>
          ) : source.kind === "source-missing" ? (
            <p data-testid="record-can-start" class={styles.why}>
              {t("app.record.canStart")}
            </p>
          ) : null}

          {/*
            «Neste opptak» og «Siste opptak» står her og ikke i høyrekolonnen.

            De hører sammen med Start: det ene er når knappen trykker seg selv,
            det andre er hva den lagde sist. Men grunnen de FLYTTET er målt —
            venstrekolonnen sluttet under Start, og med en kort side var det en
            halv skjerm tom luft under den mens høyrekolonnen fortsatte. Nå
            fyller de to kortene den luften, og de to kolonnene ender omtrent
            samtidig.

            De står UNDER Start og påvirker derfor ikke hvor Start havner —
            det tallet er tetthetsaksens jobb, ikke denne flyttingens.
          */}
          <NextAutoCard />
          <LastRecordingCard />
        </div>

        {/* HØYRE: klargjøringen — bildet, og de fem kortene. */}
        <div class={styles.prep}>
          <CameraPreviewBlock />
          <ControlStack open={open} toggle={toggle} anchor={anchor} />
        </div>
      </div>

      <Done />
    </div>
  );
}

// ── Kortene, og hvilke som står åpne ────────────────────────────────────────

/**
 * Hvilke kort som er foldet ut.
 *
 * En LISTE og ikke ett navn: mappe og kvalitet er to spørsmål man gjerne har
 * åpne samtidig når man setter opp en ny maskin, og en trekkspill-regel som
 * lukket det ene fordi man åpnet det andre ville vært appen som bestemmer
 * hvilken rekkefølge man tenker i.
 *
 * De to tilleggene er derimot STYRT av bryteren sin: å slå på «Ta med kamera»
 * folder ut kameravalget, og å slå det av lukker det. Det er canvasens sett 5
 * («to tillegg som utvider siden når de slås på»), og det er det eneste stedet
 * en effekt skriver til lista uten at noen trykket på en kortrad.
 *
 * ⚠️ Den tredje effekten er VU-REGELEN: et utfoldet kilde-kort har sin egen
 * måler, og begge må ut av treet når opptaket starter. Se toppen av fila.
 */
function useControlCards(recording: boolean): {
  open: readonly ControlId[];
  toggle: (id: ControlId) => void;
  setOpen: (next: (prev: ControlId[]) => ControlId[]) => void;
} {
  const s = settings.value;
  const [open, setOpen] = useState<ControlId[]>([]);
  const cameraOn = s.videoEnabled === true;
  const autoOn = autoExpandable(s);

  useEffect(() => {
    setOpen((prev) => withCard(prev, "camera", cameraOn));
  }, [cameraOn]);

  useEffect(() => {
    setOpen((prev) => withCard(prev, "auto", autoOn));
  }, [autoOn]);

  useEffect(() => {
    if (!recording) return;
    setOpen((prev) => withCard(prev, "sound", false));
  }, [recording]);

  return {
    open,
    toggle: (id) => setOpen((prev) => withCard(prev, id, !prev.includes(id))),
    setOpen,
  };
}

/** Lista med `id` lagt til eller tatt ut. Ny array bare når noe faktisk
 *  endret seg — ellers ville hver effekt-kjøring vært en ny render. */
function withCard(
  open: ControlId[],
  id: ControlId,
  wanted: boolean,
): ControlId[] {
  const has = open.includes(id);
  if (has === wanted) return open;
  return wanted ? [...open, id] : open.filter((entry) => entry !== id);
}

/**
 * Kortstabelen i høyrekolonnen: mappe, kvalitet, kamera, automatisk opptak,
 * varsling — rekkefølgen `STACK_IDS` navngir.
 *
 * Skrevet ut og ikke løkket: de fem har hver sin kropp og hvert sitt sett med
 * props (to av dem har en bryter i topplinja), så en løkke måtte hatt en
 * `switch` inni seg for å skille dem — altså den samme lista, bare gjemt.
 *
 * De tre som er ett av de fem spørsmålene henter svaret sitt fra
 * `decisions-core` gjennom `decisionRows`, og oversetter det med
 * `decision-text` — det ene stedet den oversettelsen bor, delt med
 * sjekklisten i første-gangs-sekvensen. To kopier ville før eller siden sagt
 * to forskjellige ting om nøyaktig samme tilstand.
 */
function ControlStack({
  open,
  toggle,
  anchor,
}: {
  open: readonly ControlId[];
  toggle: (id: ControlId) => void;
  anchor: string | undefined;
}) {
  const s = settings.value;
  const rows = decisionRows(
    decisionsFor({
      settings: s,
      devices: audioDevices.value,
      diskFreeBytes: diskFreeBytes.value,
      roomMinutes: currentRoomMinutes(),
      emailTransport: emailTransport(),
      locale: locale.value,
      // Ingen måler i en kompaktrad: kortet sier hva som er VALGT, og
      // hørselstesten står i venstrekolonnen.
      vuWord: null,
    }),
  );
  const highlight = route.value.highlight === true;
  const row = (id: string) => rows.find((entry) => entry.id === id)!;
  const props = (id: ControlId) => ({
    expanded: open.includes(id),
    onExpand: () => toggle(id),
    highlight: highlight && anchor === id,
  });

  return (
    <div class={styles.stack}>
      <ControlCard
        id="folder"
        title={t("app.setup.q2")}
        value={answerText(row("folder").answer)}
        detail={detailText(row("folder").detail)}
        tone={row("folder").tone}
        expandLabel={
          row("folder").needsSetUp
            ? t("app.setup.setUp")
            : t("app.setup.change")
        }
        collapseLabel={t("app.record.close")}
        {...props("folder")}
      >
        <FolderPage />
      </ControlCard>

      <ControlCard
        id="quality"
        title={t("app.setup.q3")}
        value={answerText(row("quality").answer)}
        detail={detailText(row("quality").detail)}
        tone={row("quality").tone}
        expandLabel={t("app.setup.change")}
        collapseLabel={t("app.record.close")}
        {...props("quality")}
      >
        <QualityPage />
      </ControlCard>

      <CameraCard {...props("camera")} />
      <AutoRecordCard {...props("auto")} />

      <ControlCard
        id="notify"
        title={t("app.setup.q5")}
        value={answerText(row("notify").answer)}
        detail={detailText(row("notify").detail)}
        tone={row("notify").tone}
        expandLabel={
          row("notify").needsSetUp
            ? t("app.setup.setUp")
            : t("app.setup.change")
        }
        collapseLabel={t("app.record.close")}
        {...props("notify")}
      >
        <NotifyPage />
      </ControlCard>
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
 * Hvor lyden kommer fra — og hele «Hvilken lyd?»-skjermen bak den.
 *
 * Tre tilstander, som før (`record-core.ts` avgjør hvilken), og de to gule
 * beholder sine egne kort: en advarsel med to nødutganger er ikke en kompakt
 * rad, og `record-no-source` / `record-source-missing` er kontrakten resten av
 * appen kjenner dem på.
 *
 * Det som er nytt er at ALLE tre folder ut den samme skjermen på stedet, i
 * stedet for å navigere til den. Knappen som gjorde det heter fortsatt det den
 * gjorde — «Velg lyd» når ingenting er valgt, «Endre» når noe er det — for det
 * er fortsatt det den gjør.
 */
function SoundControl({
  source,
  expanded,
  onExpand,
  highlight,
}: {
  source: ReturnType<typeof sourceState>;
  expanded: boolean;
  onExpand: () => void;
  highlight: boolean;
}) {
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

  const body = expanded ? (
    <div id="sound-body" data-testid="control-sound-body">
      <SoundPage />
    </div>
  ) : null;

  const frame = (children: ComponentChildren) => (
    <section
      id="sound"
      data-anchor="sound"
      data-testid="control-sound"
      data-expanded={expanded ? "true" : "false"}
      data-highlight={highlight ? "true" : undefined}
      class={`${styles.sound} ${highlight ? styles.pulse : ""}`}
    >
      {children}
    </section>
  );

  if (source.kind === "no-source") {
    return frame(
      <Card
        tone="warn"
        testId="record-no-source"
        title={t("app.record.noSource")}
        description={t("app.record.noSourceDesc")}
        actions={
          <Button
            variant="primary"
            testId="record-choose-sound"
            expanded={expanded}
            controls="sound-body"
            onClick={onExpand}
          >
            {expanded ? t("app.record.close") : t("app.record.chooseSound")}
          </Button>
        }
      >
        {body}
      </Card>,
    );
  }

  if (source.kind === "source-missing") {
    return frame(
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
              expanded={expanded}
              controls="sound-body"
              onClick={onExpand}
            >
              {expanded ? t("app.record.close") : t("app.setup.change")}
            </Button>
          </>
        }
      >
        {body}
      </Card>,
    );
  }

  return frame(
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
          expanded={expanded}
          controls="sound-body"
          onClick={onExpand}
        >
          {expanded ? t("app.record.close") : t("app.setup.change")}
        </Button>
      </div>
      {body}
    </Card>,
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
      <LiveCameraPreview />
    </div>
  );
}

// ── «Neste automatiske opptak» ──────────────────────────────────────────────

/**
 * «Neste automatiske opptak» — når det finnes ett.
 *
 * ⚠️ Kortet hadde en ANDRE tilstand: «Skal SundayRec ta opp hver søndag av seg
 * selv?» med en «Sett opp»-knapp, for de gangene ingen tid var kjent. Den er
 * borte i D2, og grunnen sto rett over den i kontrollrommet: auto-kortet stiller
 * allerede det spørsmålet, med den samme setningen under seg («Sett en tid én
 * gang. Maskinen vekkes og starter selv.») og en bryter som svarer på det.
 * WKWebView-proben viste de to under hverandre, ord for ord like — to kort som
 * spør om det samme er hvordan en frivillig lærer at appen ikke vet hva den
 * mener.
 */
function NextAutoCard() {
  const state = nextRecording.value;
  const next = state.next;
  // Ingenting kommer til å skje av seg selv ⇒ ingen påstand. Auto-kortet over
  // sier hva som mangler, og har knappen som fikser det.
  if (!next) return null;

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
                onClick={() => navigate("edit")}
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
                onClick={() => navigate("edit")}
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
              onClick={() => navigate("record", { anchor: "folder" })}
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
