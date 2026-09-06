/**
 * Auto-stoppen, sett fra overlegget.
 *
 * ## ⚠️ Nedtellingen var en kunngjøring, ikke en kontroll
 *
 * Overlegget har hele tiden VIST fristen — «Stopper av seg selv om 12:04» —
 * og aldri kunnet flytte den. De to kommandoene har vært registrert i Rust og
 * klassifisert som unåbare i reachability-baselinen siden byttet, altså uten
 * dør. `manualMaxMinutes` er 0 som standard, så det rammet bare en rigg som
 * hadde slått PÅ sikkerhetsnettet — og da rammet det midt i gudstjenesten, på
 * det ene tidspunktet ingen kan gjøre noe med det.
 *
 * ## Ingen lokal gjetning på den nye fristen
 *
 * Verken `+ 15 min` eller «Avbryt» skriver `scheduledStopMs`. Motoren flytter
 * sin egen timer og re-emitterer `recording://state` med den nye fristen, og
 * `state/recording.ts` tar den derfra — akkurat som den gjør for hver annen
 * tilstandsemit. Et lokalt anslag her ville vært en nedtelling som viste noe
 * annet enn den som faktisk stopper opptaket, og det er verre enn ingen
 * nedtelling.
 *
 * ## En feil skal SES
 *
 * Shimmen lar avvisningen reise (samme husregel som `stopRecordingNow`). En
 * fabrikkert suksess her ville betydd en teller som fortsetter mot en stopp
 * brukeren tror hun avlyste, og det er nøyaktig den løgnen hele denne runden
 * handler om. Katalogen har allerede setningen: «Kunne ikke endre auto-stopp.
 * Opptaket fortsetter — stopp manuelt hvis du må.»
 */

import { t } from "../../i18n";
import { toast } from "../../ui/toast";

/**
 * Hvor mye ett trykk flytter fristen.
 *
 * Femten og ikke tretti: den som trykker vet sjelden hvor mye lenger det blir,
 * og et lite steg man kan ta to ganger er ærligere enn et stort man ikke kan ta
 * tilbake. Motoren tar minutter som argument, så tallet bor her.
 */
export const AUTOSTOP_EXTEND_MINUTES = 15;

/** Skyv fristen. Motorens svar kommer som en tilstandsemit. */
export async function extendAutostop(): Promise<void> {
  try {
    await window.api.recordingExtendAutostop(AUTOSTOP_EXTEND_MINUTES);
  } catch (err) {
    console.warn("[record] kunne ikke forlenge auto-stopp:", err);
    toast("error", t("recording.autostopFailed"));
  }
}

/** Slå auto-stoppen av for denne økta. Opptaket går til noen trykker stopp. */
export async function cancelAutostop(): Promise<void> {
  try {
    await window.api.recordingCancelAutostop();
  } catch (err) {
    console.warn("[record] kunne ikke avbryte auto-stopp:", err);
    toast("error", t("recording.autostopFailed"));
  }
}
