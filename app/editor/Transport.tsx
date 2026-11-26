/**
 * Spill/pause, klokka og Inn/Ut — og bildet, når opptaket har video (E7).
 *
 * ## Bildet er en stum `<video>` som følger motoren
 *
 * Lyden spilles av avspillingsmotoren i Rust (`playback.ts`); bildet er en
 * `muted` `<video>` som følger `playheadSec` og `playing`. Beslutningen om når
 * den spiller, står stille eller rettes (drift over 0,25 s) er ren og
 * testet: `@lib/pages/editor/video-sync`.
 *
 * ## Hvordan fila når `<video>`
 *
 * Gjennom Tauris asset-protokoll, som er skrudd på uten NOEN tillatt sti
 * (`tauri.conf.json`: `assetProtocol.scope.allow: []`). Det eneste som åpner
 * den er at Rust åpner opptaket: `open_source` åpner scopet for akkurat den ene
 * kanoniske fila etter at `chosen_paths::vet` har godkjent den
 * (`allow_asset_path`), samme grant som lyden alltid har brukt. Her lages bare
 * adressen (`toAssetUrl(E.filePath)`) for fila som allerede er åpnet — ingen
 * ny kommando, ingen bredere tilgang, og en sti webviewen finner på har ingen
 * grant.
 */

import { effect } from "@preact/signals";
import { videoSyncAction } from "@lib/pages/editor/video-sync";
import { useEffect, useRef } from "preact/hooks";

import { t } from "../i18n";
import { Icon } from "../ui/Icon/Icon";
import { keepWindow, timecode } from "./editor-core";
import {
  cuts,
  duration,
  filePath,
  mediaInfo,
  playheadSec,
  playing,
} from "./model";
import { togglePlay } from "./playback";
import { PLAY_KEYS_ARIA, playShortcutHint } from "./shortcut-labels";
import styles from "./editor.module.css";

/** Har opptaket et bilde å vise? */
export function hasVideo(): boolean {
  return mediaInfo.value?.hasVideo === true;
}

/** Spill/pause, tid og Inn/Ut. `video` = står i stripa nederst i bildet. */
export function Transport({ video = false }: { video?: boolean }) {
  const total = duration.value;
  const window_ = keepWindow({ cuts: cuts.value, duration: total });
  const inAt = window_ ? window_.start : 0;
  const outAt = window_ ? window_.end : total;

  return (
    <div
      class={video ? styles.videoBar : styles.transport}
      data-testid="editor-transport"
    >
      <button
        type="button"
        class={video ? `${styles.play} ${styles.playGlass}` : styles.play}
        data-testid="editor-play"
        aria-label={playing.value ? t("app.editor.pause") : t("tooltip.play")}
        title={playShortcutHint()}
        aria-keyshortcuts={PLAY_KEYS_ARIA}
        onClick={togglePlay}
      >
        {playing.value ? <Icon name="pause" /> : <Icon name="play" />}
      </button>
      <span class={styles.clock}>
        <b data-testid="editor-time" class={styles.time}>
          {timecode(playheadSec.value)}
        </b>
        <span class={styles.total} aria-hidden="true">
          /
        </span>
        <span data-testid="editor-total" class={styles.total}>
          {timecode(total)}
        </span>
      </span>
      <span class={styles.marks}>
        <span class={styles.mark}>
          <span class={styles.swatch} aria-hidden="true" />
          {t("app.editor.markIn")}{" "}
          <b data-testid="editor-in">{timecode(inAt)}</b>
        </span>
        <span class={styles.mark}>
          <span class={styles.swatch} aria-hidden="true" />
          {t("app.editor.markOut")}{" "}
          <b data-testid="editor-out">{timecode(outAt)}</b>
        </span>
      </span>
    </div>
  );
}

/** Det store bildet (16:9) med transporten i en mørk stripe nederst. */
export function VideoStage() {
  const ref = useRef<HTMLVideoElement | null>(null);
  // `filePath`-signalet er satt når fila ble åpnet og endres ikke mens den er
  // åpen; adressen er den samme som lyden alltid har hatt.
  const src = window.api.toAssetUrl(filePath.value);

  useEffect(() => {
    const dispose = effect(() => {
      const target = playheadSec.value;
      const isPlaying = playing.value;
      const v = ref.current;
      if (!v) return;
      const act = videoSyncAction({
        videoTime: v.currentTime,
        target,
        playing: isPlaying,
        videoPaused: v.paused,
      });
      if (act.seek !== null) {
        try {
          v.currentTime = act.seek;
        } catch {
          /* ingenting lastet ennå: neste melding prøver igjen */
        }
      }
      if (act.pause) v.pause();
      if (act.play) {
        // Autoplay-regler og manglende kodek avviser `play()`; bildet er
        // pynt til lyden, så et avslag er ikke en feil å vise.
        void Promise.resolve(v.play()).catch(() => {});
      }
    });
    return () => {
      dispose();
      ref.current?.pause();
    };
  }, []);

  return (
    <div class={styles.stage}>
      <div class={styles.video} data-testid="editor-video-box">
        <span class={styles.videoPlaceholder} aria-hidden="true">
          <Icon name="camera" size={96} strokeWidth={1} />
        </span>
        <video
          ref={ref}
          class={styles.videoEl}
          data-testid="editor-video"
          src={src}
          muted
          playsInline
          preload="auto"
          tabIndex={-1}
          aria-label={t("app.editor.videoLabel")}
        />
        <Transport video />
      </div>
    </div>
  );
}
