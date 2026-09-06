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

/** De tre stedene en snarvei bryr seg om — resten av appen er «other». */
export type ShortcutPage = "record" | "library" | "other";

export interface ShortcutInput {
  /** `event.key`, ubehandlet — tabellen selv gjør små/stor bokstav-normalisering. */
  key: string;
  meta: boolean;
  ctrl: boolean;
  page: ShortcutPage;
  isRecording: boolean;
  /** `app/ui/dialog.ts`s `activeDialog.value !== null`. */
  dialogOpen: boolean;
  /** Fokus står i et `input`/`textarea`/`select`/`[contenteditable]`. */
  targetIsEditable: boolean;
  /** Samme vilkår som Start-knappens klikkhåndterer: kilde valgt, ikke opptak. */
  startEnabled: boolean;
}

export type ShortcutAction = "start" | "search" | null;

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
 *   3. Space/R er tegn-taster og krever derfor at INGEN modifikator er nede
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
    startEnabled,
  } = input;

  if (dialogOpen) return null;

  const lower = key.length === 1 ? key.toLowerCase() : key;

  // ⌘F / Ctrl+F — søk i biblioteket. Platform avgjør ALDRI her; se filhodet.
  if ((meta || ctrl) && lower === "f") {
    return page === "library" ? "search" : null;
  }

  // Alt annet med en modifikator nede er en snarvei som hører til noen andre
  // (nettleseren, OS-et) — aldri vår.
  if (meta || ctrl) return null;

  // Space eller R — start opptak.
  if (key !== " " && lower !== "r") return null;
  if (page !== "record") return null;
  if (targetIsEditable) return null;
  if (isRecording) return null;
  if (!startEnabled) return null;
  return "start";
}

/** Elementformen `document.activeElement` faktisk har — strukturell med
 *  vilje, så testen slipper en DOM (se `vitest.config.ts`: `environment:
 *  "node"`). */
export interface EditableTargetShape {
  tagName?: string | null;
  isContentEditable?: boolean | null;
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
