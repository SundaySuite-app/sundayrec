/**
 * `useReceipt()` — «Lagret ✓» som en KVITTERING, ikke en tilstand.
 *
 * ## Hvorfor den finnes
 *
 * `Receipt`-komponenten kan si fire ting, og bare tre av dem skal bli stående.
 * «Lagret ✓» er et svar på noe brukeren nettopp gjorde; blir den stående, er
 * den ikke lenger et svar — den er en påstand om at det som står på skjermen er
 * det som står i basen, og den påstanden er usann i det sekundet noen endrer
 * noe annet.
 *
 * `useSetting` har alltid ryddet den opp igjen etter `SAVED_CHIP_MS`. De tolv
 * flatene som IKKE går gjennom `useSetting` — enhetsvalget, mappevalget,
 * kameravelgeren, de to skjemaene med Lagre/Avbryt, SMTP-passordet,
 * telemetriraden, motorvalget, de to tallbryterne — skrev hver sin
 * `useState<ReceiptState>` og satte `"saved"` uten en nedtelling. Resultatet
 * var et «Lagret ✓» som ble stående til siden ble forlatt, på tolv skjermer.
 *
 * Én hjelper, ett sted nedtellingen bor, og en kilde-vakt
 * (`use-receipt.test.ts`) som feller den neste håndlagde kopien.
 *
 * ## Hva den IKKE gjør
 *
 * Den lagrer ingenting og vet ingenting om innstillinger. Den holder ETT ord og
 * en timer. Skrivningen hører hjemme i `useSetting` (én nøkkel) eller
 * `usePatch` (flere nøkler); denne hooken er det de to — og de få ekte
 * særtilfellene, som en nøkkelring-skrivning — sier ifra gjennom.
 */

import { useCallback, useEffect, useRef, useState } from "preact/hooks";

import { SAVED_CHIP_MS } from "@lib/ui/bind-setting-core";

import type { Receipt } from "./use-setting-core";

export interface UseReceiptResult {
  /** Ordet som vises nå. */
  receipt: Receipt;
  /**
   * Si hva som skjedde. `"saved"` teller ned til `"idle"` av seg selv;
   * `"failed"` blir stående, fordi den ikke er lest ennå.
   */
  show: (next: Receipt) => void;
  /** Tilbake til stille — for «brukeren begynte å skrive igjen» og for en
   *  avbrutt handling, der en kvittering ville vært et svar på ingenting. */
  reset: () => void;
}

export function useReceipt(): UseReceiptResult {
  const [receipt, setReceipt] = useState<Receipt>("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clear = (): void => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  };

  // Timeren overlever ikke at kontrollen forsvinner — en `setState` på et
  // avmontert tre er en advarsel i konsollen og et lager som ikke frigis.
  useEffect(() => clear, []);

  const show = useCallback((next: Receipt): void => {
    clear();
    setReceipt(next);
    if (next === "saved") {
      timer.current = setTimeout(() => {
        timer.current = null;
        setReceipt("idle");
      }, SAVED_CHIP_MS);
    }
  }, []);

  const reset = useCallback((): void => {
    clear();
    setReceipt("idle");
  }, []);

  return { receipt, show, reset };
}
