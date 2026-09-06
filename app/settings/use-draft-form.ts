/**
 * `useDraftForm(read, write)` — unntaket fra auto-anvend.
 *
 * Nesten alt i appen lagrer seg selv. De få stedene som IKKE skal gjøre det er
 * der en HALVSKREVET verdi er aktivt skadelig:
 *
 *   • slot-redigereren i tidsplanen — «søndag 10:0» er en tid appen ville
 *     armet en vekking på
 *   • den ene e-postadressen feilvarsler går til — «post@» er en adresse
 *     ingenting kommer fram til, og du oppdager det den dagen opptaket feiler
 *
 * De er bordede kort med Lagre/Avbryt, ikke auto-anvend-kontroller. Forskjellen
 * fra legacy er at «Avbryt» her FAKTISK angrer: utkastet er en egen kopi som
 * ingenting utenfor skjemaet ser før `save()`. I legacy hadde halvparten av
 * kontrollene i en slik fot allerede skrevet, så «Avbryt» var en knapp som
 * ikke gjorde det den het.
 *
 * ## Hvorfor en feilet `save()` IKKE ruller tilbake
 *
 * Motsatt av `useSetting`, og med vilje. Der er verdien ett klikk unna å
 * gjentas; her er den noe brukeren har skrevet, og å kaste det på gulvet fordi
 * en disk-skrivning feilet er å straffe brukeren for appens problem. Utkastet
 * blir stående og skittent, så «Lagre» kan prøves igjen.
 */

import { useCallback, useEffect, useRef, useState } from "preact/hooks";

export interface DraftForm<T> {
  /** Verdien i skjemaet nå. */
  draft: T;
  /** Skiller den seg fra det som er lagret? */
  dirty: boolean;
  /**
   * Endre én eller flere felter i utkastet. TILLEGG til det dirigenten
   * spesifiserte: uten den finnes det ingen vei fra en tastetrykk til
   * `draft`, og skjemaet ville vært skrivebeskyttet.
   */
  set: (patch: Partial<T>) => void;
  /** Skriv utkastet. `false` = det landet ikke; utkastet blir stående. */
  save: () => Promise<boolean>;
  /** Kast utkastet og hent det lagrede tilbake. */
  cancel: () => void;
}

/** Grunn likhet over unionen av nøkler — nok for et skjema av skalarer, og
 *  lettere å lese enn en serialisering som også ville vært avhengig av
 *  nøkkelrekkefølge. */
function shallowEqual<T extends object>(a: T, b: T): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) {
    if (
      (a as Record<string, unknown>)[key] !==
      (b as Record<string, unknown>)[key]
    ) {
      return false;
    }
  }
  return true;
}

export function useDraftForm<T extends object>(
  read: () => T,
  write: (value: T) => Promise<boolean>,
): DraftForm<T> {
  const stored = read();
  const [draft, setDraft] = useState<T>(() => ({ ...stored }));
  const draftRef = useRef<T>(draft);
  const dirtyRef = useRef(false);
  const writeRef = useRef(write);
  writeRef.current = write;

  // En lagret verdi som endrer seg utenfra tas inn — men bare når skjemaet er
  // rent. Ellers ville en bakgrunnsoppdatering slettet det brukeren skriver.
  useEffect(() => {
    if (dirtyRef.current) return;
    draftRef.current = { ...stored };
    setDraft(draftRef.current);
    // `stored` er et nytt objekt hver render; sammenligningen som betyr noe er
    // innholdet, som `shallowEqual` under svarer på.
  }, [JSON.stringify(stored)]);

  const set = useCallback((patch: Partial<T>): void => {
    draftRef.current = { ...draftRef.current, ...patch };
    dirtyRef.current = true;
    setDraft(draftRef.current);
  }, []);

  const save = useCallback(async (): Promise<boolean> => {
    const ok = await writeRef.current(draftRef.current);
    if (ok) dirtyRef.current = false;
    return ok;
  }, []);

  const cancel = useCallback((): void => {
    dirtyRef.current = false;
    draftRef.current = { ...read() };
    setDraft(draftRef.current);
  }, [read]);

  return {
    draft,
    dirty: !shallowEqual(draft, stored),
    set,
    save,
    cancel,
  };
}
