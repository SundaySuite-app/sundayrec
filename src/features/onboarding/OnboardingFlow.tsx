import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import type { Settings } from "@/lib/bindings/Settings";
import type { AudioDeviceList } from "@/lib/bindings/AudioDeviceList";
import type { VuLevels } from "@/lib/bindings/VuLevels";
import { SETTINGS_QUERY_KEY } from "@/features/settings/queryKey";
import { Btn } from "@/design/atoms";
import { Icon } from "@/design/Icon";

/**
 * First-run onboarding wizard — mirrors the Electron `onboarding.ts` flow,
 * trimmed to the four steps this phase targets:
 *   1. welcome,
 *   2. pick an audio device,
 *   3. test the audio (live VU off `start_vu`/`vu://levels`),
 *   4. ready.
 *
 * Progress dots, a skip-all action, and a per-step skip are all present. On
 * finish (or skip) it persists `onboardingDone: true` via `settings_save` and
 * dismisses. It only shows on first run (`!settings.onboardingDone`); once the
 * setting flips it renders nothing. Step routing, the device selection, and
 * the VU wiring are exercised in tests with `invoke`/`listen` mocked — only
 * the pixel paint is GUI-UNVERIFIED.
 */

const STEP_COUNT = 4;

/** Floor for the live-test meter — quieter than this reads as "waiting". */
const FLOOR_DBFS = -60;

/** Classify a peak dBFS into a localized signal verdict. */
export function classifySignal(
  db: number | null,
): "waiting" | "weak" | "good" | "loud" | "clip" {
  if (db === null || !Number.isFinite(db) || db <= FLOOR_DBFS) return "waiting";
  if (db >= -3) return "clip";
  if (db >= -12) return "loud";
  if (db >= -40) return "good";
  return "weak";
}

export function OnboardingFlow() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const { data: settings, isLoading } = useQuery<Settings>({
    queryKey: SETTINGS_QUERY_KEY,
    queryFn: () => invoke<Settings>("settings_get"),
  });

  const [step, setStep] = useState(1);
  const [dismissed, setDismissed] = useState(false);
  const [pickedName, setPickedName] = useState<string | null>(null);
  const [seeded, setSeeded] = useState(false);

  // Seed the picked device from the persisted setting once settings arrive.
  useEffect(() => {
    if (settings && !seeded) {
      setPickedName(settings.deviceName ?? null);
      setSeeded(true);
    }
  }, [settings, seeded]);

  const visible =
    !isLoading && !!settings && !settings.onboardingDone && !dismissed;

  const finish = useCallback(async () => {
    setDismissed(true);
    void invoke("stop_vu").catch(() => {});
    if (!settings) return;
    const next: Settings = {
      ...settings,
      onboardingDone: true,
      ...(pickedName ? { deviceName: pickedName } : {}),
    };
    try {
      const saved = await invoke<Settings>("settings_save", { settings: next });
      queryClient.setQueryData(SETTINGS_QUERY_KEY, saved);
    } catch {
      // Persisting onboardingDone failed — the wizard is already dismissed for
      // this session; it will reappear next launch, which is acceptable.
    }
  }, [settings, pickedName, queryClient]);

  if (!visible) return null;

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center bg-bg/80 p-4"
      role="dialog"
      aria-modal="true"
      aria-label={t("wizard.title", "Oppsett")}
    >
      <div className="flex w-full max-w-md flex-col gap-5 rounded-2xl border border-border bg-surface p-6 shadow-[var(--sr-shadow-lg)]">
        {/* Progress dots */}
        <div className="flex justify-center gap-2" aria-hidden>
          {Array.from({ length: STEP_COUNT }).map((_, i) => {
            const n = i + 1;
            return (
              <span
                key={n}
                data-dot={n}
                data-state={n === step ? "active" : n < step ? "done" : "todo"}
                className={`h-2 w-2 rounded-full transition-colors ${
                  n === step
                    ? "bg-accent"
                    : n < step
                      ? "bg-[var(--sr-green)]"
                      : "bg-surface3"
                }`}
              />
            );
          })}
        </div>

        {step === 1 && (
          <StepWelcome
            onNext={() => setStep(2)}
            onSkipAll={() => void finish()}
          />
        )}
        {step === 2 && (
          <StepDevice
            picked={pickedName}
            onPick={setPickedName}
            onNext={() => setStep(3)}
            onSkip={() => setStep(3)}
          />
        )}
        {step === 3 && (
          <StepAudioTest
            deviceName={pickedName}
            onNext={() => setStep(4)}
            onSkip={() => setStep(4)}
          />
        )}
        {step === 4 && <StepReady onDone={() => void finish()} />}
      </div>
    </div>
  );
}

// ── Step 1: Welcome ──────────────────────────────────────────────────────────

function StepWelcome({
  onNext,
  onSkipAll,
}: {
  onNext: () => void;
  onSkipAll: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <div
        style={{
          width: 56,
          height: 56,
          borderRadius: "50%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--sr-gold-tint)",
          border: "1px solid var(--sr-gold-line)",
          color: "var(--sr-gold)",
        }}
      >
        <Icon name="mic" size={26} />
      </div>
      <h2 className="text-xl font-semibold text-text">
        {t("wizard.welcomeTitle", "Velkommen til SundayRec")}
      </h2>
      <p className="text-sm text-text2">
        {t(
          "wizard.welcomeBody",
          "La oss sette opp programmet, slik at alt er klart til søndagen.",
        )}
      </p>
      <div className="mt-1 flex w-full flex-col gap-2">
        <Btn variant="gold" block onClick={onNext}>
          {t("wizard.start", "Kom i gang →")}
        </Btn>
        <button
          type="button"
          className="text-sm text-text3 transition-colors hover:text-text2"
          onClick={onSkipAll}
        >
          {t("wizard.skipAll", "Hopp over — sett opp manuelt")}
        </button>
      </div>
    </div>
  );
}

// ── Step 2: Pick device ──────────────────────────────────────────────────────

function StepDevice({
  picked,
  onPick,
  onNext,
  onSkip,
}: {
  picked: string | null;
  onPick: (name: string) => void;
  onNext: () => void;
  onSkip: () => void;
}) {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);

  const { data: devices } = useQuery<AudioDeviceList>({
    queryKey: ["onboarding", "input-devices"],
    queryFn: () => invoke<AudioDeviceList>("list_input_devices"),
  });

  // Pre-select a non-built-in device (the church-mixer case) if none is set.
  useEffect(() => {
    if (picked || !devices?.inputs.length) return;
    const preferred =
      devices.inputs.find(
        (d) => !/built-in|innebygd|default|standard/i.test(d.name),
      ) ?? devices.inputs[0];
    if (preferred) onPick(preferred.name);
  }, [devices, picked, onPick]);

  const inputs = devices?.inputs ?? [];

  return (
    <div className="flex flex-col gap-4">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-text">
          {t("wizard.deviceTitle", "Hvilken lydenhet bruker dere?")}
        </h2>
        <p className="text-sm text-text2">
          {t(
            "wizard.deviceBody",
            "Velg mikseren eller lydkortet som er koblet til datamaskinen.",
          )}
        </p>
      </div>

      {error && <p className="text-sm text-[var(--sr-red-bright)]">{error}</p>}

      {inputs.length === 0 ? (
        <p
          className="text-sm text-text3"
          style={{
            textAlign: "center",
            padding: "16px 12px",
            borderRadius: "var(--sr-r-sm)",
            background: "var(--sr-line-faint)",
            border: "1px solid var(--sr-line)",
          }}
        >
          {t("wizard.noDevices", "Ingen lydenheter funnet.")}
        </p>
      ) : (
        <ul className="flex max-h-56 flex-col gap-2 overflow-y-auto">
          {inputs.map((d) => {
            const selected = d.name === picked;
            return (
              <li key={d.name}>
                <button
                  type="button"
                  aria-pressed={selected}
                  className={`w-full rounded-lg border px-3 py-2 text-left transition-colors ${
                    selected
                      ? "border-accent-border bg-accent-bg text-text"
                      : "border-border text-text2 hover:bg-surface2 hover:text-text"
                  }`}
                  onClick={() => onPick(d.name)}
                >
                  <span className="block text-sm font-medium">{d.name}</span>
                  <span className="block text-xs text-text3">
                    {d.is_default
                      ? t("wizard.deviceDefault", "Systemets standard")
                      : t("wizard.deviceMixer", "Mikser / lydkort")}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <div className="flex w-full flex-col gap-2">
        <Btn
          variant="gold"
          block
          onClick={() => {
            if (!picked) {
              setError(
                t(
                  "wizard.pickDeviceFirst",
                  "Velg en lydenhet før du fortsetter",
                ),
              );
              return;
            }
            onNext();
          }}
        >
          {t("wizard.useDevice", "Bruk valgt enhet →")}
        </Btn>
        <button
          type="button"
          className="text-sm text-text3 transition-colors hover:text-text2"
          onClick={onSkip}
        >
          {t("wizard.skipStep", "Hopp over dette steget")}
        </button>
      </div>
    </div>
  );
}

// ── Step 3: Audio test (live VU) ─────────────────────────────────────────────

function StepAudioTest({
  deviceName,
  onNext,
  onSkip,
}: {
  deviceName: string | null;
  onNext: () => void;
  onSkip: () => void;
}) {
  const { t } = useTranslation();
  const [levels, setLevels] = useState<VuLevels | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Subscribe to the metering event for the step's lifetime.
  useEffect(() => {
    const unlisten = listen<VuLevels>("vu://levels", (e) =>
      setLevels(e.payload),
    );
    return () => void unlisten.then((off) => off());
  }, []);

  // Start the VU engine on enter; stop it on leave.
  useEffect(() => {
    let cancelled = false;
    invoke("start_vu", { deviceName: deviceName ?? null }).catch((e) => {
      if (!cancelled)
        setError(String((e as { message?: string })?.message ?? e));
    });
    return () => {
      cancelled = true;
      void invoke("stop_vu").catch(() => {});
    };
  }, [deviceName]);

  const peak = useMemo(() => {
    const peaks = levels?.peak_dbfs ?? [];
    return peaks.length ? Math.max(...peaks) : null;
  }, [levels]);

  const verdict = classifySignal(peak);
  const VERDICT_TEXT: Record<typeof verdict, string> = {
    waiting: t("wizard.waiting", "Venter på lyd…"),
    weak: t("home.signalWeak", "Svakt"),
    good: t("home.signalGood", "Bra"),
    loud: t("home.signalLoud", "Høyt"),
    clip: t("home.signalClipping", "Klipper!"),
  };
  const fraction =
    peak === null || peak <= FLOOR_DBFS
      ? 0
      : Math.min(1, Math.max(0, (peak - FLOOR_DBFS) / -FLOOR_DBFS));

  return (
    <div className="flex flex-col gap-4">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-text">
          {t("wizard.testTitle", "Test at lyden fungerer")}
        </h2>
        <p className="text-sm text-text2">
          {t(
            "wizard.testBody",
            "Si noe i mikrofonen — sjekk at indikatoren beveger seg.",
          )}
        </p>
      </div>

      {error && <p className="text-sm text-[var(--sr-red-bright)]">{error}</p>}

      <div className="flex flex-col gap-1">
        <div
          className="h-4 overflow-hidden rounded-lg bg-surface2"
          role="meter"
          aria-label={t("home.audioLevel", "Lydnivå — live")}
          aria-valuenow={Math.round(fraction * 100)}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div
            data-verdict={verdict}
            className={`h-full transition-[width] duration-75 ${
              verdict === "clip"
                ? "bg-[var(--sr-red)]"
                : verdict === "loud"
                  ? "bg-accent"
                  : verdict === "good"
                    ? "bg-[var(--sr-green)]"
                    : "bg-surface3"
            }`}
            style={{ width: `${Math.round(fraction * 100)}%` }}
          />
        </div>
        <p className="text-center text-sm text-text" data-verdict={verdict}>
          {VERDICT_TEXT[verdict]}
        </p>
      </div>

      <div className="flex w-full flex-col gap-2">
        <Btn variant="gold" block onClick={onNext}>
          {t("wizard.audioWorks", "Lyden fungerer →")}
        </Btn>
        <button
          type="button"
          className="text-sm text-text3 transition-colors hover:text-text2"
          onClick={onSkip}
        >
          {t("wizard.skipStep", "Hopp over dette steget")}
        </button>
      </div>
    </div>
  );
}

// ── Step 4: Ready ────────────────────────────────────────────────────────────

function StepReady({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <div
        style={{
          width: 56,
          height: 56,
          borderRadius: "50%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--sr-green-tint)",
          border: "1px solid rgba(52,199,123,0.4)",
          color: "var(--sr-green)",
        }}
      >
        <Icon name="check" size={28} strokeWidth={2.4} />
      </div>
      <h2 className="text-xl font-semibold text-text">
        {t("wizard.readyTitle", "Alt er klart!")}
      </h2>
      <p className="text-sm text-text2">
        {t(
          "wizard.readyBody",
          "SundayRec er klar til å ta opp gudstjenester. Du kan endre alle innstillinger i menyen når som helst.",
        )}
      </p>
      <div className="mt-1 w-full">
        <Btn variant="gold" block onClick={onDone}>
          {t("wizard.open", "Åpne SundayRec →")}
        </Btn>
      </div>
    </div>
  );
}
