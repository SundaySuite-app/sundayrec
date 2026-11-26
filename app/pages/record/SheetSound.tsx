/**
 * Lydarkets øvre del: enhetskort, kanalpar-rutenett og den store L/R-måleren.
 *
 * Et trykk på et enhetskort ER valget — ingen «Bruk denne»-knapp (E4). Et
 * enhetsvalg er likevel tre nøkler som må skrives sammen (`deviceId`,
 * `deviceName` og — for en mikser — `deviceChannels[id]`), så det går gjennom
 * `usePatch`: ÉN lagring, én kvittering, og tilbakerulling om den feiler.
 *
 * Vakten fra `SoundPage` er med: å bytte lydenhet fire minutter før
 * gudstjenesten er endringen som stille koster opptaket, så et BYTTE spør
 * først (`audio.guardDevice`). Å trykke på paret på enheten man allerede bruker
 * er ikke farlig og spør ikke.
 */

import { isBuiltInDevice } from "@lib/audio/capture";

import { t, tf } from "../../i18n";
import { confirmIfRecordingImminent } from "../../settings/guards";
import { usePatch } from "../../settings/use-patch";
import {
  audioDevices,
  loadAudioDevices,
  type AudioDeviceOption,
} from "../../state/devices";
import { reconcilePreroll } from "../../state/preroll";
import { isRecording } from "../../state/recording";
import { settings, type Settings } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { Icon, type IconName } from "../../ui/Icon/Icon";
import { Receipt } from "../../ui/Receipt/Receipt";
import { channelPairFor, channelPairs } from "../settings/decisions-core";
import { pickForSource } from "./source-pick";
import styles from "./SheetSound.module.css";
import { StereoMeter } from "./StereoMeter";
import { useChannelSignals } from "./use-channel-signals";

function iconOf(device: AudioDeviceOption): IconName {
  if (device.channels > 2) return "sliders";
  return isBuiltInDevice(device.name) ? "laptop" : "mic";
}

export function SheetSound() {
  const s = settings.value;
  const devices = audioDevices.value;
  const save = usePatch();

  const stored = (s.deviceId ?? "").trim();
  const device = devices?.find((d) => d.id === stored) ?? null;
  const multi = (device?.channels ?? 0) > 2;
  const pair = device ? channelPairFor(s, device.id) : null;
  const pairL = pair ? pair.l - 1 : 0;
  const lit = useChannelSignals(
    device?.name ?? null,
    device?.channels ?? 0,
    // Samme invariant som måleren: ikke be om enheten mens den tar opp.
    !isRecording.value,
  );

  async function choose(next: AudioDeviceOption): Promise<void> {
    if (save.busy || next.id === stored) return;
    const patch: Partial<Settings> = {
      deviceId: next.id,
      deviceName: next.name,
      // Kanalmodus settes BARE ved bytte: en 1-kanals enhet kan ikke levere
      // stereo (en død høyrekanal), så den tvinges til monoL.
      channels: next.channels === 1 ? "monoL" : "stereo",
    };
    if (next.channels > 2) {
      // Enhetens EGET lagrede par — ikke det forrige valgets.
      const own = channelPairFor(settings.peek(), next.id);
      patch.deviceChannels = {
        ...(settings.peek().deviceChannels ?? {}),
        [next.id]: own
          ? { channelL: own.l - 1, channelR: own.r - 1 }
          : { channelL: 0, channelR: 1 },
      };
    }
    await save.write(patch, {
      confirm: () => confirmIfRecordingImminent(t("audio.guardDevice")),
      // Forhåndsbufferen adresserer enheten ved NAVN — pek den på den nye før
      // noe annet åpner enheten.
      after: async () => {
        await reconcilePreroll(true);
      },
    });
  }

  async function choosePair(left: number): Promise<void> {
    if (save.busy || !device || left === pairL) return;
    await save.write(
      {
        deviceChannels: {
          ...(settings.peek().deviceChannels ?? {}),
          [device.id]: { channelL: left, channelR: left + 1 },
        },
      },
      { after: async () => void (await reconcilePreroll(true)) },
    );
  }

  if (devices !== null && devices.length === 0) {
    return (
      <div data-testid="setup-sound" class={styles.root}>
        <EmptyState
          testId="sound-empty"
          title={t("app.setup.sound.empty")}
          description={t("app.setup.sound.emptyDesc")}
          action={
            <Button
              variant="secondary"
              testId="sound-retry"
              onClick={() => void loadAudioDevices()}
            >
              {t("app.setup.sound.retry")}
            </Button>
          }
        />
      </div>
    );
  }

  return (
    <div data-testid="setup-sound" class={styles.root}>
      <div data-testid="sound-devices" role="group" class={styles.devices}>
        {(devices ?? []).map((d) => (
          <button
            key={d.id}
            type="button"
            data-testid={`sound-device-${d.id}`}
            aria-pressed={d.id === stored}
            aria-label={d.name}
            class={`${styles.card} ${d.id === stored ? styles.on : ""}`}
            onClick={() => void choose(d)}
          >
            <span
              class={d.channels > 2 ? styles.mixer : styles.plain}
              aria-hidden="true"
            >
              <Icon name={iconOf(d)} size={26} />
            </span>
            <span class={styles.name}>
              {isBuiltInDevice(d.name) ? t("app.record.builtInShort") : d.name}
            </span>
          </button>
        ))}
      </div>

      {multi ? (
        <div class={styles.pairs} data-testid="sound-pairs">
          <span class={styles.caption}>{t("app.sheet.channels")}</span>
          <div class={styles.pairGrid}>
            {channelPairs(device?.channels ?? 0).map((left) => {
              const live = lit[left] || lit[left + 1];
              return (
                <button
                  key={left}
                  type="button"
                  data-testid={`sound-pair-${left + 1}`}
                  data-live={live ? "true" : undefined}
                  aria-pressed={pairL === left}
                  aria-label={tf("app.setup.sound.pairLabel", {
                    l: left + 1,
                    r: left + 2,
                  })}
                  class={`${styles.pair} ${pairL === left ? styles.on : ""}`}
                  onClick={() => void choosePair(left)}
                >
                  <span>{`${left + 1}–${left + 2}`}</span>
                  <span aria-hidden="true" class={styles.leds}>
                    <span class={live ? styles.ledOn : styles.led} />
                    <span class={live ? styles.ledOn : styles.led} />
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      ) : null}

      <div class={styles.meterCard}>
        <StereoMeter
          testId="sound-vu"
          on="card"
          // AVSLÅTT mens det tas opp og når ingen enhet er valgt.
          off={isRecording.value || !device}
          deviceName={device?.name ?? null}
          pick={() =>
            device
              ? pickForSource(settings.peek(), {
                  pair: multi ? { l: pairL + 1, r: pairL + 2 } : null,
                })
              : { mode: "stereo", chL: 0, chR: 1 }
          }
        />
        <span class={styles.hint}>{t("app.setup.sound.testHint")}</span>
      </div>
      <Receipt state={save.receipt} testId="sound-receipt" />
    </div>
  );
}
