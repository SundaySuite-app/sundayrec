/**
 * Ark-kontrakten som alle arkene i «Én flate» deler — som en hook.
 *
 *   - fokus går inn i arket når det åpnes (til `initialFocus`), og tilbake til
 *     det som åpnet det når det lukkes,
 *   - Esc lukker (med mindre `escapeBlocked` sier at noe annet eier tasten, som
 *     en bekreftelsesdialog over arket),
 *   - Tab fanges mens arket står, så fokus aldri havner bak sløret. Elementer
 *     merket `data-focus-companion` (første-gang-boblen, som peker på arket)
 *     er med i runden etter arkets egne: ellers kunne tastaturet aldri nå dem.
 *     Er arket selv `inert` (opptaksoverlegget eller en dialog eier fokus),
 *     fanger det ingenting.
 *
 * Lytteren sitter på `document` og ikke på arket: når knappen som hadde fokus
 * byttes ut (eksportknappen blir til fremdrift), står fokus på `<body>`, og en
 * lytter på arket ville aldri sett tasten.
 */

import type { RefObject } from "preact";
import { useEffect, useRef } from "preact/hooks";

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface SheetBehaviorOptions {
  open: boolean;
  onClose: () => void;
  sheet: RefObject<HTMLElement>;
  initialFocus: RefObject<HTMLElement>;
  escapeBlocked?: () => boolean;
  /** Hvor fokus lander når arket lukkes, når det ikke er det som hadde det
   *  før (WebKit fokuserer ikke en knapp ved klikk). */
  returnFocus?: () => HTMLElement | null;
}

export function useSheetBehavior({
  open,
  onClose,
  sheet,
  initialFocus,
  escapeBlocked,
  returnFocus,
}: SheetBehaviorOptions): void {
  const opener = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    opener.current = document.activeElement as HTMLElement | null;
    initialFocus.current?.focus();
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        if (escapeBlocked?.()) return;
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab" || !sheet.current) return;
      // Fokus er ikke vårt mens noe annet har slått av resten av appen.
      if (sheet.current.closest("[inert]")) return;
      const visible = (el: HTMLElement): boolean => el.offsetParent !== null;
      const own = [
        ...sheet.current.querySelectorAll<HTMLElement>(FOCUSABLE),
      ].filter(visible);
      const companions = [
        ...document.querySelectorAll<HTMLElement>("[data-focus-companion]"),
      ]
        .filter((c) => !sheet.current!.contains(c) && !c.closest("[inert]"))
        .flatMap((c) => [...c.querySelectorAll<HTMLElement>(FOCUSABLE)])
        .filter(visible);
      // Runden: arkets egne, så boblens. Bare kantene og «fokus er utenfor»
      // gås eksplisitt; resten er nettleserens vanlige rekkefølge.
      const items = [...own, ...companions];
      if (items.length === 0) return;
      const at = items.indexOf(document.activeElement as HTMLElement);
      const back = event.shiftKey;
      let to: HTMLElement | null = null;
      if (at === -1) to = back ? items[items.length - 1]! : items[0]!;
      else if (!back && at === items.length - 1) to = items[0]!;
      else if (back && at === 0) to = items[items.length - 1]!;
      else if (companions.length > 0 && !back && at === own.length - 1) {
        to = companions[0]!;
      } else if (companions.length > 0 && back && at === own.length) {
        to = own[own.length - 1]!;
      }
      if (to) {
        event.preventDefault();
        to.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      // Etter at gjengivelsen er ferdig: siden bak arket kan fortsatt være
      // `inert` i dette øyeblikket, og en inert knapp tar ikke imot fokus.
      const back = opener.current;
      setTimeout(() => (returnFocus?.() ?? back)?.focus?.(), 0);
    };
    // `onClose`/`escapeBlocked` leses ferskt av håndtereren hver gang; arket
    // skal ikke miste fokus fordi en callback fikk ny identitet.
  }, [open]);
}
