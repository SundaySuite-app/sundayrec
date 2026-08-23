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

import type { TaggedAudioInput } from "@lib/../bindings/TaggedAudioInput";

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
 * har sett etter er ikke bevis for at enheten er borte. TA OPP-siden leser ikke
 * enhetslisten, så der er svaret det samme som før.
 *
 * Ren og eksportert, så regelen kan tabelltestes ett sted.
 */
export function soundChosen(
  stored: { deviceId: string | null; deviceName: string | null },
  devices: readonly { id: string }[] | null,
): boolean {
  const id = (stored.deviceId ?? "").trim();
  const name = (stored.deviceName ?? "").trim();
  if (!id && !name) return false;
  if (devices === null) return true;
  return !!id && devices.some((d) => d.id === id);
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
    audioDevices.value = toDeviceOptions(await window.api.listAudioDevices());
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
