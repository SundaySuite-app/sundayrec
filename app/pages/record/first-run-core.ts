/**
 * Første gang — hintene på TA OPP, som en ren avgjørelse.
 *
 * Tre hintbobler, og ingen egen veiviser-kode: boblene peker på ting som
 * allerede finnes på TA OPP — lydkilde-brikka, mappen under «Mer» i lydarket
 * og kalenderknappen. Denne fila er bare rekkefølgen og lydporten.
 *
 * Språk følger systemet (`resolveStartupLocale`) og velges i tannhjulet;
 * kvalitet og varsler har gode standardverdier og ligger i lydarket og stripa.
 */

/** De tre stegene, i den rekkefølgen de stilles. */
export const FIRST_RUN_STEPS = ["source", "folder", "plan"] as const;

export type FirstRunStepId = (typeof FIRST_RUN_STEPS)[number];

/** Hvor mange prikker linjalen i boblen har. */
export const FIRST_RUN_STEP_COUNT = FIRST_RUN_STEPS.length;

/**
 * Steget ved en posisjon, klampet.
 *
 * Klampingen er ikke pynt: en «neste» på det siste steget kan komme to ganger
 * (dobbeltklikk), og en indeks utenfor listen ville gitt en tom
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
 * Porten på steg 1 (lydkilden): er «Neste» åpen?
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
  return stepAt(index).id === "source";
}
