import { describe, expect, it } from "vitest";

import { soundChosen, toDeviceOptions } from "./devices";
import type { TaggedAudioInput } from "@legacy/bindings/TaggedAudioInput";

function input(over: Partial<TaggedAudioInput> = {}): TaggedAudioInput {
  return {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 32,
    sampleRates: [48000],
    isDefault: false,
    ...over,
  };
}

describe("toDeviceOptions", () => {
  it("gir ASIO-enheter `asio::`-prefikset, som legacy gjør", () => {
    // Prefikset er UI-ets håndtak: en ASIO-enhet og dens WASAPI-stereoskygge er
    // samme maskinvare, og uten to id-er betyr ikke et valg én av dem.
    expect(
      toDeviceOptions([input({ backend: "asio", name: "Focusrite USB ASIO" })]),
    ).toEqual([
      {
        id: "asio::Focusrite USB ASIO",
        name: "Focusrite USB ASIO",
        channels: 32,
        asio: true,
        isDefault: false,
      },
    ]);
  });

  it("setter ASIO først — det er den foretrukne, flerkanals veien inn", () => {
    const list = toDeviceOptions([
      input({ id: "mbp", name: "MacBook Pro Microphone", isDefault: true }),
      input({ backend: "asio", name: "Focusrite" }),
    ]);
    expect(list.map((d) => d.asio)).toEqual([true, false]);
  });

  it("lar en verts-enhet beholde sin egen id", () => {
    expect(toDeviceOptions([input()])[0].id).toBe("x32");
  });
});

describe("soundChosen", () => {
  const chosen = { deviceId: "x32", deviceName: "Behringer X32" };

  it("ingenting valgt ⇒ nei", () => {
    expect(soundChosen({ deviceId: null, deviceName: null }, [])).toBe(false);
  });

  it("listen ikke lest ⇒ det lagrede får stå", () => {
    // At vi ikke har sett etter er ikke bevis for at enheten er borte.
    expect(soundChosen(chosen, null)).toBe(true);
  });

  it("valgt enhet som FINNES ⇒ ja", () => {
    expect(soundChosen(chosen, [{ id: "x32" }])).toBe(true);
  });

  it("valgt enhet som er BORTE ⇒ nei", () => {
    // Uten denne kunne skinnen si «Alt er klart» på samme skjerm som spørsmål 1
    // sa «Finner ikke Behringer X32».
    expect(soundChosen(chosen, [{ id: "annen" }])).toBe(false);
  });

  it("et navn uten id teller ikke når listen ER lest", () => {
    expect(
      soundChosen({ deviceId: null, deviceName: "Behringer X32" }, [
        { id: "x32" },
      ]),
    ).toBe(false);
  });
});
