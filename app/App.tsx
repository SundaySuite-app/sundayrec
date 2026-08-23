// The shell's root component.
//
// Separate from `main.tsx` so it can be rendered in a node test: `main.tsx`
// imports the shim for its side effects and touches `document`, which is a
// browser's job, while a component is just a function of its inputs. That split
// (`*-core` / shell) is the same one the legacy renderer uses, and it is what
// keeps the new tree inside the unit gate from its first file.
//
// The heading is a CATALOGUE key, not a literal — `app/**` lints hardcoded JSX
// text as an error, and it is also the proof that `@lib/*` reaches the legacy
// renderer's i18n and its seven locales rather than a second copy of them.

import { t } from "@lib/i18n";

export function App() {
  return <h1 data-testid="app-heading">{t("nav.home")}</h1>;
}
