/**
 * «Avansert: åpne mikseren» — hele lydteknikk-bordet, bak én lenke.
 *
 * Atlasets §4.6 er grunnen til at det ER bak en lenke: `Lavkutt (HPF)`,
 * `Støygulv −60…−10 dB`, `Romdemping (tilnærmet)`, `De-esser`, `3.5 kHz
 * (nærvær)` — tjue kontroller en frivillig ikke kan rangere, midt i skjermen
 * hun skal eksportere fra. Og de var **uten i18n i det hele tatt**: tjuetre
 * norske etiketter i en app som sendes i sju språk. De har nøkler nå.
 *
 * ## Mikseren ERSTATTER profilen, den legger seg ikke oppå
 *
 * Legacy lot begge stå på samtidig — `processing` (mikserens kjede) OG
 * `masterPreset` (mastringens kjede) i den samme nyttelasten — og resultatet
 * var to høypass, to kompressorer og to EQ-kurver over det samme materialet.
 * Bakenden advarer mot nøyaktig den stablingen, med ord, i `auto_process`.
 *
 * Så her er regelen én linje lang, og den STÅR i panelet: er mikseren på, er
 * det mikserens kjede som gjelder, og ingen mastring legges under. Den som har
 * åpnet dette bordet har tatt over.
 *
 * ⚠️ Kanalreparasjonen rir fortsatt med — mikseren har ingen kanalkontroll, og
 * en stille venstrekanal er ikke en smakssak.
 *
 * ## Alt er portet, ingenting er kuttet
 *
 * Sju trinn, tretten glidere, tre EQ-bånd og sluttgain — de samme feltene,
 * områdene og trinnene som `@lib/pages/editor/mixer`, som selv
 * speiler `EditorProcessing` i Rust. Startverdiene importeres DERFRA
 * (`defaultProcessing`) i stedet for å skrives på nytt: det ene stedet
 * `VocalChain::default()` finnes i TypeScript skal fortsette å være ett sted.
 */

import type { Processing } from "@lib/pages/editor/mixer";

import { t, tDyn, tf } from "../i18n";
import { Card } from "../ui/Card/Card";
import { Slider } from "../ui/Slider/Slider";
import { Toggle } from "../ui/Toggle/Toggle";
import { mixer, patchMixer, setEqBand, soundProfile, useMixer } from "./sound";
import styles from "./editor.module.css";

/** Enheter, ikke prosa — samme regel som `dBFS` i S1b. */
const HZ = "Hz";
const DB = "dB";
const MS = "ms";

type NumericKey = {
  [K in keyof Processing]: Processing[K] extends number ? K : never;
}[keyof Processing];

type BoolKey = {
  [K in keyof Processing]: Processing[K] extends boolean ? K : never;
}[keyof Processing];

interface SliderSpec {
  /** Nøkkelsuffikset under `app.editor.mx`. */
  id: string;
  key: NumericKey;
  min: number;
  max: number;
  step: number;
  unit?: string;
}

interface StageSpec {
  id: string;
  enableKey: BoolKey;
  sliders: readonly SliderSpec[];
}

/** Trinnene, ordrett legacys `STAGES` — samme rekkefølge, samme områder. */
const STAGES: readonly StageSpec[] = [
  {
    id: "hpf",
    enableKey: "highpassEnabled",
    sliders: [
      { id: "freq", key: "highpassHz", min: 40, max: 200, step: 5, unit: HZ },
    ],
  },
  {
    id: "denoise",
    enableKey: "denoiseEnabled",
    sliders: [
      { id: "reduction", key: "denoiseDb", min: 0, max: 40, step: 1, unit: DB },
      {
        id: "floor",
        key: "denoiseFloorDb",
        min: -60,
        max: -10,
        step: 1,
        unit: DB,
      },
    ],
  },
  {
    id: "dereverb",
    enableKey: "dereverbEnabled",
    sliders: [
      { id: "strength", key: "dereverbStrength", min: 0, max: 1, step: 0.05 },
    ],
  },
  {
    id: "gate",
    enableKey: "gateEnabled",
    sliders: [
      {
        id: "threshold",
        key: "gateThresholdDb",
        min: -70,
        max: -10,
        step: 1,
        unit: DB,
      },
      { id: "ratio", key: "gateRatio", min: 1, max: 10, step: 0.5 },
    ],
  },
  {
    id: "comp",
    enableKey: "compEnabled",
    sliders: [
      {
        id: "threshold",
        key: "compThresholdDb",
        min: -40,
        max: 0,
        step: 1,
        unit: DB,
      },
      { id: "ratio", key: "compRatio", min: 1, max: 12, step: 0.5 },
      {
        id: "attack",
        key: "compAttackMs",
        min: 1,
        max: 100,
        step: 1,
        unit: MS,
      },
      {
        id: "release",
        key: "compReleaseMs",
        min: 10,
        max: 500,
        step: 10,
        unit: MS,
      },
      {
        id: "makeup",
        key: "compMakeupDb",
        min: 0,
        max: 12,
        step: 0.5,
        unit: DB,
      },
    ],
  },
  {
    id: "deesser",
    enableKey: "deesserEnabled",
    sliders: [
      { id: "intensity", key: "deesserIntensity", min: 0, max: 1, step: 0.05 },
    ],
  },
  {
    id: "limiter",
    enableKey: "limiterEnabled",
    sliders: [
      { id: "ceiling", key: "limiterDb", min: -6, max: 0, step: 0.5, unit: DB },
    ],
  },
];

/** De tre EQ-båndene mikseren tilbyr — mudder, nærvær, luft. Legacys egne. */
const EQ_BANDS = [
  { id: "eqMud", freqHz: 250, q: 1.0 },
  { id: "eqPresence", freqHz: 3500, q: 1.0 },
  { id: "eqAir", freqHz: 10000, q: 1.5 },
] as const;

const EQ_MIN = -12;
const EQ_MAX = 12;
const EQ_STEP = 0.5;

function withUnit(value: number, unit?: string): string {
  return unit ? `${value} ${unit}` : String(value);
}

export function MixerPanel() {
  const p = mixer.value;
  const on = useMixer.value;

  return (
    <Card testId="editor-mixer" title={t("app.editor.mixerTitle")}>
      <div class={styles.autoRow}>
        <div class={styles.autoGrow}>
          <b id="editor-mixer-label" class={styles.autoTitle}>
            {t("app.editor.mixerUse")}
          </b>
          <div id="editor-mixer-desc" class={styles.hint}>
            {tf("app.editor.mixerUseDesc", {
              profile: tDyn("app.editor.profile", soundProfile.value),
            })}
          </div>
        </div>
        <Toggle
          testId="editor-mixer-toggle"
          checked={on}
          labelId="editor-mixer-label"
          describedBy="editor-mixer-desc"
          onChange={(next) => (useMixer.value = next)}
        />
      </div>

      {/* Kontrollene står alltid — de er en BESKRIVELSE av kjeden også når den
          ikke er i bruk, og en frivillig som skrur på bryteren skal se hva hun
          nettopp skrudde på. Dempet, ikke borte. */}
      <div
        data-testid="editor-mixer-stages"
        class={on ? styles.stages : `${styles.stages} ${styles.stagesOff}`}
      >
        {STAGES.map((stage) => (
          <div key={stage.id} data-testid={`editor-mx-${stage.id}`}>
            <div class={styles.autoRow}>
              <b id={`editor-mx-${stage.id}-label`} class={styles.stageTitle}>
                {tDyn("app.editor.mx", stage.id)}
              </b>
              <Toggle
                testId={`editor-mx-${stage.id}-on`}
                checked={p[stage.enableKey]}
                disabled={!on}
                labelId={`editor-mx-${stage.id}-label`}
                onChange={(next) =>
                  patchMixer({ [stage.enableKey]: next } as Partial<Processing>)
                }
              />
            </div>
            {stage.sliders.map((s) => (
              <div key={s.id} class={styles.sliderRow}>
                <span id={`editor-mx-${stage.id}-${s.id}`} class={styles.hint}>
                  {tDyn("app.editor.mx", s.id)}
                </span>
                <Slider
                  testId={`editor-mx-${stage.id}-${s.id}-slider`}
                  value={p[s.key]}
                  min={s.min}
                  max={s.max}
                  step={s.step}
                  disabled={!on}
                  labelId={`editor-mx-${stage.id}-${s.id}`}
                  format={(v) => withUnit(v, s.unit)}
                  onChange={(next) =>
                    patchMixer({ [s.key]: next } as Partial<Processing>)
                  }
                />
              </div>
            ))}
          </div>
        ))}

        <div data-testid="editor-mx-eq">
          <b class={styles.stageTitle}>{t("app.editor.mxEq")}</b>
          {EQ_BANDS.map((band) => {
            const gain =
              p.eq.find((b) => b.freqHz === band.freqHz)?.gainDb ?? 0;
            return (
              <div key={band.id} class={styles.sliderRow}>
                <span id={`editor-mx-${band.id}`} class={styles.hint}>
                  {tDyn("app.editor.mx", band.id)}
                </span>
                <Slider
                  testId={`editor-mx-${band.id}-slider`}
                  value={gain}
                  min={EQ_MIN}
                  max={EQ_MAX}
                  step={EQ_STEP}
                  disabled={!on}
                  labelId={`editor-mx-${band.id}`}
                  format={(v) => withUnit(v, DB)}
                  onChange={(next) => setEqBand(band.freqHz, next, band.q)}
                />
              </div>
            );
          })}
        </div>

        <div class={styles.sliderRow}>
          <span id="editor-mx-gain" class={styles.hint}>
            {tDyn("app.editor.mx", "gain")}
          </span>
          <Slider
            testId="editor-mx-gain-slider"
            value={p.gainDb}
            min={EQ_MIN}
            max={EQ_MAX}
            step={EQ_STEP}
            disabled={!on}
            labelId="editor-mx-gain"
            format={(v) => withUnit(v, DB)}
            onChange={(next) => patchMixer({ gainDb: next })}
          />
        </div>
      </div>
    </Card>
  );
}
