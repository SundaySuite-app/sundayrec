/**
 * Enhetene maskinen har — lyd og kamera.
 *
 * ## `null` er ikke «ingen»
 *
 * Begge signalene starter på `null`, som betyr «ikke lest ennå», og blir en
 * (muligens tom) liste etter første lesning. Forskjellen er hele grunnen til
 * at `decisions-core` har en tredje tilstand: en app som maler «Finner ikke
 * Behringer X32» i det halvsekundet før enumereringen svarer, har sagt noe
 * usant høyt. Tom liste er derimot et ekte svar, og den fortjener
 * tomtilstanden («Finner ingen lydenheter»).
 *
 * ## Ingen `getUserMedia` noe sted
 *
 * Listen kommer fra `list_audio_devices` (cpal + ASIO) gjennom `window.api`,
 * som er den samme enumereringen opptakeren selv slår opp mot. Legacy-skallets
 * gamle vei — `enumerateDevices()` etter et blink av `getUserMedia` — gjorde
 * webviewet til eier av mikrofonen hver gang en velger ble tegnet, og det er
 * feilklassen bak Qu-5-hendelsen 2026-07-31 (en 32-kanals mikser låst til
 * stereo fordi gUM hadde forhandlet det formatet). `app/` gjør det aldri.
 */

import { signal } from "@preact/signals";

import type { DeviceChannels } from "@legacy/bindings/DeviceChannels";
import type { TaggedAudioInput } from "@legacy/bindings/TaggedAudioInput";

import {
  patchSettings,
  saveSettingsDebounced,
  settings,
  type Settings,
} from "./settings";

/** Én valgbar lydinngang, slik siden trenger den. */
export interface AudioDeviceOption {
  /** Id-en opptakeren adresseres med — og det som lagres i `deviceId`. */
  id: string;
  name: string;
  /** Antall inngangskanaler. 0 = ukjent. */
  channels: number;
  /** ASIO-enheter er Windows' proffvei og får sin egen brikke. */
  asio: boolean;
  /** Vertens standardenhet. */
  isDefault: boolean;
}

/**
 * Bakendens liste → valgene siden viser.
 *
 * ⚠️ ASIO-enheter får `asio::`-prefikset her, ikke i bakenden. Det er
 * nøyaktig det `legacy/renderer/pages/audio-page.ts` gjør
 * (`const devId = \`asio::${name}\``), og prefikset er UI-ets håndtak: en
 * ASIO-enhet og dens WASAPI-stereoskygge er samme maskinvare, så de to må ha
 * hver sin id for at et valg skal bety én av dem. Bakenden adresserer den
 * rå-navnet.
 *
 * ASIO først, som i legacy: det er den foretrukne, flerkanals veien inn.
 */
export function toDeviceOptions(
  list: readonly TaggedAudioInput[],
): AudioDeviceOption[] {
  const asio = list
    .filter((d) => d.backend === "asio")
    .map((d) => ({
      id: `asio::${d.name}`,
      name: d.name,
      channels: d.inputChannels,
      asio: true,
      isDefault: false,
    }));
  const host = list
    .filter((d) => d.backend !== "asio")
    .map((d) => ({
      id: d.id,
      name: d.name,
      channels: d.inputChannels,
      asio: false,
      isDefault: d.isDefault,
    }));
  return [...asio, ...host];
}

/**
 * Er det valgt en lydkilde som faktisk FINNES?
 *
 * Statuslinjens `nosound` spurte bare om det sto noe i `deviceName`, og da
 * kunne skinnen si «Alt er klart» på nøyaktig den samme skjermen der spørsmål 1
 * sto gult og sa «Finner ikke Behringer X32». To sanne halvdeler som er uenige
 * i skjøten, side om side — nettopp formen `reference-seam-bugs` handler om, og
 * her synlig for en frivillig i ett blikk.
 *
 * `devices === null` (ikke lest ennå) faller tilbake på det lagrede: at vi ikke
 * har sett etter er ikke bevis for at enheten er borte. Men «det lagrede» er
 * en ID, ikke et navn — et navn uten id er ALDRI et valg noen har tatt.
 * `deviceName` alene kan bæres inn av en migrering, og opptakssidens
 * `sourceState` sperrer Start på nøyaktig det: `deviceId` tom ⇒ `no-source`.
 * Sa skinnen «Alt er klart» i det samme øyeblikket, var det to sanne halvdeler
 * som var uenige i skjøten — og den frivillige stod med en grønn statuslinje
 * over en Start-knapp hun ikke fikk trykke på.
 *
 * Ren og eksportert, så regelen kan tabelltestes ett sted — sammen med
 * `sourceState`, som er den andre halvdelen av den samme skjøten
 * (`status-line.test.ts`).
 */
export function soundChosen(
  stored: { deviceId: string | null; deviceName: string | null },
  devices: readonly { id: string }[] | null,
): boolean {
  const id = (stored.deviceId ?? "").trim();
  if (!id) return false;
  if (devices === null) return true;
  return devices.some((d) => d.id === id);
}

// ── Helingen: en lagret id som ikke lenger finnes ───────────────────────────
//
// Windows omfordeler enhets-id-er etter en omstart eller en driveroppdatering,
// og id-er skrevet av en bygg FØR enumereringen flyttet til bakenden er
// Web-Audio-hasher. Enheten er den samme, NAVNET står fortsatt, men id-en
// treffer ingenting — og siden kanalparet (`deviceChannels`) er nøklet PÅ ID,
// faller en Qu-5-rigg stille tilbake til kanal 1/2. Ingen finner det ut før
// opptaket er av feil kilde.
//
// `legacy/renderer/audio/capture.ts` hadde `healStoredDeviceId()` for dette.
// Den fulgte ikke med i det nye skallet (den leste et modulnivå-speil `app/`
// ikke fyller), og den var en navngitt restanse i `app/lib/audio/capture.ts`.
// Dette er den, bygget over `state/settings.ts` — og med to forskjeller:
//
//   • **Nøyaktig ÉN navnetreffer.** Legacy tok `find()`, altså den første av
//     flere like. To enheter med samme navn (to identiske USB-kort, en
//     ASIO-enhet og dens WASAPI-skygge) er ikke en heling, det er en gjetning
//     — og en gjetning som peker opptaket på feil boks er verre enn å la
//     `source-missing` si ærlig fra.
//   • **Kanalparet FLYTTES,** ikke kopieres. Legacy lot den gamle
//     oppføringen bli liggende; da vokser kartet med én død nøkkel per
//     omfordeling, og en id som en dag brukes om igjen arver et par ingen
//     valgte.
//
// Ren og eksportert, så betingelsen kan tabelltestes uten en enhetsliste.

/** Nøklene helingen vil skrive, eller `null` når det ikke er noe å hele. */
export interface DeviceHeal {
  deviceId: string;
  /** Hele det nye kartet — bare med når det gamle bar et kanalpar. */
  deviceChannels?: { [key in string]: DeviceChannels };
}

/** Det helingen trenger å vite om det som står lagret. */
export interface StoredDevice {
  deviceId: string | null;
  deviceName: string | null;
  deviceChannels?: { [key in string]: DeviceChannels } | null;
}

/**
 * Skal den lagrede enhets-id-en pekes på nytt?
 *
 * Betingelsen, hele veien: det står BÅDE en id og et navn lagret · listen ER
 * lest · id-en treffer ingen enhet i den · nøyaktig ÉN enhet har nøyaktig det
 * lagrede navnet (trimmet, uten hensyn til store bokstaver) · og den enhetens
 * id er en annen enn den lagrede. Alt annet ⇒ `null`, altså la det stå.
 */
export function planDeviceHeal(
  stored: StoredDevice,
  devices: readonly { id: string; name: string }[] | null,
): DeviceHeal | null {
  if (devices === null) return null;
  const id = (stored.deviceId ?? "").trim();
  const name = (stored.deviceName ?? "").trim();
  if (!id || !name) return null;
  if (devices.some((d) => d.id === id)) return null;

  const wanted = name.toLowerCase();
  const matches = devices.filter((d) => d.name.trim().toLowerCase() === wanted);
  if (matches.length !== 1) return null;
  const found = matches[0];
  if (found.id === id) return null;

  const heal: DeviceHeal = { deviceId: found.id };
  const pair = stored.deviceChannels?.[id];
  if (pair) {
    const next = { ...(stored.deviceChannels ?? {}) };
    delete next[id];
    next[found.id] = pair;
    heal.deviceChannels = next;
  }
  return heal;
}

/**
 * Kjør helingen mot det som nettopp ble lest, hvis den gjelder.
 *
 * Stille — men ikke usynlig: en logglinje, som legacy. Det er ikke en endring
 * brukeren gjorde, og en toast om noe appen ordnet selv er støy hun ikke kan
 * gjøre noe med. Skrivningen går gjennom den VANLIGE lagringen, ikke utenom:
 * en heling som bare levde i minnet ville vært borte ved neste oppstart, og
 * en som skrev direkte til et lager ville vært den andre døra inn i
 * innstillingene.
 */
async function healStoredDevice(
  devices: readonly AudioDeviceOption[],
): Promise<void> {
  const s = settings.peek();
  const heal = planDeviceHeal(s, devices);
  if (!heal) return;
  console.info(
    "[devices] lagret enhets-id fantes ikke — peker «%s» på nytt: %s → %s",
    s.deviceName,
    s.deviceId,
    heal.deviceId,
  );
  patchSettings(heal as Partial<Settings>);
  if (!(await saveSettingsDebounced(120))) {
    // Ikke rull tilbake: den lagrede id-en er allerede ugyldig, og å sette den
    // tilbake ville betydd at opptaket peker på ingenting. Skjermen viser den
    // riktige enheten, neste ekte lagring bærer den med seg, og helingen kjøres
    // uansett på nytt ved neste enhetslesning.
    console.warn("[devices] helingen ble ikke lagret — prøver igjen senere");
  }
}

/** Lydinngangene. `null` = ikke lest ennå. */
export const audioDevices = signal<AudioDeviceOption[] | null>(null);

/** Kameraene. `null` = ikke lest ennå. */
export const videoDevices = signal<Array<{
  name: string;
  index: number;
}> | null>(null);

/**
 * Les lydinngangene.
 *
 * En feilet lesning lander på tom liste og ikke på `null`: shimmen svarer
 * allerede med `[]` for en avvist kommando, og å bli stående på «ikke lest»
 * for alltid ville gitt et kort som aldri sier noe. Tomt er det ærlige svaret
 * — og tomtilstanden sier hva man gjør med det.
 */
export async function loadAudioDevices(): Promise<void> {
  try {
    const list = toDeviceOptions(await window.api.listAudioDevices());
    audioDevices.value = list;
    // ETTER at signalet er satt: helingen skriver innstillinger, og skjermen
    // skal kunne finne den nye id-en i listen den allerede har.
    await healStoredDevice(list);
  } catch (err) {
    console.warn("[devices] kunne ikke lese lydenhetene:", err);
    audioDevices.value = [];
  }
}

export async function loadVideoDevices(): Promise<void> {
  try {
    videoDevices.value = await window.api.listVideoDevices();
  } catch (err) {
    console.warn("[devices] kunne ikke lese kameraene:", err);
    videoDevices.value = [];
  }
}
