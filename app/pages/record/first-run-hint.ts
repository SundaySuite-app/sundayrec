/**
 * Første-gang-hintene — tilstanden, som moduldsignaler.
 *
 * Tre ting leser den og ikke bare hintboblen selv: lydkilde-brikka (`SourceChip`),
 * mappe-raden i lydarket (`SheetMore`) og kalenderknappen (`WeekStrip`) — alle tre
 * pulserer gull (`spot`) når det er DEM boblen peker på. Derfor signaler, ikke
 * `useState` inni boblen.
 *
 * ## Når er hintene på?
 *
 * Bare ved en ekte førstegangsinstallasjon: innstillingene er LEST
 * (`settingsHydrated`), lesingen gikk bra (`hydrateError` er tom), og
 * `onboardingDone` er falsk. En feilet lesing gir standardverdier med
 * `onboardingDone: false` — det er en base vi ikke fikk lest, ikke «første
 * gang», og en kirke med et ferdig oppsett skal ikke møte hintene.
 * `?goto=` tvinger `onboardingDone` sann i api-shimmen, så en dyplenke får
 * aldri hint.
 */

import { computed, signal } from "@preact/signals";

import { hydrateError, settings, settingsHydrated } from "../../state/settings";
import { stepAt, type FirstRunStepId } from "./first-run-core";

/** Hvilken boble som står (0-basert). Modulnivå: overlever et sidebytte. */
export const hintIndex = signal(0);

/** «Fortsett uten lyd» er trykket: lydporten er åpen for godt. */
export const soundSkipped = signal(false);

/** Står hintene på akkurat nå? */
export const hintsActive = computed(
  () =>
    settingsHydrated.value &&
    hydrateError.value === null &&
    !settings.value.onboardingDone,
);

/** Det som skal pulsere nå, eller `null` når hintene er av. */
export const hintSpot = computed<FirstRunStepId | null>(() =>
  hintsActive.value ? stepAt(hintIndex.value).id : null,
);

/** Tilbake til første boble — etter «Ferdig», og for testene. */
export function resetHints(): void {
  hintIndex.value = 0;
  soundSkipped.value = false;
}
