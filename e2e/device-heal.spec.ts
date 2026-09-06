import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// Helingen av en lagret enhets-id som ikke lenger finnes.
//
// Windows omfordeler enhets-id-er etter en omstart eller en driveroppdatering,
// og en id skrevet av en bygg fra før enumereringen flyttet til bakenden er en
// Web-Audio-hash. Enheten er den samme og NAVNET står, men id-en treffer
// ingenting — og fordi kanalparet er nøklet PÅ ID faller en Qu-5-rigg stille
// tilbake til kanal 1/2. Ingen finner det ut før opptaket er av feil kilde.
//
// `legacy/renderer/audio/capture.ts` hadde `healStoredDeviceId()`; den fulgte
// ikke med i det nye skallet. Dette er den, gjenoppbygget over
// `app/state/devices.ts` — og beslutningen selv er tabelltestet i
// `app/state/devices.test.ts`. Her er beviset for at den faktisk KJØRER, og at
// den skriver gjennom den vanlige lagringen.

/** Én rad fra `list_audio_devices`, i bakendens form. */
function device(id: string, name: string) {
  return {
    id,
    name,
    backend: "wasapi",
    inputChannels: 32,
    sampleRates: [48000],
    isDefault: false,
  };
}

const OLD_PAIR = { channelL: 14, channelR: 15 };

test.describe("enhets-id som ble omfordelt", () => {
  test("navnetreff under en ny id peker om — og KANALPARET følger med", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        list_audio_devices: [device("ny-id", "Behringer X32")],
      },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: "gammel-id",
        deviceName: "Behringer X32",
        deviceChannels: { "gammel-id": OLD_PAIR },
      },
      goto: "home",
    });

    // Lagringslaget er fasiten: helingen skal PERSISTERE, ikke bare leve i
    // minnet til neste oppstart.
    await expect
      .poll(async () => (await storedSettings(page)).deviceId)
      .toBe("ny-id");
    await expect
      .poll(async () => (await storedSettings(page)).deviceChannels)
      // FLYTTET, ikke kopiert: den gamle nøkkelen blir ikke liggende igjen og
      // vente på en id som en dag brukes om igjen.
      .toEqual({ "ny-id": OLD_PAIR });

    // …og den gikk gjennom `settings_save` som alt annet, ikke utenom.
    const payloads = await settingsSavePayloads(page);
    expect(payloads.length).toBeGreaterThan(0);
    expect(payloads[payloads.length - 1].deviceId).toBe("ny-id");
  });

  test("to enheter med samme navn ⇒ ingen gjetning", async ({ page }) => {
    // En heling som peker opptaket på feil boks er verre enn et ærlig «finner
    // ikke enheten». To identiske USB-kort er et ekte oppsett.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        list_audio_devices: [
          device("a", "Behringer X32"),
          device("b", "Behringer X32"),
        ],
      },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: "gammel-id",
        deviceName: "Behringer X32",
        deviceChannels: { "gammel-id": OLD_PAIR },
      },
      goto: "home",
    });

    // «Finner ikke Behringer X32» — enhetslista ER lest, altså har helingen
    // hatt sin sjanse og valgt å la det stå. Det ærlige svaret er nettopp
    // dette kortet, ikke en id vi fant på.
    await expect(page.getByTestId("record-source-missing")).toBeVisible();
    expect((await storedSettings(page)).deviceId).toBe("gammel-id");
    expect((await storedSettings(page)).deviceChannels).toEqual({
      "gammel-id": OLD_PAIR,
    });
  });
});
