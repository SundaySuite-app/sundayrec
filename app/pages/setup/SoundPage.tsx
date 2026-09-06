/**
 * 1 — Hvilken lyd? (canvasens artboard 5.2)
 *
 * Enhet, kanalpar og hørselstest på ÉN skjerm. I dag er det tre steder:
 * enhetslisten i Lyd-fanen, et rutenett på opptil 32 ruter under den, og
 * VU-måleren på Hjem — og ingen av dem svarer på spørsmålet «tar vi opp det
 * riktige?» alene.
 *
 * ## Hvorfor dette IKKE går gjennom `useSetting`
 *
 * Et enhetsvalg er tre nøkler som må skrives sammen: `deviceId`, `deviceName`
 * og — for en mikser — `deviceChannels[id]`. `useSetting` eier én nøkkel og har
 * én kvittering; tre av dem ville betydd tre skrivninger, tre kvitteringer og et
 * vindu der basen holder halve valget. Så: eksplisitt «Bruk denne», og ÉN
 * lagring. Det er også hvorfor legacy `selectDevice` gjør det samme.
 *
 * ## Rutenettet ble til brikker
 *
 * Et 32-ruters rutenett med en stolpe per kanal svarer på «hvilke kanaler har
 * signal?» — men spørsmålet en frivillig har er «hvilket PAR skal vi ta opp?».
 * Kanaler kommer i par ut av et miksebord, og en frivillig som velger «15» og
 * «16» hver for seg kan velge «15» og «3». Så: par-brikker, og «de som har lyd
 * nå, lyser» er den samme informasjonen rutenettet ga.
 *
 * Terskelen som avgjør hva som «lyser» er `nextSignalState` fra
 * `channel-grid-logic.ts` — legacy-rutenettets egen, med hysterese (på over
 * −50 dB, av under −55) så en kanal som ligger og vaker på grensen ikke
 * blinker. Kjernen er gjenbrukt, ikke portet: to terskler som er «omtrent
 * like» er hvordan de to skjermene begynner å si forskjellige ting.
 *
 * ## Vakten
 *
 * Å bytte lydenhet fire minutter før gudstjenesten er endringen som stille
 * koster deg opptaket, så den spør først — samme vakt og samme tekstnøkkel som
 * legacy (`audio.guardDevice`, «Bytte lydenhet»). Bare ved BYTTE: å bekrefte
 * kanalparet på enheten man allerede bruker er ikke farlig.
 *
 * ## `scheduler_reschedule`
 *
 * Kalles ikke herfra, og skal ikke det: `window.api.saveSettings` gjør det
 * selv etter hver skrivning (api-shim.ts). Et kall til fra en side ville vært
 * det andre stedet den regelen bor.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { acquireVuFeed } from "@lib/audio/vu-feed";
import { isBuiltInDevice } from "@lib/audio/capture";
import { nextSignalState } from "@lib/pages/channel-grid-logic";
import type { VuLevels } from "@legacy/bindings/VuLevels";

import { t, tf } from "../../i18n";
import { navigate } from "../../router/router";
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
import { Card } from "../../ui/Card/Card";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { RadioCards, type RadioOption } from "../../ui/RadioCards/RadioCards";
import { Receipt } from "../../ui/Receipt/Receipt";
import { VuMeter } from "../../ui/VuMeter/VuMeter";
import { channelPairFor, channelPairs } from "./decisions-core";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

/**
 * Drivernavnet på Windows' proffvei inn. En KONSTANT og ikke en katalognøkkel:
 * ASIO heter ASIO på alle sju språk, og et navn som ikke oversettes skal ikke
 * ligge i katalogen og be om å bli oversatt.
 */
const ASIO = "ASIO";

export function SoundPage() {
  const s = settings.value;
  const devices = audioDevices.value;

  const stored = (s.deviceId ?? "").trim();
  const [picked, setPicked] = useState<string>(stored);
  const storedPair = channelPairFor(s, picked);
  // 0-indeksert internt (det er slik `deviceChannels` lagres); brikkene viser
  // +1, fordi et miksebord er merket fra 1.
  const [pairL, setPairL] = useState<number>(storedPair ? storedPair.l - 1 : 0);
  // Én lagringsmodell, også her: `usePatch` kjører den samme sekvensen som
  // `useSetting` (anvend → skriv → kvittering | rull tilbake), bare over de tre
  // nøklene et enhetsvalg ER. Kvitteringen teller ned av seg selv.
  const save = usePatch();

  const device = devices?.find((d) => d.id === picked) ?? null;
  const multi = (device?.channels ?? 0) > 2;
  const lit = useChannelSignals(
    device?.name ?? null,
    device?.channels ?? 0,
    // Samme invariant som måleren under: ikke be om enheten mens den er
    // opptatt av å ta opp en gudstjeneste.
    !isRecording.value,
  );

  // Et enhetsbytte i lista skal ta med seg enhetens EGET lagrede par — ikke
  // det forrige valgets. Uten dette ville «kanal 15–16» fulgt med over på et
  // stereokort som ikke har en kanal 15.
  function choose(id: string): void {
    setPicked(id);
    const pair = channelPairFor(settings.peek(), id);
    setPairL(pair ? pair.l - 1 : 0);
    save.reset();
  }

  const changed =
    picked !== stored ||
    (multi && pairL !== (storedPair ? storedPair.l - 1 : 0));

  async function useThis(): Promise<void> {
    if (save.busy || !device) return;

    const patch: Partial<Settings> = {
      deviceId: device.id,
      deviceName: device.name,
    };
    if (multi) {
      patch.deviceChannels = {
        ...(settings.peek().deviceChannels ?? {}),
        [device.id]: { channelL: pairL, channelR: pairL + 1 },
      };
    }
    // Kanalmodus settes BARE når enheten faktisk byttes: et bevisst mono-valg
    // på enheten man allerede bruker skal ikke overskrives fordi noen
    // bekreftet kanalparet. En 1-kanals enhet kan ikke levere stereo — den
    // ville gitt en død høyrekanal — så den tvinges til monoL, akkurat som
    // legacy-rutenettets `onGridChannelCount` gjør.
    const switching = picked !== stored;
    if (switching) {
      patch.channels = device.channels === 1 ? "monoL" : "stereo";
    }

    await save.write(patch, {
      // `changed` er sidens eget spørsmål: å bekrefte det SAMME kanalparet på
      // den samme enheten er ingen endring, men patchen bærer likevel
      // `deviceChannels`, som er et nytt objekt hver gang.
      changed,
      // Bare ved BYTTE — se toppen av fila.
      confirm: switching
        ? () => confirmIfRecordingImminent(t("audio.guardDevice"))
        : undefined,
      after: async () => {
        // Forhåndsbufferen adresserer enheten ved NAVN — pek den på den nye før
        // noe annet åpner enheten (legacy gjør det i samme rekkefølge).
        await reconcilePreroll(true);
        // ⚠️ INGEN navigering her lenger (D2). Skjermen sto på sin egen rute
        // før, og et `navigate` tilbake til nivå 1 var veien ut. Nå er den
        // innbygget i kilde-kortet på OPPTAK, og et rutebytte herfra ville
        // revet kortet igjen i det øyeblikket lagringen landet — altså rykket
        // skjermen bort under den som nettopp trykket «Bruk denne».
        // Kortraden er den eksplisitte kollapsen; den som vil se resultatet
        // ser det i kvitteringen, på stedet.
      },
    });
  }

  if (devices !== null && devices.length === 0) {
    return (
      <SubPage lede={t("app.setup.sound.lede")} testId="setup-sound">
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
      </SubPage>
    );
  }

  return (
    <SubPage lede={t("app.setup.sound.lede")} testId="setup-sound">
      <RadioCards
        testId="sound-devices"
        value={picked}
        options={(devices ?? []).map(toOption)}
        onChange={choose}
      />

      {multi ? (
        <Card testId="sound-pairs" title={t("app.setup.sound.whichChannels")}>
          <div class={styles.pairs}>
            {channelPairs(device?.channels ?? 0).map((left) => (
              <button
                key={left}
                type="button"
                data-testid={`sound-pair-${left + 1}`}
                data-live={lit[left] || lit[left + 1] ? "true" : undefined}
                aria-pressed={pairL === left}
                aria-label={tf("app.setup.sound.pairLabel", {
                  l: left + 1,
                  r: left + 2,
                })}
                class={`${styles.pair} ${pairL === left ? styles.pairOn : ""} ${
                  lit[left] || lit[left + 1] ? styles.pairLive : ""
                }`}
                onClick={() => {
                  setPairL(left);
                  save.reset();
                }}
              >
                {`${left + 1}–${left + 2}`}
              </button>
            ))}
          </div>
          <p class={styles.hint}>{t("app.setup.sound.channelHint")}</p>
        </Card>
      ) : null}

      <Card testId="sound-meter" description={t("app.setup.sound.testHint")}>
        <VuMeter
          testId="sound-vu"
          // AVSLÅTT mens det tas opp — samme invariant som nivå 1 og
          // sekvensen (`use-vu-word.ts`): Rust stopper VU-strømmen når
          // `start_recording` åpner enheten, og en `start_vu` etterpå ber om
          // nøyaktig den enheten opptaket holder. Midt i en gudstjeneste.
          off={isRecording.value}
          deviceName={device?.name ?? null}
          pick={() => ({
            mode: multi ? "stereo" : settings.peek().channels,
            chL: multi ? pairL : 0,
            chR: multi ? pairL + 1 : 1,
          })}
        />
      </Card>

      <div class={styles.footer}>
        {/*
          «Avansert lyd» — opptaksmotoren og forhåndsbufferen. P1a lot den være
          ute fordi Avansert ikke fantes; nå gjør den det, og lenken lander på
          raden den handler om i stedet for på toppen av en lang liste.
        */}
        <Button
          variant="ghost"
          testId="sound-advanced"
          onClick={() => navigate("setup", { anchor: "engine" })}
        >
          {t("app.setup.advanced.soundLink")}
        </Button>
        <Receipt state={save.receipt} testId="sound-receipt" />
        <Button
          variant="primary"
          size="lg"
          busy={save.busy}
          disabled={!device || !changed}
          disabledReason={
            !device
              ? t("app.setup.sound.noneDesc")
              : t("app.setup.sound.noChange")
          }
          testId="sound-use"
          onClick={() => void useThis()}
        >
          {t("app.setup.sound.useThis")}
        </Button>
      </div>
    </SubPage>
  );
}

function toOption(device: AudioDeviceOption): RadioOption {
  const builtIn = isBuiltInDevice(device.name);
  return {
    value: device.id,
    // Maskinens egen mikrofon får sitt eget navn i stedet for CoreAudios
    // («MacBook Pro Microphone») — men den er fortsatt en ekte enhet med en
    // ekte id, så et valg her skriver ALDRI `deviceId: null`.
    title: builtIn ? t("app.setup.sound.builtIn") : device.name,
    description: device.asio
      ? ASIO
      : device.channels > 2
        ? tf("app.setup.sound.mixer", { n: device.channels })
        : builtIn
          ? t("app.setup.sound.builtInDesc")
          : t("app.setup.sound.external"),
  };
}

/**
 * Hvilke native kanaler som har signal NÅ.
 *
 * Et eget abonnement på den delte strømmen, ved siden av målerens. `acquireVuFeed`
 * er refcountet, så de to blir én økt på enheten — ikke to (`@lib/audio/vu-feed`).
 *
 * Tilstanden holdes i en `ref` og speiles til `state` bare når den ENDRER seg:
 * pakkene kommer 30 ganger i sekundet, og en `setState` per pakke ville rendret
 * hele siden 30 ganger i sekundet for å tenne en brikke som skifter noen ganger
 * i minuttet.
 *
 * `active` er `false` mens det TAS OPP, av nøyaktig samme grunn som `off` på
 * måleren og `active` i `use-vu-word.ts`: opptaksmotoren eier enheten, og en
 * `start_vu` derfra ville bedt om den midt i en gudstjeneste. Brikkene slukner
 * i stedet, som er sant — vi måler ingenting.
 */
function useChannelSignals(
  deviceName: string | null,
  channels: number,
  active: boolean,
): boolean[] {
  const [lit, setLit] = useState<boolean[]>([]);
  const state = useRef<boolean[]>([]);

  useEffect(() => {
    state.current = [];
    setLit([]);
    if (!active || !deviceName || channels <= 2) return;
    const release = acquireVuFeed({
      deviceName,
      onLevels: (_l, _r, raw: VuLevels) => {
        const peaks = raw.peak_dbfs ?? [];
        let changed = false;
        const next = state.current.slice();
        for (let i = 0; i < channels; i++) {
          // RÅ verdi, ikke utjevnet: hysteresen på en dempet verdi ville holdt
          // en kanal «tent» like lenge som fallet varer etter at lyden sluttet.
          const on = nextSignalState(next[i] ?? false, peaks[i] ?? -120);
          if (on !== next[i]) {
            next[i] = on;
            changed = true;
          }
        }
        if (changed) {
          state.current = next;
          setLit(next);
        }
      },
    });
    return release;
  }, [deviceName, channels, active]);

  return lit;
}
