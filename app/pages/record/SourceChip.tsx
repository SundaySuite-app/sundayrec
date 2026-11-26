/**
 * Kilde-brikka øverst på TA OPP — enhet, kanalpar og en mini-måler. Trykk åpner
 * lydarket (`panel: "source"`).
 *
 * De to gule tilstandene beholder sine egne kort (en advarsel med to
 * nødutganger er ikke en kompakt brikke): `record-no-source` og
 * `record-source-missing` er kontrakten resten av appen kjenner dem på.
 */

import { useState } from "preact/hooks";

import { isBuiltInDevice } from "@lib/audio/capture";

import { t, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { audioDevices, loadAudioDevices } from "../../state/devices";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Icon } from "../../ui/Icon/Icon";
import { toast } from "../../ui/toast";
import { defaultDeviceOf, type SourceState } from "./record-core";
import { pickForSource } from "./source-pick";
import styles from "./SourceChip.module.css";
import { StereoMeter } from "./StereoMeter";

export function SourceControl({
  source,
  meter,
}: {
  source: SourceState;
  /** Mini-måleren er av mens det starter/tas opp. */
  meter: boolean;
}) {
  const [searching, setSearching] = useState(false);
  const [switching, setSwitching] = useState(false);
  const fallback = defaultDeviceOf(audioDevices.value);
  const goSound = () =>
    navigate("record", { panel: "source", anchor: "sound" });

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
    <section id="sound" data-testid="control-sound" class={styles.wrap}>
      <button
        type="button"
        data-testid="record-change-source"
        aria-label={t("app.record.sourceChip")}
        title={t("app.record.sourceChipTitle")}
        class={styles.chip}
        onClick={goSound}
      >
        <span aria-hidden="true" class={styles.chipIcon}>
          <Icon name="sliders" size={18} />
        </span>
        <span data-testid="record-source-value" class={styles.name}>
          {isBuiltInDevice(source.name)
            ? t("app.record.builtInShort")
            : source.name}
        </span>
        {source.pair ? (
          <span data-testid="record-source-pair" class={styles.pair}>
            {`${source.pair.l}–${source.pair.r}`}
          </span>
        ) : null}
        {/* Mini-måleren: `off` mens det starter/tas opp — se StereoMeter. */}
        <StereoMeter
          variant="mini"
          off={!meter}
          deviceName={source.name}
          pick={() => pickForSource(settings.peek(), source)}
        />
      </button>
    </section>
  );
}
