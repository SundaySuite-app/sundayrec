/**
 * Første gang — tre hintbobler oppå TA OPP («Én flate»).
 *
 *   1. «Velg lydkilde»  under kilde-brikka (brikka pulserer gull)
 *   2. «Velg mappe»     lydarket åpnes med «Mer» utfoldet, mappe-raden pulserer
 *   3. «Faste opptak»   over kalenderknappen (eller stripa)
 *
 * Ingen veiviser: boblene peker på de ekte kontrollene, og alt er trykkbart
 * mens de står. Å åpne brikka og lydarket er tillatt under steg 1 — det er
 * nettopp det boblen ber om.
 *
 * ## Porten på steg 1
 *
 * «Neste» er sperret til appen HØRER lyd. Det er den ene tingen som ikke kan
 * repareres etterpå: en gudstjeneste tatt opp fra feil inngang er borte. Porten
 * har en nødutgang — «Fortsett uten lyd» — fordi en port uten utgang er en app
 * som ikke kan brukes på en maskin der mikseren ikke er slått på ennå. Reglene
 * bor i `first-run-core.ts` (tabelltestet); her er bare knappene.
 *
 * ⚠️ Sperret betyr `aria-disabled` + en GRUNN (`title`), ikke en grå knapp med
 * `disabled`: en tastaturbruker skal kunne nå knappen for å høre hvorfor.
 *
 * ## «Ferdig» og krysset
 *
 * Begge setter `onboardingDone` — lagre, og rull tilbake med en toast hvis
 * lagringen feiler. En «ferdig» som ikke ble lagret betyr at hintene kommer
 * tilbake ved neste oppstart, og da er det bedre å si fra nå enn å la dem dukke
 * opp igjen uten forklaring.
 *
 * ## Tilgjengelighet
 *
 * Boblens tekst står i en `aria-live="polite"`-region, og fokus flyttes til
 * boblens hovedknapp når steget skifter (etter at `PageShell` har flyttet det
 * til overskriften). Bevegelse følger `prefers-reduced-motion` globalt
 * (`styles/base.css`).
 *
 * ## Plassering
 *
 * Boblen er `position: fixed` og regner koordinatene fra målets rektangel, som
 * leses på nytt hvert 200. ms: lydarket glir inn i 420 ms, og mappe-raden finnes
 * ikke i treet før det er åpent.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { t, tDyn, tf } from "../../i18n";
import { navigate, panel, route } from "../../router/router";
import { audioDevices } from "../../state/devices";
import { isRecording, recordStarting } from "../../state/recording";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Icon, type IconName } from "../../ui/Icon/Icon";
import { toast } from "../../ui/toast";
import { useVuWord } from "../settings/use-vu-word";
import {
  dots,
  FIRST_RUN_STEP_COUNT,
  isGatedStep,
  soundGateOpen,
  stepAt,
  type FirstRunStepId,
} from "./first-run-core";
import {
  hintIndex,
  hintsActive,
  resetHints,
  soundSkipped,
} from "./first-run-hint";
import styles from "./FirstRunHints.module.css";

/** Elementet hver boble peker på. */
const TARGET: Record<FirstRunStepId, string> = {
  source: '[data-testid="control-sound"]',
  folder: '[data-testid="setup-folder"]',
  plan: '[data-testid="record-next-plan"]',
};

/** Pila i boblen peker mot målet. */
const ARROW: Record<FirstRunStepId, IconName> = {
  source: "arrow-up",
  folder: "arrow-right",
  plan: "arrow-down",
};

/** Luften mellom boblen og målet (px). */
const GAP = 14;

/** Lagringen pågår — modulnivå, så en dobbelttrykk-«Ferdig» ikke lagrer to ganger. */
let finishing = false;

export function FirstRunHints() {
  // Bare på TA OPP, og aldri oppå et opptak eller tannhjulet.
  if (!hintsActive.value || route.value.page !== "record") return null;
  if (isRecording.value) return null;
  return <Bubble />;
}

interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Målets rektangel, lest på nytt jevnlig. `null` når det ikke finnes (ennå). */
function useTargetBox(selector: string): Box | null {
  const [box, setBox] = useState<Box | null>(null);
  useEffect(() => {
    const read = (): void => {
      const el = document.querySelector(selector);
      if (!el) return setBox((prev) => (prev === null ? prev : null));
      const r = el.getBoundingClientRect();
      setBox((prev) =>
        prev &&
        prev.left === r.left &&
        prev.top === r.top &&
        prev.right === r.right &&
        prev.bottom === r.bottom
          ? prev
          : { left: r.left, top: r.top, right: r.right, bottom: r.bottom },
      );
    };
    read();
    const id = setInterval(read, 200);
    window.addEventListener("resize", read);
    return () => {
      clearInterval(id);
      window.removeEventListener("resize", read);
    };
  }, [selector]);
  return box;
}

/** CSS-posisjonen for en boble, regnet fra målet. */
function place(id: FirstRunStepId, box: Box | null): string {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  if (!box) {
    // Målet er ikke i treet ennå (lydarket glir inn): midt øverst, som skissen.
    return "top:92px;left:50%;transform:translateX(-50%)";
  }
  const cx = (box.left + box.right) / 2;
  const cy = (box.top + box.bottom) / 2;
  if (id === "source") {
    const x = Math.min(Math.max(cx, 176), vw - 176);
    return `top:${box.bottom + GAP}px;left:${x}px;transform:translateX(-50%)`;
  }
  if (id === "folder") {
    const maxW = Math.max(box.left - GAP - 12, 200);
    return `top:${cy}px;right:${vw - box.left + GAP}px;max-width:${maxW}px;transform:translateY(-50%)`;
  }
  return `bottom:${vh - box.top + GAP}px;left:${Math.max(box.left, 12)}px`;
}

function Bubble() {
  const index = hintIndex.value;
  const { step, id } = stepAt(index);
  const last = step === FIRST_RUN_STEP_COUNT;
  const s = settings.value;
  const nextRef = useRef<HTMLButtonElement | null>(null);

  // Porten lytter bare på steg 1, og bare når en enhet FINNES å lytte på.
  const chosen = (s.deviceId ?? "").trim();
  const found = !!chosen && !!audioDevices.value?.some((d) => d.id === chosen);
  // Av når opptaket STARTER: Rust slipper VU-strømmen i `start_recording`, og en
  // `start_vu` etter det ber om enheten opptaket er i ferd med å åpne.
  const vuWord = useVuWord(
    s.deviceName,
    isGatedStep(index) && found && !recordStarting.value,
  );
  const gated = isGatedStep(index);
  const gateOpen = !gated || soundGateOpen(vuWord, soundSkipped.value);

  const box = useTargetBox(TARGET[id]);

  // Steget bestemmer hva TA OPP viser: mappe-steget åpner lydarket med «Mer»
  // utfoldet; kalender-steget lukker arket så knappen står fri.
  useEffect(() => {
    if (id === "folder") navigate("record", { panel: "source-more" });
    else if (id === "plan" && panel.peek()) navigate("record");
  }, [id]);

  // Fokus til boblens hovedknapp — ETTER at `PageShell` har flyttet det til
  // overskriften (barn-effekter kjører før foreldrenes).
  useEffect(() => {
    const timer = setTimeout(() => nextRef.current?.focus(), 60);
    return () => clearTimeout(timer);
  }, [index]);

  function go(next: number): void {
    hintIndex.value = stepAt(next).step - 1;
  }

  async function finish(): Promise<void> {
    if (finishing) return;
    finishing = true;
    try {
      patchSettings({ onboardingDone: true });
      const ok = await saveSettingsDebounced(120);
      if (!ok) {
        // Rull tilbake OG si fra: hintene står igjen, og neste oppstart ville
        // uansett vist dem.
        patchSettings({ onboardingDone: false });
        toast("error", t("general.saveFailed"));
        return;
      }
      resetHints();
      navigate("record");
    } finally {
      finishing = false;
    }
  }

  const text = tDyn("app.first.hint", id);

  return (
    <div
      role="group"
      aria-label={t("app.first.hintLabel")}
      data-testid="hint-bubble"
      data-step={id}
      // Lydarket fanger Tab; boblen er med i runden (`sheet-behavior.ts`).
      data-focus-companion=""
      class={styles.bubble}
      style={place(id, box)}
    >
      <button
        type="button"
        class={styles.skip}
        data-testid="hint-skip"
        aria-label={t("app.first.skip")}
        title={t("app.first.skip")}
        onClick={() => void finish()}
      >
        <Icon name="x" size={20} strokeWidth={2} />
      </button>

      <span class={styles.line}>
        <span aria-hidden="true" class={styles.arrow}>
          <Icon name={ARROW[id]} size={20} strokeWidth={2} />
        </span>
        <span aria-live="polite" data-testid="hint-text">
          {text}
        </span>
      </span>

      {gated && !gateOpen ? (
        <p data-testid="hint-gate" class={styles.gate}>
          {t("app.first.hintGate")}
        </p>
      ) : null}

      <span class={styles.actions}>
        <ol
          aria-label={tf("app.first.step", {
            n: step,
            total: FIRST_RUN_STEP_COUNT,
          })}
          data-testid="hint-dots"
          class={styles.dots}
        >
          {dots(index).map((state, i) => (
            <li key={i} data-state={state} class={styles.dot} />
          ))}
        </ol>
        <span class={styles.buttons}>
          {gated && !soundSkipped.value ? (
            <button
              type="button"
              class={styles.link}
              data-testid="hint-skip-sound"
              onClick={() => {
                soundSkipped.value = true;
                go(index + 1);
              }}
            >
              {t("app.first.skipSound")}
            </button>
          ) : null}
          <button
            ref={nextRef}
            type="button"
            class={styles.next}
            data-testid="hint-next"
            aria-disabled={gateOpen ? undefined : "true"}
            title={gateOpen ? undefined : t("app.first.gateReason")}
            onClick={() => {
              if (!gateOpen) return;
              if (last) void finish();
              else go(index + 1);
            }}
          >
            {last ? t("app.first.done") : t("app.first.next")}
          </button>
        </span>
      </span>
    </div>
  );
}
