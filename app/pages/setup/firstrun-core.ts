/**
 * Første gang — sekvensen, som en ren avgjørelse.
 *
 * ## Ingen egen veiviser
 *
 * Canvasens sett 6: «Første gang = Oppsett i sekvens. Samme komponenter, samme
 * nøkler — ingen egen veiviser-kode å vedlikeholde.» Legacys veiviser er 521
 * linjer som bygger sine EGNE enhetslister, sin egen VU-måler og sitt eget
 * slot-skjema, og de har allerede kommet i utakt med skjermene de speiler:
 * veiviseren spør aldri om lagringsmappe, og sier likevel «Alt er klart!» til
 * en app som ikke kan ta opp.
 *
 * Her er stegene de fem skjermene som allerede finnes. Denne fila er bare
 * rekkefølgen, porten og hva den siste skjermen har lov til å påstå.
 */

import type { DecisionId } from "./decisions-core";

/**
 * De fem spørsmålene, i den rekkefølgen de stilles.
 *
 * Samme rekkefølge som nivå 1, og ikke tilfeldig: lyd først fordi den er den
 * eneste som ikke kan repareres etterpå, mappe før kvalitet fordi kvalitet
 * bestemmer hvor mye plass mappen trenger, og «hvem får beskjed» sist fordi den
 * handler om det som skjer NÅR noe går galt.
 *
 * «Ta opp automatisk» er IKKE et steg (canvasens sett 6, beslutning 3):
 * manuelt opptak først, tillegget tilbys etterpå.
 */
export const FIRST_RUN_STEPS: readonly DecisionId[] = [
  "sound",
  "folder",
  "quality",
  "church",
  "notify",
];

/** Hvor mange prikker linjalen har. */
export const FIRST_RUN_STEP_COUNT = FIRST_RUN_STEPS.length;

/** Skjermen ved en gitt posisjon. `ready` er sjekklisten etter det siste. */
export type FirstRunScreen =
  { kind: "question"; step: number; tab: DecisionId } | { kind: "ready" };

/**
 * Skjermen for en posisjon, klampet.
 *
 * Klampingen er ikke pynt: posisjonen kan komme fra en «tilbake» på det første
 * steget eller en «neste» på det siste hvis en knapp blir trykket to ganger, og
 * en indeks utenfor listen ville rendret en tom skjerm uten knapper — en app en
 * frivillig ikke kommer seg videre fra.
 */
export function screenAt(index: number): FirstRunScreen {
  if (index >= FIRST_RUN_STEP_COUNT) return { kind: "ready" };
  const safe = Math.max(0, Math.trunc(index));
  return { kind: "question", step: safe + 1, tab: FIRST_RUN_STEPS[safe] };
}

/** Hva en prikk i linjalen viser. */
export type DotState = "done" | "active" | "todo";

/** Prikkene for en posisjon. Sjekklisten (`index >= 5`) har alle fem grønne. */
export function dots(index: number): DotState[] {
  return FIRST_RUN_STEPS.map((_, i) =>
    i < index ? "done" : i === index ? "active" : "todo",
  );
}

/**
 * Porten på steg 1: er «Neste» åpen?
 *
 * `hear` og `loud` er begge lyd — «for høyt» er et problem, men det er ikke
 * «vi hører ingenting», og en port som holdt en frivillig fast fordi mikseren
 * står litt for høyt ville vært en app hun ikke kommer inn i.
 *
 * `skipped` er nødutgangen: «Fortsett uten lyd» finnes, i grått, og etterpå er
 * porten åpen for godt i denne sekvensen. En port uten en utgang er en app som
 * ikke kan brukes på en maskin med en mikrofon som ikke virker ennå.
 */
export function soundGateOpen(
  word: "nothing" | "hear" | "loud" | null,
  skipped: boolean,
): boolean {
  if (skipped) return true;
  return word === "hear" || word === "loud";
}

/** Gjelder porten i det hele tatt på dette steget? Bare det første. */
export function isGatedStep(index: number): boolean {
  return index === 0;
}

/**
 * R6: steget «Fortsett oppsettet»-chippen skal gå tilbake til.
 *
 * `remembered` er `firstRunReturn.value` i `FirstRun.tsx`: posisjonen sekvensen
 * sist STO i, eller `null` hvis den aldri har vært åpen denne økten.
 *
 * ⚠️ F2-T4 flyttet hvem som skriver den. Fram til nå var det sjekklistens
 * «Sett opp», som var det ene stedet man kunne forlate sekvensen fra — og som
 * derfor alltid husket sjekklisten selv. Nå folder radene seg ut PÅ STEDET, så
 * den knappen navigerer ikke lenger noe sted; det som fortsatt kan forlate
 * sekvensen er bunnlinja (den står under første gang også) og en lenke inne i
 * et utfoldet kort («Avansert lyd» i `SoundPage`). Begge kan skje fra et
 * SPØRSMÅL og ikke bare fra sjekklisten, så `FirstRun` speiler posisjonen
 * fortløpende — og reserven under er fortsatt sjekklisten, som er det ene
 * fornuftige stedet å lande når ingenting er husket.
 */
export function firstRunResumeIndex(remembered: number | null): number {
  return remembered ?? FIRST_RUN_STEP_COUNT;
}

/**
 * Skal «Fortsett oppsettet»-chippen vises?
 *
 * Ett vilkår: sekvensen er ikke fullført. `Shell` bytter ut HELE innholdet
 * med `FirstRun` så lenge `route.firstRun` er sann (se `Shell.tsx`), så
 * chippen og sekvensen selv kan aldri stå på skjermen samtidig — regelen
 * trenger derfor ikke vite hvilken side den rendres på, bare om «Åpne
 * SundayRec» alt er trykket.
 */
export function showFirstRunResumeChip(onboardingDone: boolean): boolean {
  return !onboardingDone;
}

/**
 * F2-T4: hvilke rader i sjekklisten som er FOLDET UT.
 *
 * En liste og ikke ett navn: kontrollrommet lar flere kort stå åpne samtidig
 * (`useControlCards` i `RecordPage.tsx`), og sjekklistas rader er de samme
 * skjermene i en annen ramme. En sjekkliste som lukket mappe-raden fordi noen
 * åpnet kvalitet-raden ville hatt en annen adferd enn den samme raden har på
 * OPPTAK — på to skjermer en frivillig ser rett etter hverandre.
 *
 * Ren, og ikke en oppdaterer inne i komponenten, av samme grunn som resten av
 * denne fila: «ny array bare når noe faktisk endret seg» er det som hindrer at
 * en effekt-kjøring blir en ny render, og den regelen er verdt en test.
 */
export function withRow(
  open: readonly DecisionId[],
  id: DecisionId,
  wanted: boolean,
): readonly DecisionId[] {
  const has = open.includes(id);
  if (has === wanted) return open;
  return wanted ? [...open, id] : open.filter((entry) => entry !== id);
}
