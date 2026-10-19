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

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  dismissReconnecting,
  enterFinalizing,
  FINALIZE_TIMEOUT_MS,
  finalizing,
  finishedRecording,
  forgetMovedPath,
  hydrateRecordingState,
  initRecording,
  isRecording,
  markSessionStarted,
  recorderState,
  reconnecting,
  scheduledStopMs,
  sessionCameraLost,
  sessionVideoDropped,
  sessionVideoProblem,
  silenceActive,
} from "./recording";
import { toasts } from "../ui/toast";
import { banners, clearBanners } from "./banners";
import type { StatePayload } from "./recording-hydrate-core";

interface Harness {
  emit: (channel: string, payload?: unknown) => void;
  off: () => void;
}

/** Hva `recordingSnapshot()` skal svare, og når. */
interface FakeApiOpts {
  /** Svaret oppstarts-snapshotet får. `undefined` = kommandoen finnes ikke. */
  snapshot?: StatePayload | null;
}

/** Et minimalt `window.api` med `on` (+ snapshotet), og en vei til å fyre kanalene. */
function withFakeApi(opts: FakeApiOpts = {}): Harness {
  const handlers = new Map<string, Array<(p: unknown) => void>>();
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      on(channel: string, fn: (p: unknown) => void) {
        handlers.set(channel, [...(handlers.get(channel) ?? []), fn]);
        return () => {};
      },
      recordingSnapshot: () => Promise.resolve(opts.snapshot ?? null),
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
  vi.useRealTimers();
  recorderState.value = null;
  finalizing.value = false;
  clearBanners();
  reconnecting.value = false;
  silenceActive.value = false;
  isRecording.value = false;
  scheduledStopMs.value = null;
  finishedRecording.value = null;
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

  it("en enhetsbytte-melding ved start er en toast — ikke «Kobler til igjen»", () => {
    const h = withFakeApi();
    markSessionStarted();
    const before = toasts.value.length;
    h.emit("recording-device-substituted", {
      code: "device_substituted",
      message: "Microphone Array (Realtek(R) Audio)",
    });
    expect(reconnecting.value).toBe(false);
    expect(banners.value).toHaveLength(0);
    const added = toasts.value.slice(before);
    expect(added).toHaveLength(1);
    expect(added[0]?.kind).toBe("warn");
    expect(added[0]?.msg).toContain("Microphone Array (Realtek(R) Audio)");
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
      reasonCodes: ["large-gap"],
    });
    expect(qualityBanner().reasonCodes).toEqual(["large-gap"]);
    h.off();
  });

  it("leser også snake_case, så en serde-omdøping ikke slår oversettelsen av", () => {
    const h = withFakeApi();
    h.emit("recording-quality", {
      measuredSec: 1,
      expectedSec: 2,
      reasons: [],
      reason_codes: ["low-signal"],
    });
    expect(qualityBanner().reasonCodes).toEqual(["low-signal"]);
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

// ── F2-9: kvitteringen glemmer en fil papirkurven eller retensjonen tok ────

describe("forgetMovedPath", () => {
  // ⚠️ FUNNET, og det er ekte. Ingenting kalte denne funksjonen før F2-9 —
  // `record-done-edit` (kvitteringens «Rediger»-knapp) åpnet `finished.path`
  // uten at noe visste at papirkurven eller retensjonen kunne ha flyttet
  // fila i mellomtiden.
  it("kvitteringen forsvinner når dens fil er blant de flyttede stiene", () => {
    finishedRecording.value = {
      path: "/Opptak/2026-08-23.flac",
      hasVideo: false,
      atMs: 0,
      recordingId: null,
    };
    forgetMovedPath(["/annet.flac", "/Opptak/2026-08-23.flac"]);
    expect(finishedRecording.value).toBeNull();
  });

  it("en ANNEN fil i papirkurven rører ikke kvitteringen", () => {
    const receipt = {
      path: "/Opptak/2026-08-23.flac",
      hasVideo: false,
      atMs: 0,
      recordingId: null,
    };
    finishedRecording.value = receipt;
    forgetMovedPath(["/Opptak/en-helt-annen-dag.flac"]);
    expect(finishedRecording.value).toBe(receipt);
  });

  it("ingen kvittering å glemme er ikke en feil", () => {
    finishedRecording.value = null;
    expect(() => forgetMovedPath(["/hva-som-helst.flac"])).not.toThrow();
    expect(finishedRecording.value).toBeNull();
  });

  it("en tom liste med flyttede stier lar kvitteringen stå", () => {
    const receipt = {
      path: "/Opptak/2026-08-23.flac",
      hasVideo: false,
      atMs: 0,
      recordingId: null,
    };
    finishedRecording.value = receipt;
    forgetMovedPath([]);
    expect(finishedRecording.value).toBe(receipt);
  });
});

/**
 * F2-T5: skjøten mellom kappløpsregelen (tabelltestet i
 * `recording-hydrate-core.test.ts`) og skallet som teller hendelsene.
 *
 * Regelen kan være riktig og likevel virkningsløs: teller ingen opp, holder
 * `snapshotStillApplies` alltid, og et utdatert svar vinner over motoren. Det
 * er nøyaktig formen skjøtefeil har, så den prøves her — mot de EKTE
 * lytterne, ikke mot en kopi.
 */
describe("oppstarts-snapshotet", () => {
  const RECORDING: StatePayload = {
    state: "recording",
    reconnect_count: 0,
    scheduled_stop_ms: 1_700_000_000_000,
  };

  it("reiser overlegget etter en reload midt i et opptak", async () => {
    const h = withFakeApi({ snapshot: RECORDING });
    expect(isRecording.value).toBe(false);
    await hydrateRecordingState();
    expect(isRecording.value).toBe(true);
    // Nedtellingen kommer med, i den samme runden.
    expect(scheduledStopMs.value).toBe(1_700_000_000_000);
    h.off();
  });

  it("tar med gjenkoblingen, så stripa står etter en reload midt i den", async () => {
    const h = withFakeApi({
      snapshot: { ...RECORDING, state: "reconnecting", reconnect_count: 3 },
    });
    await hydrateRecordingState();
    expect(isRecording.value).toBe(true);
    expect(reconnecting.value).toBe(true);
    h.off();
  });

  it("forkastes av en hendelse som landet mens spørsmålet var i flukt", async () => {
    // MUTASJONSPRØVEN: fjern `stateGeneration += 1` fra
    // `recording-overlay-stop`-handleren, og denne blir rød — snapshotet
    // maler «tar opp» over en økt som er slutt, og ingen ny hendelse kommer
    // for å rette det opp.
    const h = withFakeApi({ snapshot: RECORDING });
    const inFlight = hydrateRecordingState();
    h.emit("recording-overlay-stop", { state: "stopped" });
    await inFlight;
    expect(isRecording.value).toBe(false);
    h.off();
  });

  it("river ikke kvitteringen når opptaket ble ferdig mens vi spurte", async () => {
    // `markSessionStarted()` nullstiller `finishedRecording`. Et snapshot som
    // sier «recording» og lander etter at opptaket faktisk ble ferdig ville
    // altså tatt kvitteringen med seg — filen brukeren nettopp fikk beskjed om.
    const h = withFakeApi({ snapshot: RECORDING });
    const inFlight = hydrateRecordingState();
    h.emit("recording-finished", { file_path: "/tmp/gudstjeneste.flac" });
    await inFlight;
    expect(isRecording.value).toBe(false);
    expect(finishedRecording.value?.path).toBe("/tmp/gudstjeneste.flac");
    h.off();
  });

  it("kvitteringen bærer radens id fra Rust, og en hendelse uten id gir ingen id", () => {
    const h = withFakeApi({ snapshot: RECORDING });
    h.emit("recording-finished", {
      file_path: "/tmp/a.flac",
      recording_id: "rad-1",
    });
    expect(finishedRecording.value?.recordingId).toBe("rad-1");
    h.emit("recording-finished", { file_path: "/tmp/b.flac" });
    expect(finishedRecording.value?.recordingId).toBeNull();
    h.emit("recording-finished", {
      file_path: "/tmp/c.flac",
      recording_id: "",
    });
    expect(finishedRecording.value?.recordingId).toBeNull();
    h.off();
  });

  it("lar kvitteringen stå når motoren svarer «idle»", async () => {
    // Den vanlige oppstarten rett etter et opptak: motoren ER idle, og det
    // skal ikke fjerne kvitteringen som allerede står på skjermen.
    const h = withFakeApi({
      snapshot: { state: "idle", reconnect_count: 0, scheduled_stop_ms: null },
    });
    finishedRecording.value = {
      path: "/tmp/forrige.flac",
      hasVideo: false,
      atMs: 1,
      recordingId: null,
    };
    await hydrateRecordingState();
    expect(isRecording.value).toBe(false);
    expect(finishedRecording.value?.path).toBe("/tmp/forrige.flac");
    h.off();
  });

  it("«vi vet ikke» er ikke «ingenting går»", async () => {
    // Shimmens pessimistiske reserve. Et opptak som ALLEREDE er kjent skal
    // ikke rives ned fordi kommandoen ikke svarte.
    const h = withFakeApi({ snapshot: null });
    markSessionStarted();
    await hydrateRecordingState();
    expect(isRecording.value).toBe(true);
    h.off();
  });
});

describe("ferdig-melding og fullfører-frist", () => {
  it("en ferdig-melding fra en ERSTATTET økt river ikke ned den nye", () => {
    // MUTASJONSPRØVEN: fjern `if (d?.superseded === true) return;` fra
    // `recording-finished`-handleren, og denne blir rød — den gamle økta sin
    // sene melding setter `isRecording = false` mens det nye opptaket går.
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-finished", {
      file_path: "/tmp/gammel.mp3",
      recording_id: "rad-gammel",
      superseded: true,
    });
    expect(isRecording.value).toBe(true);
    expect(finishedRecording.value).toBeNull();
    // Den AKTIVE økta sin melding virker som før (felt fraværende = ikke erstattet).
    h.emit("recording-finished", { file_path: "/tmp/ny.mp3" });
    expect(isRecording.value).toBe(false);
    expect(finishedRecording.value?.path).toBe("/tmp/ny.mp3");
    h.off();
  });

  it("fristen slår ikke til mens motoren fortsatt melder «stopping»", () => {
    // MUTASJONSPRØVEN: fjern `recorderState.peek() === "stopping"`-grenen i
    // `armFinalizeTimer`, og denne blir rød — overlegget forsvinner etter 30 s
    // midt i en 90-minutters koding.
    vi.useFakeTimers();
    const h = withFakeApi();
    markSessionStarted();
    h.emit("recording-overlay-stop", { state: "stopping" });
    enterFinalizing();
    vi.advanceTimersByTime(FINALIZE_TIMEOUT_MS * 3 + 1);
    expect(isRecording.value).toBe(true);
    expect(finalizing.value).toBe(true);
    // Når motoren er ferdig, rydder den terminale tilstanden som vanlig.
    h.emit("recording-overlay-stop", { state: "stopped" });
    expect(isRecording.value).toBe(false);
    expect(finalizing.value).toBe(false);
    h.off();
  });

  it("fristen er fortsatt bakstopperen når motoren ikke har sagt «stopping»", () => {
    vi.useFakeTimers();
    const h = withFakeApi();
    markSessionStarted();
    enterFinalizing();
    vi.advanceTimersByTime(FINALIZE_TIMEOUT_MS + 1);
    expect(isRecording.value).toBe(false);
    expect(finalizing.value).toBe(false);
    h.off();
  });
});

describe("tapte kamerabilder (C3c)", () => {
  it("følger framdriftshendelsen oppover, og nullstilles av en ny økt", () => {
    const h = withFakeApi();
    markSessionStarted();
    expect(sessionVideoDropped.value).toBe(0);
    h.emit("recording-progress", {
      bytes_written: 10,
      video_frames_dropped: 5,
    });
    expect(sessionVideoDropped.value).toBe(5);
    // Et sent eller gammelt event skal aldri skru tallet ned midt i økta.
    h.emit("recording-progress", {
      bytes_written: 20,
      video_frames_dropped: 3,
    });
    expect(sessionVideoDropped.value).toBe(5);
    // Et event uten feltet (lydopptak, eldre motor) rører det ikke.
    h.emit("recording-progress", { bytes_written: 30 });
    expect(sessionVideoDropped.value).toBe(5);
    markSessionStarted();
    expect(sessionVideoDropped.value).toBe(0);
    h.off();
  });
});

describe("kameraet er borte (C5b)", () => {
  it("vises ved utfall, forsvinner ved gjenkobling, og nullstilles av en ny økt", () => {
    const h = withFakeApi();
    markSessionStarted();
    expect(sessionCameraLost.value).toBe(false);
    // Kameraet leverer: feltet er null.
    h.emit("recording-progress", { bytes_written: 10, video_lost_ms: null });
    expect(sessionCameraLost.value).toBe(false);
    // Utfall: meldingen skal opp med en gang, lenge før 60 s.
    h.emit("recording-progress", { bytes_written: 20, video_lost_ms: 1_000 });
    expect(sessionCameraLost.value).toBe(true);
    // Hvert sekund sier det samme; verdien endrer seg ikke.
    h.emit("recording-progress", { bytes_written: 30, video_lost_ms: 2_000 });
    h.emit("recording-progress", { bytes_written: 40, video_lost_ms: 61_000 });
    expect(sessionCameraLost.value).toBe(true);
    // Et event uten feltet (lydopptak, eldre motor) rører det ikke.
    h.emit("recording-progress", { bytes_written: 45 });
    expect(sessionCameraLost.value).toBe(true);
    // Gjenkobling: borte igjen.
    h.emit("recording-progress", { bytes_written: 50, video_lost_ms: null });
    expect(sessionCameraLost.value).toBe(false);
    // Nytt utfall, og en ny økt midt i det skal ikke arve det.
    h.emit("recording-progress", { bytes_written: 60, video_lost_ms: 500 });
    expect(sessionCameraLost.value).toBe(true);
    markSessionStarted();
    expect(sessionCameraLost.value).toBe(false);
    h.off();
  });

  it("signalet skrives bare når verdien skifter, ikke hvert sekund", () => {
    const h = withFakeApi();
    markSessionStarted();
    let writes = 0;
    const off = sessionCameraLost.subscribe(() => writes++);
    const base = writes; // abonnementet kaller én gang med startverdien
    for (let s = 1; s <= 90; s++) {
      h.emit("recording-progress", {
        bytes_written: s,
        video_lost_ms: s * 1000,
      });
    }
    expect(writes - base).toBe(1);
    off();
    h.off();
  });
});

describe("bildet mangler mens lyden tas opp (M3R)", () => {
  it("hvert hjerteslag er fasit, og en ny økt arver ikke forrige", () => {
    const h = withFakeApi();
    markSessionStarted();
    expect(sessionVideoProblem.value).toBe(null);
    h.emit("recording-progress", { bytes_written: 10, video_problem: null });
    expect(sessionVideoProblem.value).toBe(null);
    h.emit("recording-progress", {
      bytes_written: 20,
      video_problem: "encoder_stopped",
    });
    expect(sessionVideoProblem.value).toBe("encoder_stopped");
    // Et event uten feltet (lydopptak, eldre motor) rører det ikke.
    h.emit("recording-progress", { bytes_written: 25 });
    expect(sessionVideoProblem.value).toBe("encoder_stopped");
    // Et nytt fragment med friskt bilde: borte igjen.
    h.emit("recording-progress", { bytes_written: 30, video_problem: null });
    expect(sessionVideoProblem.value).toBe(null);
    h.emit("recording-progress", {
      bytes_written: 40,
      video_problem: "camera_not_opened",
    });
    expect(sessionVideoProblem.value).toBe("camera_not_opened");
    markSessionStarted();
    expect(sessionVideoProblem.value).toBe(null);
    h.off();
  });

  it("signalet skrives bare når verdien skifter", () => {
    const h = withFakeApi();
    markSessionStarted();
    let writes = 0;
    const off = sessionVideoProblem.subscribe(() => writes++);
    const base = writes;
    for (let s = 1; s <= 60; s++) {
      h.emit("recording-progress", {
        bytes_written: s,
        video_problem: "camera_not_opened",
      });
    }
    expect(writes - base).toBe(1);
    off();
    h.off();
  });
});
