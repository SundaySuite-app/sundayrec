/**
 * Statuslinjen — den ene setningen nederst i skinnen som alltid er sann.
 *
 * ## Fem setninger, aldri fri tekst
 *
 * Canvasens sett 1 låser det: linjen kan si nøyaktig fem ting, og hver av dem
 * betyr én ting.
 *
 *   `rec`      rød    det tas opp NÅ
 *   `lowdisk`  gul    under to timer igjen på disken
 *   `nosound`  gul    ingen lydkilde valgt — eller mikseren er borte
 *   `next`     grå    «Ta opp automatisk» er på, og neste tid er kjent
 *   `ready`    grønn  kilde valgt, plass på disken
 *
 * Gul betyr «du må gjøre noe før søndag». Rød betyr BARE at opptaket går —
 * aldri en feil. Fri tekst er ikke et alternativ, fordi en linje som kan si
 * hva som helst er en linje ingen leser nøye.
 *
 * ## Rekkefølgen er hele avgjørelsen
 *
 *     rec > lowdisk > nosound > next > ready
 *
 * `rec` først fordi et opptak som går er det viktigste faktumet på skjermen.
 * `lowdisk` FØR `nosound`: begge er gule, men tom disk stopper opptaket midt i
 * gudstjenesten, mens en manglende kilde stopper det før det begynner — den
 * første er den man har minst tid til å oppdage. `ready` sist, og bare når
 * ingen av de andre gjelder: appen sier ikke «Alt er klart» så lenge det
 * finnes noe som ikke er det.
 *
 * ## Hvorfor en ren funksjon
 *
 * Rekkefølgen er den eneste logikken her, og den er nøyaktig den slags som
 * ser riktig ut i en komponent og er feil i to av fem tilfeller. Som ren
 * funksjon er den en tabell (`status-line.test.ts`) i stedet for noe man må
 * klikke seg fram til.
 */

import type { DotTone } from "../ui/StatusDot/StatusDot";

export type StatusKind = "rec" | "lowdisk" | "nosound" | "next" | "ready";

/**
 * Under dette er det «lite plass igjen». To timer, fordi en gudstjeneste med
 * lovsang og kirkekaffe er halvannen — grensen er «neste søndag får ikke
 * plass», ikke «disken er nesten full».
 */
export const LOW_DISK_MINUTES = 120;

export interface StatusInput {
  /** Et opptak går akkurat nå. */
  isRecording: boolean;
  /** En lydkilde er VALGT (et enhetsnavn eller en enhets-id står lagret). */
  soundChosen: boolean;
  /** Minutter opptak det er plass til, eller `null` når disken ikke er lest. */
  roomMinutes: number | null;
  /** Neste planlagte opptak (ms siden epoke), eller `null`. */
  nextAtMs: number | null;
}

export interface StatusLine {
  kind: StatusKind;
  tone: DotTone;
  /** Katalognøkkelen setningen kommer fra. */
  key: string;
}

const TONE: Record<StatusKind, DotTone> = {
  rec: "rec",
  lowdisk: "warn",
  nosound: "warn",
  next: "neutral",
  ready: "good",
};

function line(kind: StatusKind): StatusLine {
  return { kind, tone: TONE[kind], key: `app.status.${kind}` };
}

/** Hvilken av de fem setningene som gjelder. */
export function statusLine(input: StatusInput): StatusLine {
  if (input.isRecording) return line("rec");
  // `null` er «vi har ikke lest disken», ikke «det er god plass». Å gjette
  // grønt på manglende data er nøyaktig løgnen linjen finnes for å stoppe.
  if (input.roomMinutes !== null && input.roomMinutes < LOW_DISK_MINUTES) {
    return line("lowdisk");
  }
  if (!input.soundChosen) return line("nosound");
  if (input.nextAtMs !== null) return line("next");
  return line("ready");
}

/**
 * «søndag 11:00» — halvparten av `next`-setningen.
 *
 * Holdt utenfor `statusLine` med vilje: formatering avhenger av `Intl` og
 * dermed av hvilken ICU-versjon node/WebKit er bygget med, mens rekkefølgen
 * over er ren aritmetikk. Å blande dem ville gjort prioritetstabellen
 * skjør av en grunn som ikke har noe med prioritet å gjøre.
 */
export function formatNextWhen(atMs: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    weekday: "long",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(atMs));
}
