/**
 * Gjenkoblingsstripa, og de tre veiene ned.
 *
 * ## ⚠️ Den hadde bare én, og den kom ikke alltid
 *
 * `recording://warning` er bakendens KLASSIFISERTE ikke-terminale feil. Noen av
 * dem er en frakobling som ender i `recording://reconnected`; andre er ikke.
 * Handleren satte `reconnecting = true` for dem alle, og INGENTING annet skrev
 * flagget ned. Et hikst som ikke endte i et gjenkoblingsevent etterlot
 * «Kobler til igjen …» stående over et opptak som gikk helt fint — ut
 * gudstjenesten, og inn i den neste.
 *
 * To veier ned nå, og ingen av dem er avhengig av `reconnected`: motorens EGEN
 * tilstand (`RecorderState` har «reconnecting» i vokabularet sitt, så
 * «recording» er dens kvittering på at den er tilbake), og krysset.
 */

import { afterEach, describe, expect, it } from "vitest";

import {
  dismissReconnecting,
  initRecording,
  isRecording,
  markSessionStarted,
  reconnecting,
  scheduledStopMs,
  silenceActive,
} from "./recording";
import { banners, clearBanners } from "./banners";

interface Harness {
  emit: (channel: string, payload?: unknown) => void;
  off: () => void;
}

/** Et minimalt `window.api` med bare `on`, og en vei til å fyre kanalene. */
function withFakeApi(): Harness {
  const handlers = new Map<string, Array<(p: unknown) => void>>();
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      on(channel: string, fn: (p: unknown) => void) {
        handlers.set(channel, [...(handlers.get(channel) ?? []), fn]);
        return () => {};
      },
    },
  };
  const dispose = initRecording();
  return {
    emit: (channel, payload = null) =>
      (handlers.get(channel) ?? []).forEach((h) => h(payload)),
    off: () => {
      dispose();
      delete (globalThis as unknown as { window?: unknown }).window;
    },
  };
}

afterEach(() => {
  clearBanners();
  reconnecting.value = false;
  silenceActive.value = false;
  isRecording.value = false;
  scheduledStopMs.value = null;
});

describe("gjenkoblingsstripa", () => {
  it("reises av et ikke-terminalt varsel — økta lever", () => {
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-warning", { code: "device_hiccup" });
    expect(reconnecting.value).toBe(true);
    // Og overlegget står fortsatt: dette er ikke en terminal feil.
    expect(isRecording.value).toBe(true);
    h.off();
  });

  // MUTASJONSPRØVEN: fjern `if (st) reconnecting.value = st === "reconnecting"`
  // fra `recording-overlay-stop`-handleren, og denne blir rød.
  it("ryddes av neste tilstandsemit som sier «recording»", () => {
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-warning", {});
    expect(reconnecting.value).toBe(true);

    h.emit("recording-overlay-stop", { state: "recording" });
    expect(reconnecting.value).toBe(false);
    h.off();
  });

  it("…og REISES av en tilstandsemit som sier «reconnecting»", () => {
    // Den samme linja begge veier: motoren har ordet i sitt eget vokabular, og
    // da er det motoren som eier svaret — ikke et event som kanskje kommer.
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-overlay-stop", { state: "reconnecting" });
    expect(reconnecting.value).toBe(true);
    expect(isRecording.value).toBe(true);
    h.off();
  });

  it("ryddes IKKE av måleren — tall er ikke det samme som «enheten er tilbake»", () => {
    // Den nærliggende tredje veien ned, og den er feil: en motor som har byttet
    // til en reservekilde, eller som strømmer stillhet mens den prøver igjen,
    // måler også. `e2e/record.spec.ts` pinner det samme utenfra: gjenkobling og
    // stillhet er TO varsler, og bare det ene ryddes av lyd.
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-warning", {});
    h.emit("recording-levels", { peak_db_left: -14, peak_db_right: -15 });
    expect(reconnecting.value).toBe(true);
    h.off();
  });

  it("ryddes av krysset — den som hører at lyden er tilbake har rett", () => {
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-warning", {});
    dismissReconnecting();
    expect(reconnecting.value).toBe(false);
    h.off();
  });

  it("ryddes av `recording://reconnected` som før — veien som fantes består", () => {
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-reconnecting");
    expect(reconnecting.value).toBe(true);
    h.emit("recording-reconnected");
    expect(reconnecting.value).toBe(false);
    h.off();
  });

  it("arves ikke av neste økt", () => {
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-warning", {});
    h.emit("recording-overlay-stop", { state: "stopped" });
    expect(isRecording.value).toBe(false);
    expect(reconnecting.value).toBe(false);
    h.off();
  });
});

// ── Kvalitetsalarmens årsaker: kode eller prosa ─────────────────────────────

describe("kvalitetsalarmen skiller «ingen koder» fra «koder finnes ikke»", () => {
  function qualityBanner() {
    const b = banners.value.find((x) => x.key === "recording-quality");
    if (!b || b.key !== "recording-quality") throw new Error("intet banner");
    return b;
  }

  // MUTASJONSPRØVEN: bytt `Array.isArray(rawCodes) ? … : null` mot
  // `rawCodes ?? []`, og den første blir rød — en eldre motor ville da fått
  // sin norske prosa skjult i stedet for vist.
  it("en ELDRE motor uten feltet gir `null`, og prosaen får stå", () => {
    const h = withFakeApi();
    h.emit("recording-quality", {
      measuredSec: 3120,
      expectedSec: 5400,
      reasons: ["3.42s manglende/stille lyd — hakking/dropp"],
    });
    expect(qualityBanner().reasonCodes).toBeNull();
    expect(qualityBanner().reasons).toHaveLength(1);
    h.off();
  });

  it("en motor MED feltet gir kodene, og prosaen blir bare reserve", () => {
    const h = withFakeApi();
    h.emit("recording-quality", {
      measuredSec: 3120,
      expectedSec: 5400,
      reasons: ["3.42s manglende/stille lyd — hakking/dropp"],
      reasonCodes: ["gap_fail"],
    });
    expect(qualityBanner().reasonCodes).toEqual(["gap_fail"]);
    h.off();
  });

  it("leser også snake_case, så en serde-omdøping ikke slår oversettelsen av", () => {
    const h = withFakeApi();
    h.emit("recording-quality", {
      measuredSec: 1,
      expectedSec: 2,
      reasons: [],
      reason_codes: ["low_signal"],
    });
    expect(qualityBanner().reasonCodes).toEqual(["low_signal"]);
    h.off();
  });

  it("et TOMT kodefelt er ikke det samme som et manglende", () => {
    const h = withFakeApi();
    h.emit("recording-quality", {
      measuredSec: 1,
      expectedSec: 2,
      reasons: ["noe"],
      reasonCodes: [],
    });
    expect(qualityBanner().reasonCodes).toEqual([]);
    h.off();
  });
});
