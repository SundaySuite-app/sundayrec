/**
 * Tillegg — «Ta med kamera».
 *
 * Tre kontroller: av/på, hvilket kamera, og om lydfila skal ligge ved siden av
 * MP4-en. Oppløsning, bildefrekvens, container, kodek, koder-backend og bitrate
 * VAR innstillinger her; siden v0.15 er de konstanter i
 * `crates/sundayrec-core/src/capture.rs` (1080p / 30 fps / mp4 / H.264). En
 * frivillig kan ikke sette feil det som ikke er en innstilling.
 *
 * ## Kortet utvider seg, det har ingen «Endre»
 *
 * Se toppen av `Level1.tsx`: canvasens innledning til sett 5 sier at
 * tilleggene «utvider siden når de slås på», og en «Endre»-knapp ved siden av
 * rader som allerede står åpne ville ikke hatt noe å gjøre.
 *
 * ## Kameravalget går utenom `useSetting`
 *
 * Det skriver TO nøkler: `videoDeviceName` (som bakenden matcher på) og
 * `videoDeviceIndex` (avfoundation-indeksen, reserven når navneoppslaget
 * bommer etter en gjeninnkobling). To `useSetting` ville gitt to skrivninger og
 * en base som et øyeblikk har det ene kameraets navn og det andres indeks.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tf } from "../../i18n";
import { confirmIfRecordingImminent } from "../../settings/guards";
import { loadVideoDevices, videoDevices } from "../../state/devices";
import { usePatch } from "../../settings/use-patch";
import { settings } from "../../state/settings";
import { BoundToggle } from "../../ui/Bound/Bound";
import { Card } from "../../ui/Card/Card";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { Receipt } from "../../ui/Receipt/Receipt";
import { Select } from "../../ui/Select/Select";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Toggle } from "../../ui/Toggle/Toggle";
import { useSetting } from "../../settings/use-setting";
import styles from "./setup.module.css";

export function CameraCard() {
  const s = settings.value;
  const on = s.videoEnabled === true;
  const devices = videoDevices.value;
  const enabled = useSetting("videoEnabled", { kind: "toggle" });

  // Kameraene leses først når noen faktisk slår tillegget på: enumereringen
  // spør OS-et om videoenheter, og det er ingen grunn til å gjøre det for
  // menigheter som bare tar opp lyd.
  useEffect(() => {
    if (on && videoDevices.peek() === null) void loadVideoDevices();
  }, [on]);

  return (
    <Card testId="setup-camera" anchor="camera">
      <div class={styles.addonHead}>
        <Toggle
          checked={enabled.draft === true}
          onChange={(next) => enabled.set(next)}
          disabled={enabled.busy}
          labelId="setup-camera-label"
          testId="setup-camera-toggle"
        />
        <div class={styles.grow}>
          <div id="setup-camera-label" class={styles.addonTitle}>
            {t("app.setup.camera.title")}
          </div>
          <div data-testid="setup-camera-summary" class={styles.addonSummary}>
            {on
              ? (s.videoDeviceName ?? t("app.setup.camera.noneChosen"))
              : t("app.setup.camera.desc")}
          </div>
        </div>
        <Receipt state={enabled.receipt} testId="setup-camera-receipt" />
      </div>

      {on ? (
        <div class={styles.addonBody}>
          {devices !== null && devices.length === 0 ? (
            <EmptyState
              testId="camera-empty"
              title={t("app.setup.camera.none")}
              description={t("app.setup.camera.noneDesc")}
            />
          ) : (
            <>
              <CameraPicker />
              <BoundToggle
                setting="keepSeparateAudio"
                label={t("app.setup.camera.keepAudio")}
                description={t("app.setup.camera.keepAudioDesc")}
                testId="camera-keep-audio"
              />
            </>
          )}
        </div>
      ) : null}
    </Card>
  );
}

/** Hvilket kamera — og hva det faktisk klarer å levere. */
function CameraPicker() {
  const s = settings.value;
  const devices = videoDevices.value ?? [];
  // Samme lagringsmodell som resten: kvitteringen teller ned, og en feilet
  // skrivning ruller tilbake — den håndlagde utgaven her gjorde ingen av
  // delene, så et mislykket kamerabytte ble stående på skjermen som om det
  // hadde landet.
  const save = usePatch();
  const [capability, setCapability] = useState<string | null>(null);

  const current =
    devices.find((d) => d.name === (s.videoDeviceName ?? "")) ?? devices[0];

  // Si hva kameraet leverer, og si det ÆRLIG når vi ikke får spurt. Det er
  // forskjellen på «dette er en 720p-webkamera» og «vi vet ikke», og en
  // frivillig som skal filme en gudstjenesteoverføring vil vite hvilken.
  useEffect(() => {
    let alive = true;
    if (!current) {
      setCapability(null);
      return;
    }
    void window.api
      .getCameraCapabilities(String(current.index))
      .then((cap) => {
        if (!alive) return;
        setCapability(
          cap && cap.supportedResolutions.length > 0
            ? tf("app.setup.camera.delivers", {
                height: cap.maxHeight,
                fps: cap.maxFps,
              })
            : t("app.setup.camera.probeFailed"),
        );
      })
      .catch(() => {
        if (alive) setCapability(t("app.setup.camera.probeFailed"));
      });
    return () => {
      alive = false;
    };
  }, [current?.index]);

  async function choose(indexValue: string): Promise<void> {
    const device = devices.find((d) => String(d.index) === indexValue);
    if (!device || save.busy) return;
    await save.write(
      { videoDeviceName: device.name, videoDeviceIndex: device.index },
      {
        // Samme vakt som lydenheten, og av samme grunn: å bytte kamera fire
        // minutter før gudstjenesten er endringen som stille koster opptaket.
        confirm: () => confirmIfRecordingImminent(t("video.guardDevice")),
      },
    );
  }

  return (
    <SettingRow
      label={t("app.setup.camera.pick")}
      description={capability ?? undefined}
      receipt={save.receipt}
      testId="camera-device"
    >
      {(ids) => (
        <Select
          value={String(current?.index ?? "")}
          options={devices.map((d) => ({
            value: String(d.index),
            label: d.name,
          }))}
          onChange={(next) => void choose(next)}
          disabled={save.busy || devices.length === 0}
          labelId={ids.labelId}
          describedBy={ids.describedBy}
          testId="camera-device-control-input"
        />
      )}
    </SettingRow>
  );
}
