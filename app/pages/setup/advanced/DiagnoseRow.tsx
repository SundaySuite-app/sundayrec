/**
 * Diagnose — verktøyet som mistet flaten sin i fase B, som en rad på Avansert.
 *
 * ## Hvorfor den er tilbake, og hvorfor akkurat her
 *
 * `run_diagnostics` har virket hele tiden. Det som forsvant var skjermen som
 * viste svaret: modalen bodde på «Innstillinger → Lyd → Diagnose», en fane som
 * ikke finnes i det nye skallet, og den ble ikke bygget om. Konsekvensen står i
 * røykboken §5b — en riggtester ble bedt om å ÅPNE `last-recording.json` i en
 * teksteditor for å lese tall appen selv regner ut. Det er ikke en manglende
 * skjerm, det er en manglende SETNING: «hva er galt med maskinen min».
 *
 * Avansert er stedet fordi det er der de andre «gjør noe»-radene bor (loggen,
 * innstillingsprofilen), og fordi diagnosen ikke er en innstilling. Den har
 * ingen verdi å lagre; den har et svar å gi.
 *
 * ## UX: UTFOLDET PÅ STEDET, ikke en dialog
 *
 * Legacy åpnet en modal. Resultatet er en LISTE man leser, sammenlikner og
 * kopierer fra mens man snakker i telefonen med noen — og en modal er nettopp
 * formen som ikke tåler det: den dekker skjermen, den lukkes av et uhell, og
 * den kan ikke stå åpen ved siden av innstillingen man er i ferd med å endre.
 * Så resultatet folder seg ut under raden og blir stående.
 *
 * ## Rekkefølgen på det som vises, og hvorfor den er legacys
 *
 *   1. **Fem statusrader** — svaret på «virker lyden?», som er spørsmålet folk
 *      faktisk kommer med. Alltid fem, også når en av dem ikke har noe å si
 *      (`diagnose-core.statusRows` forklarer hvorfor en rad aldri forsvinner).
 *   2. **Funnene**, oversatt på kode — se `diagnose-core.ts`.
 *   3. **Enhetslista** bak en `<details>` — for lang til å stå åpen, for
 *      viktig til å mangle.
 *   4. **IPC-feilringen** bak en `<details>` — hvilke kommandoer som ikke
 *      svarte denne økten. Ringen har eksistert ubrukt siden E2.4; den svarer
 *      selv når bakenden som feiler ikke kan spørres.
 *   5. **Hvor rapporten ble lagret**, og **«Kopier full rapport»** — det
 *      support ber om, sist, fordi det ikke er det brukeren kom for å lese.
 *
 * ## ⚠️ Test-opptaket og opptaket som går
 *
 * `run_test_recording` kaller `vu.stop()` og åpner enheten for ekte i ~10 s.
 * Gjøres det MENS en gudstjeneste tas opp, kjemper to klienter om det samme
 * lydkortet — på macOS taper den ene. Knappen er derfor av under opptak, med
 * grunnen skrevet på seg (Button viser den tre steder).
 *
 * VU-måleren trenger ingen gjenstart her, og det er en observasjon og ikke en
 * forglemmelse: feeden er refcountet (`lib/audio/vu-feed.ts`), måleren bor på
 * OPPTAK, og på Innstillinger er abonnenttallet null — altså finnes det ingen
 * økt å ta opp igjen. Legacy startet den på nytt fordi modalen bodde på en
 * skjerm som HADDE en måler.
 */

import { useEffect, useState } from "preact/hooks";

import type { AudioDiagnostics } from "@legacy/bindings/AudioDiagnostics";
import type { DiagnosticFinding } from "@legacy/bindings/DiagnosticFinding";
import type { DiagnosticsReport } from "@legacy/bindings/DiagnosticsReport";
import type { TestRecordingResult } from "@legacy/bindings/TestRecordingResult";
import type { IpcFailure } from "@lib/ipc-failures-core";

import { t, tDyn, tf } from "../../../i18n";
import { consumePendingAction, pendingAction } from "../../../router/router";
import { isRecording } from "../../../state/recording";
import { settings } from "../../../state/settings";
import { Button } from "../../../ui/Button/Button";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { toast } from "../../../ui/toast";
import {
  findingSlug,
  statusRows,
  testErrorSlug,
  testSignalSlug,
  type StatusRow,
} from "./diagnose-core";
import styles from "./DiagnoseRow.module.css";

/** Alt én kjøring samlet inn, som ÉN verdi — se `run()` for hvorfor. */
interface DiagnoseResult {
  report: DiagnosticsReport;
  audio: AudioDiagnostics;
  micStatus: string | null;
  ffmpeg: { available: boolean; version: string | null } | null;
  /** Ringen slik den så ut da diagnosen kjørte. */
  ipc: readonly IpcFailure[];
}

export function DiagnoseRow() {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<DiagnoseResult | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [testBusy, setTestBusy] = useState(false);
  const [test, setTest] = useState<TestRecordingResult | null>(null);
  const [testFailure, setTestFailure] = useState<string | null>(null);
  const live = isRecording.value;

  /**
   * Kjør, og bytt HELE resultatet på én gang.
   *
   * Fire kall, ett `Promise.all`, én `setState`. Ikke fire delresultater som
   * siger inn hver for seg: da tegnes lista i mellomtilstander der «0
   * lydenheter» står med et rødt kryss i et halvsekund fordi enhetslista ikke
   * er kommet ennå — og det er nøyaktig den setningen ingen skal lese feil.
   *
   * De to helseprobene er `?.`-et fordi de er valgfrie i `api.d.ts`, og
   * `.catch(() => null)` fordi en probe som ikke svarer skal gi «kan ikke
   * avgjøres» på sin rad — ikke rive hele diagnosen med seg. `runDiagnostics`
   * derimot får LOV til å kaste: uten rapporten er det ingenting å vise, og en
   * tom liste ville lest som «ingen problemer funnet».
   */
  async function run(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    try {
      const [audio, permissions, ffmpeg, report] = await Promise.all([
        window.api.diagnoseAudio(),
        window.api.mediaPermissions?.().catch(() => null) ??
          Promise.resolve(null),
        window.api.ffmpegHealth?.().catch(() => null) ?? Promise.resolve(null),
        window.api.runDiagnostics(),
      ]);
      setResult({
        report,
        audio,
        micStatus: permissions?.microphone ?? null,
        ffmpeg: ffmpeg
          ? { available: ffmpeg.available, version: ffmpeg.version }
          : null,
        // Leses ETTER de andre kallene med vilje: feilet noe av dette nettopp,
        // hører det med i bildet.
        ipc: window.api.getRecentIpcFailures(),
      });
    } catch (err) {
      setResult(null);
      setFailure(errText(err));
    } finally {
      setBusy(false);
    }
  }

  /**
   * Menylinjens «Diagnostikk» — en blindvei fram til nå.
   *
   * Ruteren har alltid ARMET `run-diagnostics` og navigert til Innstillinger,
   * men ingen flate plukket den opp: man landet på tannhjulet og sto der. Raden
   * er flaten som utfører handlingen, så den er også flaten som tar imot den —
   * samme doktrine som `RecordPage` og `Shell` («de andre blir stående til
   * flaten sin»).
   *
   * Rull FØR kjøringen: knappen skal være synlig mens den står og sier «Kjører
   * …», ellers ser menyvalget ut som at ingenting skjedde.
   */
  const armed = pendingAction.value;
  useEffect(() => {
    if (armed !== "run-diagnostics") return;
    consumePendingAction();
    document
      .getElementById("diagnose")
      ?.scrollIntoView({ block: "start", behavior: "auto" });
    void run();
    // Avhengigheten er HANDLINGEN, ikke `run`: effekten skal fyre når
    // menylinjen ber om det, ikke på hver gjengivelse som lager en ny lukning
    // over den samme tilstanden.
  }, [armed]);

  async function copyReport(): Promise<void> {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.report.markdown);
      toast("success", t("app.diagnose.copied"));
    } catch {
      toast("error", t("app.diagnose.copyFailed"));
    }
  }

  /**
   * Test-opptaket: ~10 s mot enheten som ER valgt.
   *
   * Kommandoen tar ingen enhet — den leser innstillingen selv — så raden lyver
   * ikke om at man kan prøve «en annen». Feiler kallet, sies det som en feil og
   * ikke som et resultat: et oppdiktet `ok: false` ville navngitt en årsak
   * bakenden aldri ga.
   */
  async function runTest(): Promise<void> {
    if (testBusy || live) return;
    setTestBusy(true);
    setTest(null);
    setTestFailure(null);
    try {
      setTest(await window.api.runTestRecording());
    } catch (err) {
      setTestFailure(errText(err));
    } finally {
      setTestBusy(false);
    }
  }

  return (
    <div id="diagnose">
      <SettingRow
        label={t("app.diagnose.title")}
        description={t("app.diagnose.desc")}
        testId="adv-diagnose"
      >
        <Button
          variant="ghost"
          testId="adv-diagnose-run"
          busy={busy}
          onClick={() => void run()}
        >
          {busy ? t("app.diagnose.running") : t("app.diagnose.run")}
        </Button>
        <Button
          variant="ghost"
          testId="adv-diagnose-test"
          busy={testBusy}
          disabled={live}
          disabledReason={t("app.diagnose.testBlocked")}
          onClick={() => void runTest()}
        >
          {testBusy ? t("app.diagnose.testRunning") : t("app.diagnose.test")}
        </Button>
      </SettingRow>

      {failure ? (
        <p class={styles.failure} data-testid="adv-diagnose-failed">
          {tf("app.diagnose.failed", { err: failure })}
        </p>
      ) : null}

      {test || testFailure ? (
        <p class={styles.testResult} data-testid="adv-diagnose-test-result">
          {testFailure
            ? tf("app.diagnose.failed", { err: testFailure })
            : testText(test as TestRecordingResult)}
        </p>
      ) : null}

      {result ? <Result result={result} onCopy={copyReport} /> : null}
    </div>
  );
}

/** Det utfoldede svaret. Egen komponent så `DiagnoseRow` handler om HANDLINGEN
 *  og denne om VISNINGEN — og så testen kan gjengi den uten en kjøring. */
function Result({
  result,
  onCopy,
}: {
  result: DiagnoseResult;
  onCopy: () => Promise<void>;
}) {
  const { report, audio, ipc } = result;
  const inputs = audio.dshow;
  const rows = statusRows({
    inputs,
    storedDevice: settings.value.deviceName ?? null,
    micStatus: result.micStatus,
    ffmpeg: result.ffmpeg,
    captureOk: report.captureOk,
    probeSkipped: report.captureProbeSkipped,
  });

  return (
    <div class={styles.result} data-testid="adv-diagnose-result">
      {/*
        Proben hoppet over: motorens egen grunn, ÆRLIG. En rad som bare sto
        «ikke kjørt» ville latt den viktigste sjekken se ut som en detalj —
        capture-proben er den ene som beviser at enheten faktisk gir lyd.
      */}
      {report.captureProbeSkipped ? (
        <p class={styles.skipped} data-testid="adv-diagnose-probe-skipped">
          {tf("app.diagnose.probeSkipped", { why: report.captureProbeSkipped })}
        </p>
      ) : null}

      <ul class={styles.rows}>
        {rows.map((row) => (
          <StatusLine key={row.id} row={row} />
        ))}
      </ul>

      {report.findings.length ? (
        <ul class={styles.findings} data-testid="adv-diagnose-findings">
          {report.findings.map((f, i) => (
            <Finding key={`${f.code}-${i}`} finding={f} />
          ))}
        </ul>
      ) : null}

      <details class={styles.details} data-testid="adv-diagnose-devices">
        <summary>{t("app.diagnose.devicesTitle")}</summary>
        {inputs.length ? (
          <ul class={styles.deviceList}>
            {inputs.map((name) => (
              <li key={name}>{name}</li>
            ))}
          </ul>
        ) : (
          <p class={styles.empty}>{t("app.diagnose.noDevices")}</p>
        )}
      </details>

      {/*
        Ringen vises bare når den har noe i seg. En tom `<details>` som heter
        «Kommandoer som ikke svarte (0)» er en invitasjon til å lete etter en
        feil som ikke finnes.
      */}
      {ipc.length ? (
        <details class={styles.details} data-testid="adv-diagnose-ipc">
          <summary>{tf("app.diagnose.ipcTitle", { n: ipc.length })}</summary>
          <ul class={styles.ipcList}>
            {ipc.map((f, i) => (
              <li key={`${f.cmd}-${f.at}-${i}`}>
                <code>{f.cmd}</code> — {f.error}
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      <div class={styles.actions}>
        <Button
          variant="ghost"
          testId="adv-diagnose-copy"
          onClick={() => void onCopy()}
        >
          {t("app.diagnose.copy")}
        </Button>
      </div>

      {report.savedTo ? (
        <p class={styles.saved} data-testid="adv-diagnose-saved">
          {t("app.diagnose.savedTo")} <code>{report.savedTo}</code>
        </p>
      ) : null}
    </div>
  );
}

/** Én statusrad: merket, etiketten, verdien. */
function StatusLine({ row }: { row: StatusRow }) {
  const value = [
    row.valueText,
    row.valueSlug ? tDyn("app.diagnose.v", row.valueSlug) : null,
  ]
    .filter(Boolean)
    .join(" — ");
  return (
    <li
      class={styles.row}
      data-testid={`adv-diagnose-row-${row.id}`}
      data-tone={row.tone === null ? "unknown" : row.tone ? "ok" : "bad"}
    >
      <span class={styles.mark} aria-hidden="true">
        {row.tone === null ? "–" : row.tone ? "✓" : "✕"}
      </span>
      <span class={styles.rowLabel}>{tDyn("app.diagnose.r", row.id)}</span>
      <span class={styles.rowValue}>{value}</span>
    </li>
  );
}

/**
 * Ett funn: husets setning når koden er kjent, motorens når den ikke er det.
 *
 * ⚠️ Faktalinja (`detail`) er ALLTID motorens. Den er satt sammen med `format!`
 * av tall som ikke finnes andre steder i rapporten, så den kan ikke oversettes
 * uten at Rust sender råverdiene ved siden av. Den rendres derfor som det den
 * er — diagnostikk — og gjelden er notert til språkrunden.
 */
function Finding({ finding }: { finding: DiagnosticFinding }) {
  const slug = findingSlug(finding.code);
  const title = slug ? tDyn("app.diagnose.f", `${slug}.title`) : finding.title;
  const hint = slug ? tDyn("app.diagnose.f", `${slug}.hint`) : finding.hint;
  return (
    <li
      class={styles.finding}
      data-testid="adv-diagnose-finding"
      data-code={finding.code}
      data-severity={finding.severity}
    >
      <div class={styles.findingHead}>
        <code class={styles.code}>{finding.code}</code>
        <span class={styles.findingTitle}>{title}</span>
      </div>
      <p class={styles.detail}>{finding.detail}</p>
      <p class={styles.hint}>{hint}</p>
    </li>
  );
}

/**
 * Setningen om test-opptaket.
 *
 * En variant vi ikke kjenner viser den RÅ koden i stedet for tom tekst. Den er
 * stygg, men den er noe en frivillig kan lese opp i telefonen — som er hele
 * grunnen til at kodene er stabile.
 */
function testText(r: TestRecordingResult): string {
  if (r.ok) {
    const slug = testSignalSlug(r.signal);
    return tf("app.diagnose.testOk", {
      signal: slug ? tDyn("app.diagnose.testSig", slug) : (r.signal ?? "?"),
    });
  }
  const slug = testErrorSlug(r.error);
  return tf("app.diagnose.testFail", {
    why: slug ? tDyn("app.diagnose.testErr", slug) : (r.error ?? "?"),
  });
}

/** Feilteksten en bruker faktisk kan gi videre. «[object Object]» er ikke en. */
function errText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
