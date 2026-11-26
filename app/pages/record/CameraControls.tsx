/**
 * Kameraknappen ved opptaksknappen — og pilen som åpner kamera-popoveren.
 *
 * Knappen slår kameratillegget av/på (`videoEnabled`, én lagringsmodell med
 * kvittering som resten: `useSetting`). Pilen åpner en liten popover med to
 * valg som hører til kameraet og ikke trenger et ark: hvilket kamera
 * (`videoDeviceName`) og «egen lydfil» (`keepSeparateAudio`).
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { t, tDyn, tf } from "../../i18n";
import { confirmIfRecordingImminent } from "../../settings/guards";
import { usePatch } from "../../settings/use-patch";
import { useSetting } from "../../settings/use-setting";
import {
  loadVideoDevices,
  videoDevices,
  videoDevicesFailed,
} from "../../state/devices";
import { settings } from "../../state/settings";
import { BoundToggle } from "../../ui/Bound/Bound";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { Icon } from "../../ui/Icon/Icon";
import { Select } from "../../ui/Select/Select";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { motionWord, resolutionWord } from "../settings/camera-words-core";
import styles from "./CameraControls.module.css";

export interface CameraControlsProps {
  /** `stage` = over kamerabildet (mørk knapp); `rest` = ved opptaksknappen. */
  place: "rest" | "stage";
}

export function CameraControls({ place }: CameraControlsProps) {
  const on = settings.value.videoEnabled === true;
  const enabled = useSetting("videoEnabled", { kind: "toggle" });
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement | null>(null);

  // Kameralista leses først når noen åpner valget eller slår tillegget på.
  useEffect(() => {
    if ((open || on) && videoDevices.peek() === null) void loadVideoDevices();
  }, [open, on]);

  // Utenfor-klikk og Esc lukker popoveren.
  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent): void => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const label = on ? t("app.record.camOff") : t("app.record.camOn");
  return (
    <span ref={root} class={`${styles.root} ${styles[place]}`}>
      <button
        type="button"
        data-testid="record-camera-toggle"
        aria-label={label}
        title={label}
        aria-pressed={on}
        class={`${styles.cam} ${on ? styles.on : ""}`}
        onClick={() => enabled.set(!on)}
      >
        <Icon name="camera" size={place === "stage" ? 22 : 26} />
      </button>
      <button
        type="button"
        data-testid="record-camera-more"
        aria-label={t("app.record.camChoose")}
        title={t("app.record.camChoose")}
        aria-expanded={open}
        class={styles.more}
        onClick={() => setOpen(!open)}
      >
        <Icon name="chevron-up" size={14} class={open ? "" : styles.flip} />
      </button>
      {open ? (
        <div
          data-testid="record-camera-popover"
          role="group"
          aria-label={t("app.record.camPopover")}
          class={styles.popover}
        >
          <CameraPopoverBody />
        </div>
      ) : null}
    </span>
  );
}

function deliversText(maxHeight: number, maxFps: number): string {
  const res = tDyn("app.setup.camera.res", resolutionWord(maxHeight));
  const motion = motionWord(maxFps);
  return motion
    ? tf("app.setup.camera.delivers", {
        res,
        motion: tDyn("app.setup.camera.motion", motion),
      })
    : tf("app.setup.camera.deliversNoMotion", { res });
}

function CameraPopoverBody() {
  const devices = videoDevices.value;
  const failed = videoDevicesFailed.value;
  if (devices !== null && devices.length === 0) {
    return (
      <EmptyState
        testId="camera-empty"
        title={
          failed ? t("app.record.camera.listError") : t("app.setup.camera.none")
        }
        description={t("app.setup.camera.noneDesc")}
      />
    );
  }
  return (
    <>
      <CameraPicker />
      <BoundToggle
        setting="keepSeparateAudio"
        label={t("app.setup.camera.keepAudio")}
        description={t("app.setup.camera.keepAudioDesc")}
        testId="camera-keep-audio"
      />
    </>
  );
}

function CameraPicker() {
  const s = settings.value;
  const devices = videoDevices.value ?? [];
  const save = usePatch();
  const [capability, setCapability] = useState<string | null>(null);
  const current =
    devices.find((d) => d.name === (s.videoDeviceName ?? "")) ?? devices[0];

  // Si hva kameraet leverer, og si det ÆRLIG når vi ikke får spurt.
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
            ? deliversText(cap.maxHeight, cap.maxFps)
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
        // Å bytte kamera fire minutter før gudstjenesten er endringen som
        // stille koster opptaket — samme vakt som lydenheten.
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
