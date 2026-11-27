/**
 * Vekking av maskinen — popoveren bak hjørneknappen på «Vekk»-flisen i
 * ukestripa. Logikken kommer fra det gamle `WakeCard` (PLAN): setningen om hva
 * DENNE maskinen kan, «Legg inn vekking», «Prøv vekking om 2 min» + «Avbryt»,
 * og loggen over feilede vekkinger med «Tøm».
 *
 * ## Hvorfor de tre knappene MÅ finnes
 *
 * Bryteren (flisen) skriver bare en intensjon. Planleggeren armerer OS-vekkingen
 * i sin egen stille runde, men den kan ikke be om et administratorpassord, og på
 * macOS er det nøyaktig det arming koster. `wake_reschedule` er bakendens EGEN
 * vei rundt det (`allow_admin = true`): uten «Legg inn vekking» fantes den
 * veien ikke i skallet, og bryteren sto på «på» mens OS-et ikke hadde noen
 * vekking. Svaret vises som en setning, ikke en toast — «trenger
 * administratorpassord» skal kunne leses om igjen mens man leter etter
 * passordet.
 *
 * Testvekkingen bor VED SIDEN AV den ekte (egen `pmset`-eier, egen timer-slot),
 * så den er trygg å trykke på en lørdag. At maskinen FAKTISK våkner kan bare en
 * rigg bevise: loggen er derfor ofte tom, og sier det ærlig i stedet for å la
 * et hull bety «alt gikk bra».
 *
 * `knownIssues`/`recommendations` kommer som KODER (`WakeIssue`/
 * `WakeRecommendation`); setningen står i katalogen på begge språk.
 */

import { useEffect, useState } from "preact/hooks";

import type { TestWakeResult } from "@legacy/bindings/TestWakeResult";
import type { WakeFailureEntry } from "@legacy/bindings/WakeFailureEntry";
import type { WakeIssue } from "@legacy/bindings/WakeIssue";
import type { WakeRecommendation } from "@legacy/bindings/WakeRecommendation";

import { locale, t, tDyn, tf } from "../../i18n";
import { refreshWakeAfterReschedule } from "../../state/next-recording";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { toast } from "../../ui/toast";
import {
  testWakeWord,
  wakeArmWord,
  wakeWord,
  type TestWakeWord,
  type WakeArmResult,
  type WakeFacts,
} from "../plan/slots-core";
import styles from "./WakePopover.module.css";

/** Sekundene «Prøv vekking om 2 min» ber `wake_test` planlegge fram i tid —
 *  navnet på knappen OG tallet den sender, samlet ett sted. */
const TEST_WAKE_SECONDS_AHEAD = 120;

export function WakePopover() {
  return (
    <div class={styles.pop} data-testid="strip-wake-pop">
      <ArmSection />
      <TestSection />
    </div>
  );
}

function ArmSection() {
  const [facts, setFacts] = useState<WakeFacts | null>(null);
  const [issues, setIssues] = useState<readonly WakeIssue[]>([]);
  const [advice, setAdvice] = useState<readonly WakeRecommendation[]>([]);
  const [armResult, setArmResult] = useState<WakeArmResult | null>(null);
  const [arming, setArming] = useState(false);
  const on = settings.value.wakeFromSleep !== false;

  useEffect(() => {
    void window.api
      .wakeDetectCapabilities()
      .then((caps) => {
        setFacts({
          canWakeFromSleep: caps.canWakeFromSleep === true,
          needsAdmin: caps.needsAdmin === true,
        });
        setIssues(caps.knownIssues ?? []);
        setAdvice(caps.recommendations ?? []);
      })
      // En probe vi ikke fikk kjørt er ikke bevis i noen retning.
      .catch(() => setFacts(null));
  }, []);

  async function arm(): Promise<void> {
    if (arming) return;
    setArming(true);
    try {
      const result = await window.api.wakeReschedule();
      setArmResult(result);
      // ÉN vekking registrert er en kvittering; NULL er det ikke. Stripa leser
      // `wake_verify` på nytt etter en vellykket armering.
      if (result.ok && (result.count ?? 0) > 0) {
        await refreshWakeAfterReschedule();
      }
    } catch (err) {
      console.warn("[schedule] wake_reschedule failed:", err);
      setArmResult({ ok: false, reason: "error", count: null });
    } finally {
      setArming(false);
    }
  }

  return (
    <section class={styles.section}>
      <p data-testid="strip-wake-caps" class={styles.sentence}>
        {tDyn("app.setup.advanced.wakeWord", wakeWord(facts))}
      </p>
      {issues.length > 0 || advice.length > 0 ? (
        <ul class={styles.notes} data-testid="strip-wake-notes">
          {issues.map((code) => (
            <li key={code} data-code={code}>
              {tDyn("app.setup.advanced.wakeIssue", code)}
            </li>
          ))}
          {advice.map((code) => (
            <li key={code} data-code={code}>
              {tDyn("app.setup.advanced.wakeAdvice", code)}
            </li>
          ))}
        </ul>
      ) : null}
      <div class={styles.row}>
        <Button
          variant="secondary"
          busy={arming}
          // Grået ut når bryteren er av: å registrere en vekking for en
          // innstilling som sier «ikke vekk meg» har ingen mening.
          disabled={!on}
          disabledReason={t("app.setup.advanced.wakeArmWord.disabled")}
          testId="strip-wake-arm"
          onClick={() => void arm()}
        >
          {t("app.setup.advanced.wakeArm")}
        </Button>
      </div>
      <p data-testid="strip-wake-arm-word" class={styles.sentence}>
        {tDyn("app.setup.advanced.wakeArmWord", wakeArmWord(armResult))}
      </p>
    </section>
  );
}

function TestSection() {
  const [testResult, setTestResult] = useState<TestWakeResult | null>(null);
  const [testBusy, setTestBusy] = useState(false);
  const [history, setHistory] = useState<WakeFailureEntry[] | null>(null);
  const [historyBusy, setHistoryBusy] = useState(false);
  const live = isRecording.value;

  useEffect(() => {
    void window.api
      .wakeFailureHistory()
      .then(setHistory)
      // Raden har ingen tredje tilstand; en logg som aldri slutter å laste er
      // verre enn en som (feilaktig) sier «ingen hendelser ennå».
      .catch(() => setHistory([]));
  }, []);

  async function runTest(): Promise<void> {
    if (testBusy || live) return;
    setTestBusy(true);
    try {
      setTestResult(await window.api.wakeTest(TEST_WAKE_SECONDS_AHEAD));
    } catch (err) {
      console.warn("[schedule] wake_test failed:", err);
      setTestResult({
        ok: false,
        jobId: null,
        scheduledAt: null,
        reason: "error",
      });
    } finally {
      setTestBusy(false);
    }
  }

  async function cancelTest(): Promise<void> {
    if (testBusy) return;
    setTestBusy(true);
    try {
      await window.api.wakeCancelTest();
    } catch (err) {
      console.warn("[schedule] wake_cancel_test failed:", err);
    } finally {
      // Best-effort ved kontrakt: raden går tilbake til «Prøv» uansett.
      setTestResult(null);
      setTestBusy(false);
    }
  }

  async function clearHistory(): Promise<void> {
    if (historyBusy || !history || history.length === 0) return;
    setHistoryBusy(true);
    try {
      await window.api.wakeClearFailureHistory();
      setHistory([]);
    } catch (err) {
      console.warn("[schedule] wake_clear_failure_history failed:", err);
      toast("error", t("general.saveFailed"));
    } finally {
      setHistoryBusy(false);
    }
  }

  const word = testWakeWord(testResult);
  const armed = word === "scheduled";

  return (
    <section class={styles.section}>
      <div class={styles.row}>
        {armed ? (
          <Button
            variant="ghost"
            busy={testBusy}
            testId="strip-wake-cancel"
            onClick={() => void cancelTest()}
          >
            {t("app.dialog.cancel")}
          </Button>
        ) : (
          <Button
            variant="secondary"
            busy={testBusy}
            disabled={live}
            disabledReason={t("app.setup.advanced.wakeTestBlocked")}
            testId="strip-wake-test"
            onClick={() => void runTest()}
          >
            {t("app.setup.advanced.wakeTest")}
          </Button>
        )}
      </div>
      <p data-testid="strip-wake-test-word" class={styles.sentence}>
        {testWakeSentence(word, testResult)}
      </p>

      <div class={styles.historyHead}>
        <span class={styles.historyTitle}>
          {t("app.setup.advanced.wakeHistoryTitle")}
        </span>
        {history && history.length > 0 ? (
          <Button
            variant="ghost"
            busy={historyBusy}
            testId="strip-wake-clear"
            onClick={() => void clearHistory()}
          >
            {t("app.setup.advanced.wakeHistoryClear")}
          </Button>
        ) : null}
      </div>
      {history && history.length > 0 ? (
        <ul class={styles.notes} data-testid="strip-wake-history-list">
          {history.map((entry, i) => (
            // `timestamp` alene er ikke stabil nok til en `key`.
            <li key={`${entry.timestamp}-${i}`}>{wakeHistoryLine(entry)}</li>
          ))}
        </ul>
      ) : (
        <p class={styles.sentence} data-testid="strip-wake-history-empty">
          {t("app.setup.advanced.wakeHistoryEmpty")}
        </p>
      )}
    </section>
  );
}

/** «Planlagt til kl. 10:52 …», eller den ærlige setningen for et utfall som
 *  ikke lyktes — de fire feilordene siterer `wakeArmWord`s katalog. */
function testWakeSentence(
  word: TestWakeWord,
  result: TestWakeResult | null,
): string {
  if (word === "idle") return t("app.setup.advanced.wakeTestIdle");
  if (word === "scheduled") {
    return tf("app.setup.advanced.wakeTestScheduled", {
      time: result?.scheduledAt ? formatClock(result.scheduledAt) : "—",
    });
  }
  return tDyn("app.setup.advanced.wakeArmWord", word);
}

/** Bakendens zone-løse lokale ISO → «10:52», i appens språk. */
function formatClock(iso: string): string {
  return new Date(iso).toLocaleTimeString(locale.value, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Dag, dato og klokkeslett: loggen holder tapte søndager ved siden av testene. */
function formatWhen(iso: string): string {
  return new Date(iso).toLocaleString(locale.value, {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Én logglinje: «søn. 6. sep., 11:00 — Gikk glipp av en planlagt vekking». */
function wakeHistoryLine(entry: WakeFailureEntry): string {
  const time = formatWhen(entry.scheduledAt);
  const reason = entry.reason
    ? ` (${wakeHistoryReasonWord(entry.reason)})`
    : "";
  return `${time} — ${wakeHistoryKindWord(entry.kind)}${reason}`;
}

function wakeHistoryKindWord(kind: WakeFailureEntry["kind"]): string {
  switch (kind) {
    case "test_ok":
      return t("app.setup.advanced.wakeHistoryKind.testOk");
    case "test_fail":
      return t("app.setup.advanced.wakeHistoryKind.testFail");
    default:
      return t("app.setup.advanced.wakeHistoryKind.missed");
  }
}

/** De tre grunnene `test_wake_outcome` faktisk skriver. En grunn vi ikke har et
 *  ord for er fortsatt DATA: den rå strengen sier mer enn stillhet. */
function wakeHistoryReasonWord(reason: string): string {
  switch (reason) {
    case "no_resume":
      return t("app.setup.advanced.wakeHistoryReason.noResume");
    case "too_late":
      return t("app.setup.advanced.wakeHistoryReason.tooLate");
    case "on_battery":
      return t("app.setup.advanced.wakeHistoryReason.onBattery");
    default:
      return reason;
  }
}
