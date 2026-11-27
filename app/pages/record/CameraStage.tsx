/**
 * Kamerascenen — er kamera på, tar bildet hele flaten (venstre/høyre 40 px,
 * 96 px fra toppen, over ukestripa). Over bildet: kameranavnet øverst til
 * venstre, kamera-av øverst til høyre, loddrett L/R-måler i kanten og den
 * lille opptaksknappen nederst i midten.
 *
 * Bildet er `CameraPreview` (bakendens side-kopi som aldri bremser opptaket).
 * Scenen blir stående montert mens et opptak starter — «bildet blinker ikke»
 * er pinnet i `e2e/record.spec.ts`.
 */

import { t } from "../../i18n";
import { settings } from "../../state/settings";
import { CameraPreview } from "../../ui/CameraPreview/CameraPreview";
import { BigButton } from "./BigButton";
import { CameraControls, type CameraToggle } from "./CameraControls";
import styles from "./CameraStage.module.css";
import { pickForSource } from "./source-pick";
import { StereoMeter } from "./StereoMeter";
import type { SourceState } from "./record-core";

export interface CameraStageProps {
  source: SourceState;
  onStart: () => void;
  busy: boolean;
  /** Mens det starter/tas opp står ingen måler i treet. */
  meter: boolean;
  camera: CameraToggle;
  /**
   * Bildet bak scenen. Av mens opptaksoverlegget står oppå: overlegget viser
   * SAMME kameraøkt, og to bilder som males og dekodes hver gang et bilde
   * kommer er CPU brukt på noe ingen ser (svak Windows-PC midt i et opptak).
   * Økta deles (`camera-feed.ts`), så den som blir igjen mister ikke bildet.
   */
  picture?: boolean;
}

export function CameraStage({
  source,
  onStart,
  busy,
  meter,
  camera,
  picture = true,
}: CameraStageProps) {
  const name = (settings.value.videoDeviceName ?? "").trim();
  return (
    <div data-testid="camera-stage" class={styles.stage}>
      <div class={styles.picture}>
        {picture ? <CameraPreview fill /> : null}
      </div>
      <span class={styles.name}>
        <span aria-hidden="true" class={styles.live} />
        {name || t("app.record.camDefault")}
      </span>
      <div class={styles.corner}>
        <CameraControls place="stage" enabled={camera} />
      </div>
      {meter ? (
        <span class={styles.meter}>
          <StereoMeter
            variant="vertical"
            testId="record-vu"
            deviceName={source.kind === "no-source" ? null : source.name}
            off={source.kind === "no-source"}
            pick={() => pickForSource(settings.peek(), source)}
          />
        </span>
      ) : null}
      <div class={styles.start}>
        <BigButton
          size="small"
          onStart={onStart}
          busy={busy}
          canStart={source.canStart}
          reason={t("app.record.whyBlocked")}
        />
      </div>
    </div>
  );
}
