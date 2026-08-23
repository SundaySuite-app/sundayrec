/**
 * REDIGER, steg 2 — LYD. Canvasens artboard 4.2.
 *
 * Atlasets §3c er tabellen dette steget erstatter: fem inngangsdører til
 * mix/master, tre forskjellige flater, to helt ulike utfall, og ingenting i
 * UI-et som sier hvilken som gjelder. Her er det én bryter, tre ord og en
 * lytteknapp.
 *
 * ## De fire tingene som IKKE finnes her, og som fantes før
 *
 *   **«Normaliser lydnivå»** (−1 dBFS via `editor_probe_peak`) — et løfte som
 *   uansett ble overstyrt av mastringen: bakenden hopper over `volume=`-
 *   filteret så snart et preset er aktivt, fordi loudnorm setter nivået. En
 *   bryter som gjør noe bare når en annen bryter er av er ikke en bryter.
 *
 *   **Mastring-panelet** som skriver `<navn>_mastert.<ext>` ved siden av
 *   originalen og IKKE påvirker eksporten. To filer i mappen, og den ene er
 *   ikke den man deler.
 *
 *   **«Mastering (utgivelsesnivå)»** med `−19 / −16 / −14 LUFS` — tallene
 *   ligger her, men bak navnene. `sound-profiles.ts` er tabellen.
 *
 *   **«Kanalreparasjon»-velgeren** med sine fem moduser. Reparasjonen skjer
 *   fortsatt; den er bare ikke et spørsmål lenger — analysen ser at venstre
 *   kanal er stille, og sier det i én setning i stedet for å be om et valg.
 *
 * ## Bryteren og «Ingen» er den SAMME sannheten
 *
 * Canvasen tegner begge deler, og det er ikke dobbelt opp: bryteren er
 * inngangen for den som bare vil at det skal virke, kortene er for den som vil
 * vite hva som skjer. Under dem er det ett felt (`soundProfile`), og bryteren
 * husker hvilket kort som var valgt sist — «av, så på» skal ikke flytte noen
 * fra «Tale og musikk» til «Tale».
 */

import { useEffect } from "preact/hooks";

import { t, tDyn, tf } from "../i18n";
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { RadioCards, type RadioOption } from "../ui/RadioCards/RadioCards";
import { Toggle } from "../ui/Toggle/Toggle";
import { timecode } from "./editor-core";
import { MixerPanel } from "./MixerPanel";
import { playbackSource } from "./model";
import {
  analyzingSound,
  channelCode,
  ensureSoundAnalysis,
  listenBusy,
  listenError,
  listenPlaying,
  listenSide,
  listenStart,
  mixerOpen,
  refreshListenStart,
  setAutoEnhance,
  setListenSide,
  setSoundProfile,
  soundProfile,
  soundVisited,
  stopListen,
  toggleListen,
} from "./sound";
import { channelCodeKey, SOUND_PROFILES } from "./sound-profiles";
import styles from "./editor.module.css";

export function SoundStep() {
  const profile = soundProfile.value;
  const on = profile !== "none";

  useEffect(() => {
    soundVisited.value = true;
    refreshListenStart();
    // Kanalanalysen er memoisert per fil, så den koster ingenting ved en
    // retur hit. Den startes HER og ikke ved filåpning: der konkurrerer
    // topputtrekket og segmentanalysen allerede om den samme fila.
    void ensureSoundAnalysis();
    // Å forlate steget stopper lyttingen. Uten dette går de tjue sekundene
    // videre på en side ingen ser — den samme regelen `stopPlay` har.
    return stopListen;
  }, []);

  const options: RadioOption[] = SOUND_PROFILES.map((id) => ({
    value: id,
    title: tDyn("app.editor.profile", id),
    description: tDyn("app.editor.profileDesc", id),
    recommended: id === "speech",
  }));

  return (
    <div data-testid="editor-sound" data-profile={profile} class={styles.step}>
      <Card testId="editor-auto-card">
        <div class={styles.autoRow}>
          <div class={styles.autoGrow}>
            <b id="editor-auto-label" class={styles.autoTitle}>
              {t("app.editor.autoSound")}
            </b>
            <div id="editor-auto-desc" class={styles.hint}>
              {t("app.editor.autoSoundDesc")}
            </div>
          </div>
          <Toggle
            testId="editor-auto-toggle"
            checked={on}
            labelId="editor-auto-label"
            describedBy="editor-auto-desc"
            onChange={setAutoEnhance}
          />
        </div>
      </Card>

      <RadioCards
        testId="editor-profile"
        value={profile}
        options={options}
        // Av betyr «Ingen», og da er de tre kortene en beskrivelse av et valg
        // som ikke er i spill. De står, dempet, i stedet for å forsvinne — en
        // skjerm som skifter høyde når man trykker på en bryter er en skjerm
        // man mister plassen sin i.
        disabled={!on}
        onChange={(next) => setSoundProfile(next as typeof profile)}
      />

      <ChannelNote />
      {on ? <ListenCard /> : null}

      <div class={styles.toolbar}>
        <Button
          variant="ghost"
          testId="editor-mixer-open"
          onClick={() => (mixerOpen.value = !mixerOpen.value)}
        >
          {mixerOpen.value
            ? t("app.editor.hideMixer")
            : t("app.editor.advancedMixer")}
        </Button>
      </div>
      {mixerOpen.value ? <MixerPanel /> : null}
    </div>
  );
}

/**
 * Den ene ærlige setningen om kanalene.
 *
 * Vises bare når analysen har svart og koden er en vi har ord for. Tekstene er
 * legacys egne (`editor.chanDeadLeft` og de fem andre) og finnes derfor i alle
 * sju språk fra før — og de sier akkurat det en frivillig kan gjøre noe med:
 * «Venstre kanal er stille (sjekk kabel)».
 */
function ChannelNote() {
  if (analyzingSound.value) return null;
  // `tDyn` og ikke `t()` med en variabel: prefikset er en literal gaten kan slå
  // opp, og suffikset er halvdelen ingen gate kan kjenne — så et bom KASTER i
  // DEV i stedet for å male en tom linje som ser ut som «her står det ingenting».
  const suffix = channelCodeKey(channelCode.value ?? undefined);
  if (!suffix) return null;
  return (
    <p data-testid="editor-channel-note" class={styles.hint}>
      {tDyn("editor", suffix)}
    </p>
  );
}

/**
 * «Lytt: Før | Etter», og tjue sekunder fra midten av prekenen.
 *
 * «Etter» er en EKTE gjengivelse gjennom profilens kjede — `editor_master_preview`
 * med det samme presettet eksporten kommer til å bruke, rendret til en temp-fil.
 * Ikke en simulering, og ikke `_mastert`-fila: den skrives aldri av dette
 * skallet.
 *
 * «Før» spiller originalen fra det samme sekundet. Går ikke avspilling i det
 * hele tatt (atlasets forbehold: i en ren nettleser er `asset://` død), sier
 * kortet det med legacys egen setning i stedet for å la knappen stå og ikke
 * gjøre noe.
 */
function ListenCard() {
  const side = listenSide.value;
  const dead = playbackSource.value === "none";
  const beforeDead = side === "before" && dead;

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
          aria-disabled={beforeDead ? "true" : undefined}
          aria-label={
            listenPlaying.value ? t("app.editor.pause") : t("tooltip.play")
          }
          title={beforeDead ? t("editor.qualityFallback") : undefined}
          onClick={() => {
            if (beforeDead) return;
            void toggleListen();
          }}
        >
          {listenPlaying.value ? (
            <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <rect x="6" y="5" width="4" height="14" rx="1" />
              <rect x="14" y="5" width="4" height="14" rx="1" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M8 5v14l11-7z" />
            </svg>
          )}
        </button>
        <span data-testid="editor-listen-at" class={styles.hint}>
          {tf("app.editor.sample", { at: timecode(listenStart.value) })}
        </span>
      </div>
      {listenBusy.value ? (
        <p data-testid="editor-listen-busy" class={styles.hint}>
          {t("app.editor.rendering")}
        </p>
      ) : listenError.value ? (
        <p data-testid="editor-listen-error" class={styles.hint}>
          {beforeDead
            ? t("editor.qualityFallback")
            : t("app.editor.listenFailed")}
        </p>
      ) : null}
    </Card>
  );
}
