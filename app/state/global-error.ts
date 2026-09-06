/**
 * Det som gikk galt uten at noen fanget det.
 *
 * En uventet feil i et Preact-tre stopper rendringen av grenen den skjedde i.
 * Uten dette signalet er resultatet en halv skjerm og en linje i en konsoll
 * ingen frivillig noensinne åpner. Med det kan S1b vise «noe gikk galt» med
 * teksten, og en skjermdump fra en menighet blir en feilrapport.
 *
 * Meldingen er RÅ (ikke oversatt): den er for oss, ikke for brukeren. Flaten
 * setter den oversatte rammen rundt.
 */

import { signal } from "@preact/signals";

export const globalError = signal<string | null>(null);

/** Fang begge veiene en feil kan slippe unna. Returnerer en oppryddingsfunksjon. */
export function installErrorHandlers(): () => void {
  const onError = (e: ErrorEvent): void => {
    globalError.value = e.message || String(e.error);
  };
  const onRejection = (e: PromiseRejectionEvent): void => {
    const reason = e.reason as { message?: string } | undefined;
    globalError.value = reason?.message ?? String(e.reason);
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onRejection);
  };
}
