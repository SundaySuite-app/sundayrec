/**
 * Intro og outro i eksportarket — beslutningene, uten DOM og uten IPC.
 *
 * Jinglene er innstillinger (`editorIntroPath` / `editorOutroPath`), og de
 * settes BARE av en dialog Rust åpner (`settings_pick_editor_intro` /
 * `_outro`): `settings_save` tar aldri en jingle fra siden. Siden her LESER
 * stien for å vise filnavnet, og sender i eksporten en bryter, ikke en sti
 * (`useIntro` / `useOutro`) — Rust leser klippet fra de lagrede innstillingene
 * og sjekker det på nytt idet eksporten starter.
 *
 * ## Hva en flis betyr (E7)
 *
 * Uten fil er flisen «+ Velg fil». Med fil er den en av/på-bryter: trykk slår
 * klippet av og på UTEN å glemme fila (`on`, bare for denne økta og aldri
 * lagret), og en liten mappeknapp i hjørnet bytter fil. Et valgt klipp som er
 * slått av sender ingen bryter — Rust får `useIntro: false` og lar det ligge.
 *
 * ## Video
 *
 * Motoren legger jingler rundt LYDSPORET i lydformatene; mp4-veien ignorerer
 * dem. Med «Ta med video» på sender vi derfor ingen bryter, og arket sier hvorfor.
 */

export type JingleSlot = "intro" | "outro";

export const JINGLE_SLOTS: readonly JingleSlot[] = ["intro", "outro"];

/** Det siste leddet i en sti, begge skilletegn; `null` for alt som ikke er en
 *  ikke-tom streng — en gammel eller ukjent verdi i basen er «ingen». */
export function jingleName(path: unknown): string | null {
  if (typeof path !== "string") return null;
  const trimmed = path.trim();
  if (!trimmed) return null;
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : null;
}

/** De to innstillingsfeltene jingle-valget leses fra. */
export interface JingleSettings {
  editorIntroPath?: unknown;
  editorOutroPath?: unknown;
}

/** Hva eksporten ber Rust om. Video får ingen jingler (se toppen av fila). */
export function jingleFlags(
  video: boolean,
  s: JingleSettings,
  on: Readonly<Record<JingleSlot, boolean>> = { intro: true, outro: true },
): { useIntro: boolean; useOutro: boolean } {
  if (video) return { useIntro: false, useOutro: false };
  return {
    useIntro: on.intro && jingleName(s.editorIntroPath) !== null,
    useOutro: on.outro && jingleName(s.editorOutroPath) !== null,
  };
}

/** Innstillingsfeltet en jingle bor i. */
export function jingleField(
  slot: JingleSlot,
): "editorIntroPath" | "editorOutroPath" {
  return slot === "intro" ? "editorIntroPath" : "editorOutroPath";
}

/** Feltene som skal inn i innstillingene etter et svar fra Rust: begge,
 *  slik Rust lagret dem, med alt som ikke er en streng som `null`. */
export function jinglePatch(stored: JingleSettings): {
  editorIntroPath: string | null;
  editorOutroPath: string | null;
} {
  const clean = (v: unknown): string | null =>
    jingleName(v) === null ? null : (v as string);
  return {
    editorIntroPath: clean(stored.editorIntroPath),
    editorOutroPath: clean(stored.editorOutroPath),
  };
}
