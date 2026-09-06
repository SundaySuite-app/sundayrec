import { describe, expect, it } from "vitest";

import { jpegDimsFromBase64, readJpegDims } from "./jpeg-dims";

/**
 * De første 384 bytene av en EKTE JPEG — 1920×1080, skrevet av den samme
 * ffmpeg-en som bakendens preview-sink bruker
 * (`ffmpeg -f lavfi -i testsrc=size=1920x1080 -frames:v 1`).
 *
 * En håndlaget bytesekvens ville bevist at parseren leser sin egen fixtur.
 * Dette er fila: JFIF-header, Lavc-kommentar, kvantiseringstabell,
 * Huffman-tabeller, og SOF0 på offset 293 — nøyaktig den rekkefølgen og de
 * segmentlengdene overlegget faktisk får servert. Strengen er et EKTE prefiks
 * av filas base64, altså formen `recording_preview_frame` svarer med.
 */
const REAL_JPEG_1080P_HEAD =
  "/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYyLjI4LjEwMgD/2wBDAAgYGBwY" +
  "HCEhISEhISckJygoKCcnJycoKCgrKyszMzMrKysoKCsrMDAzMzc5NzQ0MzQ5OTw8" +
  "PEhIRUVUVFdnZ3z/xAC4AAEAAQUBAQAAAAAAAAAAAAAAAwIHAQYIBAUBAQACAgMB" +
  "AAAAAAAAAAAAAAACAwYBBwgFBBABAAEBBAYIAwUGBgEFAQAAAAECAxEEUROh0SGi" +
  "EtJTUjEVFGFBgbEiBnHBBbIy4ZFCczTwI/Fig3IzwjVDgkSzEQEAAQEDCgUBBwQB" +
  "BAEDBQEAAQIDEQTRoiFSFFGh0lMSExUxBUEiYbFxQjKjgWLh8JHBBvEzciPCszU0" +
  "krKDRHP/wAARCAQ4B4ADARIAAhIAAxIA/9oADAMBAAIRAxEAPwDn8AAAAAAAAAAA" +
  "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFzBiY5/AAAAAAAAA";

function bytesOf(b64: string): Uint8Array {
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

/** Et segment: `ff <marker> <len hi> <len lo> <payload…>`. */
function segment(marker: number, payload: number[]): number[] {
  const len = payload.length + 2;
  return [0xff, marker, (len >> 8) & 0xff, len & 0xff, ...payload];
}

/** SOF-en, med høyde og bredde der standarden sier de står. */
function sof(marker: number, w: number, h: number): number[] {
  return segment(marker, [
    8, // presisjon
    (h >> 8) & 0xff,
    h & 0xff,
    (w >> 8) & 0xff,
    w & 0xff,
    1, // én komponent — nok til at lengden stemmer
    1,
    0x11,
    0,
  ]);
}

describe("readJpegDims — ekte header", () => {
  it("leser 1920×1080 ut av en ekte ffmpeg-JPEG", () => {
    expect(readJpegDims(bytesOf(REAL_JPEG_1080P_HEAD))).toEqual({
      w: 1920,
      h: 1080,
    });
  });

  it("finner den også gjennom base64-veien overlegget bruker", () => {
    expect(jpegDimsFromBase64(REAL_JPEG_1080P_HEAD)).toEqual({
      w: 1920,
      h: 1080,
    });
  });
});

describe("readJpegDims — formene skanningen må tåle", () => {
  it("leser progressiv JPEG (SOF2) likt som baseline", () => {
    const bytes = new Uint8Array([0xff, 0xd8, ...sof(0xc2, 1280, 720)]);
    expect(readJpegDims(bytes)).toEqual({ w: 1280, h: 720 });
  });

  it("hopper OVER et segment som inneholder noe som ser ut som en SOF", () => {
    // Kommentarens innmat er `ff c0 …` med 16×32 i seg. En skanning som ikke
    // følger segmentlengdene ville rapportert det tullet som bildestørrelsen.
    const decoy = [0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20];
    const bytes = new Uint8Array([
      0xff,
      0xd8,
      ...segment(0xfe, decoy),
      ...sof(0xc0, 1920, 1080),
    ]);
    expect(readJpegDims(bytes)).toEqual({ w: 1920, h: 1080 });
  });

  it("tåler `ff ff`-fyll foran markøren", () => {
    const bytes = new Uint8Array([
      0xff,
      0xd8,
      0xff,
      0xff,
      ...sof(0xc0, 640, 480),
    ]);
    expect(readJpegDims(bytes)).toEqual({ w: 640, h: 480 });
  });

  it("stopper ved SOS i stedet for å vandre inn i entropidataene", () => {
    // Etter SOS ligger `ff c0`-lignende bytes overalt som fyll. Uten stoppet
    // fant den gamle løkka «bildestørrelser» i komprimert støy.
    const noise = [0xff, 0xc0, 0x00, 0x11, 0x08, 0x07, 0x80, 0x04, 0x38];
    const bytes = new Uint8Array([
      0xff,
      0xd8,
      ...segment(0xda, [1, 1, 0]),
      ...noise,
      ...noise,
    ]);
    expect(readJpegDims(bytes)).toBeNull();
  });

  it("sier null når størrelsen ikke står i det vi fikk se", () => {
    expect(readJpegDims(new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBeNull();
    expect(readJpegDims(new Uint8Array(0))).toBeNull();
  });

  it("en SOF med null i seg er ikke et svar", () => {
    const bytes = new Uint8Array([0xff, 0xd8, ...sof(0xc0, 0, 0)]);
    expect(readJpegDims(bytes)).toBeNull();
  });
});

describe("jpegDimsFromBase64", () => {
  it("gir null i stedet for å kaste på søppel", () => {
    expect(jpegDimsFromBase64("ikke base64 i det hele tatt !!!")).toBeNull();
    expect(jpegDimsFromBase64("")).toBeNull();
  });

  it("leser bare hodet, uansett hvor stor framen er", () => {
    // 4 MB base64 foran: en parsing som dekodet ALT ville brukt hovedtråden på
    // 12 slike i sekundet.
    const padded = REAL_JPEG_1080P_HEAD + "A".repeat(4_000_000);
    const before = Date.now();
    expect(jpegDimsFromBase64(padded)).toEqual({ w: 1920, h: 1080 });
    expect(Date.now() - before).toBeLessThan(200);
  });
});
