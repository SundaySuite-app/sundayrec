import { describe, expect, it } from "vitest";

import {
  decideShortcut,
  isArrowTarget,
  isEditableTarget,
  isInteractiveTarget,
  type ShortcutInput,
} from "./shortcuts-core";

/** Alt som skal til for at Space skal starte et opptak. Hver rad under
 *  MUTERER ett felt fra denne og forventer at svaret detter til `null` —
 *  samme mønster som `record.spec.ts`s mutasjonsprøve, bare i node-gaten. */
const START_READY: ShortcutInput = {
  key: " ",
  meta: false,
  ctrl: false,
  page: "record",
  isRecording: false,
  dialogOpen: false,
  targetIsEditable: false,
  targetIsInteractive: false,
  repeat: false,
  startEnabled: true,
  shift: false,
  alt: false,
  targetIsSlider: false,
  editorStep: "cut",
  canUndo: false,
  canRedo: false,
};

/** Alt som skal til for at ⌘F/Ctrl+F skal fokusere biblioteksøket. */
const SEARCH_READY: ShortcutInput = {
  key: "f",
  meta: true,
  ctrl: false,
  page: "library",
  isRecording: false,
  dialogOpen: false,
  targetIsEditable: false,
  targetIsInteractive: false,
  repeat: false,
  startEnabled: false,
  shift: false,
  alt: false,
  targetIsSlider: false,
  editorStep: "cut",
  canUndo: false,
  canRedo: false,
};

describe("decideShortcut — start (Space/R)", () => {
  it("Space starter når alt stemmer", () => {
    expect(decideShortcut(START_READY)).toBe("start");
  });

  it("R (lowercase) starter like godt som Space", () => {
    expect(decideShortcut({ ...START_READY, key: "r" })).toBe("start");
  });

  it("R med Shift (stor R) starter også — ingen skiller på store/små", () => {
    expect(decideShortcut({ ...START_READY, key: "R" })).toBe("start");
  });

  // Mutasjonstabellen: ETT felt av gangen, alltid tilbake til `null`.
  it.each<[string, Partial<ShortcutInput>]>([
    ["fokus står i et tekstfelt", { targetIsEditable: true }],
    [
      "et opptak går allerede — Space skal ikke kunne STOPPE",
      {
        isRecording: true,
      },
    ],
    ["en dialog er åpen", { dialogOpen: true }],
    ["Start-knappen er faktisk sperret", { startEnabled: false }],
    ["siden er Redigering, ikke Opptak", { page: "library" }],
    ["siden er verken Opptak eller Redigering", { page: "other" }],
    ["Cmd holdes nede (Cmd+R er ikke vår snarvei)", { meta: true, key: "r" }],
    ["Ctrl holdes nede (Ctrl+R er ikke vår snarvei)", { ctrl: true, key: "r" }],
    [
      "Space havner på en fokusert knapp/bryter (browseren klikker den)",
      { targetIsInteractive: true },
    ],
    ["tasten holdes nede (auto-repeat)", { repeat: true }],
    ["tasten er noe helt annet", { key: "a" }],
  ])("null når %s", (_label, override) => {
    expect(decideShortcut({ ...START_READY, ...override })).toBeNull();
  });

  it("R på en fokusert knapp starter fortsatt (R aktiverer ingen knapp)", () => {
    expect(
      decideShortcut({ ...START_READY, key: "r", targetIsInteractive: true }),
    ).toBe("start");
  });

  it("R på Redigering-siden gjør ingenting (siden er feil, ikke tasten)", () => {
    expect(
      decideShortcut({ ...START_READY, key: "r", page: "library" }),
    ).toBeNull();
  });
});

describe("decideShortcut — søk (⌘F / Ctrl+F)", () => {
  it("⌘F fokuserer søket på Redigering-siden", () => {
    expect(decideShortcut(SEARCH_READY)).toBe("search");
  });

  it("Ctrl+F gjør akkurat det samme — platform avgjør aldri i tabellen", () => {
    expect(decideShortcut({ ...SEARCH_READY, meta: false, ctrl: true })).toBe(
      "search",
    );
  });

  it("stor F (Shift+⌘F) svarer likt som liten f", () => {
    expect(decideShortcut({ ...SEARCH_READY, key: "F" })).toBe("search");
  });

  it("⌘F på Opptak-siden gjør ingenting — søket finnes ikke der", () => {
    expect(decideShortcut({ ...SEARCH_READY, page: "record" })).toBeNull();
  });

  it("⌘F utenfor Opptak og Redigering gjør ingenting", () => {
    expect(decideShortcut({ ...SEARCH_READY, page: "other" })).toBeNull();
  });

  it("⌘F virker selv om fokus allerede står i søkefeltet selv", () => {
    // Snarveien er en global «hopp dit», ikke en tegn-tast — re-markering av
    // et felt som allerede har fokus er en harmløs no-op, ikke noe å blokkere.
    expect(decideShortcut({ ...SEARCH_READY, targetIsEditable: true })).toBe(
      "search",
    );
  });

  it("en åpen dialog vinner over søket også", () => {
    expect(decideShortcut({ ...SEARCH_READY, dialogOpen: true })).toBeNull();
  });

  it("⌘ eller Ctrl uten F gjør ingenting", () => {
    expect(decideShortcut({ ...SEARCH_READY, key: "g" })).toBeNull();
  });

  it("F uten noen modifikator gjør ingenting (det er bare bokstaven f)", () => {
    expect(
      decideShortcut({ ...SEARCH_READY, meta: false, ctrl: false }),
    ).toBeNull();
  });
});

describe("isEditableTarget", () => {
  it.each(["INPUT", "TEXTAREA", "SELECT", "input", "textarea", "select"])(
    "%s er skrivbart",
    (tagName) => {
      expect(isEditableTarget({ tagName })).toBe(true);
    },
  );

  it.each(["BUTTON", "DIV", "A", "BODY"])("%s er IKKE skrivbart", (tagName) => {
    expect(isEditableTarget({ tagName })).toBe(false);
  });

  it("contenteditable regnes som skrivbart uansett tagg", () => {
    expect(isEditableTarget({ tagName: "DIV", isContentEditable: true })).toBe(
      true,
    );
  });

  it("null/undefined (ingenting har fokus) er ikke skrivbart", () => {
    expect(isEditableTarget(null)).toBe(false);
    expect(isEditableTarget(undefined)).toBe(false);
  });
});

describe("isInteractiveTarget", () => {
  it.each(["BUTTON", "button", "A", "SUMMARY"])(
    "%s er aktiverbart",
    (tagName) => {
      expect(isInteractiveTarget({ tagName })).toBe(true);
    },
  );

  it.each(["switch", "button", "checkbox", "tab"])(
    "role=%s er aktiverbart",
    (role) => {
      expect(
        isInteractiveTarget({ tagName: "DIV", getAttribute: () => role }),
      ).toBe(true);
    },
  );

  it("div uten rolle, body og null er ikke aktiverbart", () => {
    expect(
      isInteractiveTarget({ tagName: "DIV", getAttribute: () => null }),
    ).toBe(false);
    expect(isInteractiveTarget({ tagName: "BODY" })).toBe(false);
    expect(isInteractiveTarget(null)).toBe(false);
  });
});

/** Redigerings arbeidsflate, Klipp-steget, ingenting fokusert. */
const EDITOR: ShortcutInput = {
  ...START_READY,
  page: "editor",
  startEnabled: false,
  canUndo: true,
  canRedo: true,
};

describe("decideShortcut — Redigering (spill/pause, hopp, angre)", () => {
  it("mellomrom spiller/pauser", () => {
    expect(decideShortcut(EDITOR)).toBe("togglePlay");
  });

  it("mellomrom spiller også i Lyd-steget (Før/Etter)", () => {
    expect(decideShortcut({ ...EDITOR, editorStep: "sound" })).toBe(
      "togglePlay",
    );
  });

  it("mellomrom på en fokusert knapp SPILLER — klikker ikke knappen", () => {
    // Etter et klikk på «Angre» står fokus der. Et redigeringsprogram spiller.
    expect(decideShortcut({ ...EDITOR, targetIsInteractive: true })).toBe(
      "togglePlay",
    );
  });

  it("mellomrom på et preken-håndtak spiller også", () => {
    expect(
      decideShortcut({
        ...EDITOR,
        targetIsInteractive: true,
        targetIsSlider: true,
      }),
    ).toBe("togglePlay");
  });

  it("mellomrom spiller selv om et opptak går (redigering stopper ikke opptaket)", () => {
    expect(decideShortcut({ ...EDITOR, isRecording: true })).toBe("togglePlay");
  });

  it.each<[string, Partial<ShortcutInput>]>([
    ["fokus står i et tekstfelt", { targetIsEditable: true }],
    ["en dialog er åpen", { dialogOpen: true }],
    ["tasten holdes nede (auto-repeat)", { repeat: true }],
    ["Shift holdes nede", { shift: true }],
    ["Cmd holdes nede", { meta: true }],
    ["Ctrl holdes nede", { ctrl: true }],
    ["Alt holdes nede", { alt: true }],
    ["biblioteklisten står, ikke arbeidsflaten", { page: "library" }],
    ["en annen side står", { page: "other" }],
  ])("mellomrom gjør ingenting når %s", (_label, override) => {
    expect(decideShortcut({ ...EDITOR, ...override })).toBeNull();
  });

  it.each<[string, Partial<ShortcutInput>, string]>([
    ["←", { key: "ArrowLeft" }, "seekBack"],
    ["→", { key: "ArrowRight" }, "seekForward"],
    ["Shift+←", { key: "ArrowLeft", shift: true }, "seekBackLong"],
    ["Shift+→", { key: "ArrowRight", shift: true }, "seekForwardLong"],
    ["← holdt nede (gjentas)", { key: "ArrowLeft", repeat: true }, "seekBack"],
    [
      "← på en fokusert knapp",
      { key: "ArrowLeft", targetIsInteractive: true },
      "seekBack",
    ],
    ["Home", { key: "Home" }, "seekStart"],
    ["End", { key: "End" }, "seekEnd"],
  ])("%s flytter spillehodet", (_label, override, expected) => {
    expect(decideShortcut({ ...EDITOR, ...override })).toBe(expected);
  });

  it.each<[string, Partial<ShortcutInput>]>([
    [
      "på et preken-håndtak (håndtaket eier piltastene)",
      { targetIsSlider: true },
    ],
    ["i Lyd-steget (ingen tidslinje)", { editorStep: "sound" }],
    ["i et tekstfelt", { targetIsEditable: true }],
    ["med Cmd (linjestart/-slutt i OS-et)", { meta: true }],
    ["med Alt", { alt: true }],
  ])("← gjør ingenting %s", (_label, override) => {
    expect(
      decideShortcut({ ...EDITOR, key: "ArrowLeft", ...override }),
    ).toBeNull();
  });

  it.each<[string, Partial<ShortcutInput>, string]>([
    ["⌘Z", { key: "z", meta: true }, "undo"],
    ["Ctrl+Z", { key: "z", ctrl: true }, "undo"],
    ["⇧⌘Z", { key: "Z", meta: true, shift: true }, "redo"],
    ["Ctrl+Shift+Z", { key: "Z", ctrl: true, shift: true }, "redo"],
    ["Ctrl+Y", { key: "y", ctrl: true }, "redo"],
    [
      "⌘Z på en fokusert knapp",
      { key: "z", meta: true, targetIsInteractive: true },
      "undo",
    ],
  ])("%s angrer/gjør om kuttene", (_label, override, expected) => {
    expect(decideShortcut({ ...EDITOR, ...override })).toBe(expected);
  });

  it.each<[string, Partial<ShortcutInput>]>([
    [
      "i et tekstfelt (feltet angrer selv)",
      { key: "z", meta: true, targetIsEditable: true },
    ],
    ["når det ikke er noe å angre", { key: "z", meta: true, canUndo: false }],
    [
      "når det ikke er noe å gjøre om",
      { key: "Z", meta: true, shift: true, canRedo: false },
    ],
    ["med Alt i tillegg", { key: "z", meta: true, alt: true }],
    ["med en dialog åpen", { key: "z", meta: true, dialogOpen: true }],
    ["uten modifikator (bare Z)", { key: "z" }],
  ])("angre gjør ingenting %s", (_label, override) => {
    expect(decideShortcut({ ...EDITOR, ...override })).toBeNull();
  });

  it("R starter aldri et opptak fra Redigering", () => {
    expect(
      decideShortcut({ ...EDITOR, key: "r", startEnabled: true }),
    ).toBeNull();
  });

  it("⌘F på arbeidsflaten gjør ingenting (søket bor i listen)", () => {
    expect(decideShortcut({ ...EDITOR, key: "f", meta: true })).toBeNull();
  });
});

describe("isArrowTarget", () => {
  const withRole = (role: string | null) => ({
    tagName: "BUTTON",
    getAttribute: (name: string) => (name === "role" ? role : null),
  });

  it.each(["slider", "SLIDER", "radio", "tab", "option", "spinbutton"])(
    "role=%s bruker piltastene selv",
    (role) => {
      expect(isArrowTarget(withRole(role))).toBe(true);
    },
  );

  it.each([null, "button", "switch", "checkbox"])(
    "role=%s gjør ikke det",
    (role) => {
      expect(isArrowTarget(withRole(role))).toBe(false);
    },
  );

  it("null-element", () => {
    expect(isArrowTarget(null)).toBe(false);
  });
});
