import { describe, expect, it } from "vitest";

import { planDeviceHeal, soundChosen, toDeviceOptions } from "./devices";
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

  it("listen ikke lest ⇒ den lagrede ID-en får stå", () => {
    // At vi ikke har sett etter er ikke bevis for at enheten er borte.
    expect(soundChosen(chosen, null)).toBe(true);
  });

  it("navn uten id, og listen IKKE lest ⇒ nei", () => {
    // Funnet: dette svarte `true`, så skinnen sa «Alt er klart» i det
    // halvsekundet før enumereringen svarte — over en Start-knapp
    // `sourceState` sperret, fordi `deviceId` er tom. Et navn kan bæres inn av
    // en migrering; det er ikke et valg noen har tatt.
    expect(
      soundChosen({ deviceId: null, deviceName: "Behringer X32" }, null),
    ).toBe(false);
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

describe("planDeviceHeal — en lagret id som ikke lenger finnes", () => {
  const PAIR = { channelL: 14, channelR: 15 };
  const stored = {
    deviceId: "gammel-id",
    deviceName: "Behringer X32",
    deviceChannels: { "gammel-id": PAIR },
  };
  const x32 = { id: "ny-id", name: "Behringer X32" };

  it("navnetreff under en ny id ⇒ pek om, og TA KANALPARET MED", () => {
    // Uten dette faller en Qu-5-rigg stille tilbake til kanal 1/2 etter en
    // Windows-omstart, og ingen finner det ut før opptaket er av feil kilde.
    expect(planDeviceHeal(stored, [x32])).toEqual({
      deviceId: "ny-id",
      deviceChannels: { "ny-id": PAIR },
    });
  });

  it("den gamle nøkkelen FLYTTES, den blir ikke liggende igjen", () => {
    const heal = planDeviceHeal(stored, [x32])!;
    expect(Object.keys(heal.deviceChannels!)).toEqual(["ny-id"]);
  });

  it("uten et lagret kanalpar skrives bare id-en", () => {
    expect(
      planDeviceHeal({ deviceId: "gammel-id", deviceName: "Behringer X32" }, [
        x32,
      ]),
    ).toEqual({ deviceId: "ny-id" });
  });

  it("TO enheter med samme navn ⇒ ingen heling", () => {
    // En gjetning som peker opptaket på feil boks er verre enn et ærlig
    // «finner ikke enheten». To identiske USB-kort er et ekte oppsett.
    expect(
      planDeviceHeal(stored, [
        { id: "a", name: "Behringer X32" },
        { id: "b", name: "Behringer X32" },
      ]),
    ).toBeNull();
  });

  it("id-en finnes fortsatt ⇒ ingenting å hele", () => {
    expect(
      planDeviceHeal(stored, [
        { id: "gammel-id", name: "Behringer X32" },
        { id: "ny-id", name: "Behringer X32" },
      ]),
    ).toBeNull();
  });

  it("listen ikke lest ⇒ ingen heling (og ingen skrivning)", () => {
    expect(planDeviceHeal(stored, null)).toBeNull();
  });

  it("navnet mangler ⇒ ingenting å matche på", () => {
    expect(
      planDeviceHeal({ deviceId: "gammel-id", deviceName: null }, [x32]),
    ).toBeNull();
  });

  it("ingen id lagret ⇒ ikke en heling, det er et valg som aldri er tatt", () => {
    expect(
      planDeviceHeal({ deviceId: null, deviceName: "Behringer X32" }, [x32]),
    ).toBeNull();
  });

  it("navnet matches trimmet og uten hensyn til store bokstaver", () => {
    expect(
      planDeviceHeal({ deviceId: "gammel-id", deviceName: " behringer x32 " }, [
        x32,
      ]),
    ).toEqual({ deviceId: "ny-id" });
  });

  it("et annet navn er en annen enhet", () => {
    expect(
      planDeviceHeal(stored, [{ id: "ny-id", name: "Focusrite" }]),
    ).toBeNull();
  });
});
