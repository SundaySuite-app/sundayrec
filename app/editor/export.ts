/**
 * Steg 3 — EKSPORTER. Valgene, kjøringen og kvitteringen.
 *
 * Nyttelasten bygges av `buildExportRequest` fra
 * `@lib/pages/editor/export-params` — uendret, importert, allerede
 * enhetstestet. To ting der er lærepenger noen har betalt for:
 * `outputFolder` er ALLTID en streng («» = ved siden av kilden, som bakenden
 * løser opp), og det finnes ikke noe `mode`-felt, fordi «Erstatt original»
 * stille oppførte seg som «ny fil» i årevis.
 *
 * ## Fremdriften er bakendens egen
 *
 * `editor-export-progress` bærer `{ pct, phase }`, og fasen er en av to
 * ledningskoder som er festet mot Rust-siden med en test på hvert sted
 * (`EXPORT_PHASE_MEASURING` / `EXPORT_PHASE_ENCODING`). Mastringens
 * måle-passering har ingen prosent av seg selv — den melder 0 — og en bar som
 * står på null i to minutter leses som «hengt». Derfor er baren UBESTEMT
 * (`fraction === null`) helt til et ekte tall kommer, og ETA-estimatoren fôres
 * ikke med en brøk som ikke er en brøk.
 *
 * Den ENE fasen som ikke er bakendens er `EXPORT_PHASE_PREPARING`
 * («Forbereder …»), og den finnes fordi kjøringen begynner FØR bakenden hører
 * om den: kanalanalysen er en full passering over opptaket. Se `runExport`.
 *
 * ## Abonnementet varer så lenge eksporten varer
 *
 * Ett abonnement per kjøring, revet ned i `finally`. Et modulnivå-abonnement
 * ville skrevet videre til en bar som ikke står der lenger — legacys egen
 * kommentar, og legacys egen feil.
 */

import { signal } from "@preact/signals";
import { buildExportRequest } from "@lib/pages/editor/export-params";
import { createEtaEstimator } from "@lib/ui/progress-core";
import type { EditorExportProgress } from "@legacy/bindings/EditorExportProgress";

import { isRecording } from "../state/recording";
import { settings } from "../state/settings";
import {
  bitrateKbps,
  exportErrorKey,
  EXPORT_PHASE_PREPARING,
  isCancelled,
  VIDEO_CODEC,
  VIDEO_FORMAT,
  type ExportFormat,
} from "./export-core";
import { DEFAULT_EXPORT_FORMAT } from "./export-core";
import { clearDirty, E, mediaInfo } from "./model";
import { clearDraft } from "./cuts";
import {
  channelRepair,
  ensureSoundAnalysis,
  mixerProcessing,
  soundProfile,
  useMixer,
} from "./sound";
import { soundExportFields } from "./sound-profiles";

// ── Valgene ─────────────────────────────────────────────────────────────────

export const exportFormat = signal<ExportFormat>(DEFAULT_EXPORT_FORMAT);
/** «» = «Samme mappe som opptaket». En valgt mappe er en absolutt sti. */
export const exportFolder = signal("");
/** «Ta med video (MP4)». Bare synlig når kilden HAR et videospor. */
export const includeVideo = signal(false);

// ── Kjøringen ───────────────────────────────────────────────────────────────

export const exporting = signal(false);
/** 0–1, eller `null` for «ingen nevner ennå» (mastringens måle-passering). */
export const exportFraction = signal<number | null>(null);
export const exportEtaMs = signal<number | null>(null);
/** Ledningskoden for fasen som pågår, eller `null`. */
export const exportPhase = signal<string | null>(null);
export const cancelling = signal(false);

/** Kvitteringen: stien bakenden faktisk skrev til. */
export const exportedPath = signal<string | null>(null);
/** Sekundene som ble eksportert — kvitteringens «28 min 10 s». */
export const exportedSeconds = signal(0);
/** Anslåtte byte for fila som ble skrevet. Anslag, ikke en `stat`. */
export const exportedBytes = signal<number | null>(null);
/** Mappen fila havnet i. */
export const exportedFolder = signal("");
/** Nøkkelen som forklarer hvorfor det ikke gikk, eller `null`. Kan være
 *  `null` MENS `exportFailed` er sann — en kode `exportErrorKey` ikke
 *  kjenner er fortsatt en feil, bare en uten en egen setning. */
export const exportErrorText = signal<string | null>(null);
/** Var «feilen» at brukeren trykte Avbryt? Da er den ikke rød. */
export const exportWasCancelled = signal(false);
/**
 * Gikk eksporten dårlig, uansett om koden er kjent?
 *
 * Skilt fra `exportErrorText` med vilje: FØR dette signalet fantes ble en
 * feil UTEN kjent kode (en USB-pinne trukket ut, full disk, ffmpeg som
 * feiler av en grunn appen ikke har en setning for) til stillhet —
 * `ExportProblem`s vakt så `!exportErrorText.value` og viste ingenting, og
 * den generelle setningen den selv skrev inn som fallback var død kode. Nå
 * er «gikk det dårlig» og «har vi en presis setning for det» to spørsmål,
 * og flaten kan svare «ja» på det første uten det andre.
 */
export const exportFailed = signal(false);

/*
 * ⚠️ `exportDone` sto her fram til D3. Den var stegstripas hake på steg 3, og
 * da eksporten ble en egen destinasjon var den ENESTE leseren borte. Et signal
 * som fortsatt skrives og aldri leses er verre enn ingen: det ser ut som en
 * tilstand noen bryr seg om. Kvitteringen (`exportedPath`) er svaret på det
 * samme spørsmålet, og den har en leser.
 */

/** Er kilden en videofil? Fra `editor_load_recording`, uten et ekstra kall. */
export function sourceHasVideo(): boolean {
  return mediaInfo.value?.hasVideo === true;
}

/**
 * Blir dette en VIDEO-eksport?
 *
 * Bare når kilden har bilde OG brukeren ba om å ta det med. En videofil man
 * eksporterer uten bryteren gir en ren lydfil i det valgte formatet — som er
 * hva de aller fleste vil ha med en gudstjeneste-mp4.
 */
export function isVideoExport(): boolean {
  return sourceHasVideo() && includeVideo.value;
}

/** Filendelsen eksporten får. */
export function exportExtension(): string {
  return isVideoExport() ? VIDEO_FORMAT : exportFormat.value;
}

/**
 * Hvilken KJØRING som gjelder.
 *
 * `E.loadSeq` svarer på «er det fortsatt den samme FILA»; denne svarer på «er
 * det fortsatt den samme EKSPORTEN». De to er ikke det samme spørsmålet siden
 * F2-A-B: `runExport` setter `exporting` FØR kanalanalysen, og i det vinduet
 * (30–60 s på en gudstjeneste) kan brukeren rekke å trykke Avbryt uten at fila
 * har byttet. Bakenden har ingen ffmpeg å drepe da, så det er DENNE telleren
 * som gjør at kjøringen faktisk stopper i stedet for å eksportere videre etter
 * en avbryting som så ut til å virke.
 *
 * Bumpes av `cancelExport` og av `resetExport` — altså av alt som sier «det som
 * var i gang, gjelder ikke lenger».
 */
let runSeq = 0;

export function resetExport(): void {
  // Enhver kjøring som fortsatt henger i en `await` er foreldreløs herfra.
  runSeq += 1;
  exportFormat.value = DEFAULT_EXPORT_FORMAT;
  exportFolder.value = "";
  includeVideo.value = false;
  exporting.value = false;
  exportFraction.value = null;
  exportEtaMs.value = null;
  exportPhase.value = null;
  cancelling.value = false;
  exportedPath.value = null;
  exportedSeconds.value = 0;
  exportedBytes.value = null;
  exportedFolder.value = "";
  exportErrorText.value = null;
  exportWasCancelled.value = false;
  exportFailed.value = false;
}

/** «Velg mappe …». Et avbrutt valg lar det forrige stå. */
export async function pickExportFolder(): Promise<void> {
  const picked = await window.api.editorPickOutputFolder();
  if (picked) exportFolder.value = picked;
}

/** Legg kvitteringen bort og kom tilbake til valgene, med dem stående. */
export function exportAgain(): void {
  exportedPath.value = null;
  exportErrorText.value = null;
  exportWasCancelled.value = false;
  exportFailed.value = false;
}

export async function cancelExport(): Promise<void> {
  // IKKE deaktivert etter klikk: legacy gjorde det, permanent, og en avbryting
  // som landet i et av eksportens barnløse opphold (kildeprobingen,
  // loudnorm-JSON-parsingen) lot brukeren stirre på «Avbryter…» på en død knapp
  // mens eksporten gikk til ende. Et andre klikk er ufarlig — avbryting er
  // idempotent.
  cancelling.value = true;
  // Avbryt FØR bakenden har hørt om eksporten i det hele tatt.
  //
  // I forberedelsesfasen finnes det ingen ffmpeg å drepe — kjøringen henger i
  // kanalanalysen — så `editor_cancel_export` svarer et sant «nei, ingenting
  // kjørte», og uten dette gikk eksporten videre etter en avbryting som SÅ ut
  // til å virke. Kjøringsnummeret er det som stopper den; kvitteringen under er
  // ordrett den bakenden ville gitt, så flaten ikke kan se forskjell på hvilken
  // side av spawnen brukeren rakk å trykke.
  if (exportPhase.peek() === EXPORT_PHASE_PREPARING) {
    runSeq += 1;
    exporting.value = false;
    cancelling.value = false;
    exportPhase.value = null;
    exportFraction.value = null;
    exportEtaMs.value = null;
    exportWasCancelled.value = true;
    exportErrorText.value = "errCancelled";
    exportFailed.value = false;
  }
  try {
    // Kalles UANSETT fase: en eksport skallet tror er i forberedelse, men som
    // bakenden faktisk har spawnet (en tilstand bare en feil kan lage), skal
    // ikke overleve fordi flaten var sikker på at den ikke fantes.
    await window.api.editorCancelExport();
  } catch {
    /* ingenting kjørte, eller bakenden svarte ikke — samme utfall for brukeren */
  }
}

/**
 * Kjør eksporten.
 *
 * Originalen røres ikke: bakenden skriver en NY fil i mappen, med et
 * kollisjonsfritt navn. Lykkes den, slettes kutt-utkastets sidevogn — den
 * finnes for å overleve en krasj midt i en redigering, og redigeringen er nå
 * ute av huset.
 *
 * ## Generasjonsvakten (R8)
 *
 * `openFile` (`loader.ts`) bumper `E.loadSeq` SYNKRONT, før noe annet, i
 * det øyeblikket brukeren åpner en annen fil — også midt i en eksport som
 * fortsatt henger i en `await` her. Uten en vakt landet resultatet likevel:
 * `exportedPath`/`exportedFolder`/`exportedSeconds` ble skrevet som om det
 * var en kvittering for fila som nå står åpen, og — verre — `clearDraft()`
 * leser `E.filePath` PÅ DET TIDSPUNKTET den kalles, som da alt er den NYE
 * fila. En vellykket eksport av fil A slettet dermed kutt-utkastet til fil
 * B, den man faktisk sitter og redigerer.
 *
 * `seq` er `E.loadSeq` slik den var da DENNE kjøringen startet. Sjekket
 * etter HVER `await`, FØR noe skrives — samme mønster som `loader.ts` og
 * `sermon.ts`.
 *
 * ## Vakten må stå FØR analysen, ikke etter (F2-2)
 *
 * `exporting`-vakten på første linje var sann — og verdiløs, fordi den vernet
 * om et flagg som ble satt LANGT senere. `ensureSoundAnalysis()` er en full
 * `astats`-passering over hele opptaket; på en 90 minutters gudstjeneste tar
 * den 30–60 sekunder. I hele det vinduet så «Eksporter» uberørt ut, og et
 * andre klikk gikk rett gjennom vakten, ventet på den SAMME memoiserte
 * analysen, og sendte en andre `editor_export`. Hva to eksporter på én motor
 * gjør med hverandres filer står i `ExportEngine`s `in_flight`-felt; kort sagt
 * meldte den ene suksess på en trunkert fil og den andre «avbrutt» på en hel.
 *
 * Så: flagget settes først, fasen sier «Forbereder …», og Kjører-visningen
 * står med en ubestemt bar og en Avbryt-knapp som faktisk avbryter (se
 * `cancelExport`). Analysen kommer etterpå.
 */
export async function runExport(
  keptSeconds: number,
  estimate: number | null,
): Promise<void> {
  if (!E.filePath || exporting.value) return;
  // Eksport UNDER opptak (F2-11): begge er full-fil ffmpeg-arbeid, og CPU-en de
  // konkurrerer om er den samme som capture-tråden trenger. Et stall der er
  // ikke en treg eksport — det er tapte samples i gudstjenesten som tas opp NÅ
  // (målt 2026-07-31: 15–56 % av samplene forsvant da en ffmpeg strupte seg
  // selv). Opptaket vinner, og skjermen sier hvorfor i stedet for at knappen
  // bare ikke gjør noe.
  if (isRecording.peek()) {
    exportedPath.value = null;
    exportWasCancelled.value = false;
    exportErrorText.value = "errRecordingInProgress";
    exportFailed.value = true;
    return;
  }
  const seq = E.loadSeq;
  const run = ++runSeq;

  exporting.value = true;
  cancelling.value = false;
  exportedPath.value = null;
  exportErrorText.value = null;
  exportWasCancelled.value = false;
  exportFailed.value = false;
  exportFraction.value = null;
  exportEtaMs.value = null;
  // Skallets egen fase: det finnes ingen ffmpeg å melde prosent for ennå.
  exportPhase.value = EXPORT_PHASE_PREPARING;

  // Kanalanalysen: en frivillig som gikk rett fra Klipp til Eksporter skal få
  // den samme reparasjonen som en som stoppet innom Lyd. Den er memoisert, så
  // den koster ingenting når steget har vært åpent.
  if (soundProfile.value !== "none") await ensureSoundAnalysis();
  // Fila kan ha byttet MENS analysen ventet. Ingenting er skrevet ennå — bare
  // gå stille ut, som om eksporten aldri ble bedt om. Og RØR IKKE signalene:
  // `openFile`/`closeFile` har alt nullstilt dem for fila som står nå.
  if (seq !== E.loadSeq) return;
  // …eller brukeren rakk å trykke Avbryt mens analysen gikk. `cancelExport`
  // har skrevet kvitteringen; det eneste som gjenstår er å ikke eksportere.
  if (run !== runSeq) return;

  // Forberedelsen er over; herfra er fasen bakendens egen. `null` og ikke
  // «encoding»: den koden skal komme fra Rust, ikke gjettes her.
  exportPhase.value = null;

  const video = isVideoExport();
  const sound = soundExportFields({
    profile: soundProfile.value,
    useMixer: useMixer.value,
    processing: useMixer.value ? mixerProcessing() : undefined,
    repair: channelRepair.value,
  });

  const params = buildExportRequest({
    kind: video ? "video" : "audio",
    inputPath: E.filePath,
    cutRegions: E.cuts,
    duration: E.duration,
    outputFolder: exportFolder.value,
    format: exportFormat.value,
    bitrate: bitrateKbps(settings.value.bitrate),
    videoFormat: VIDEO_FORMAT,
    videoCodec: VIDEO_CODEC,
    ...sound,
  });

  // Estimatoren eies av JOBBEN og ikke av stripa: en bar som forsvinner og
  // kommer tilbake skal ikke miste det den har lært om farten.
  const eta = createEtaEstimator();
  const unsub = window.api.on?.(
    "editor-export-progress",
    (payload: unknown) => {
      // `payload` crosses an untyped event channel, so this is still a cast,
      // not a check — but casting to the GENERATED `EditorExportProgress`
      // binding (rather than a hand-typed `{ pct?; phase? }` twin) means a
      // Rust rename of either field fails `npm run typecheck` right here,
      // at the destructure below, instead of leaving the progress bar
      // silently stuck.
      const { pct, phase } = (payload ?? {}) as Partial<EditorExportProgress>;
      if (typeof phase === "string" && phase) exportPhase.value = phase;
      if (typeof pct !== "number" || !Number.isFinite(pct)) return;
      const shown = Math.max(0, Math.min(100, pct));
      // Null prosent er måle-passeringen som melder seg uten en nevner. Navngi
      // fasen, la stripa gli, og ikke fôr estimatoren med en brøk som ikke er en.
      if (shown <= 0) {
        exportFraction.value = null;
        exportEtaMs.value = null;
        return;
      }
      const fraction = shown / 100;
      exportFraction.value = fraction;
      exportEtaMs.value = eta.push(fraction, performance.now()).etaMs;
    },
  );

  let result: { ok: boolean; outputPath?: string; error?: string };
  try {
    result = video
      ? await window.api.editorExportVideo(params)
      : await window.api.editorExportFile(params);
  } catch (err) {
    result = { ok: false, error: String((err as Error)?.message ?? err) };
  } finally {
    // `unsub` er ALLTID denne kjøringens egen — riv den uansett. De andre
    // fire er DELTE signaler UI-et leser NÅ, for filen som er åpen NÅ; en
    // foreldet kjøring som endelig kommer tilbake skal ikke få lov til å
    // slukke en ekte, pågående eksport av den nye fila.
    unsub?.();
    if (seq === E.loadSeq) {
      exporting.value = false;
      exportFraction.value = null;
      exportEtaMs.value = null;
      exportPhase.value = null;
      cancelling.value = false;
    }
  }
  // Samme vakt igjen, FØR resultatet skrives noe sted: hverken som en
  // kvittering for fila som nå er åpen, eller — det som faktisk skjedde —
  // som en `clearDraft()` av DENS utkast. Se filhodet.
  if (seq !== E.loadSeq) return;

  if (result.ok && result.outputPath) {
    exportedPath.value = result.outputPath;
    exportedFolder.value =
      exportFolder.value || result.outputPath.replace(/[/\\][^/\\]*$/, "");
    exportedSeconds.value = keptSeconds;
    exportedBytes.value = video ? null : estimate;
    // Eksporten lyktes — utkastet har gjort jobben sin.
    clearDraft();
    clearDirty();
    return;
  }
  exportWasCancelled.value = isCancelled(result.error);
  exportErrorText.value = exportErrorKey(result.error);
  // Avbrutt er brukerens eget valg — ikke en feil, og ikke noe å varsle om.
  // Alt annet er en eksport som gikk dårlig, uansett om `exportErrorText`
  // over fant en kjent kode: `ExportProblem` leser DETTE signalet for om den
  // skal vise noe i det hele tatt, den generelle setningen for om den ikke
  // fant en presis en. Se filhodet for hvorfor de to ikke er det samme.
  exportFailed.value = !exportWasCancelled.value;
  if (exportFailed.value) {
    console.warn("[export] eksport feilet:", result.error);
  }
}
