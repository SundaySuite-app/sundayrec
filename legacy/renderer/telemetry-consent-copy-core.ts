/**
 * Which words the consent card asks with — the pure half of
 * telemetry-consent-prompt.ts, split out so it is testable in the node-only
 * vitest gate (see vitest.config.ts: no jsdom, by decision).
 *
 * `needsPrompt` is true for two people who need opposite things said to them:
 * someone who has never been asked, and someone who answered a NARROWER
 * question and is now being asked a wider one. Showing the second person the
 * first person's words is how a privacy prompt turns into either a bug report
 * ("it forgot my answer") or nagging ("it keeps asking") — and both spend the
 * trust the prompt exists to earn.
 *
 * The re-ask copy therefore has three jobs, and the test below pins all three:
 * say that the question is being asked AGAIN and why, name what was added, and
 * say plainly that the previous answer was not carried over in either
 * direction. What it must never do is lean: the two answers are equally
 * weighted in the copy exactly as they are in the CSS.
 */

import type { ConsentStatus } from '../bindings/ConsentStatus'

export interface PromptCopy {
  titleKey: string
  titleFallback: string
  descKey: string
  descFallback: string
}

/** The first-ask wording, matching index.html's static fallbacks. */
export const FIRST_ASK: PromptCopy = {
  titleKey: 'onboarding.promptTitle',
  titleFallback: 'Del anonym diagnostikk?',
  descKey: 'onboarding.promptDesc',
  descFallback:
    'SundayRec kan sende anonym feil- og bruksstatistikk til Sunday Suite for å forbedre appen. Frivillig, og du kan svare når som helst under Innstillinger → System.',
}

/** The wording for someone whose recorded answer predates a scope widening. */
export const RE_ASK: PromptCopy = {
  titleKey: 'onboarding.rePromptTitle',
  titleFallback: 'Vi spør deg om igjen',
  descKey: 'onboarding.rePromptDesc',
  descFallback:
    'Du har svart på dette før, men vi spør på nytt fordi det som eventuelt deles er utvidet: korrigeringene dine i redigeringsverktøyet kan nå også si omtrent hvor mye et automatisk forslag ble flyttet — for eksempel «30–60 sekunder tidligere» — og ikke bare at det ble flyttet. Det forrige svaret ditt er ikke lagt til grunn, verken som ja eller som nei, og ingenting sendes før du svarer her.',
}

/** Pick the copy for a card that has already been decided to show.
 *
 *  Keyed on `status`, not on comparing version numbers: "has this person
 *  answered before" is the only thing that changes the words, and a caller
 *  doing its own version arithmetic would be a second copy of the state
 *  machine that consent.rs owns. */
export function promptCopyFor(status: ConsentStatus): PromptCopy {
  return status === 'never-asked' ? FIRST_ASK : RE_ASK
}
