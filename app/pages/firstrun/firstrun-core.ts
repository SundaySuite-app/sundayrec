/**
 * Første gang — sekvensen, som en ren avgjørelse.
 *
 * Tre steg, og ingen egen veiviser-kode: stegene er skjermene som allerede
 * finnes i INNSTILLINGER (`SoundPage`, `FolderPage`) og ukevisningen fra PLAN
 * (`FirstRunAuto` + `WeekView`). Denne fila er bare rekkefølgen og porten.
 *
 * Språk følger systemet (`resolveStartupLocale`) og velges under Innstillinger;
 * kvalitet og varsler har gode standardverdier og ligger der også.
 */

/** De tre stegene, i den rekkefølgen de stilles. */
export const FIRST_RUN_STEPS = ["sound", "folder", "plan"] as const;

export type FirstRunStepId = (typeof FIRST_RUN_STEPS)[number];

/** Hvor mange prikker linjalen har. */
export const FIRST_RUN_STEP_COUNT = FIRST_RUN_STEPS.length;

/**
 * Steget ved en posisjon, klampet.
 *
 * Klampingen er ikke pynt: en «tilbake» på første steg eller en «neste» på det
 * siste kan komme to ganger, og en indeks utenfor listen ville gitt en tom
 * skjerm uten knapper.
 */
export function stepAt(index: number): { step: number; id: FirstRunStepId } {
  const safe = Math.min(
    FIRST_RUN_STEP_COUNT - 1,
    Math.max(0, Math.trunc(index) || 0),
  );
  return { step: safe + 1, id: FIRST_RUN_STEPS[safe] };
}

/** Hva en prikk i linjalen viser. */
export type DotState = "done" | "active" | "todo";

/** Prikkene for en posisjon. */
export function dots(index: number): DotState[] {
  const at = stepAt(index).step - 1;
  return FIRST_RUN_STEPS.map((_, i) =>
    i < at ? "done" : i === at ? "active" : "todo",
  );
}

/**
 * Porten på steg 1: er «Neste» åpen?
 *
 * `hear` og `loud` er begge lyd — «for høyt» er et problem, men det er ikke
 * «vi hører ingenting». `skipped` er nødutgangen («Fortsett uten lyd»): en port
 * uten utgang er en app som ikke kan brukes på en maskin der mikseren ikke er
 * slått på ennå.
 */
export function soundGateOpen(
  word: "nothing" | "hear" | "loud" | null,
  skipped: boolean,
): boolean {
  if (skipped) return true;
  return word === "hear" || word === "loud";
}

/** Gjelder porten på dette steget? Bare det første. */
export function isGatedStep(index: number): boolean {
  return stepAt(index).id === "sound";
}

/** Skal «Fortsett oppsettet»-chippen vises? Så lenge første gang ikke er fullført. */
export function showFirstRunResumeChip(onboardingDone: boolean): boolean {
  return !onboardingDone;
}
