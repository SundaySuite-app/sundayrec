import { describe, expect, it } from "vitest";

import {
  jingleField,
  jingleFlags,
  jingleName,
  jinglePatch,
} from "./jingles-core";
import { exportErrorKey } from "./export-core";

describe("jingleName", () => {
  it("er siste ledd, med begge skilletegn", () => {
    expect(jingleName("/Users/kirke/Musikk/intro.wav")).toBe("intro.wav");
    expect(jingleName("C:\\Kirke\\Lyd\\Utgang.mp3")).toBe("Utgang.mp3");
    expect(jingleName("  /a/b/c.flac  ")).toBe("c.flac");
  });

  it("er null for alt som ikke er en sti: gamle og ukjente verdier ignoreres", () => {
    for (const v of [null, undefined, "", "   ", "/", 0, 42, true, {}, []]) {
      expect(jingleName(v)).toBeNull();
    }
  });
});

describe("jingleFlags", () => {
  it("en valgt fil er en bryter på, ingen fil er av", () => {
    expect(
      jingleFlags(false, {
        editorIntroPath: "/m/intro.wav",
        editorOutroPath: null,
      }),
    ).toEqual({ useIntro: true, useOutro: false });
    expect(jingleFlags(false, {})).toEqual({
      useIntro: false,
      useOutro: false,
    });
  });

  it("video får aldri jingler, selv med begge valgt", () => {
    expect(
      jingleFlags(true, {
        editorIntroPath: "/m/intro.wav",
        editorOutroPath: "/m/outro.wav",
      }),
    ).toEqual({ useIntro: false, useOutro: false });
  });

  it("en ukjent verdi i basen er ingen jingle", () => {
    expect(
      jingleFlags(false, { editorIntroPath: 7, editorOutroPath: "" }),
    ).toEqual({ useIntro: false, useOutro: false });
  });
});

describe("jinglePatch / jingleField", () => {
  it("tar begge feltene fra Rusts svar og renser det som ikke er tekst", () => {
    expect(
      jinglePatch({ editorIntroPath: "/m/i.wav", editorOutroPath: 3 }),
    ).toEqual({ editorIntroPath: "/m/i.wav", editorOutroPath: null });
  });

  it("feltene følger plassen", () => {
    expect(jingleField("intro")).toBe("editorIntroPath");
    expect(jingleField("outro")).toBe("editorOutroPath");
  });
});

describe("feilkodene en jingle-dialog kan svare med", () => {
  it("har hver sin setning", () => {
    expect(
      exportErrorKey("validation: clip_refused: that file cannot be used"),
    ).toBe("errClipRefused");
    expect(
      exportErrorKey("validation: clip_missing: the picked file is gone"),
    ).toBe("errClipMissing");
  });
});
