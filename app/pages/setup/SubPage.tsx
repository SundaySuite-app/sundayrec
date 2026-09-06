/**
 * Rammen rundt de fem spørsmålsskjermene: én setning som sier hva skjermen er
 * for.
 *
 * ## Hvor de fem skjermene rendres nå (D2)
 *
 * Tre steder, og rammen skal se forskjellig ut i to av dem:
 *
 *   1. **Kontrollrommet på OPPTAK.** Kortet bærer allerede spørsmålet som
 *      etikett og svaret som verdi, og selve kortraden er veien ut igjen. En
 *      lede til under den ville gjentatt det raden nettopp sa.
 *   2. **Første gang** (`FirstRun`), ett spørsmål om gangen. Leden er hele
 *      forklaringen der — det er den eneste teksten som sier hva skjermen er
 *      for — så den står.
 *   3. **Innstillinger** (`SetupPage`), der Avansert er én av to seksjoner.
 *      Samme sak som 2: leden sier hva lista er.
 *
 * ## `embedded`, og hvorfor den er et signal
 *
 * Et modulnivå-signal og ikke en prop, av samme grunn som resten av
 * `app/state` er signaler: den ene som trenger å vite dette er rammen, og en
 * prop måtte ellers tres gjennom `SetupPage`, `RecordPage` og alle fem sidene
 * — seks filer endret for én boolean ingen av dem bruker.
 *
 * ⚠️ `useEmbedded` MÅ være symmetrisk. Forsvinner oppryddingen, blir signalet
 * stående sant etter at man har forlatt OPPTAK, og da mister Innstillinger
 * leden sin uten at noe har feilet. Det er nøyaktig den vakten
 * `e2e/control-room.spec.ts` («Innstillinger beholder rammen sin …») står for.
 *
 * ## «Tilbake» er borte (D2)
 *
 * Knappen navigerte til Oppsett-destinasjonen. Etter D2 finnes ikke den
 * destinasjonen som et sted man kommer FRA en underside: kontrollrommet folder
 * kortet ut på stedet (kortraden lukker det igjen), første gang har sin egen
 * foot, og Innstillinger ER roten. En «Tilbake» på hver av de tre ville vært en
 * knapp uten et sted å gå — og en død knapp lærer en frivillig at knappene i
 * denne appen ikke er til å stole på.
 */

import { signal } from "@preact/signals";
import type { ComponentChildren } from "preact";
import { useEffect } from "preact/hooks";

import styles from "./setup.module.css";

/**
 * Står skjermen INNE i noe annet som allerede har sagt hva den er?
 *
 * Settes av `useEmbedded` og av ingen andre — en direkte skrivning ville vært
 * den halvdelen som glemmer å rydde.
 */
export const embedded = signal(false);

/** Monter = innbygget, avmonter = ikke. Symmetrisk ved konstruksjon. */
export function useEmbedded(): void {
  useEffect(() => {
    embedded.value = true;
    return () => {
      embedded.value = false;
    };
  }, []);
}

export interface SubPageProps {
  /** Én setning: hva skjermen er for. */
  lede: string;
  children: ComponentChildren;
  testId?: string;
}

export function SubPage({ lede, children, testId }: SubPageProps) {
  return (
    <div
      data-testid={testId}
      data-embedded={embedded.value ? "true" : undefined}
      class={styles.sub}
    >
      {embedded.value ? null : (
        <p
          data-testid={testId ? `${testId}-lede` : undefined}
          class={styles.lede}
        >
          {lede}
        </p>
      )}
      {children}
    </div>
  );
}
