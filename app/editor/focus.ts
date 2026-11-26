/**
 * Fokus når knappen man sto på forsvinner.
 *
 * Et klikk på «Fjern» på en kuttrad fjerner selve
 * knappen fra DOM-en. Nettleseren slipper da fokuset til `<body>`, og en
 * tastaturbruker står igjen uten noe sted — neste Tab begynner øverst i appen
 * (audit 2026-11-11 🟠). Her flyttes fokus til noe som fortsatt finnes, ETTER
 * at Preact har tegnet på nytt.
 *
 * Adressert med `data-testid`, samme mønster som `Shell.tsx` bruker for
 * biblioteksøket: `Button` har ingen ref-prop.
 */

/** Flytt fokus til det første som finnes av `selectors`, etter neste tegning. */
export function focusAfterRender(
  ...selectors: Array<() => Element | null>
): void {
  requestAnimationFrame(() => {
    for (const pick of selectors) {
      const el = pick();
      if (el instanceof HTMLElement) {
        el.focus();
        return;
      }
    }
  });
}

export function byTestId(selector: string): () => Element | null {
  return () => document.querySelector(selector);
}
