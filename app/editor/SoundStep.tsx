/**
 * REDIGER, steg 2 — LYD.
 *
 * Én bryter: «Podkastklar lyd». På (standard) gir den ÉN fast oppskrift — tydeligere stemme,
 * jevnere lyd og −16 LUFS med sann topp under taket — av gir fila slik den ble tatt opp.
 * Under bryteren: en linje om hva som ble gjort med akkurat dette opptaket, og en
 * lytteknapp med «Før» og «Etter».
 *
 * Før F3 var det her tre profiler, en avansert mikser og en kanalreparasjon. De er borte:
 * oppskriften er den samme for alle opptak, og tallene står i `sound-recipe.ts`.
 */

import { useEffect } from "preact/hooks";

import { t, tDyn, tf } from "../i18n";
import { Card } from "../ui/Card/Card";
import { Icon } from "../ui/Icon/Icon";
import { Toggle } from "../ui/Toggle/Toggle";
import { timecode } from "./editor-core";
import {
  ensureSoundAnalysis,
  listenAt,
  listenError,
  listenStopped,
  listenPreparing,
  listenPlaying,
  listenSide,
  listenStart,
  podcastAudio,
  refreshListenStart,
  setListenSide,
  setPodcastAudio,
  soundVisited,
  stopListen,
  taleSaid,
  toggleListen,
  watchListenPrepare,
} from "./sound";
import { taleSaidKeys } from "./sound-recipe";
import styles from "./editor.module.css";
import { PLAY_KEYS_ARIA, playShortcutHint } from "./shortcut-labels";

export function SoundStep() {
  const on = podcastAudio.value;

  useEffect(() => {
    soundVisited.value = true;
    refreshListenStart();
    // «Tale»-analysen er memoisert per fil, så den koster ingenting ved en
    // retur hit. Den startes HER og ikke ved filåpning: der konkurrerer
    // topputtrekket og segmentanalysen allerede om den samme fila.
    void ensureSoundAnalysis();
    // Å forlate steget stopper lyttingen. Uten dette går de tjue sekundene
    // videre på en side ingen ser — den samme regelen `stopPlay` har.
    // Gjør «Etter» klar i bakgrunnen: programmet måles nå, og på nytt når valget eller
    // kuttene endres, så første trykk på «Etter» som regel finner målingen ferdig.
    const unwatch = watchListenPrepare();
    return () => {
      unwatch();
      stopListen();
    };
  }, []);

  return (
    <div
      data-testid="editor-sound"
      data-podcast={on ? "on" : "off"}
      class={styles.step}
    >
      <Card testId="editor-podcast-card">
        <div class={styles.autoRow}>
          <div class={styles.autoGrow}>
            <b id="editor-podcast-label" class={styles.autoTitle}>
              {t("app.editor.podcastAudio")}
            </b>
            <div id="editor-podcast-desc" class={styles.hint}>
              {t("app.editor.podcastAudioDesc")}
            </div>
          </div>
          <Toggle
            testId="editor-podcast-toggle"
            checked={on}
            labelId="editor-podcast-label"
            describedBy="editor-podcast-desc"
            onChange={setPodcastAudio}
          />
        </div>
      </Card>

      <TaleSaid />
      {on ? <ListenCard /> : null}
    </div>
  );
}

/**
 * Tre ord om hva «Podkastklar lyd» gjorde med akkurat dette opptaket (B3b).
 *
 * Én linje per ting: «Vi fjernet brum fra strømnettet». Ble ingenting gjort, står
 * den rolige «Opptaket var rent.» — og ikke noe som helst før analysen har svart,
 * for den som ikke fikk lese fila (`taleSaidKeys`). Setningene er kortere enn en
 * linje og har ingen fagord: «brum», «ekko», «klikk», ikke «dehum» eller «RT60».
 */
function TaleSaid() {
  const keys = taleSaidKeys(podcastAudio.value, taleSaid.value);
  if (keys.length === 0) return null;
  return (
    <ul data-testid="editor-tale-said" class={styles.saidList}>
      {keys.map((k) => (
        <li key={k} data-testid={`editor-tale-said-${k}`}>
          {tDyn("app.editor.said", k)}
        </li>
      ))}
    </ul>
  );
}

/**
 * «Lytt: Før | Etter», fra midten av det som beholdes.
 *
 * Lyttingen spilles av avspillingsmotoren (B5b), ikke av et `<audio>`-element: Rust spiller
 * opptaket med kuttene på lydkortet og har både originalen («Før») og lyden gjennom oppskriftens
 * kjede («Etter») klare. Å trykke «Før» eller «Etter» er derfor et bytte, ikke en gjengivelse
 * — og det går også mens det spiller. Slår man bryteren av og på underveis, kommer den nye
 * lyden inn av seg selv, uten klikk.
 *
 * «Etter» er en EKTE gjengivelse gjennom det samme presettet og den samme kjeden eksporten
 * kommer til å bruke, på det samme nivået. Ikke en simulering, og ikke `_mastert`-fila: den
 * skrives aldri av dette skallet.
 *
 * Det eneste som kan vente er målingen av programmet («Gjør klar lytting …»), første gang for
 * et valg: den er nivået eksporten vil gi, og uten den ville «Etter» vært et annet nivå enn
 * filen man får.
 */
function ListenCard() {
  const side = listenSide.value;

  return (
    <Card testId="editor-listen">
      <div class={styles.listenRow}>
        <b class={styles.autoTitle}>{t("app.editor.listen")}</b>
        <div
          role="radiogroup"
          aria-label={t("app.editor.listenAria")}
          class={styles.seg}
        >
          {(["before", "after"] as const).map((id) => (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={side === id ? "true" : "false"}
              data-testid={`editor-listen-${id}`}
              class={`${styles.segButton} ${side === id ? styles.segOn : ""}`}
              onClick={() => setListenSide(id)}
            >
              {id === "before" ? t("app.editor.before") : t("app.editor.after")}
            </button>
          ))}
        </div>
        <button
          type="button"
          class={styles.play}
          data-testid="editor-listen-play"
          aria-label={
            listenPlaying.value ? t("app.editor.pause") : t("tooltip.play")
          }
          title={playShortcutHint()}
          aria-keyshortcuts={PLAY_KEYS_ARIA}
          onClick={() => void toggleListen()}
        >
          {listenPlaying.value ? <Icon name="pause" /> : <Icon name="play" />}
        </button>
        <span data-testid="editor-listen-at" class={styles.hint}>
          {tf("app.editor.sample", {
            at: timecode(listenAt.value ?? listenStart.value),
          })}
        </span>
      </div>
      {listenPreparing.value ? (
        <p data-testid="editor-listen-busy" class={styles.hint}>
          {t("app.editor.listenPreparing")}
        </p>
      ) : listenError.value ? (
        <p data-testid="editor-listen-error" class={styles.hint}>
          {t("app.editor.listenFailed")}
        </p>
      ) : listenStopped.value ? (
        <p data-testid="editor-listen-stopped" class={styles.hint}>
          {t("app.editor.listenStopped")}
        </p>
      ) : null}
    </Card>
  );
}
