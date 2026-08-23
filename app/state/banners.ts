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

/**
 * Bakendens fire advarsler, én nøkkel hver — og én oppsamler.
 *
 * Én nøkkel PER KODE, ikke én felles: «gjenopprettingen hoppet over en fil» og
 * «disken fylles» er to fakta, og et nøklet banner som erstattet det andre med
 * det første ville stilltiende kastet en av dem. Fem stykker fordi
 * `backend-warning` fanger en kode denne katalogen ikke kjenner — se
 * `state/backend-warning.ts` for hvorfor motorens egen setning er bedre enn
 * ingen setning.
 */
export type BackendWarningKey =
  | "backend-preroll-dead"
  | "backend-recovery-skipped"
  | "backend-device-missing"
  | "backend-disk-low"
  | "backend-warning";

/** Nøklene. Lukket liste: et nytt banner er en beslutning, ikke noe som siger
 *  inn i en tilfeldig handler. */
export type BannerKey =
  "recording-error" | "recording-quality" | "update" | BackendWarningKey;

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
      /**
       * Motorens egen PROSA, hardkodet norsk fra `sundayrec_core::selftest`.
       * Diagnostikk og reserve — aldri UI-tekst når `reasonCodes` finnes.
       */
      reasons: readonly string[];
      /**
       * De samme årsakene som STABILE KODER, parallelt med `reasons`.
       *
       * `null` betyr «motoren sendte ikke feltet» — en eldre bakende — og BARE
       * da leses prosaen over som tekst. Skillet er hele grunnen til at feltet
       * er nullbart i stedet for en tom liste: «ingen koder» og «koder finnes
       * ikke i denne versjonen» er to forskjellige svar, og bare det andre er
       * en grunn til å vise norsk til en engelsk bruker.
       */
      reasonCodes: readonly string[] | null;
    }
  /**
   * En nyere versjon finnes (P3). NØKLET er hele poenget her: den samme
   * oppdateringen går gjennom «tilgjengelig» → «laster ned 40 %» → «klar», og
   * det er ÉN beskjed som endrer seg, ikke tre som stables.
   *
   * De tre andre fasene (`checking`, `upToDate`, `failed`) reiser ikke noe
   * banner i det hele tatt — se `state/auto-update.ts`.
   *
   * Køen er delt, men flatene er ikke: de to opptaksbannerne over rendres av
   * OPPTAK, dette ene av skallet — fordi en oppdatering ikke hører til noen
   * side.
   */
  | {
      key: "update";
      state: "available" | "downloading" | "ready";
      /** Versjonen, når den er kjent. Tom under nedlasting — shimmen sender
       *  bare prosenten der. */
      version: string;
      /** 0–100. Meningsløs utenfor `downloading`. */
      percent: number;
    }
  /**
   * `backend://warning` — motoren så noe den vil ha på skjermen NÅ.
   *
   * FAKTA og ikke setning, av fillas egen grunn: `code` slås opp i `notify.*`
   * og `params` settes inn av siden, så et banner som ble reist på norsk ikke
   * blir stående på norsk gjennom et språkbytte. `msg` er motorens EGEN
   * setning, og brukes bare når koden er ukjent for katalogen — se
   * `state/backend-warning.ts`.
   */
  | {
      key: BackendWarningKey;
      /** Motorens stabile kode (`preroll_dead`, …). Tom = ukjent payload. */
      code: string;
      /** Motorens egen (norske) setning, eller `null`. Reserve, ikke UI-tekst. */
      msg: string | null;
      severity: "warn" | "error";
      /** `{file}`, `{device}`, `{freeGb}` … — innsettingene siden trenger. */
      params: Readonly<Record<string, string>>;
    };

/** Bare bakende-advarslenes variant, for de som bygger en. */
export type BackendWarningBanner = Extract<
  BannerData,
  { key: BackendWarningKey }
>;

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
