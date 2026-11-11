/**
 * Globale tastatursnarveier — den rene tabellen. F2-T3.
 *
 * ## Hvorfor én tabell og ikke tre `if`-er spredt i komponenter
 *
 * Tre regler («Space/R starter», «Esc lukker» — allerede DialogHosts egen,
 * «⌘F/Ctrl+F søker») deler samme fare: en snarvei som fyrer når den ikke skal.
 * Space er en TEGN-tast — den skrives i hvert tekstfelt i appen — så feilen
 * som koster mest her er ikke «snarveien virker ikke», det er «snarveien
 * stjeler et mellomrom fra et filnavn, eller starter et opptak mens en
 * frivillig taster i et annet felt». `decideShortcut` er derfor EN
 * beslutning, testet som en tabell (som `dialog-core.ts` og
 * `record-core.ts`), i stedet for logikk som bor inni en `onKeyDown` ingen
 * vitest-gate når.
 *
 * `app/Shell.tsx`s `useGlobalShortcuts()` er den tynne DOM-siden: én
 * `keydown`-lytter på `window` (samme sted og samme mønster som
 * `useTrayFolder()` — begge må virke uansett hvilken side som står), som
 * leser fersk tilstand fra signalene og spør denne tabellen. Den bor i
 * `Shell.tsx` og ikke i `app/ui/` fordi den trenger sidespesifikke fakta
 * (`route`, `loadState`, biblioteksøkets `data-testid`) — nøyaktig de tingene
 * `app/ui/` ellers aldri importerer fra `app/pages/`.
 *
 * ## `preventDefault` er IKKE denne fila sitt ansvar
 *
 * Tabellen svarer bare med hva som skal skje. Verten kaller `preventDefault`
 * bare når svaret ikke er `null` — Space på en fokusert knapp skal fortsatt
 * «klikke» knappen (nettleserens egen oppførsel) når INGEN av reglene her
 * treffer.
 *
 * ## Platform hører ikke hjemme i tabellen
 *
 * ⌘F og Ctrl+F behandles likt — «har brukeren en av de to modifikatorene
 * nede» — uansett OS. Hvilket symbol som vises i et HINT («⌘F» vs «Ctrl+F»)
 * er et visningsspørsmål verten løser med `state/platform-core.ts`; denne
 * tabellen trenger ikke vite hvilket operativsystem den kjører på for å svare
 * riktig på et tastetrykk.
 */

/** De fire stedene en snarvei bryr seg om — resten av appen er «other».
 *  `"editor"` er Redigerings ARBEIDSFLATE (et opptak er lastet), ikke
 *  biblioteklisten — den er `"library"`. */
export type ShortcutPage = "record" | "library" | "editor" | "other";

/** Hvilket steg arbeidsflaten viser — `app/editor/model.ts`s `Step`. */
export type ShortcutEditorStep = "cut" | "sound";

export interface ShortcutInput {
  /** `event.key`, ubehandlet — tabellen selv gjør små/stor bokstav-normalisering. */
  key: string;
  meta: boolean;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  page: ShortcutPage;
  isRecording: boolean;
  /** `app/ui/dialog.ts`s `activeDialog.value !== null`. */
  dialogOpen: boolean;
  /** Fokus står i et `input`/`textarea`/`select`/`[contenteditable]`. */
  targetIsEditable: boolean;
  /** Fokus står på noe Space AKTIVERER (knapp, lenke, `role=button|switch|…`). */
  targetIsInteractive: boolean;
  /** Fokus står på noe som bruker PILTASTENE selv (`role=slider`,
   *  `input[type=range]`, radiogrupper, faner) — preken-håndtakene i Klipp. */
  targetIsSlider: boolean;
  /** `event.repeat` — tasten holdes nede (auto-repeat). */
  repeat: boolean;
  /** Samme vilkår som Start-knappens klikkhåndterer: kilde valgt, ikke opptak. */
  startEnabled: boolean;
  /** Steget arbeidsflaten viser. Betyr bare noe når `page === "editor"`. */
  editorStep: ShortcutEditorStep;
  /** `app/editor/cuts.ts`s `canUndo` / `canRedo`. */
  canUndo: boolean;
  canRedo: boolean;
}

export type ShortcutAction =
  | "start"
  | "search"
  | "togglePlay"
  | "seekBack"
  | "seekForward"
  | "seekBackLong"
  | "seekForwardLong"
  | "seekStart"
  | "seekEnd"
  | "undo"
  | "redo"
  | null;

/**
 * Avgjørelsen. Ren funksjon — ingen DOM, ingen signal, testbar i node-gaten.
 *
 * Rekkefølgen på sjekkene under er ikke tilfeldig:
 *
 *   1. En åpen dialog vinner over ALT. Biblioteksøket ville uansett trykket
 *      mot et `inert` felt (S1b setter det på `#app`), og Space/R skal aldri
 *      nå forbi et spørsmål appen venter svar på.
 *   2. ⌘F/Ctrl+F sjekkes FØR `targetIsEditable`, med vilje: søket er en
 *      global «hopp dit»-snarvei (samme idé som nettleserens egen Ctrl+F),
 *      ikke en tegn-tast, så den skal virke uansett hvor fokus står på
 *      Redigering-siden — inkludert når fokus allerede står i søkefeltet
 *      selv (der den bare re-markerer det som allerede står).
 *   3. På Redigerings arbeidsflate (`page === "editor"`) avgjør
 *      `decideEditor` resten — se den. Den har egne regler for modifikatorer
 *      (⌘Z) og for fokuserte knapper (mellomrom spiller likevel).
 *   4. Space/R er tegn-taster og krever derfor at INGEN modifikator er nede
 *      (Cmd+R/Ctrl+R er nettleserens/OS-ets egne snarveier, ikke våre), at
 *      fokus ikke står i et skrivefelt, og at alt Start-knappen selv krever
 *      (side, ikke-opptak, faktisk aktiv) også stemmer.
 */
export function decideShortcut(input: ShortcutInput): ShortcutAction {
  const {
    key,
    meta,
    ctrl,
    page,
    isRecording,
    dialogOpen,
    targetIsEditable,
    targetIsInteractive,
    repeat,
    startEnabled,
  } = input;

  if (dialogOpen) return null;

  const lower = key.length === 1 ? key.toLowerCase() : key;

  // ⌘F / Ctrl+F — søk i biblioteket. Platform avgjør ALDRI her; se filhodet.
  if ((meta || ctrl) && lower === "f") {
    return page === "library" ? "search" : null;
  }

  if (page === "editor") return decideEditor(input, lower);

  // Alt annet med en modifikator nede er en snarvei som hører til noen andre
  // (nettleseren, OS-et) — aldri vår.
  if (meta || ctrl) return null;

  // Space eller R — start opptak.
  if (key !== " " && lower !== "r") return null;
  if (page !== "record") return null;
  if (targetIsEditable) return null;
  // Space er nettleserens «trykk den fokuserte knappen» (en bryter, «Åpne
  // mappe», …). Å starte et opptak oppå det er et uønsket opptak. R er ikke en
  // aktiveringstast og får lov.
  if (key === " " && targetIsInteractive) return null;
  // Auto-repeat av en holdt tast er aldri en ny, bevisst start.
  if (repeat) return null;
  if (isRecording) return null;
  if (!startEnabled) return null;
  return "start";
}

/**
 * Redigerings arbeidsflate — tastene alle redigeringsprogrammer har.
 *
 *   - **Mellomrom = spill/pause**, i begge steg (Klipp spiller programmet,
 *     Lyd spiller Før/Etter). I MOTSETNING til Opptak gjelder det også når
 *     fokus står på en knapp: etter et klikk på «Angre» eller «Demp» står
 *     fokuset igjen der, og i et redigeringsprogram forventer man at
 *     mellomrom spiller — ikke at det angrer en gang til. Verten kaller
 *     `preventDefault` på både keydown og keyup, så knappen ikke klikkes i
 *     tillegg. Bare skrivefelt (der mellomrom er et tegn) slipper den.
 *   - **⌘Z / Ctrl+Z angrer, ⇧⌘Z / Ctrl+Shift+Z / Ctrl+Y gjør om** — kuttene.
 *     Ikke i et skrivefelt: der eier feltet sin egen angre.
 *   - **←/→ flytter spillehodet 1 s (Shift: 5 s), Home/End til start/slutt** —
 *     bare i Klipp (Lyd har ingen tidslinje), og aldri når fokus står på noe
 *     som bruker piltastene selv (preken-håndtakene er `role=slider`).
 */
function decideEditor(input: ShortcutInput, lower: string): ShortcutAction {
  const {
    key,
    meta,
    ctrl,
    shift,
    alt,
    targetIsEditable,
    targetIsSlider,
    repeat,
    editorStep,
    canUndo,
    canRedo,
  } = input;

  if (targetIsEditable) return null;

  if (meta || ctrl) {
    if (alt) return null;
    if (lower === "z") {
      if (shift) return canRedo ? "redo" : null;
      return canUndo ? "undo" : null;
    }
    // Ctrl+Y er Windows' «gjør om». ⌘Y er ikke det på Mac, men skader ingen.
    if (lower === "y" && !shift) return canRedo ? "redo" : null;
    return null;
  }
  if (alt) return null;

  if (key === " ") {
    if (shift || repeat) return null;
    return "togglePlay";
  }

  if (editorStep !== "cut" || targetIsSlider) return null;
  switch (key) {
    case "ArrowLeft":
      return shift ? "seekBackLong" : "seekBack";
    case "ArrowRight":
      return shift ? "seekForwardLong" : "seekForward";
    case "Home":
      return shift || repeat ? null : "seekStart";
    case "End":
      return shift || repeat ? null : "seekEnd";
    default:
      return null;
  }
}

/** Elementformen `document.activeElement` faktisk har — strukturell med
 *  vilje, så testen slipper en DOM (se `vitest.config.ts`: `environment:
 *  "node"`). */
export interface EditableTargetShape {
  tagName?: string | null;
  isContentEditable?: boolean | null;
  getAttribute?: ((name: string) => string | null) | null;
}

const INTERACTIVE_TAGS = new Set(["BUTTON", "A", "SUMMARY"]);
const INTERACTIVE_ROLES = new Set([
  "button",
  "switch",
  "checkbox",
  "radio",
  "tab",
  "menuitem",
  "menuitemcheckbox",
  "menuitemradio",
  "link",
  "option",
  "slider",
]);

/**
 * Står fokuset på noe Space AKTIVERER? Knapper, lenker og ARIA-widgets
 * (`role=switch` osv.) — der er Space «klikk», ikke en global snarvei.
 */
export function isInteractiveTarget(
  el: EditableTargetShape | null | undefined,
): boolean {
  if (!el) return false;
  if (INTERACTIVE_TAGS.has((el.tagName ?? "").toUpperCase())) return true;
  const role = el.getAttribute?.("role");
  return role != null && INTERACTIVE_ROLES.has(role.toLowerCase());
}

const ARROW_ROLES = new Set([
  "slider",
  "spinbutton",
  "radio",
  "radiogroup",
  "tab",
  "tablist",
  "listbox",
  "option",
  "menu",
  "menuitem",
  "menuitemradio",
  "menuitemcheckbox",
]);

/**
 * Bruker det fokuserte elementet PILTASTENE selv? Da skal ←/→ bli der og
 * ikke flytte spillehodet. `input type=range` er allerede et skrivefelt
 * (`isEditableTarget`), så her er det ARIA-rollene som teller.
 */
export function isArrowTarget(
  el: EditableTargetShape | null | undefined,
): boolean {
  if (!el) return false;
  const role = el.getAttribute?.("role");
  return role != null && ARROW_ROLES.has(role.toLowerCase());
}

const EDITABLE_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/**
 * Står fokuset i noe som SKRIVER tegn?
 *
 * `select` er med selv om den ikke tar fritekst: piltastene og bokstavsøket
 * en `<select>` svarer på skal ikke krysses av en global Space/R-snarvei
 * heller — samme regel, samme grunn.
 */
export function isEditableTarget(
  el: EditableTargetShape | null | undefined,
): boolean {
  if (!el) return false;
  if (el.isContentEditable) return true;
  return EDITABLE_TAGS.has((el.tagName ?? "").toUpperCase());
}
