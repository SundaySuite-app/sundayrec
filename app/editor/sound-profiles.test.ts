/**
 * Lyd-steget har én oppskrift: bryteren «Podkastklar lyd» (F3).
 *
 * MUTASJONSPRØVEN: la `soundExportFields(true)` returnere `{}` og eksporten mister
 * oppskriften uten at noe annet i skallet går rødt — de to første testene her gjør det.
 */

import { describe, expect, it } from "vitest";

import {
  DEFAULT_PODCAST_AUDIO,
  LISTEN_SPAN_SEC,
  listenStartSec,
  soundExportFields,
  taleSaidKeys,
} from "./sound-profiles";

describe("bryteren «Podkastklar lyd»", () => {
  it("er på som standard", () => {
    expect(DEFAULT_PODCAST_AUDIO).toBe(true);
  });

  it("på sender oppskriften og ingenting annet", () => {
    expect(soundExportFields(true)).toEqual({ podcastAudio: true });
  });

  it("av sender ingenting: fila kommer ut slik den gikk inn", () => {
    expect(soundExportFields(false)).toEqual({});
  });
});

describe("lytteutsnittet", () => {
  it("ligger midt i prekenvinduet", () => {
    // 210–420 → midten er 315, og 20 sekunder derfra begynner på 305.
    expect(listenStartSec({ start: 210, end: 420 }, 600)).toBe(305);
  });

  it("uten et vindu brukes midten av fila", () => {
    expect(listenStartSec(null, 600)).toBe(290);
  });

  it("klemmes så hele utsnittet får plass", () => {
    // Et vindu helt i halen: startpunktet trekkes inn så det blir 20 sekunder
    // å høre på, ikke fem.
    expect(listenStartSec({ start: 590, end: 600 }, 600)).toBe(
      600 - LISTEN_SPAN_SEC,
    );
    // Og en fil som er kortere enn utsnittet begynner på null.
    expect(listenStartSec({ start: 0, end: 8 }, 8)).toBe(0);
  });

  it("en varighet vi ikke kjenner gir null, ikke NaN", () => {
    expect(listenStartSec(null, 0)).toBe(0);
    expect(listenStartSec(null, Number.NaN)).toBe(0);
  });
});

describe("tre ord om hva «Podkastklar lyd» gjorde (B3b)", () => {
  const said = (codes: string[], analysed = true) => ({
    analysed,
    said: codes,
  });

  it("én linje per ting, i kjedens rekkefølge, og ukjente koder hoppes over", () => {
    expect(
      taleSaidKeys(true, said(["hum", "noise", "ny_ting", "reverb"])),
    ).toEqual(["hum", "noise", "reverb"]);
    expect(taleSaidKeys(true, said(["clicks"]))).toEqual(["clicks"]);
  });

  it("ingenting gjort på en analysert fil er «rent» — ikke tomt", () => {
    expect(taleSaidKeys(true, said([]))).toEqual(["clean"]);
    expect(taleSaidKeys(true, said(["ny_ting"]))).toEqual(["clean"]);
  });

  it("sier ingenting uten en analyse: «rent» om en fil vi ikke fikk lest er en løgn", () => {
    expect(taleSaidKeys(true, null)).toEqual([]);
    expect(taleSaidKeys(true, said([], false))).toEqual([]);
  });

  it("sier ingenting når bryteren er av", () => {
    expect(taleSaidKeys(false, said(["hum"]))).toEqual([]);
  });
});
