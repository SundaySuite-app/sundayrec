/**
 * Bannerkøen — det som gikk galt og blir stående til noen har sett det.
 *
 * ## Hvorfor en egen kø, og ikke en toast
 *
 * Legacy har `banner(id, kind, msg, actions)` i `ui/toast.ts`, og kommentaren
 * over den sier hvorfor den ikke er en toast: «Something is wrong and stays
 * wrong.» Et opptak som ble avbrutt kl. 11:42 er ikke en beskjed som skal
 * forsvinne av seg selv mens den som skulle lese den henter kaffe.
 *
 * NØKLET, som legacy: en enhet som kobler seg fra fem ganger oppdaterer ETT
 * banner i stedet for å stable fem. Og lukket er lukket — helt til tilstanden
 * oppstår på nytt.
 *
 * ## Data, ikke tekst
 *
 * Køen bærer FAKTA («fila holder 3120 av 5400 sekunder»), ikke ferdige
 * setninger. Grunnen er den samme som i `decisions-core`: en ferdig streng her
 * ville betydd at butikken valgte språk, og et banner som ble reist på norsk
 * ville blitt stående på norsk gjennom et språkbytte. Siden oversetter.
 *
 * ## Hva som IKKE bor her
 *
 * `scheduler://missed` og `scheduler://preflight` har allerede en butikk
 * (`state/next-recording.ts`, med `dismissMissed` og `dismissPreflight`), og
 * lite diskplass er AVLEDET av `state/disk.ts`. De rendres som bannere av
 * opptakssiden, men tilstanden deres bor der den alltid har bodd — to butikker
 * for det samme svaret er nøyaktig skjøten hele skallet er skrevet for å unngå.
 */

import { signal } from "@preact/signals";

/** Nøklene. Lukket liste: et nytt banner er en beslutning, ikke noe som siger
 *  inn i en tilfeldig handler. */
export type BannerKey = "recording-error" | "recording-quality";

export type BannerData =
  /**
   * Opptaket ble avbrutt. `atMs` er halve informasjonen — «kl. 11:42» er det
   * som lar noen finne ut hva som skjedde i rommet akkurat da.
   */
  | {
      key: "recording-error";
      atMs: number;
      /** Motorens stabile kode (`device_disconnected`, …), oversettes av siden. */
      code: string | null;
      /** Motorens egen detaljlinje. Diagnostikk, ikke UI-tekst. */
      message: string | null;
    }
  /**
   * Sannhetsmålingen ved øktslutt sa nei: fila holder beviselig mindre lyd enn
   * økta varte. Alarmen 2026-07-31-hendelsen manglet.
   */
  | {
      key: "recording-quality";
      measuredSec: number;
      expectedSec: number;
      reasons: readonly string[];
    };

/** Bannerne som står nå, eldst først. */
export const banners = signal<readonly BannerData[]>([]);

/**
 * Reis et banner, eller oppdater det som allerede står med samme nøkkel.
 *
 * Erstatning PÅ PLASS: rekkefølgen er den de oppsto i, og et banner som
 * oppdaterer seg skal ikke hoppe til bunnen av stabelen mens noen leser det.
 */
export function raiseBanner(next: BannerData): void {
  const current = banners.peek();
  const at = current.findIndex((b) => b.key === next.key);
  if (at < 0) {
    banners.value = [...current, next];
    return;
  }
  const copy = current.slice();
  copy[at] = next;
  banners.value = copy;
}

/** Lukk ett banner. Idempotent. */
export function dismissBanner(key: BannerKey): void {
  const current = banners.peek();
  const after = current.filter((b) => b.key !== key);
  if (after.length !== current.length) banners.value = after;
}

/** Tøm alt. For teardown og tester — aldri for en vanlig navigasjon. */
export function clearBanners(): void {
  if (banners.peek().length > 0) banners.value = [];
}
