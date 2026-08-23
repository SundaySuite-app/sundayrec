/**
 * Opptakets EGEN telemetri som VU-kilde.
 *
 * Under en økt eier opptaksmotoren enheten — `start_recording` stopper
 * VU-strømmen selv — og den delte `acquireVuFeed` ville derfor bedt om den
 * samme enheten opptaket holder, midt i en gudstjeneste. `recording://levels`
 * er per-kanals topp-dBFS regnet av motorens egen ringbuffer, altså av lyden
 * som FAKTISK blir tatt opp. Måleren i overlegget viser dermed opptaket, ikke
 * en ANDRE avlesning av det samme rommet.
 *
 * Det er den samme veien legacy-overlegget går (`startLevelsMeter` i
 * `pages/recording.ts`), og grunnen står der: mikrofonen åpnes NØYAKTIG én
 * gang. To åpninger av den innebygde mikrofonen fikk macOS til å konfigurere
 * den delte enheten på nytt og slippe sampler — hakkete opptak.
 *
 * ## Ingen RMS
 *
 * Payloaden har bare topper (`peak_db_left` / `peak_db_right`). Stolpene
 * tegner derfor toppen, ikke RMS-en. Det er også det legacy-overlegget gjør,
 * og forskjellen er synlig: nålen står litt høyere enn på hjem-måleren.
 *
 * ## Modulnivå, ikke en lukning
 *
 * `VuMeter` har `source` i avhengighetslista si. En ny funksjon per render
 * ville revet abonnementet opp og satt det opp igjen 60 ganger i sekundet —
 * på den ene flaten der et tapt abonnement betyr en død måler over et opptak
 * som går.
 */

import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";
import type { RecordingLevels } from "@lib/../bindings/RecordingLevels";

import type { VuSource } from "../../ui/VuMeter/VuMeter";

export const recordingLevelsSource: VuSource = (emit) => {
  const off = window.api.on("recording-levels", (payload: unknown) => {
    const d = payload as RecordingLevels | undefined;
    const left =
      typeof d?.peak_db_left === "number" ? d.peak_db_left : VU_FLOOR_DB;
    const right = d?.peak_db_right;
    // `null` er MONO og ikke «høyre er stille»: en tom andre stolpe ville
    // sett ut som en død høyrekanal på et opptak som er helt i orden.
    const mono = right === null || right === undefined;
    const r = mono ? left : right;
    emit({ l: left, r, peakL: left, peakR: r, mono });
  });
  return () => off?.();
};
