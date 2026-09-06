import { describe, expect, it } from "vitest";

import {
  decideShortcut,
  isEditableTarget,
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
  startEnabled: true,
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
  startEnabled: false,
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
    ["tasten er noe helt annet", { key: "a" }],
  ])("null når %s", (_label, override) => {
    expect(decideShortcut({ ...START_READY, ...override })).toBeNull();
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
